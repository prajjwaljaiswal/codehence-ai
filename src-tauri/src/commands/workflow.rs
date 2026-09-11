//! Autonomous workflow runner.
//!
//! Wraps a normal agent run in a supervised loop:
//!
//!   preflight -> [ agent turn -> test ]xN -> commit -> push -> draft PR
//!
//! The retry cap, the git safety rules and the "did the tests actually pass"
//! decision all live here in Rust rather than in the agent's prompt, so they
//! hold even when the model misbehaves.
//!
//! multi-branch: DISABLED. Runs used to cut a branch (or take a worktree)
//! per ticket and merge the work back; now every run works and commits
//! directly on the default branch. The old branch/merge code is kept, marked
//! `multi-branch: DISABLED`, so it can be restored.

use crate::claude_binary::{create_command_with_env, find_claude_binary};
use crate::commands::agents::AgentDb;
use crate::commands::git;
use crate::commands::slack;
use crate::commands::tickets;
use log::{error, info, warn};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{oneshot, watch};
use tokio::time::{timeout, Duration};

/// How long a single test command may run before it is considered hung.
const TEST_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// How long one agent turn may run.
const AGENT_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// What a run reports when someone stops it.
const STOPPED_MESSAGE: &str = "Stopped by request";

/// How long a run waits for someone to answer before giving up.
///
/// Long enough to step away from the desk, short enough that a forgotten run
/// does not hold a repository overnight. Stopping it by hand always works.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// Question exchanges allowed within one attempt.
///
/// Answering is part of the same attempt rather than another try at the work,
/// so it does not spend the retry budget - but an agent that only ever asks
/// still has to terminate.
const MAX_QUESTIONS_PER_ITERATION: u32 = 3;

const QUESTION_START: &str = "===OPCODE-NEEDS-INPUT===";
const QUESTION_END: &str = "===END===";

/// Told to the agent so that a question comes back in a shape the runner can
/// recognise, rather than as prose it would have to guess at.
const QUESTION_CONTRACT: &str = "\n\n---\n\
    If you cannot go on without a decision that is not yours to make, do not guess and do \
    not stop quietly. End your turn with exactly this block, and nothing after it:\n\n\
    ===OPCODE-NEEDS-INPUT===\n\
    question: the one thing you need decided\n\
    options: first choice | second choice\n\
    ===END===\n\n\
    Leave the `options` line out when it is not a choice between alternatives. You will be \
    given the answer and can carry on from where you stopped. Use this only for decisions a \
    person has to make - never for anything you could settle by reading the code.";

/// A decision the agent stopped to ask for.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentQuestion {
    pub run_id: i64,
    pub question: String,
    /// Choices to pick between. Empty when the answer is free text.
    pub options: Vec<String>,
    /// True when the question reached Slack, and so must *not* also be raised
    /// in the app: the answer is expected in the Slack thread.
    pub asked_on_slack: bool,
    /// Channel it went to, so the board can say where to answer.
    pub slack_channel: Option<String>,
    /// Set when Slack is configured but the post failed, so the board can say
    /// why it is asking in the app after all.
    pub slack_error: Option<String>,
}

/// Emitted once a waiting run has its answer, so anything showing the question
/// can stop showing it. `via` is "app" or "slack".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionAnswered {
    pub run_id: i64,
    pub answer: String,
    pub via: String,
}

/// Find the question an agent ended its turn with, if it did.
///
/// The last block wins: an agent that mentions the format earlier in its
/// reasoning has not asked anything until it writes one at the end.
pub fn parse_question(text: &str) -> Option<(String, Vec<String>)> {
    let start = text.rfind(QUESTION_START)? + QUESTION_START.len();
    let rest = &text[start..];
    let body = match rest.find(QUESTION_END) {
        Some(end) => &rest[..end],
        // An unterminated block still carries the question; refusing to read it
        // would strand the run over a missing line.
        None => rest,
    };

    let mut question = String::new();
    let mut options = Vec::new();

    for line in body.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("question:") {
            question = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("options:") {
            options = rest
                .split('|')
                .map(str::trim)
                .filter(|o| !o.is_empty())
                .map(str::to_string)
                .collect();
        }
    }

    (!question.is_empty()).then_some((question, options))
}

/// Runs waiting on an answer, and where to deliver it.
static PENDING_ANSWERS: OnceLock<Mutex<HashMap<i64, oneshot::Sender<String>>>> = OnceLock::new();

fn pending_answers() -> &'static Mutex<HashMap<i64, oneshot::Sender<String>>> {
    PENDING_ANSWERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Answer the question a run is waiting on.
#[tauri::command]
pub async fn answer_workflow_question(run_id: i64, answer: String) -> Result<(), String> {
    let sender = pending_answers()
        .lock()
        .map_err(|e| e.to_string())?
        .remove(&run_id);

    match sender {
        Some(tx) => tx
            .send(answer)
            .map_err(|_| format!("Run {} stopped waiting for an answer.", run_id)),
        None => Err(format!("Run {} is not waiting for an answer.", run_id)),
    }
}

/// Watched by a run to learn it has been asked to stop.
type CancelRx = watch::Receiver<bool>;

/// Runs that can still be stopped, keyed by run id.
///
/// A run lives in a detached task, so stopping one means reaching it from the
/// outside; the entry is removed when the run ends, which is also what makes
/// "is this run still going" answerable.
static RUNNING: OnceLock<Mutex<HashMap<i64, watch::Sender<bool>>>> = OnceLock::new();

fn running_runs() -> &'static Mutex<HashMap<i64, watch::Sender<bool>>> {
    RUNNING.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Resolve once the run has been asked to stop.
///
/// `wait_for` returns immediately when the flag is already set, so a stop that
/// lands between two phases is acted on rather than missed.
async fn stopped(cancel: &CancelRx) {
    let mut rx = cancel.clone();
    let _ = rx.wait_for(|stop| *stop).await;
}

/// Lines of failing test output fed back to the agent. Enough to diagnose,
/// small enough not to swamp the context window.
const FEEDBACK_TAIL_LINES: usize = 120;

/// Caller-supplied configuration for one workflow run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowConfig {
    pub project_path: String,
    pub task: String,
    pub system_prompt: String,
    pub model: String,
    /// Shell command that must exit 0 for the work to count as verified.
    /// When absent the loop runs a single agent turn and skips verification.
    pub test_command: Option<String>,
    pub branch_prefix: String,
    pub max_iterations: u32,
    pub auto_push: bool,
    pub open_pr: bool,
    /// Fast-forward the default branch onto the run's work once its tests pass,
    /// so the next ticket starts from it rather than from where this one did.
    pub merge_to_main: bool,
    /// When set, the run works on the branch already checked out where it was
    /// pointed, rather than cutting one of its own - and leaves landing the
    /// work to whoever set that up. This is how a module runs its tickets side
    /// by side, each in its own worktree.
    pub existing_branch: Option<String>,
}

/// A phase of the run, surfaced to the UI as a stepper.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Preflight,
    Branch,
    Agent,
    Test,
    Commit,
    Merge,
    Push,
    Pr,
    Done,
    Failed,
    /// Stopped by someone, rather than by anything going wrong.
    Cancelled,
    /// Held, waiting for someone to answer the agent's question.
    Waiting,
}

/// Progress event emitted on `workflow-progress:<run_id>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowEvent {
    pub run_id: i64,
    pub phase: Phase,
    pub iteration: u32,
    pub max_iterations: u32,
    pub message: String,
    /// Set once the loop reaches a terminal phase.
    pub ok: Option<bool>,
}

/// Final outcome of a run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowOutcome {
    pub run_id: i64,
    pub branch: Option<String>,
    pub base_branch: String,
    pub base_sha: String,
    pub iterations_used: u32,
    pub tests_passed: bool,
    /// Whether the run stopped because it was asked to, rather than failing.
    pub cancelled: bool,
    /// Whether the work reached the default branch.
    pub merged: bool,
    /// Whether the agent never got going at all, which is the environment's
    /// problem rather than this ticket's.
    pub agent_never_ran: bool,
    pub commit_sha: Option<String>,
    pub pushed: bool,
    pub pr_url: Option<String>,
    pub error: Option<String>,
}

/// Where a run's agent output is kept.
///
/// Progress events vanish with the session that saw them, so without a file on
/// disk a finished run has nothing to show but its final status - and the
/// panel would claim the agent said nothing when it had said plenty.
fn output_path(app: &AppHandle, run_id: i64) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("workflow_runs");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join(format!("{}.jsonl", run_id)))
}

/// Path to the database the rest of the app uses.
fn db_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("agents.db"))
}

/// Record the branch a run is working on the moment it exists.
///
/// The outcome is only written when the run finishes, which would leave the UI
/// unable to show a diff for a run still in flight - exactly when someone most
/// wants to watch one. Failing to record is not worth aborting a run over, so
/// the error is logged rather than propagated.
fn record_branch(app: &AppHandle, run_id: i64, base_branch: &str, base_sha: &str, branch: &str) {
    let write = || -> Result<(), String> {
        let conn = rusqlite::Connection::open(db_path(app)?).map_err(|e| e.to_string())?;
        ensure_tables(&conn)?;
        conn.execute(
            "UPDATE workflow_runs SET branch = ?1, base_branch = ?2, base_sha = ?3 WHERE id = ?4",
            params![branch, base_branch, base_sha, run_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    };
    if let Err(e) = write() {
        warn!("could not record branch for run {}: {}", run_id, e);
    }
}

/// The name a phase is stored and sent under.
fn phase_name(phase: Phase) -> &'static str {
    match phase {
        Phase::Preflight => "preflight",
        Phase::Branch => "branch",
        Phase::Agent => "agent",
        Phase::Test => "test",
        Phase::Commit => "commit",
        Phase::Merge => "merge",
        Phase::Push => "push",
        Phase::Pr => "pr",
        Phase::Done => "done",
        Phase::Failed => "failed",
        Phase::Cancelled => "cancelled",
        Phase::Waiting => "waiting",
    }
}

/// Keep the run's latest phase on the run itself, not only in the event.
///
/// Events exist for as long as something is listening, so a panel opened
/// part-way through a run had a blank stepper and no sign of life - which
/// reads as stuck when the agent is working perfectly well. A separate
/// connection, as elsewhere here, so a run never waits on the UI's lock.
fn record_phase(app: &AppHandle, ev: &WorkflowEvent) {
    let write = || -> Result<(), String> {
        let conn = rusqlite::Connection::open(db_path(app)?).map_err(|e| e.to_string())?;
        ensure_tables(&conn)?;
        conn.execute(
            "UPDATE workflow_runs SET phase = ?1, phase_message = ?2 WHERE id = ?3",
            params![phase_name(ev.phase), ev.message, ev.run_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    };
    if let Err(e) = write() {
        warn!("could not record the phase of run {}: {}", ev.run_id, e);
    }
}

fn emit(app: &AppHandle, ev: WorkflowEvent) {
    record_phase(app, &ev);
    let _ = app.emit(&format!("workflow-progress:{}", ev.run_id), &ev);
    // Generic channel so a UI can follow runs it did not start.
    let _ = app.emit("workflow-progress", &ev);
}

/// Keep only the last `n` lines of `text`.
fn tail_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= n {
        return text.to_string();
    }
    let skipped = lines.len() - n;
    format!(
        "... ({} earlier lines omitted) ...\n{}",
        skipped,
        lines[skipped..].join("\n")
    )
}

/// Result of running the project's test command.
struct TestResult {
    passed: bool,
    output: String,
}

/// Run the configured test command through a shell, capturing combined output.
async fn run_tests(repo: &Path, command: &str, cancel: &CancelRx) -> Result<TestResult, String> {
    info!("running test command: {}", command);

    #[cfg(windows)]
    let mut cmd = {
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/C").arg(command);
        c
    };

    #[cfg(not(windows))]
    let mut cmd = {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let mut c = tokio::process::Command::new(shell);
        c.arg("-lc").arg(command);
        c
    };

    cmd.current_dir(repo)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        // Stopping drops the future that owns this child; without this the test
        // command would carry on running after the run it belongs to is gone.
        .kill_on_drop(true);

    let child = cmd
        .spawn()
        .map_err(|e| format!("failed to start test command: {}", e))?;

    let out = tokio::select! {
        // Biased so an already-requested stop is honoured immediately rather
        // than left to race a test command that may take minutes.
        biased;
        _ = stopped(cancel) => {
            return Ok(TestResult {
                passed: false,
                output: STOPPED_MESSAGE.to_string(),
            })
        }
        res = timeout(TEST_TIMEOUT, child.wait_with_output()) => match res {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => return Err(format!("test command failed to run: {}", e)),
            Err(_) => {
                return Ok(TestResult {
                    passed: false,
                    output: format!(
                        "Test command timed out after {} seconds:\n  {}",
                        TEST_TIMEOUT.as_secs(),
                        command
                    ),
                })
            }
        },
    };

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    Ok(TestResult {
        passed: out.status.success(),
        output: combined,
    })
}

/// Why an agent turn ended without a result.
struct TurnFailure {
    message: String,
    /// Whether the agent said anything at all before it stopped. Nothing points
    /// at the agent not having run - a binary that is missing, a session that
    /// was refused - rather than at the task being beyond it.
    produced_output: bool,
}

impl TurnFailure {
    /// A failure with nothing to show for it, which is most of them.
    fn silent(message: String) -> Self {
        Self {
            message,
            produced_output: false,
        }
    }
}

/// What one agent turn leaves behind.
struct Turn {
    session_id: String,
    /// The assistant's own words, joined - which is where a question appears.
    text: String,
}

/// Run one Claude turn to completion.
///
/// `resume_session` continues an earlier turn so the agent keeps the context of
/// what it already wrote instead of rediscovering the codebase each iteration.
async fn run_agent_turn(
    app: &AppHandle,
    run_id: i64,
    cfg: &WorkflowConfig,
    repo: &Path,
    prompt: &str,
    resume_session: Option<&str>,
    cancel: &CancelRx,
) -> Result<Turn, TurnFailure> {
    let claude_path = find_claude_binary(app).map_err(TurnFailure::silent)?;

    let mut args: Vec<String> = Vec::new();
    if let Some(sid) = resume_session {
        args.push("--resume".to_string());
        args.push(sid.to_string());
    }
    args.extend([
        "-p".to_string(),
        prompt.to_string(),
        "--system-prompt".to_string(),
        cfg.system_prompt.clone(),
        "--model".to_string(),
        cfg.model.clone(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--dangerously-skip-permissions".to_string(),
    ]);

    let mut cmd = create_command_with_env(&claude_path);
    for a in &args {
        cmd.arg(a);
    }
    cmd.current_dir(repo)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());

    let mut cmd = tokio::process::Command::from(cmd);
    cmd.kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|e| TurnFailure::silent(format!("failed to spawn claude: {}", e)))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| TurnFailure::silent("failed to capture claude stdout".to_string()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| TurnFailure::silent("failed to capture claude stderr".to_string()))?;

    let app_for_stream = app.clone();
    // Appended to across iterations, so a retry adds to the record rather than
    // replacing what the earlier attempts said.
    let mut transcript = match output_path(app, run_id) {
        Ok(path) => tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .map_err(|e| warn!("cannot record run output: {}", e))
            .ok(),
        Err(e) => {
            warn!("cannot record run output: {}", e);
            None
        }
    };

    let reader = tokio::spawn(async move {
        let mut session_id = String::new();
        let mut text = String::new();
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            // Forward raw stream-json so the existing message renderer can show it.
            let _ = app_for_stream.emit(&format!("workflow-output:{}", run_id), &line);

            if let Some(file) = transcript.as_mut() {
                let _ = file.write_all(line.as_bytes()).await;
                let _ = file.write_all(b"\n").await;
            }

            let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };

            if session_id.is_empty() {
                if let Some(sid) = value.get("session_id").and_then(|s| s.as_str()) {
                    session_id = sid.to_string();
                }
            }

            // Only the assistant's own text can carry a question; tool results
            // and diagnostics are noise for that purpose.
            if value.get("type").and_then(|t| t.as_str()) == Some("assistant") {
                if let Some(blocks) = value
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                {
                    for block in blocks {
                        if let Some(part) = block.get("text").and_then(|t| t.as_str()) {
                            text.push_str(part);
                            text.push('\n');
                        }
                    }
                }
            }
        }
        (session_id, text)
    });

    let err_reader = tokio::spawn(async move {
        let mut buf = String::new();
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            buf.push_str(&line);
            buf.push('\n');
        }
        buf
    });

    let status = tokio::select! {
        biased;
        _ = stopped(cancel) => {
            // The agent holds the repository open; leaving it running would let
            // it keep writing to a tree the run no longer owns.
            let _ = child.kill().await;
            return Err(TurnFailure::silent(STOPPED_MESSAGE.to_string()));
        }
        res = timeout(AGENT_TIMEOUT, child.wait()) => match res {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                return Err(TurnFailure::silent(format!("claude process error: {}", e)))
            }
            Err(_) => {
                let _ = child.kill().await;
                return Err(TurnFailure::silent(format!(
                    "agent turn timed out after {} seconds",
                    AGENT_TIMEOUT.as_secs()
                )));
            }
        },
    };

    let (session_id, text) = reader.await.unwrap_or_default();
    let stderr_text = err_reader.await.unwrap_or_default();

    if !status.success() {
        let produced_output = !text.trim().is_empty();
        let detail = tail_lines(&stderr_text, 20);
        let message = if detail.trim().is_empty() {
            format!(
                "claude exited with {} and said nothing. Check the CLI runs: `claude --version`.",
                status.code().unwrap_or(-1)
            )
        } else {
            format!(
                "claude exited with {}: {}",
                status.code().unwrap_or(-1),
                detail
            )
        };
        return Err(TurnFailure {
            message,
            produced_output,
        });
    }

    if session_id.is_empty() {
        warn!("no session_id seen in claude output; retries will start fresh");
    }

    Ok(Turn { session_id, text })
}

/// Hold the run until someone answers, or until waiting stops being useful.
///
/// Returns `None` when the run was stopped or nobody answered in time, which
/// the caller treats as the end of the attempt rather than as an answer.
async fn wait_for_answer(
    app: &AppHandle,
    run_id: i64,
    task: &str,
    question: &str,
    options: &[String],
    cancel: &CancelRx,
) -> Option<String> {
    let (tx, rx) = oneshot::channel();
    match pending_answers().lock() {
        Ok(mut waiting) => {
            waiting.insert(run_id, tx);
        }
        Err(e) => {
            error!("cannot register a pending answer: {}", e);
            return None;
        }
    }

    // Slack first, because whether it worked decides what the app is told: a
    // question that reached Slack must not also open a dialog nobody is sitting
    // in front of to see.
    let slack = slack::configured(app);
    let mut slack_error = None;
    let thread = match &slack {
        Some(s) => match slack::ask(s, run_id, task, question, options).await {
            Ok(posted) => {
                info!("run {} asked its question in {}", run_id, s.channel_label());
                Some(posted)
            }
            Err(e) => {
                warn!("run {} could not ask its question on Slack: {}", run_id, e);
                slack_error = Some(e);
                None
            }
        },
        None => None,
    };

    let channel = thread
        .as_ref()
        .and_then(|_| slack.as_ref())
        .map(|s| s.channel_label());

    // Either way the run is blocked, so say so outside the window - with where
    // to answer, which is the only part that differs.
    slack::notify_question(app, run_id, channel.as_deref());

    let payload = AgentQuestion {
        run_id,
        question: question.to_string(),
        options: options.to_vec(),
        asked_on_slack: thread.is_some(),
        slack_channel: channel,
        slack_error,
    };
    let _ = app.emit(&format!("workflow-question:{}", run_id), &payload);
    // Also on the open channel, so the board can raise the question even when
    // nobody has the run's panel open.
    let _ = app.emit("workflow-question", &payload);

    // Answering in the app still works while Slack is waiting: whichever
    // arrives first wins, and the other is simply never read.
    let from_slack = async {
        match (&slack, &thread) {
            (Some(s), Some(posted)) => slack::wait_for_reply(s, posted).await,
            // Nothing to poll - park forever and let the other arms decide.
            _ => std::future::pending().await,
        }
    };

    let answered = tokio::select! {
        biased;
        _ = stopped(cancel) => None,
        answer = rx => answer.ok().map(|a| (a, "app")),
        reply = from_slack => reply.map(|r| (slack::resolve_choice(&r, options), "slack")),
        _ = tokio::time::sleep(ANSWER_TIMEOUT) => {
            warn!("run {} gave up waiting for an answer", run_id);
            None
        }
    };

    // Whatever happened, the run is no longer waiting: leaving the sender in
    // place would let a later answer be accepted for a question nobody is
    // holding open any more.
    if let Ok(mut waiting) = pending_answers().lock() {
        waiting.remove(&run_id);
    }

    let (answer, via) = answered?;

    if let (Some(s), Some(posted)) = (&slack, &thread) {
        // Said in the thread either way: someone watching Slack should see the
        // run move on even when the answer was typed into the app.
        slack::acknowledge(s, posted, &answer).await;
    }

    let _ = app.emit(
        "workflow-question-answered",
        QuestionAnswered {
            run_id,
            answer: answer.clone(),
            via: via.to_string(),
        },
    );

    Some(answer)
}

/// Prompt used when a test run comes back red.
fn build_retry_prompt(test_command: &str, output: &str) -> String {
    format!(
        "The test command `{}` is still failing. Here is the tail of its output:\n\n\
         ```\n{}\n```\n\n\
         Fix the cause of these failures. Do not modify or delete tests to make them \
         pass unless the test itself is genuinely wrong - if you believe a test is \
         wrong, say so explicitly and explain why. When you are done, stop; the test \
         suite will be run again automatically.",
        test_command,
        tail_lines(output, FEEDBACK_TAIL_LINES)
    )
}

/// Prompt used for the very first turn.
fn build_initial_prompt(cfg: &WorkflowConfig) -> String {
    let task = match &cfg.test_command {
        Some(tc) => format!(
            "{}\n\n---\nWhen you have implemented this, also add or update automated \
             tests that cover the change. The command `{}` will be run to verify your \
             work, and you will be given the output if it fails.",
            cfg.task, tc
        ),
        None => cfg.task.clone(),
    };

    format!("{}{}", task, QUESTION_CONTRACT)
}

/// Drive a full workflow run to completion.
///
/// Returns the outcome rather than erroring on a red test suite: a run that
/// legitimately fails its tests is a result, not a crash.
pub async fn run_workflow(
    app: AppHandle,
    run_id: i64,
    cfg: WorkflowConfig,
    cancel: CancelRx,
) -> Result<WorkflowOutcome, String> {
    let max_iterations = cfg.max_iterations.max(1);

    let say = |phase: Phase, iteration: u32, msg: &str, ok: Option<bool>| {
        emit(
            &app,
            WorkflowEvent {
                run_id,
                phase,
                iteration,
                max_iterations,
                message: msg.to_string(),
                ok,
            },
        );
    };

    // ---- Preflight -------------------------------------------------------
    say(Phase::Preflight, 0, "Checking repository state", None);

    let start = PathBuf::from(&cfg.project_path);
    if !git::is_git_repo(&start) {
        let e = format!("{} is not a git repository", cfg.project_path);
        say(Phase::Failed, 0, &e, Some(false));
        return Err(e);
    }

    let repo = git::repo_root(&start)?;
    let base_branch = git::default_branch(&repo)?;
    let base_sha = git::head_sha(&repo)?;

    // A dirty tree is refused rather than stashed. Stashing someone's
    // in-progress work automatically is the kind of "help" that loses code.
    if git::is_dirty(&repo)? {
        let e = "Working tree has uncommitted changes. Commit or stash them before \
                 starting an autonomous run."
            .to_string();
        say(Phase::Failed, 0, &e, Some(false));
        return Err(e);
    }

    let mut outcome = WorkflowOutcome {
        run_id,
        branch: None,
        base_branch: base_branch.clone(),
        base_sha: base_sha.clone(),
        iterations_used: 0,
        tests_passed: false,
        cancelled: false,
        merged: false,
        agent_never_ran: false,
        commit_sha: None,
        pushed: false,
        pr_url: None,
        error: None,
    };

    // ---- Branch --------------------------------------------------------
    // multi-branch: DISABLED. Every run works directly on the default branch
    // and commits straight to it - no per-ticket branch, no worktree hand-off.
    // The original branch-per-ticket logic is kept below; restore this block
    // (and undo the `run_module_inner` swap further down) to bring it back.
    //
    // let branch = match &cfg.existing_branch {
    //     // Already on it: the worktree was made on this branch, and cutting
    //     // another here would put the work somewhere nobody is looking.
    //     Some(existing) => {
    //         say(Phase::Branch, 0, &format!("Working on {}", existing), None);
    //         existing.clone()
    //     }
    //     None => {
    //         let branch = git::build_branch_name(&cfg.branch_prefix, &cfg.task, run_id);
    //         say(
    //             Phase::Branch,
    //             0,
    //             &format!("Creating branch {}", branch),
    //             None,
    //         );
    //         if let Err(e) = git::create_branch(&repo, &branch) {
    //             say(Phase::Failed, 0, &e, Some(false));
    //             outcome.error = Some(e.clone());
    //             return Err(e);
    //         }
    //         branch
    //     }
    // };
    let branch = base_branch.clone();
    say(
        Phase::Branch,
        0,
        &format!("Working directly on {} (branch-per-ticket disabled)", branch),
        None,
    );
    outcome.branch = Some(branch.clone());
    record_branch(&app, run_id, &base_branch, &base_sha, &branch);

    // ---- Implement / verify loop ----------------------------------------
    let mut session_id: Option<String> = None;
    let mut last_test_output = String::new();

    for iteration in 1..=max_iterations {
        if *cancel.borrow() {
            break;
        }
        outcome.iterations_used = iteration;

        let mut prompt = match (&session_id, &cfg.test_command) {
            (Some(_), Some(tc)) => build_retry_prompt(tc, &last_test_output),
            _ => build_initial_prompt(&cfg),
        };

        // An agent may stop to ask something before it can do the work. The
        // question and its answer belong to this attempt rather than counting
        // as another one, so they are handled here instead of spending an
        // iteration of the retry budget on a conversation.
        let mut questions_asked = 0;
        let mut turn_failed = false;

        loop {
            say(
                Phase::Agent,
                iteration,
                &format!("Agent turn {}/{}", iteration, max_iterations),
                None,
            );

            let turn = match run_agent_turn(
                &app,
                run_id,
                &cfg,
                &repo,
                &prompt,
                session_id.as_deref().filter(|s| !s.is_empty()),
                &cancel,
            )
            .await
            {
                Ok(turn) => turn,
                Err(failure) => {
                    error!("agent turn failed: {}", failure.message);
                    say(Phase::Failed, iteration, &failure.message, Some(false));
                    outcome.error = Some(failure.message);
                    // An agent that said nothing at all did not run, and the
                    // next ticket would meet the same wall. Recorded so the
                    // queue stops rather than working through the whole board
                    // failing every ticket in seconds.
                    outcome.agent_never_ran = !failure.produced_output && !*cancel.borrow();
                    turn_failed = true;
                    break;
                }
            };

            if !turn.session_id.is_empty() {
                session_id = Some(turn.session_id);
            }

            if *cancel.borrow() {
                break;
            }

            let Some((question, options)) = parse_question(&turn.text) else {
                break;
            };

            if questions_asked >= MAX_QUESTIONS_PER_ITERATION {
                let e = format!(
                    "The agent asked {} questions without getting on with the work.",
                    questions_asked + 1
                );
                say(Phase::Failed, iteration, &e, Some(false));
                outcome.error = Some(e);
                turn_failed = true;
                break;
            }

            say(Phase::Waiting, iteration, &question, None);

            let Some(answer) =
                wait_for_answer(&app, run_id, &cfg.task, &question, &options, &cancel).await
            else {
                if !*cancel.borrow() {
                    let e = "No answer was given, so the run could not go on.".to_string();
                    say(Phase::Failed, iteration, &e, Some(false));
                    outcome.error = Some(e);
                    turn_failed = true;
                }
                break;
            };

            questions_asked += 1;
            prompt = format!(
                "The answer to your question is: {}\n\nCarry on from where you stopped.",
                answer
            );
        }

        if turn_failed {
            // Fall through to commit so partial work is not lost.
            break;
        }

        if *cancel.borrow() {
            break;
        }

        let Some(test_command) = cfg.test_command.as_deref() else {
            // No verification configured: one turn is all we do.
            say(
                Phase::Test,
                iteration,
                "No test command configured - skipping verification",
                None,
            );
            break;
        };

        say(Phase::Test, iteration, "Running tests", None);
        match run_tests(&repo, test_command, &cancel).await {
            Ok(res) => {
                last_test_output = res.output;
                if res.passed {
                    outcome.tests_passed = true;
                    say(Phase::Test, iteration, "Tests passed", Some(true));
                    break;
                }
                say(
                    Phase::Test,
                    iteration,
                    &format!("Tests failed (attempt {}/{})", iteration, max_iterations),
                    Some(false),
                );
            }
            Err(e) => {
                say(Phase::Failed, iteration, &e, Some(false));
                outcome.error = Some(e);
                break;
            }
        }
    }

    // A stop is not a failure, and the difference matters to whoever reads the
    // run later - so it is recorded rather than folded into the error.
    outcome.cancelled = *cancel.borrow();
    if outcome.cancelled {
        // The agent's own "stopped" error would otherwise read as a fault.
        if outcome.error.as_deref() == Some(STOPPED_MESSAGE) {
            outcome.error = None;
        }
        say(
            Phase::Cancelled,
            outcome.iterations_used,
            "Stopping - committing whatever the agent finished",
            None,
        );
    }

    // ---- Commit ----------------------------------------------------------
    // Always commit: a red commit you can inspect (and revert) beats losing the
    // attempt entirely. A stopped run is committed for the same reason -
    // stopping should not also throw the work away.
    // multi-branch disabled: this commit lands directly on the default branch.
    say(Phase::Commit, outcome.iterations_used, "Committing", None);
    let verdict = if cfg.test_command.is_none() {
        "unverified"
    } else if outcome.tests_passed {
        "tests passing"
    } else {
        "TESTS FAILING"
    };
    let message = format!(
        "{}\n\nAutomated run by dotsquares-ai ({}), {} iteration(s).\n",
        cfg.task.lines().next().unwrap_or("Automated change"),
        verdict,
        outcome.iterations_used
    );

    match git::commit_all(&repo, &message) {
        Ok(Some(sha)) => outcome.commit_sha = Some(sha),
        Ok(None) => {
            // Nothing left to stage is not the same as nothing done: agents
            // routinely commit their own work, and calling that a failure threw
            // away finished runs and stalled the queue behind them.
            match git::head_sha(&repo) {
                Ok(head) if head != base_sha => {
                    info!("agent committed its own work on {}", branch);
                    outcome.commit_sha = Some(head);
                }
                _ => {
                    let e = "Agent produced no file changes".to_string();
                    say(Phase::Failed, outcome.iterations_used, &e, Some(false));
                    outcome.error.get_or_insert(e);
                }
            }
        }
        Err(e) => {
            say(Phase::Failed, outcome.iterations_used, &e, Some(false));
            outcome.error.get_or_insert(e);
        }
    }

    // ---- Merge -----------------------------------------------------------
    // multi-branch: DISABLED. The work is already committed on the default
    // branch, so there is nothing to fast-forward onto it. The original
    // merge-to-main step is kept below; restore it alongside the Branch block
    // above.
    let lands_its_own_work = cfg.existing_branch.is_none();

    // if lands_its_own_work && cfg.merge_to_main && !outcome.cancelled && outcome.commit_sha.is_some()
    // {
    //     if outcome.tests_passed {
    //         say(
    //             Phase::Merge,
    //             outcome.iterations_used,
    //             &format!("Merging {} into {}", branch, base_branch),
    //             None,
    //         );
    //         match git::merge_fast_forward(&repo, &base_branch, &branch) {
    //             Ok(()) => outcome.merged = true,
    //             Err(e) => {
    //                 warn!("merge failed: {}", e);
    //                 say(Phase::Failed, outcome.iterations_used, &e, Some(false));
    //                 outcome.error.get_or_insert(e);
    //             }
    //         }
    //     } else {
    //         say(
    //             Phase::Merge,
    //             outcome.iterations_used,
    //             "Not merging - the work is not verified",
    //             Some(false),
    //         );
    //     }
    // }

    // ---- Push / PR -------------------------------------------------------
    // Only green, committed work reaches the remote. Pushing a knowingly
    // broken branch and opening a PR for it wastes a reviewer's time and CI.
    // A stopped run is unfinished by definition, whatever its tests last said.
    // Merged work is skipped as well: a pull request for a branch already in
    // the default branch has nothing left to review.
    let publishable = lands_its_own_work
        && !outcome.cancelled
        && !outcome.merged
        && outcome.commit_sha.is_some()
        && (outcome.tests_passed || cfg.test_command.is_none());

    if cfg.auto_push && publishable {
        say(Phase::Push, outcome.iterations_used, "Pushing branch", None);
        match git::push_branch(&repo, &branch) {
            Ok(()) => outcome.pushed = true,
            Err(e) => {
                warn!("push failed: {}", e);
                say(Phase::Failed, outcome.iterations_used, &e, Some(false));
                outcome.error.get_or_insert(e);
            }
        }
    } else if cfg.auto_push {
        say(
            Phase::Push,
            outcome.iterations_used,
            "Skipping push - tests did not pass",
            Some(false),
        );
    }

    if cfg.open_pr && outcome.pushed {
        say(Phase::Pr, outcome.iterations_used, "Opening draft PR", None);
        let title = cfg
            .task
            .lines()
            .next()
            .unwrap_or("Automated change")
            .to_string();
        let body = format!(
            "Automated run by dotsquares-ai.\n\n\
             **Task**\n{}\n\n\
             **Verification**\n`{}` - {}\n\n\
             **Run details**\n- Base: `{}` @ `{}`\n- Iterations: {}/{}\n",
            cfg.task,
            cfg.test_command.as_deref().unwrap_or("(none configured)"),
            if outcome.tests_passed {
                "passing"
            } else {
                "not verified"
            },
            outcome.base_branch,
            outcome.base_sha,
            outcome.iterations_used,
            max_iterations,
        );

        match git::create_draft_pr(&repo, &branch, &base_branch, &title, &body) {
            Ok(url) => outcome.pr_url = Some(url),
            Err(e) => {
                warn!("draft PR failed: {}", e);
                say(Phase::Failed, outcome.iterations_used, &e, Some(false));
                outcome.error.get_or_insert(e);
            }
        }
    }

    let succeeded = !outcome.cancelled && outcome.error.is_none() && outcome.commit_sha.is_some();
    say(
        if succeeded {
            Phase::Done
        } else if outcome.cancelled {
            Phase::Cancelled
        } else {
            Phase::Failed
        },
        outcome.iterations_used,
        &format!(
            "Finished on {} ({})",
            branch,
            if outcome.tests_passed {
                "tests passed"
            } else {
                "tests not passing"
            }
        ),
        Some(succeeded),
    );

    Ok(outcome)
}

/// Ensure the workflow tables exist. Created lazily so an existing agents.db
/// upgrades in place without a migration step.
fn ensure_tables(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS workflow_runs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            agent_id INTEGER NOT NULL,
            task TEXT NOT NULL,
            project_path TEXT NOT NULL,
            model TEXT NOT NULL,
            test_command TEXT,
            branch TEXT,
            base_branch TEXT,
            base_sha TEXT,
            status TEXT NOT NULL DEFAULT 'running',
            iterations_used INTEGER NOT NULL DEFAULT 0,
            max_iterations INTEGER NOT NULL,
            tests_passed INTEGER NOT NULL DEFAULT 0,
            commit_sha TEXT,
            pushed INTEGER NOT NULL DEFAULT 0,
            pr_url TEXT,
            error TEXT,
            phase TEXT,
            phase_message TEXT,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            completed_at TEXT
        )",
        [],
    )
    .map_err(|e| e.to_string())?;

    let _ = conn.execute("ALTER TABLE workflow_runs ADD COLUMN phase TEXT", []);
    let _ = conn.execute(
        "ALTER TABLE workflow_runs ADD COLUMN phase_message TEXT",
        [],
    );

    Ok(())
}

/// Kick off an autonomous run and return its id immediately.
///
/// The run itself proceeds in the background, reporting progress on
/// `workflow-progress:<run_id>` and raw agent output on `workflow-output:<run_id>`.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn start_workflow(
    app: AppHandle,
    agent_id: i64,
    project_path: String,
    task: String,
    model: Option<String>,
    test_command: Option<String>,
    branch_prefix: Option<String>,
    max_iterations: Option<u32>,
    auto_push: Option<bool>,
    open_pr: Option<bool>,
    merge_to_main: Option<bool>,
    // When set, the run is attached to this ticket and drives its board column.
    ticket_id: Option<i64>,
    db: State<'_, AgentDb>,
) -> Result<i64, String> {
    begin_run(
        &app,
        &db,
        agent_id,
        project_path,
        task,
        model,
        test_command,
        branch_prefix,
        max_iterations,
        auto_push,
        open_pr,
        merge_to_main,
        ticket_id,
    )
}

/// Attempts a run makes at its own work before giving up, when nobody says.
const DEFAULT_MAX_ITERATIONS: u32 = 3;

/// How many of a module's tickets are worked at once.
///
/// Two rather than all of them: each one needs its own checkout and its own
/// dependencies installed, and siblings that touch the same file conflict more
/// the more of them run side by side.
///
/// multi-branch: DISABLED - only `run_module_inner_multibranch` reads this.
/// Modules now run their tickets one at a time on the default branch.
#[allow(dead_code)]
const PARALLEL_AGENTS: usize = 2;

/// How a module got on, for the board to show.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleEvent {
    pub project_path: String,
    pub module: String,
    pub message: String,
    /// Set once the module is finished with, one way or the other.
    pub ok: Option<bool>,
}

fn say_module(app: &AppHandle, project_path: &str, module: &str, message: &str, ok: Option<bool>) {
    let event = ModuleEvent {
        project_path: project_path.to_string(),
        module: module.to_string(),
        message: message.to_string(),
        ok,
    };
    info!("module `{}`: {}", module, message);
    let _ = app.emit("module-progress", &event);
}

/// Where a run's own checkout lives while it works.
///
/// multi-branch: DISABLED - kept for `run_module_inner_multibranch` only.
#[allow(dead_code)]
fn worktree_path(app: &AppHandle, run_id: i64) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("worktrees");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join(format!("run-{}", run_id)))
}

/// Work one ticket in a checkout of its own, on a branch off the module's.
///
/// Returns the branch its work is on when there is any, so the module can try
/// to merge it. Everything else - retries, questions, stopping, the record of
/// what it said - is the ordinary run, which is why this hands over to it
/// rather than reimplementing a smaller version.
///
/// multi-branch: DISABLED - superseded by `run_ticket_on_main`, which runs the
/// ticket directly on the default branch. Kept for restoration.
#[allow(dead_code)]
async fn run_ticket_in_worktree(
    app: &AppHandle,
    board: &tickets::BoardProject,
    module_branch: &str,
    ticket: &tickets::Ticket,
) -> Option<String> {
    let Some(agent_id) = ticket.agent_id else {
        return None;
    };
    let task = match ticket.description.as_deref() {
        Some(d) if !d.trim().is_empty() => d.to_string(),
        _ => ticket.title.clone(),
    };

    let prepare = || -> Result<(i64, PathBuf, String, WorkflowConfig), String> {
        let db = app.state::<AgentDb>();
        let (system_prompt, model) = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            ensure_tables(&conn)?;
            load_agent(&conn, agent_id)?
        };

        let run_id = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT INTO workflow_runs (agent_id, task, project_path, model, test_command,
                 max_iterations) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    agent_id,
                    task,
                    board.repo_path,
                    model,
                    board.test_command,
                    DEFAULT_MAX_ITERATIONS
                ],
            )
            .map_err(|e| e.to_string())?;
            let id = conn.last_insert_rowid();
            tickets::link_run(&conn, ticket.id, id)?;
            id
        };

        let repo = git::repo_root(&PathBuf::from(&board.repo_path))?;
        let path = worktree_path(app, run_id)?;
        let branch = git::build_branch_name("dotsquares-ai", &ticket.title, run_id);
        git::add_worktree(&repo, &path, &branch, module_branch)?;

        let cfg = WorkflowConfig {
            project_path: path.to_string_lossy().to_string(),
            task,
            system_prompt,
            model,
            test_command: board.test_command.clone(),
            branch_prefix: "dotsquares-ai".to_string(),
            max_iterations: DEFAULT_MAX_ITERATIONS,
            auto_push: false,
            open_pr: false,
            merge_to_main: false,
            existing_branch: Some(branch.clone()),
        };

        Ok((run_id, path, branch, cfg))
    };

    let (run_id, path, branch, cfg) = match prepare() {
        Ok(prepared) => prepared,
        Err(e) => {
            warn!("could not start `{}`: {}", ticket.title, e);
            return None;
        }
    };

    let (stop_tx, stop_rx) = watch::channel(false);
    if let Ok(mut runs) = running_runs().lock() {
        runs.insert(run_id, stop_tx);
    }

    let result = run_workflow(app.clone(), run_id, cfg, stop_rx).await;

    if let Ok(mut runs) = running_runs().lock() {
        runs.remove(&run_id);
    }

    let committed = matches!(&result, Ok(o) if o.commit_sha.is_some());

    if let Ok(db_path) = db_path(app) {
        if let Err(e) = persist_outcome(&db_path, run_id, &result) {
            error!("failed to persist workflow outcome: {}", e);
        }
    }

    // The checkout has done its job; the branch keeps the work.
    if let Ok(repo) = git::repo_root(&PathBuf::from(&board.repo_path)) {
        let _ = git::remove_worktree(&repo, &path);
    }

    committed.then_some(branch)
}

/// Work one ticket directly on the checked-out default branch.
///
/// multi-branch: this is the replacement for `run_ticket_in_worktree`. No
/// worktree, no per-ticket branch - the agent runs against `board.repo_path`
/// and commits straight to whatever branch is checked out there. Callers must
/// invoke this one ticket at a time; two at once would fight over the tree.
/// Returns `true` when the run left a commit behind.
async fn run_ticket_on_main(
    app: &AppHandle,
    board: &tickets::BoardProject,
    ticket: &tickets::Ticket,
) -> bool {
    let Some(agent_id) = ticket.agent_id else {
        return false;
    };
    let task = match ticket.description.as_deref() {
        Some(d) if !d.trim().is_empty() => d.to_string(),
        _ => ticket.title.clone(),
    };

    let prepare = || -> Result<(i64, WorkflowConfig), String> {
        let db = app.state::<AgentDb>();
        let (system_prompt, model) = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            ensure_tables(&conn)?;
            load_agent(&conn, agent_id)?
        };

        let run_id = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT INTO workflow_runs (agent_id, task, project_path, model, test_command,
                 max_iterations) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    agent_id,
                    task,
                    board.repo_path,
                    model,
                    board.test_command,
                    DEFAULT_MAX_ITERATIONS
                ],
            )
            .map_err(|e| e.to_string())?;
            let id = conn.last_insert_rowid();
            tickets::link_run(&conn, ticket.id, id)?;
            id
        };

        let cfg = WorkflowConfig {
            project_path: board.repo_path.clone(),
            task,
            system_prompt,
            model,
            test_command: board.test_command.clone(),
            branch_prefix: "dotsquares-ai".to_string(),
            max_iterations: DEFAULT_MAX_ITERATIONS,
            auto_push: false,
            open_pr: false,
            merge_to_main: false,
            existing_branch: None,
        };

        Ok((run_id, cfg))
    };

    let (run_id, cfg) = match prepare() {
        Ok(prepared) => prepared,
        Err(e) => {
            warn!("could not start `{}`: {}", ticket.title, e);
            return false;
        }
    };

    let (stop_tx, stop_rx) = watch::channel(false);
    if let Ok(mut runs) = running_runs().lock() {
        runs.insert(run_id, stop_tx);
    }

    let result = run_workflow(app.clone(), run_id, cfg, stop_rx).await;

    if let Ok(mut runs) = running_runs().lock() {
        runs.remove(&run_id);
    }

    let committed = matches!(&result, Ok(o) if o.commit_sha.is_some());

    if let Ok(db_path) = db_path(app) {
        if let Err(e) = persist_outcome(&db_path, run_id, &result) {
            error!("failed to persist workflow outcome: {}", e);
        }
    }

    let (phase, message) = outcome_summary(&result);
    notify_outcome(app, run_id, &ticket.title, phase, &message).await;

    committed
}

/// How a finished run reads: its terminal phase and a line saying what happened.
fn outcome_summary(result: &Result<WorkflowOutcome, String>) -> (Phase, String) {
    match result {
        Ok(o) if o.cancelled => (
            Phase::Cancelled,
            format!("Stopped after {} iteration(s)", o.iterations_used),
        ),
        Ok(o) if o.error.is_none() && o.commit_sha.is_some() => (
            Phase::Done,
            format!(
                "Finished in {} iteration(s) ({})",
                o.iterations_used,
                if o.tests_passed {
                    "tests passed"
                } else {
                    "tests not verified"
                }
            ),
        ),
        Ok(o) => (
            Phase::Failed,
            o.error
                .clone()
                .unwrap_or_else(|| "Run produced no commit".to_string()),
        ),
        Err(e) => (Phase::Failed, e.clone()),
    }
}

/// An agent's system prompt and model, by id.
fn load_agent(conn: &Connection, agent_id: i64) -> Result<(String, String), String> {
    conn.query_row(
        "SELECT system_prompt, model FROM agents WHERE id = ?1",
        params![agent_id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )
    .map_err(|e| format!("agent {} not found: {}", agent_id, e))
}

/// Have a ticket's own agent resolve a conflict its merge left behind.
///
/// One attempt, directly in `repo`, which is where `try_merge` left the
/// conflict - a fresh worktree would only have to be pointed at the same spot.
/// A conflict a model cannot resolve after looking at it once is not going to
/// be resolved by asking again with nothing new to go on, and this already
/// runs inside a module a queue is waiting on.
///
/// `Ok(true)` means resolved and committed. `Ok(false)` means it could not be,
/// the merge was backed out, and the ticket's work stays on its own branch.
/// `Err` is reserved for something going wrong with the attempt itself, not
/// with whether the conflict got resolved.
///
/// multi-branch: DISABLED - only `run_module_inner_multibranch` calls this.
/// Tickets running one at a time on the default branch cannot conflict with a
/// sibling, so there is nothing to resolve.
#[allow(dead_code)]
async fn resolve_conflict(
    app: &AppHandle,
    board: &tickets::BoardProject,
    repo: &Path,
    ticket: &tickets::Ticket,
) -> Result<bool, String> {
    let Some(agent_id) = ticket.agent_id else {
        git::abort_merge(repo)?;
        return Ok(false);
    };

    let conflicted = git::conflicted_paths(repo)?;
    if conflicted.is_empty() {
        // try_merge would not have reported a conflict without these.
        git::abort_merge(repo)?;
        return Ok(false);
    }

    let (system_prompt, model) = {
        let db = app.state::<AgentDb>();
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        load_agent(&conn, agent_id)?
    };

    let task = format!(
        "A git merge of your branch into this module's branch conflicted. Resolve the \
         conflicts below so the result is correct and complete - where both sides changed \
         something that matters, keep both rather than picking one. When every conflict \
         marker is gone, run `git add -A` and stop there. Do not run `git commit` or `git \
         merge --abort`; something else does that once you are done.\n\nConflicted files:\n{}",
        conflicted
            .iter()
            .map(|f| format!("- {}", f))
            .collect::<Vec<_>>()
            .join("\n")
    );

    let run_id = {
        let db = app.state::<AgentDb>();
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        ensure_tables(&conn)?;
        conn.execute(
            "INSERT INTO workflow_runs (agent_id, task, project_path, model, max_iterations)
             VALUES (?1, ?2, ?3, ?4, 1)",
            params![
                agent_id,
                format!("Resolve merge conflict: {}", ticket.title),
                board.repo_path,
                model
            ],
        )
        .map_err(|e| e.to_string())?;
        conn.last_insert_rowid()
    };

    let cfg = WorkflowConfig {
        project_path: repo.to_string_lossy().to_string(),
        task: task.clone(),
        system_prompt,
        model,
        test_command: None,
        branch_prefix: "dotsquares-ai".to_string(),
        max_iterations: 1,
        auto_push: false,
        open_pr: false,
        merge_to_main: false,
        existing_branch: None,
    };

    // No cancellation route in from here - a module a queue started is not
    // something a person is watching turn by turn the way a single run is.
    let (_stop_tx, stop_rx) = watch::channel(false);
    let turn = run_agent_turn(app, run_id, &cfg, repo, &task, None, &stop_rx).await;

    let (status, error) = match &turn {
        Ok(_) => ("completed", None),
        Err(f) => ("failed", Some(f.message.clone())),
    };
    if let Ok(db_path) = db_path(app) {
        if let Ok(conn) = rusqlite::Connection::open(&db_path) {
            let _ = conn.execute(
                "UPDATE workflow_runs SET status = ?1, error = ?2, completed_at = CURRENT_TIMESTAMP
                 WHERE id = ?3",
                params![status, error, run_id],
            );
        }
    }

    if turn.is_err() {
        warn!(
            "conflict resolution for `{}` did not run to completion",
            ticket.title
        );
        git::abort_merge(repo)?;
        return Ok(false);
    }

    if !git::conflicted_paths(repo)?.is_empty() {
        warn!(
            "`{}` still has unresolved conflicts after the agent's turn",
            ticket.title
        );
        git::abort_merge(repo)?;
        return Ok(false);
    }

    git::finish_merge(repo)?;
    Ok(true)
}

/// Take on a whole module: work its tickets one at a time, then verify.
///
/// A plan is written in modules, and a module is the unit that makes sense to
/// verify: a header, a hero and a footer are worth building and testing
/// together rather than one at a time against a moving target.
///
/// multi-branch: DISABLED. Tickets used to run side by side, each in its own
/// worktree and branch, then merge into a module branch. Now they run one
/// after another straight on the default branch - see `run_module_inner`. The
/// old orchestration is kept verbatim as `run_module_inner_multibranch`.
async fn run_module(app: AppHandle, board: tickets::BoardProject, module: tickets::Module) {
    let name = if module.name.is_empty() {
        "Ungrouped".to_string()
    } else {
        module.name.clone()
    };
    let project_path = board.repo_path.clone();

    let outcome = run_module_inner(&app, &board, &module, &name).await;

    match outcome {
        Ok(message) => say_module(&app, &project_path, &name, &message, Some(true)),
        Err(e) => say_module(&app, &project_path, &name, &e, Some(false)),
    }

    // Whatever happened here, the plan has more of it.
    continue_queue(&app, &project_path);
}

/// multi-branch: DISABLED. Work a module's tickets one at a time, directly on
/// the default branch, then run the module's tests over the combined result.
///
/// No module branch, no per-ticket worktrees, no inter-ticket merges: each
/// ticket commits straight onto the branch that is checked out, and the next
/// ticket starts from there. `board.merge_to_main` no longer gates anything -
/// the work is already on the default branch - but it is still reported.
async fn run_module_inner(
    app: &AppHandle,
    board: &tickets::BoardProject,
    module: &tickets::Module,
    name: &str,
) -> Result<String, String> {
    let repo = git::repo_root(&PathBuf::from(&board.repo_path))?;
    let base = git::default_branch(&repo)?;

    say_module(
        app,
        &board.repo_path,
        name,
        &format!(
            "Starting {} ticket(s) one at a time on {}",
            module.tickets.len(),
            base
        ),
        None,
    );

    let mut done = 0usize;
    for ticket in &module.tickets {
        say_module(
            app,
            &board.repo_path,
            name,
            &format!("Working `{}`", ticket.title),
            None,
        );
        if run_ticket_on_main(app, board, ticket).await {
            done += 1;
        }
    }

    if done == 0 {
        return Err(format!("`{}` produced no work", name));
    }

    // ---- Verify the module as a whole ------------------------------------
    let Some(test_command) = board.test_command.as_deref() else {
        return Ok(format!(
            "{} of {} ticket(s) committed to {}. No test command, so nothing was verified.",
            done,
            module.tickets.len(),
            base
        ));
    };

    say_module(app, &board.repo_path, name, "Running the module's tests", None);
    let (_never_cancelled, cancel) = watch::channel(false);
    let result = run_tests(&repo, test_command, &cancel).await?;

    if !result.passed {
        return Err(format!(
            "`{}` does not pass `{}`. The work is on {}.",
            name, test_command, base
        ));
    }

    let merge_note = if board.merge_to_main {
        ""
    } else {
        " (merge-to-main is off, but the work is on the default branch anyway)"
    };
    Ok(format!(
        "{} ticket(s) committed to {} and passing.{}",
        done, base, merge_note
    ))
}

/// multi-branch: DISABLED, kept verbatim for restoration. The original
/// module runner - per-ticket worktrees off a module branch, merged together
/// and conflict-resolved, then fast-forwarded onto the default branch.
#[allow(dead_code)]
async fn run_module_inner_multibranch(
    app: &AppHandle,
    board: &tickets::BoardProject,
    module: &tickets::Module,
    name: &str,
) -> Result<String, String> {
    let repo = git::repo_root(&PathBuf::from(&board.repo_path))?;
    let base = git::default_branch(&repo)?;
    let module_branch = format!(
        "dotsquares-ai/module/{}-{}",
        git::slugify(name),
        chrono::Utc::now().timestamp()
    );

    say_module(
        app,
        &board.repo_path,
        name,
        &format!(
            "Starting {} ticket(s) on {}",
            module.tickets.len(),
            module_branch
        ),
        None,
    );
    git::create_branch_at(&repo, &module_branch, &base)?;

    // In waves rather than a rolling pool: a wave finishing before the next
    // starts keeps the number of siblings editing the same files at once down,
    // and each one is a checkout with its own dependencies to install.
    let mut landed: Vec<(&tickets::Ticket, String)> = Vec::new();
    for wave in module.tickets.chunks(PARALLEL_AGENTS) {
        let running = wave
            .iter()
            .map(|ticket| run_ticket_in_worktree(app, board, &module_branch, ticket));
        let results = futures::future::join_all(running).await;
        for (ticket, branch) in wave.iter().zip(results) {
            if let Some(branch) = branch {
                landed.push((ticket, branch));
            }
        }
    }

    if landed.is_empty() {
        return Err(format!("`{}` produced no work", name));
    }

    // One at a time, in the main checkout: two merges at once would fight over
    // the index, and a conflict has to be settled before the next one starts.
    let mut merged = 0usize;
    let mut conflicted = Vec::new();
    for (ticket, branch) in &landed {
        match git::try_merge(&repo, &module_branch, branch)? {
            git::MergeAttempt::Merged(_) => merged += 1,
            git::MergeAttempt::Conflicted => {
                say_module(
                    app,
                    &board.repo_path,
                    name,
                    &format!(
                        "`{}` conflicts with its siblings; asking {} to resolve it",
                        branch, ticket.title
                    ),
                    None,
                );
                match resolve_conflict(app, board, &repo, ticket).await {
                    Ok(true) => {
                        merged += 1;
                        say_module(
                            app,
                            &board.repo_path,
                            name,
                            &format!("Resolved the conflict on `{}`", branch),
                            None,
                        );
                    }
                    Ok(false) => conflicted.push(branch.clone()),
                    Err(e) => {
                        return Err(format!(
                            "resolving the conflict on `{}` went wrong: {}",
                            branch, e
                        ))
                    }
                }
            }
        }
    }

    if !conflicted.is_empty() {
        say_module(
            app,
            &board.repo_path,
            name,
            &format!(
                "{} ticket(s) could not be resolved and were left on their own branches: {}",
                conflicted.len(),
                conflicted.join(", ")
            ),
            None,
        );
    }

    // ---- Verify the module as a whole ------------------------------------
    let Some(test_command) = board.test_command.as_deref() else {
        return Ok(format!(
            "{} of {} ticket(s) merged into {}. No test command, so nothing was verified \
             and it was not landed.",
            merged,
            landed.len(),
            module_branch
        ));
    };

    say_module(
        app,
        &board.repo_path,
        name,
        "Running the module's tests",
        None,
    );
    let (_never_cancelled, cancel) = watch::channel(false);
    let result = run_tests(&repo, test_command, &cancel).await?;

    if !result.passed {
        return Err(format!(
            "`{}` does not pass `{}`. The work is on {} - it was not landed.",
            name, test_command, module_branch
        ));
    }

    if !board.merge_to_main {
        return Ok(format!(
            "{} ticket(s) merged into {} and passing. Merging to {} is switched off, so it \
             was left there.",
            merged, module_branch, base
        ));
    }

    git::merge_fast_forward(&repo, &base, &module_branch)?;
    Ok(format!(
        "{} ticket(s) landed on {} and passing.",
        merged, base
    ))
}

/// Start the next queued ticket, when the board is set to keep going.
///
/// Only after a run that actually finished. A failed run leaves its ticket back
/// in Approved, so continuing then would pick the same ticket straight up again
/// and grind against whatever it just failed on.
fn continue_queue(app: &AppHandle, project_path: &str) -> Option<i64> {
    let attempt = || -> Result<Option<tickets::Module>, String> {
        let db = app.state::<AgentDb>();

        let (board, module) = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            let Some(board) = tickets::board_by_repo_path(&conn, project_path)? else {
                return Ok(None);
            };
            if !board.auto_approve {
                return Ok(None);
            }
            // Safe to call from anywhere because of this: work already under
            // way owns the working tree, and starting more would have them
            // fighting over the same branch and index.
            if tickets::has_ticket_in_progress(&conn, board.id)? {
                return Ok(None);
            }
            let Some(module) = tickets::take_next_module(&conn, board.id)? else {
                info!("queue for {} is empty", project_path);
                return Ok(None);
            };
            (board, module)
        };

        // Whatever stops one run here stops every one of them: a dirty tree or
        // a repository without commits has nothing to do with any ticket. A
        // queue that carried on would work through the whole board in seconds,
        // marking every ticket failed for a reason none of them caused.
        let preflight = preflight_of(project_path)?;
        if let Some(blocker) = preflight.blocker {
            info!("queue for {} is held: {}", project_path, blocker);
            return Ok(None);
        }

        // Whatever the last module left checked out, the next one is cut from
        // the default branch.
        let repo = git::repo_root(&PathBuf::from(project_path))?;
        let base = git::default_branch(&repo)?;
        git::checkout(&repo, &base)?;

        let app_bg = app.clone();
        let module_for_task = module.clone();
        tokio::spawn(async move {
            run_module(app_bg, board, module_for_task).await;
        });

        Ok(Some(module))
    };

    match attempt() {
        // The module runs in its own task; there is no single run id to hand
        // back, so the count of tickets it picked up stands in.
        Ok(Some(module)) => Some(module.tickets.len() as i64),
        Ok(None) => None,
        Err(e) => {
            // A finished module is already recorded, and a fresh import is
            // already saved; failing to start the next one loses neither.
            warn!("could not continue the queue for {}: {}", project_path, e);
            None
        }
    }
}

/// Start the queue if the board is set to run one and nothing is going.
///
/// A run finishing is not the only moment work appears: importing a sheet onto
/// an idle board leaves a queue with nobody to start it. Returns the run it
/// began, or nothing when there was nothing to do.
#[tauri::command]
pub async fn resume_queue(app: AppHandle, project_path: String) -> Result<Option<i64>, String> {
    Ok(continue_queue(&app, &project_path))
}

/// Start a run and return its id.
///
/// Split out from the command so a finished run can start the next one: by
/// then there is no request to carry the state along, only the app handle.
#[allow(clippy::too_many_arguments)]
fn begin_run(
    app: &AppHandle,
    db: &AgentDb,
    agent_id: i64,
    project_path: String,
    task: String,
    model: Option<String>,
    test_command: Option<String>,
    branch_prefix: Option<String>,
    max_iterations: Option<u32>,
    auto_push: Option<bool>,
    open_pr: Option<bool>,
    merge_to_main: Option<bool>,
    ticket_id: Option<i64>,
) -> Result<i64, String> {
    let (system_prompt, agent_model) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        ensure_tables(&conn)?;
        conn.query_row(
            "SELECT system_prompt, model FROM agents WHERE id = ?1",
            params![agent_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .map_err(|e| format!("agent {} not found: {}", agent_id, e))?
    };

    let cfg = WorkflowConfig {
        project_path: project_path.clone(),
        task: task.clone(),
        system_prompt,
        model: model.unwrap_or(agent_model),
        test_command: test_command.filter(|s| !s.trim().is_empty()),
        branch_prefix: branch_prefix.unwrap_or_else(|| "dotsquares-ai".to_string()),
        max_iterations: max_iterations
            .unwrap_or(DEFAULT_MAX_ITERATIONS)
            .clamp(1, 10),
        auto_push: auto_push.unwrap_or(false),
        open_pr: open_pr.unwrap_or(false),
        merge_to_main: merge_to_main.unwrap_or(false),
        existing_branch: None,
    };

    let run_id = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO workflow_runs (agent_id, task, project_path, model, test_command, max_iterations)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                agent_id,
                cfg.task,
                cfg.project_path,
                cfg.model,
                cfg.test_command,
                cfg.max_iterations
            ],
        )
        .map_err(|e| e.to_string())?;
        conn.last_insert_rowid()
    };

    if let Some(tid) = ticket_id {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        tickets::link_run(&conn, tid, run_id)?;
    }

    let db_path = db_path(app)?;

    // Registered before the task starts so a stop arriving immediately still
    // finds the run rather than reporting it as not running.
    let (stop_tx, stop_rx) = watch::channel(false);
    running_runs()
        .lock()
        .map_err(|e| e.to_string())?
        .insert(run_id, stop_tx);

    let app_bg = app.clone();
    let max_iterations = cfg.max_iterations;
    let queue_path = cfg.project_path.clone();
    let lone_run = cfg.existing_branch.is_none();
    tokio::spawn(async move {
        let result = run_workflow(app_bg.clone(), run_id, cfg, stop_rx).await;

        // `run_workflow` only returns an error when it gave up before the agent
        // ever ran - preflight or branching. The ticket never had a turn, so it
        // must not be charged an attempt for one.
        // Either the run gave up before the agent, or the agent never got
        // going: both are the environment's problem, and the next ticket would
        // hit them just the same.
        let blocked = match &result {
            Err(_) => true,
            Ok(outcome) => outcome.agent_never_ran,
        };

        // Dropped as soon as the run ends: a finished run is not stoppable, and
        // leaving it listed would make it look like it still were.
        if let Ok(mut runs) = running_runs().lock() {
            runs.remove(&run_id);
        }

        if let Err(e) = persist_outcome(&db_path, run_id, &result) {
            error!("failed to persist workflow outcome: {}", e);
        }
        if blocked {
            if let Err(e) = refund_attempt(&db_path, run_id) {
                warn!("could not refund the attempt for run {}: {}", run_id, e);
            }
        }

        // The board reads a ticket's column from the database, so the terminal
        // event has to come after the outcome lands. Emitting it from inside
        // the run - before the write - left the UI refreshing onto the state
        // the run had just left behind, showing work as still running.
        let (phase, message, iteration) = match &result {
            Ok(o) if o.cancelled => (
                Phase::Cancelled,
                format!(
                    "Stopped after {} iteration(s){}",
                    o.iterations_used,
                    match &o.commit_sha {
                        Some(_) => ", work committed to the branch",
                        None => ", nothing to commit",
                    }
                ),
                o.iterations_used,
            ),
            Ok(o) if o.error.is_none() && o.commit_sha.is_some() => (
                Phase::Done,
                format!(
                    "Finished on {} ({}{})",
                    o.branch.as_deref().unwrap_or("(no branch)"),
                    if o.tests_passed {
                        "tests passed"
                    } else {
                        "tests not verified"
                    },
                    if o.merged { ", merged" } else { "" }
                ),
                o.iterations_used,
            ),
            Ok(o) => (
                Phase::Failed,
                o.error
                    .clone()
                    .unwrap_or_else(|| "Run produced no commit".to_string()),
                o.iterations_used,
            ),
            Err(e) => (Phase::Failed, e.clone(), 0),
        };

        emit(
            &app_bg,
            WorkflowEvent {
                run_id,
                phase,
                iteration,
                max_iterations,
                message: message.clone(),
                ok: Some(phase == Phase::Done),
            },
        );

        notify_outcome(&app_bg, run_id, &task, phase, &message).await;

        // Only a run started on its own moves the queue on. A module's tickets
        // are finished with as a group, so chaining from each of them would
        // start the next module while its siblings were still working.
        if !blocked && lone_run && (phase == Phase::Done || phase == Phase::Failed) {
            let _ = continue_queue(&app_bg, &queue_path);
        }
    });

    Ok(run_id)
}

/// What an interrupted run reports, so it is not mistaken for a real failure.
const INTERRUPTED_MESSAGE: &str = "dotsquares-ai stopped while this run was going, so it never finished.";

/// Close out runs that were still going when dotsquares-ai last stopped.
///
/// A run lives in a task inside this process: when the process goes, so does
/// the run - but its row still says `running` and its ticket still sits in
/// progress, which blocks that board's queue for good. Nothing else notices,
/// because the run that would have reported the outcome is gone.
///
/// Called once at startup, before anything can read the board.
pub fn reconcile_interrupted_runs(conn: &Connection) -> Result<usize, String> {
    ensure_tables(conn)?;

    let interrupted: Vec<(i64, String, Option<String>)> = {
        let mut stmt = conn
            .prepare("SELECT id, project_path, branch FROM workflow_runs WHERE status = 'running'")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?;
        rows
    };

    let ids: Vec<i64> = interrupted.iter().map(|(id, _, _)| *id).collect();

    for (_, project_path, branch) in &interrupted {
        salvage_interrupted_work(project_path, branch.as_deref());
    }

    for run_id in &ids {
        conn.execute(
            "UPDATE workflow_runs SET status = 'failed', error = ?1,
             completed_at = CURRENT_TIMESTAMP WHERE id = ?2",
            params![INTERRUPTED_MESSAGE, run_id],
        )
        .map_err(|e| e.to_string())?;

        tickets::apply_run_result(conn, *run_id, false, None, None)?;
        // Being cut off by a restart is not the ticket's doing, so it keeps the
        // chance it was charged.
        tickets::refund_attempt(conn, *run_id)?;
    }

    if !ids.is_empty() {
        warn!("closed out {} interrupted run(s): {:?}", ids.len(), ids);
    }
    Ok(ids.len())
}

/// Commit whatever an interrupted run left in the working tree, on its branch.
///
/// The run would have done this itself: a commit that can be read (and
/// reverted) beats work stranded in the tree. Stranded, it also blocks every
/// run after it - the runner refuses a dirty tree - so the board stops dead
/// until somebody clears it by hand, which is what kept happening.
fn salvage_interrupted_work(project_path: &str, branch: Option<&str>) {
    let Some(branch) = branch else {
        return;
    };
    let Ok(repo) = git::repo_root(&PathBuf::from(project_path)) else {
        return;
    };

    // Only where the run left it. If someone has moved on since, whatever is in
    // their tree now is theirs and not ours to commit.
    match git::current_branch(&repo) {
        Ok(current) if current == branch => {}
        _ => return,
    }

    match git::commit_all(&repo, &format!("Interrupted run on {}", branch)) {
        Ok(Some(sha)) => info!("salvaged interrupted work as {}", sha),
        Ok(None) => {}
        Err(e) => warn!("could not salvage the interrupted work: {}", e),
    }
}

/// Take back the attempt charged to a ticket whose run never started.
fn refund_attempt(db_path: &Path, run_id: i64) -> Result<(), String> {
    let conn = rusqlite::Connection::open(db_path).map_err(|e| e.to_string())?;
    tickets::refund_attempt(&conn, run_id)
}

/// Post a run's outcome to Slack, when run notices are turned on.
///
/// One line, no thread: nothing is expected back, so unlike a question this
/// never holds anything up. Called after the outcome is persisted, for the
/// same reason the terminal event is - the message should describe the state
/// the run actually left behind.
async fn notify_outcome(app: &AppHandle, run_id: i64, task: &str, phase: Phase, message: &str) {
    let mark = match phase {
        Phase::Done => "✅",
        Phase::Cancelled => "⏹",
        _ => "❌",
    };
    slack::notify_run(
        app,
        format!(
            "{} *dotsquares-ai* · run #{} — {}\n{}",
            mark,
            run_id,
            slack::task_title(task),
            message
        ),
    )
    .await;
}

/// Write the terminal state of a run back to the database.
fn persist_outcome(
    db_path: &Path,
    run_id: i64,
    result: &Result<WorkflowOutcome, String>,
) -> Result<(), String> {
    let conn = rusqlite::Connection::open(db_path).map_err(|e| e.to_string())?;
    ensure_tables(&conn)?;

    match result {
        Ok(o) => {
            let status = if o.cancelled {
                "cancelled"
            } else if o.error.is_none() && o.commit_sha.is_some() {
                "completed"
            } else {
                "failed"
            };
            conn.execute(
                "UPDATE workflow_runs SET branch = ?1, base_branch = ?2, base_sha = ?3,
                 status = ?4, iterations_used = ?5, tests_passed = ?6, commit_sha = ?7,
                 pushed = ?8, pr_url = ?9, error = ?10, completed_at = CURRENT_TIMESTAMP
                 WHERE id = ?11",
                params![
                    o.branch,
                    o.base_branch,
                    o.base_sha,
                    status,
                    o.iterations_used,
                    o.tests_passed as i32,
                    o.commit_sha,
                    o.pushed as i32,
                    o.pr_url,
                    o.error,
                    run_id
                ],
            )
            .map_err(|e| e.to_string())?;

            tickets::apply_run_result(
                &conn,
                run_id,
                status == "completed",
                o.branch.as_deref(),
                o.pr_url.as_deref(),
            )?;
        }
        Err(e) => {
            conn.execute(
                "UPDATE workflow_runs SET status = 'failed', error = ?1,
                 completed_at = CURRENT_TIMESTAMP WHERE id = ?2",
                params![e, run_id],
            )
            .map_err(|e| e.to_string())?;

            tickets::apply_run_result(&conn, run_id, false, None, None)?;
        }
    }

    Ok(())
}

/// The change set a run has produced so far.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunDiff {
    pub run_id: i64,
    pub branch: Option<String>,
    pub base_sha: Option<String>,
    /// Per-file summary from `git diff --stat`.
    pub stat: String,
    pub patch: String,
    /// True when the patch was too large to send in full.
    pub truncated: bool,
}

/// Diff of everything a run has changed, for the preview pane.
///
/// A run still in flight is compared against the working tree, so edits show up
/// before they are committed. A finished run is compared commit to commit,
/// which keeps the answer stable no matter what is checked out now.
#[tauri::command]
pub async fn workflow_run_diff(db: State<'_, AgentDb>, run_id: i64) -> Result<RunDiff, String> {
    let (project_path, branch, base_sha, status) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        ensure_tables(&conn)?;
        conn.query_row(
            "SELECT project_path, branch, base_sha, status FROM workflow_runs WHERE id = ?1",
            params![run_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .map_err(|e| format!("run {} not found: {}", run_id, e))?
    };

    // Before the branch phase there is no baseline to compare against; an empty
    // diff is the honest answer rather than an error.
    let Some(base_sha) = base_sha else {
        return Ok(RunDiff {
            run_id,
            branch,
            base_sha: None,
            stat: String::new(),
            patch: String::new(),
            truncated: false,
        });
    };

    let repo = git::repo_root(&PathBuf::from(&project_path))?;
    let head = if status == "running" {
        None
    } else {
        branch.as_deref()
    };
    let diff = git::diff(&repo, &base_sha, head)?;

    Ok(RunDiff {
        run_id,
        branch,
        base_sha: Some(base_sha),
        stat: diff.stat,
        patch: diff.patch,
        truncated: diff.truncated,
    })
}

/// Map one `workflow_runs` row to the shape the UI reads.
fn run_row(row: &rusqlite::Row) -> rusqlite::Result<serde_json::Value> {
    Ok(serde_json::json!({
        "id": row.get::<_, i64>(0)?,
        "agent_id": row.get::<_, i64>(1)?,
        "task": row.get::<_, String>(2)?,
        "project_path": row.get::<_, String>(3)?,
        "branch": row.get::<_, Option<String>>(4)?,
        "base_branch": row.get::<_, Option<String>>(5)?,
        "status": row.get::<_, String>(6)?,
        "iterations_used": row.get::<_, i64>(7)?,
        "max_iterations": row.get::<_, i64>(8)?,
        "tests_passed": row.get::<_, i64>(9)? != 0,
        "commit_sha": row.get::<_, Option<String>>(10)?,
        "pushed": row.get::<_, i64>(11)? != 0,
        "pr_url": row.get::<_, Option<String>>(12)?,
        "error": row.get::<_, Option<String>>(13)?,
        "created_at": row.get::<_, String>(14)?,
        "completed_at": row.get::<_, Option<String>>(15)?,
        "phase": row.get::<_, Option<String>>(16)?,
        "phase_message": row.get::<_, Option<String>>(17)?,
    }))
}

const RUN_COLUMNS: &str = "id, agent_id, task, project_path, branch, base_branch, status,
     iterations_used, max_iterations, tests_passed, commit_sha, pushed,
     pr_url, error, created_at, completed_at, phase, phase_message";

/// One run's persisted state.
///
/// Progress events only exist while a run is in flight, so without this a
/// finished or failed run reads as a blank panel - the reason it failed having
/// scrolled past before anyone opened it.
#[tauri::command]
pub async fn workflow_run(
    db: State<'_, AgentDb>,
    run_id: i64,
) -> Result<serde_json::Value, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    ensure_tables(&conn)?;
    let sql = format!("SELECT {} FROM workflow_runs WHERE id = ?1", RUN_COLUMNS);
    conn.query_row(&sql, params![run_id], run_row)
        .map_err(|e| format!("run {} not found: {}", run_id, e))
}

/// Lines of agent output a run's transcript may hand back.
///
/// A long run can produce a great deal; the tail is what someone reading a
/// finished run is looking for.
const MAX_OUTPUT_LINES: usize = 4000;

/// A run's recorded agent output, oldest first.
#[tauri::command]
pub async fn workflow_run_output(app: AppHandle, run_id: i64) -> Result<Vec<String>, String> {
    let path = output_path(&app, run_id)?;
    if !path.exists() {
        return Ok(Vec::new());
    }

    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let lines: Vec<String> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect();

    let start = lines.len().saturating_sub(MAX_OUTPUT_LINES);
    Ok(lines[start..].to_vec())
}

/// Stop a run that is still going.
///
/// The run tears down its own child processes and commits whatever the agent
/// finished, so stopping costs the attempt but not the work.
#[tauri::command]
pub async fn cancel_workflow(app: AppHandle, run_id: i64) -> Result<(), String> {
    let sender = running_runs()
        .lock()
        .map_err(|e| e.to_string())?
        .get(&run_id)
        .cloned();

    if let Some(tx) = sender {
        let _ = tx.send(true);
        info!("stop requested for run {}", run_id);
        return Ok(());
    }

    // Not ours to stop, but the database may still think it is going - an
    // interrupted run leaves exactly that, and it holds up the board. Closing
    // it out here means someone can clear it without restarting the app.
    let db = app.state::<AgentDb>();
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let closed = reconcile_interrupted_runs(&conn)?;

    if closed > 0 {
        let _ = app.emit(
            "workflow-progress",
            &WorkflowEvent {
                run_id,
                phase: Phase::Failed,
                iteration: 0,
                max_iterations: 0,
                message: INTERRUPTED_MESSAGE.to_string(),
                ok: Some(false),
            },
        );
        return Ok(());
    }

    Err(format!("Run {} is not running any more.", run_id))
}

/// Add whatever is blocking a run to `.gitignore`, when it is safe to.
///
/// Only untracked paths, and only when no tracked file has been edited: build
/// output can be ignored, but someone's work in progress cannot be made to
/// disappear by a button.
#[tauri::command]
pub async fn ignore_build_output(project_path: String) -> Result<Preflight, String> {
    let repo = git::repo_root(&PathBuf::from(&project_path))?;

    let build_output: Vec<String> = git::untracked_paths(&repo)?
        .into_iter()
        .filter(|path| git::is_build_output(path))
        .collect();

    if build_output.is_empty() {
        return Err(
            "Nothing here is build output. What is in the way is either edits to tracked \
             files or new files the agent wrote, and neither should be ignored."
                .to_string(),
        );
    }

    git::ignore_paths(&repo, &build_output)?;
    preflight_of(&project_path)
}

/// Set a project's working tree aside so a run can start.
///
/// Only ever reached by someone pressing the button: the runner refuses a
/// dirty tree rather than tidying it, and that stays true.
#[tauri::command]
pub async fn stash_project_changes(project_path: String) -> Result<Preflight, String> {
    let repo = git::repo_root(&PathBuf::from(&project_path))?;
    git::stash_working_tree(&repo)?;
    workflow_preflight(project_path).await
}

/// Whether a project can host a run at all, and why not when it cannot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preflight {
    pub is_git_repo: bool,
    /// Whether the repository has anything to branch from. A repository that
    /// has only been initialised has not.
    pub has_commits: bool,
    pub is_dirty: bool,
    /// Paths in the way that are build output, and so safe to ignore. The
    /// agent's own new files are untracked too, and ignoring those would keep
    /// real work out of the repository.
    pub ignorable: Vec<String>,
    /// The reason a run would be refused, ready to show as-is.
    pub blocker: Option<String>,
}

/// Check the two conditions that make the runner refuse before it starts.
///
/// Both are discovered during preflight anyway; surfacing them up front means
/// someone finds out while setting a board up rather than by watching a run
/// fail on its first phase.
#[tauri::command]
pub async fn workflow_preflight(project_path: String) -> Result<Preflight, String> {
    preflight_of(&project_path)
}

/// List a handful of paths by name, then say how many are left.
///
/// The whole list of a `node_modules` would be unreadable and would say no
/// more than the first few already do.
fn name_a_few(paths: &[String]) -> String {
    const SHOWN: usize = 4;

    let shown = paths
        .iter()
        .take(SHOWN)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    match paths.len().saturating_sub(SHOWN) {
        0 => shown,
        rest => format!("{} and {} more", shown, rest),
    }
}

/// The same check, callable without an await - the queue needs it before it
/// starts anything.
fn preflight_of(project_path: &str) -> Result<Preflight, String> {
    let path = PathBuf::from(project_path);

    if !git::is_git_repo(&path) {
        return Ok(Preflight {
            is_git_repo: false,
            has_commits: false,
            is_dirty: false,
            ignorable: Vec::new(),
            blocker: Some(format!(
                "{} is not a git repository. Runs need one to branch from - run `git init` there, \
                 or point the board at a checkout.",
                project_path
            )),
        });
    }

    let repo = git::repo_root(&path)?;

    // Checked before anything else that resolves HEAD: on a repository with no
    // commits those all fail, and their raw git errors say nothing useful.
    if !git::has_commits(&repo) {
        return Ok(Preflight {
            is_git_repo: true,
            has_commits: false,
            is_dirty: false,
            ignorable: Vec::new(),
            blocker: Some(
                "This repository has no commits yet. A run branches from HEAD, so it needs \
                 at least one."
                    .to_string(),
            ),
        });
    }

    let dirty = git::dirty_paths(&repo)?;
    let ignorable: Vec<String> = git::untracked_paths(&repo)?
        .into_iter()
        .filter(|path| git::is_build_output(path))
        .collect();

    Ok(Preflight {
        is_git_repo: true,
        has_commits: true,
        is_dirty: !dirty.is_empty(),
        ignorable,
        blocker: (!dirty.is_empty()).then(|| {
            format!(
                "Working tree has uncommitted changes: {}. Commit or stash them before \
                 starting a run - and if they are build output, add them to .gitignore \
                 instead, or every run will be refused here.",
                name_a_few(&dirty)
            )
        }),
    })
}

/// Make a project runnable by turning it into a repository.
///
/// Returns the fresh preflight rather than nothing, so the caller shows the
/// state that now holds instead of assuming the blocker cleared.
#[tauri::command]
pub async fn init_project_repository(project_path: String) -> Result<Preflight, String> {
    git::init_repository(&PathBuf::from(&project_path))?;
    workflow_preflight(project_path).await
}

/// List past workflow runs, newest first.
#[tauri::command]
pub async fn list_workflow_runs(
    db: State<'_, AgentDb>,
    agent_id: Option<i64>,
) -> Result<Vec<serde_json::Value>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    ensure_tables(&conn)?;

    let sql = format!("SELECT {} FROM workflow_runs", RUN_COLUMNS);
    let mut stmt = conn
        .prepare(&match agent_id {
            Some(_) => format!("{} WHERE agent_id = ?1 ORDER BY id DESC", sql),
            None => format!("{} ORDER BY id DESC", sql),
        })
        .map_err(|e| e.to_string())?;

    let map = run_row;

    let rows: Vec<serde_json::Value> = match agent_id {
        Some(id) => stmt
            .query_map(params![id], map)
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<_>>()
            .map_err(|e| e.to_string())?,
        None => stmt
            .query_map([], map)
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<_>>()
            .map_err(|e| e.to_string())?,
    };

    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_lines_keeps_the_end() {
        let text = (1..=10)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let tail = tail_lines(&text, 3);
        assert!(tail.contains("8\n9\n10"), "got: {tail}");
        assert!(tail.contains("7 earlier lines omitted"), "got: {tail}");
        assert!(!tail.contains("\n1\n"), "got: {tail}");
    }

    #[test]
    fn tail_lines_passes_short_text_through() {
        assert_eq!(tail_lines("a\nb", 10), "a\nb");
    }

    #[test]
    fn retry_prompt_forbids_gutting_tests() {
        let p = build_retry_prompt("bun test", "FAIL foo.test.ts");
        assert!(p.contains("bun test"));
        assert!(p.contains("FAIL foo.test.ts"));
        assert!(p.contains("Do not modify or delete tests"));
    }

    #[test]
    fn initial_prompt_asks_for_tests_when_verified() {
        let cfg = WorkflowConfig {
            project_path: "/tmp".into(),
            task: "Add login".into(),
            system_prompt: "".into(),
            model: "sonnet".into(),
            test_command: Some("bun test".into()),
            branch_prefix: "dotsquares-ai".into(),
            max_iterations: 3,
            auto_push: true,
            open_pr: true,
            merge_to_main: false,
            existing_branch: None,
        };
        let p = build_initial_prompt(&cfg);
        assert!(p.contains("Add login"));
        assert!(p.contains("add or update automated"));
        assert!(p.contains("bun test"));

        let bare = WorkflowConfig {
            test_command: None,
            ..cfg
        };
        let bare_prompt = build_initial_prompt(&bare);
        assert!(bare_prompt.starts_with("Add login"));
        // Nothing about verification when there is none to do.
        assert!(!bare_prompt.contains("bun test"));
    }

    /// A database with a board, a ticket and a run part-way through it.
    fn interrupted_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        ensure_tables(&conn).unwrap();
        tickets::ensure_tables(&conn).unwrap();

        conn.execute(
            "INSERT INTO board_projects (id, name, repo_path) VALUES (1, 'Demo', '/tmp/demo')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tickets (id, project_id, title, priority, status)
             VALUES (1, 1, 'Work', 'medium', 'approved')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO workflow_runs (id, agent_id, task, project_path, model, max_iterations)
             VALUES (7, 1, 'Work', '/tmp/demo', 'sonnet', 3)",
            [],
        )
        .unwrap();
        tickets::link_run(&conn, 1, 7).unwrap();
        conn
    }

    fn ticket_state(conn: &Connection) -> (String, i64) {
        conn.query_row(
            "SELECT status, attempts FROM tickets WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    }

    #[test]
    fn an_interrupted_run_stops_holding_its_ticket() {
        let conn = interrupted_db();
        assert_eq!(ticket_state(&conn), ("in_progress".to_string(), 1));

        assert_eq!(reconcile_interrupted_runs(&conn).unwrap(), 1);

        // Left as it was, the ticket sits in progress for ever and the board's
        // queue never starts anything again.
        let (status, attempts) = ticket_state(&conn);
        assert_eq!(status, "approved");
        // Being cut off by a restart is not the ticket's doing.
        assert_eq!(attempts, 0, "the attempt should have been given back");

        let (run_status, error): (String, Option<String>) = conn
            .query_row(
                "SELECT status, error FROM workflow_runs WHERE id = 7",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(run_status, "failed");
        assert_eq!(error.as_deref(), Some(INTERRUPTED_MESSAGE));
    }

    #[test]
    fn reconciling_twice_changes_nothing_the_second_time() {
        let conn = interrupted_db();
        reconcile_interrupted_runs(&conn).unwrap();

        // Startup runs this every time; a finished run must not be touched
        // again, least of all have another attempt handed back.
        assert_eq!(reconcile_interrupted_runs(&conn).unwrap(), 0);
        assert_eq!(ticket_state(&conn), ("approved".to_string(), 0));
    }

    #[test]
    fn a_long_list_of_dirty_paths_is_cut_short() {
        let few: Vec<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();
        assert_eq!(name_a_few(&few), "a, b");

        let many: Vec<String> = (1..=10).map(|i| format!("file{i}")).collect();
        assert_eq!(name_a_few(&many), "file1, file2, file3, file4 and 6 more");
    }

    #[test]
    fn a_question_is_read_back_with_its_options() {
        let text = "I had a look at the schema.\n\
                    ===OPCODE-NEEDS-INPUT===\n\
                    question: Should paging happen on the server or in the browser?\n\
                    options: server | browser\n\
                    ===END===\n";

        let (question, options) = parse_question(text).expect("a question");
        assert_eq!(
            question,
            "Should paging happen on the server or in the browser?"
        );
        assert_eq!(options, vec!["server", "browser"]);
    }

    #[test]
    fn a_question_without_options_is_free_text() {
        let text = "===OPCODE-NEEDS-INPUT===\n\
                    question: What should the empty state say?\n\
                    ===END===";

        let (question, options) = parse_question(text).expect("a question");
        assert_eq!(question, "What should the empty state say?");
        assert!(options.is_empty());
    }

    #[test]
    fn ordinary_output_is_not_a_question() {
        // Prose with a question mark is the agent thinking aloud, not asking.
        assert_eq!(
            parse_question("Should I use a grid? I think so. Done."),
            None
        );
        assert_eq!(parse_question(""), None);
        // The block is there but says nothing, which is not an answerable ask.
        assert_eq!(
            parse_question("===OPCODE-NEEDS-INPUT===\nquestion:\n===END==="),
            None
        );
    }

    #[test]
    fn the_last_block_is_the_one_being_asked() {
        // An agent that quotes the format while reasoning has not asked
        // anything until it writes one at the end of its turn.
        let text = "I could ask with ===OPCODE-NEEDS-INPUT===\n\
                    question: an idea I discarded\n\
                    ===END===\n\
                    but on reflection the real question is this.\n\
                    ===OPCODE-NEEDS-INPUT===\n\
                    question: Postgres or SQLite?\n\
                    options: postgres | sqlite\n\
                    ===END===";

        let (question, _) = parse_question(text).expect("a question");
        assert_eq!(question, "Postgres or SQLite?");
    }

    #[test]
    fn an_unterminated_block_still_asks() {
        // Dropping the closing line is the likeliest way for the agent to get
        // the format slightly wrong; stranding the run over it helps nobody.
        let text = "===OPCODE-NEEDS-INPUT===\nquestion: Which database?\noptions: a | b";
        let (question, options) = parse_question(text).expect("a question");
        assert_eq!(question, "Which database?");
        assert_eq!(options, vec!["a", "b"]);
    }

    #[test]
    fn the_initial_prompt_tells_the_agent_how_to_ask() {
        let cfg = WorkflowConfig {
            project_path: "/tmp".into(),
            task: "Add login".into(),
            system_prompt: String::new(),
            model: "sonnet".into(),
            test_command: None,
            branch_prefix: "dotsquares-ai".into(),
            max_iterations: 3,
            auto_push: false,
            open_pr: false,
            merge_to_main: false,
            existing_branch: None,
        };

        let prompt = build_initial_prompt(&cfg);
        assert!(prompt.contains("Add login"));
        assert!(prompt.contains(QUESTION_START), "no contract in: {prompt}");
    }

    /// A channel for a run nobody has asked to stop.
    ///
    /// The sender is handed back rather than dropped: a dropped sender makes
    /// every `wait_for` resolve at once, which would read as a stop.
    fn never_cancelled() -> (watch::Sender<bool>, CancelRx) {
        watch::channel(false)
    }

    #[tokio::test]
    async fn run_tests_reports_pass_and_fail() {
        let dir = tempfile::tempdir().unwrap();
        let (_tx, cancel) = never_cancelled();

        let ok = run_tests(dir.path(), "exit 0", &cancel).await.unwrap();
        assert!(ok.passed);

        let bad = run_tests(dir.path(), "echo boom >&2; exit 1", &cancel)
            .await
            .unwrap();
        assert!(!bad.passed);
        assert!(bad.output.contains("boom"), "got: {}", bad.output);
    }

    #[tokio::test]
    async fn a_stopped_run_does_not_wait_for_its_test_command() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = watch::channel(false);
        tx.send(true).unwrap();

        // Without the stop this sits here for a minute, which is the whole
        // complaint: a run you cannot get out of.
        let res = timeout(
            Duration::from_secs(5),
            run_tests(dir.path(), "sleep 60", &rx),
        )
        .await
        .expect("stopping should not wait for the command")
        .unwrap();

        assert!(!res.passed);
        assert_eq!(res.output, STOPPED_MESSAGE);
    }
}
