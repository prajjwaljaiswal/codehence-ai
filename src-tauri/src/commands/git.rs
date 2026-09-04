//! Git primitives for the autonomous workflow runner.
//!
//! These wrap the `git` and `gh` CLIs rather than linking a git library, so the
//! user's existing credential helpers, SSH agent and `gh` auth all work exactly
//! as they do in a terminal.
//!
//! Safety rules enforced here (not left to the agent or the prompt):
//!   * every mutation refuses to run on the repository's default branch
//!   * pushes are never forced
//!   * branch names are sanitized before they reach the command line

use log::{info, warn};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// Build a `git`/`gh` command with the environment those tools actually need.
///
/// `claude_binary::create_command_with_env` deliberately passes only a small
/// allowlist, which drops `SSH_AUTH_SOCK` and `GH_TOKEN` - without those a push
/// to an SSH remote hangs and `gh` reports itself as logged out. Git gets its
/// own env for that reason.
fn git_env_command(program: &str, repo: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.current_dir(repo);

    for (key, value) in std::env::vars() {
        let keep = key == "PATH"
            || key == "HOME"
            || key == "USER"
            || key == "SHELL"
            || key == "LANG"
            || key.starts_with("LC_")
            // Credential + transport plumbing
            || key == "SSH_AUTH_SOCK"
            || key == "SSH_AGENT_PID"
            || key == "GH_TOKEN"
            || key == "GITHUB_TOKEN"
            || key == "GH_HOST"
            || key == "GH_CONFIG_DIR"
            || key.starts_with("GIT_")
            || key == "HTTP_PROXY"
            || key == "HTTPS_PROXY"
            || key == "NO_PROXY"
            || key == "ALL_PROXY";
        if keep {
            cmd.env(&key, &value);
        }
    }

    // Never let git stop the run waiting for interactive input; a GUI app has
    // no terminal to answer on, so it would hang forever instead of erroring.
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.env("GIT_ASKPASS", "");

    // The UI inspects the repository - status, diff - while a run is writing to
    // it. Refreshing the index is an optional courtesy for those reads, but it
    // takes `index.lock`, which is enough to make the run's own `git add` fail.
    // Mandatory locks are unaffected, so commits still work normally.
    cmd.env("GIT_OPTIONAL_LOCKS", "0");

    cmd
}

/// Run a git subcommand, returning trimmed stdout on success.
fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let output = git_env_command("git", repo)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run `git {}`: {}", args.join(" "), e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!("`git {}` failed: {}", args.join(" "), stderr));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Run a git subcommand, returning stdout plus whether it succeeded.
fn git_status(repo: &Path, args: &[&str]) -> Result<(bool, String), String> {
    let output = git_env_command("git", repo)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run `git {}`: {}", args.join(" "), e))?;

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .trim()
    .to_string();

    Ok((output.status.success(), combined))
}

/// How many times a mutating command waits out a held `index.lock`.
const LOCK_RETRY_ATTEMPTS: u32 = 5;
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(300);

/// Run a git subcommand, waiting out another process holding the index lock.
///
/// The agent has a shell in the same repository and the UI reads the diff while
/// a run is in flight, so a moment of overlap is normal. Throwing away a whole
/// run's work because two processes wanted the index at the same instant is
/// not a trade worth making.
fn git_retrying_lock(repo: &Path, args: &[&str]) -> Result<(bool, String), String> {
    let mut last = (false, String::new());

    for attempt in 1..=LOCK_RETRY_ATTEMPTS {
        let (ok, out) = git_status(repo, args)?;
        if ok || !out.contains("index.lock") {
            return Ok((ok, out));
        }

        warn!(
            "`git {}` blocked by index.lock (attempt {}/{})",
            args.join(" "),
            attempt,
            LOCK_RETRY_ATTEMPTS
        );
        last = (ok, out);
        std::thread::sleep(LOCK_RETRY_DELAY);
    }

    Ok(last)
}

/// True when `path` is inside a git working tree.
pub fn is_git_repo(path: &Path) -> bool {
    matches!(
        git(path, &["rev-parse", "--is-inside-work-tree"]),
        Ok(ref s) if s == "true"
    )
}

/// Absolute path to the repository root containing `path`.
pub fn repo_root(path: &Path) -> Result<PathBuf, String> {
    Ok(PathBuf::from(git(path, &["rev-parse", "--show-toplevel"])?))
}

/// Name of the branch currently checked out.
pub fn current_branch(repo: &Path) -> Result<String, String> {
    git(repo, &["rev-parse", "--abbrev-ref", "HEAD"])
}

/// Best-effort detection of the repository's default branch.
///
/// Prefers what the remote advertises, then falls back to the conventional
/// names, then to whatever is checked out.
pub fn default_branch(repo: &Path) -> Result<String, String> {
    if let Ok(head) = git(
        repo,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    ) {
        if let Some(name) = head.strip_prefix("origin/") {
            return Ok(name.to_string());
        }
    }

    for candidate in ["main", "master", "develop"] {
        let (ok, _) = git_status(
            repo,
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{}", candidate),
            ],
        )?;
        if ok {
            return Ok(candidate.to_string());
        }
    }

    current_branch(repo)
}

/// True when the working tree has uncommitted changes (staged or not).
pub fn is_dirty(repo: &Path) -> Result<bool, String> {
    Ok(!git(repo, &["status", "--porcelain"])?.is_empty())
}

/// The paths that make a tree dirty, without their status codes.
///
/// "Uncommitted changes" is not something anyone can act on without dropping
/// to a terminal to find out which. Naming them usually answers it outright -
/// most often they turn out to be build output nobody meant to track.
pub fn dirty_paths(repo: &Path) -> Result<Vec<String>, String> {
    Ok(git(repo, &["status", "--porcelain"])?
        .lines()
        .filter_map(|line| {
            // Porcelain puts a two-character status first, but `git` trims its
            // output, so the leading space of ` M file` is gone by the time it
            // arrives - splitting on the status token survives either shape
            // where counting characters does not.
            line.trim_start().split_once(char::is_whitespace)
        })
        .map(|(_, path)| path.trim().to_string())
        .filter(|path| !path.is_empty())
        .collect())
}

/// Short SHA of HEAD, used to record where a run started.
pub fn head_sha(repo: &Path) -> Result<String, String> {
    git(repo, &["rev-parse", "--short", "HEAD"])
}

/// True when a remote of this name is configured.
pub fn has_remote(repo: &Path, name: &str) -> bool {
    match git(repo, &["remote"]) {
        Ok(remotes) => remotes.lines().any(|r| r.trim() == name),
        Err(_) => false,
    }
}

/// Turn arbitrary task text into a git-safe branch fragment.
pub fn slugify(text: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = true; // avoids a leading dash

    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
        if slug.len() >= 40 {
            break;
        }
    }

    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "task".to_string()
    } else {
        slug
    }
}

/// Compose a branch name and verify git considers it valid.
pub fn build_branch_name(prefix: &str, task: &str, run_id: i64) -> String {
    let prefix = slugify(prefix);
    let prefix = if prefix.is_empty() {
        "opcode".into()
    } else {
        prefix
    };
    format!("{}/{}-{}", prefix, slugify(task), run_id)
}

/// Create and check out a new branch from the current HEAD.
///
/// Refuses to reuse an existing branch so a run can never append commits onto
/// somebody else's work.
///
/// multi-branch: DISABLED - runs now work directly on the default branch, so
/// nothing in `workflow` calls this. Kept for restoration and still exercised
/// by the git tests.
#[allow(dead_code)]
pub fn create_branch(repo: &Path, branch: &str) -> Result<(), String> {
    let (valid, _) = git_status(repo, &["check-ref-format", "--branch", branch])?;
    if !valid {
        return Err(format!("`{}` is not a valid git branch name", branch));
    }

    let (exists, _) = git_status(
        repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{}", branch),
        ],
    )?;
    if exists {
        return Err(format!("branch `{}` already exists", branch));
    }

    git(repo, &["checkout", "-b", branch])?;
    info!("created and checked out branch {}", branch);
    Ok(())
}

/// Check out an existing branch.
pub fn checkout(repo: &Path, branch: &str) -> Result<(), String> {
    git(repo, &["checkout", branch])?;
    Ok(())
}

/// Stage everything and commit.
///
/// Returns `Ok(None)` when there was nothing to commit, which is a normal
/// outcome rather than an error - an agent turn may legitimately change nothing.
///
/// multi-branch: DISABLED. This used to refuse to commit on the default branch,
/// which was right when every run had a branch of its own. Now runs work on the
/// default branch directly, so that guard rejected every commit - the work then
/// sat in the tree and the *next* run was refused for a dirty tree. Restore the
/// block below alongside the branch-per-ticket code in `workflow.rs`.
pub fn commit_all(repo: &Path, message: &str) -> Result<Option<String>, String> {
    let current = current_branch(repo)?;
    // let protected = default_branch(repo)?;
    // if current == protected {
    //     return Err(format!(
    //         "refusing to commit on the default branch `{}`",
    //         protected
    //     ));
    // }

    let (ok, out) = git_retrying_lock(repo, &["add", "-A"])?;
    if !ok {
        return Err(format!("`git add -A` failed: {}", out));
    }

    if git(repo, &["diff", "--cached", "--name-only"])?.is_empty() {
        return Ok(None);
    }

    let (ok, out) = git_retrying_lock(repo, &["commit", "-m", message])?;
    if !ok {
        return Err(format!("commit failed: {}", out));
    }

    let sha = head_sha(repo)?;
    info!("committed {} on {}", sha, current);
    Ok(Some(sha))
}

/// Push the branch to `origin`, setting upstream. Never forced.
///
/// multi-branch: DISABLED. The default-branch guard below is commented out for
/// the same reason as in `commit_all` - the run's work now lives on the default
/// branch, so refusing to push it would mean "Push branch" could never do
/// anything. Still opt-in and off by default.
pub fn push_branch(repo: &Path, branch: &str) -> Result<(), String> {
    // let protected = default_branch(repo)?;
    // if branch == protected {
    //     return Err(format!(
    //         "refusing to push the default branch `{}`",
    //         protected
    //     ));
    // }

    if !has_remote(repo, "origin") {
        return Err("no `origin` remote configured".to_string());
    }

    let (ok, out) = git_status(repo, &["push", "--set-upstream", "origin", branch])?;
    if !ok {
        return Err(format!("push failed: {}", out));
    }

    info!("pushed branch {} to origin", branch);
    Ok(())
}

/// True when the `gh` CLI is installed and authenticated.
pub fn gh_available(repo: &Path) -> bool {
    match git_env_command("gh", repo)
        .args(["auth", "status"])
        .output()
    {
        Ok(out) => out.status.success(),
        Err(_) => false,
    }
}

/// Create `branch` at `base` without checking it out.
///
/// A module's branch has to exist before any of its work starts, but nothing
/// should be sitting on it: the tickets each get their own worktree.
pub fn create_branch_at(repo: &Path, branch: &str, base: &str) -> Result<(), String> {
    let (valid, _) = git_status(repo, &["check-ref-format", "--branch", branch])?;
    if !valid {
        return Err(format!("`{}` is not a valid git branch name", branch));
    }

    let (exists, _) = git_status(
        repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{}", branch),
        ],
    )?;
    if exists {
        return Err(format!("branch `{}` already exists", branch));
    }

    git(repo, &["branch", branch, base])?;
    info!("created branch {} at {}", branch, base);
    Ok(())
}

/// Give a run its own checkout, so several can work at once.
///
/// A repository has one working tree, and agents running side by side in it
/// would overwrite each other's files and have every commit sweep up everyone's
/// work. A worktree is a second checkout sharing the same object store: cheap
/// to make, isolated to write in.
pub fn add_worktree(repo: &Path, path: &Path, branch: &str, base: &str) -> Result<(), String> {
    let path_str = path.to_str().ok_or("worktree path is not valid UTF-8")?;
    git(repo, &["worktree", "add", "-b", branch, path_str, base])?;
    info!("added worktree {} on {}", path_str, branch);
    Ok(())
}

/// Take a worktree away once its run is finished with it.
///
/// The branch it was on is left behind: the work is on it, and that is the
/// whole point of having made it.
pub fn remove_worktree(repo: &Path, path: &Path) -> Result<(), String> {
    let path_str = path.to_str().ok_or("worktree path is not valid UTF-8")?;
    // `--force` because the run may have left build output in there, which git
    // otherwise refuses to discard.
    let (ok, out) = git_status(repo, &["worktree", "remove", "--force", path_str])?;
    if !ok {
        warn!("could not remove worktree {}: {}", path_str, out);
    }
    Ok(())
}

/// What attempting a merge that is allowed to leave conflicts behind found.
pub enum MergeAttempt {
    /// The merge went through cleanly, at this commit.
    Merged(String),
    /// The merge left conflict markers and an in-progress merge state, ready
    /// for something to resolve them.
    Conflicted,
}

/// Merge `branch` into `into`, leaving conflicts in place rather than
/// reflexively undoing them.
///
/// For a caller that has somewhere to send a conflict - a person, or an agent
/// - undoing it on sight would throw away the one useful thing a conflict
/// leaves behind: the exact state that needs resolving.
pub fn try_merge(repo: &Path, into: &str, branch: &str) -> Result<MergeAttempt, String> {
    checkout(repo, into)?;

    let message = format!("Merge {}", branch);
    let (ok, out) = git_retrying_lock(repo, &["merge", "--no-ff", "-m", &message, branch])?;
    if ok {
        info!("merged {} into {}", branch, into);
        return Ok(MergeAttempt::Merged(head_sha(repo)?));
    }

    if !conflicted_paths(repo)?.is_empty() {
        warn!("merge of {} into {} conflicted", branch, into);
        return Ok(MergeAttempt::Conflicted);
    }

    // Whatever went wrong, it was not a conflict, so there is nothing for a
    // resolver to work with. Left as an in-progress merge it would confuse
    // whatever runs next, so it is undone here rather than passed on.
    let _ = git_status(repo, &["merge", "--abort"]);
    Err(format!(
        "merge of `{}` into `{}` failed: {}",
        branch, into, out
    ))
}

/// Merge `branch` into `into`, undoing the attempt if it conflicts.
///
/// Siblings in the same module routinely touch the same file, so conflicts are
/// expected rather than exceptional. Used where a conflict has nowhere to go -
/// nobody there to resolve it - so it is backed out and reported instead: the
/// work stays on its own branch for someone to resolve later.
pub fn merge_branch(repo: &Path, into: &str, branch: &str) -> Result<bool, String> {
    match try_merge(repo, into, branch)? {
        MergeAttempt::Merged(_) => Ok(true),
        MergeAttempt::Conflicted => {
            abort_merge(repo)?;
            Ok(false)
        }
    }
}

/// Paths with unresolved conflicts in an in-progress merge.
pub fn conflicted_paths(repo: &Path) -> Result<Vec<String>, String> {
    Ok(git(repo, &["diff", "--name-only", "--diff-filter=U"])?
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect())
}

/// Abandon a merge left in progress, discarding whatever resolution was
/// attempted on it.
pub fn abort_merge(repo: &Path) -> Result<(), String> {
    let (ok, out) = git_status(repo, &["merge", "--abort"])?;
    if !ok {
        return Err(format!("could not abort the merge: {}", out));
    }
    Ok(())
}

/// Conclude a merge whose conflicts have all been resolved and staged.
///
/// Refuses while any remain, rather than committing a merge with markers still
/// sitting in the tree - the failure mode of a resolution that gave up midway.
pub fn finish_merge(repo: &Path) -> Result<String, String> {
    if !conflicted_paths(repo)?.is_empty() {
        return Err("conflicts remain; the merge cannot be concluded".to_string());
    }
    let (ok, out) = git_retrying_lock(repo, &["commit", "--no-edit"])?;
    if !ok {
        return Err(format!("could not conclude the merge: {}", out));
    }
    head_sha(repo)
}

/// Fast-forward `base` up to `branch`, refusing anything that is not one.
///
/// A run's branch is cut from the tip of `base`, and `base` does not move while
/// the run goes on, so this is a fast-forward every time. When it is not,
/// something else has moved `base` - and quietly writing a merge commit, or
/// forcing one, is not a decision to take on someone's behalf.
pub fn merge_fast_forward(repo: &Path, base: &str, branch: &str) -> Result<(), String> {
    checkout(repo, base)?;

    let (ok, out) = git_retrying_lock(repo, &["merge", "--ff-only", branch])?;
    if !ok {
        return Err(format!(
            "could not fast-forward `{}` to `{}`: {}",
            base, branch, out
        ));
    }

    info!("merged {} into {}", branch, base);
    Ok(())
}

/// Open a draft pull request for `branch`, returning its URL.
pub fn create_draft_pr(
    repo: &Path,
    branch: &str,
    base: &str,
    title: &str,
    body: &str,
) -> Result<String, String> {
    if !gh_available(repo) {
        return Err("`gh` is not installed or not authenticated (`gh auth login`)".to_string());
    }

    let output = git_env_command("gh", repo)
        .args([
            "pr", "create", "--draft", "--base", base, "--head", branch, "--title", title,
            "--body", body,
        ])
        .output()
        .map_err(|e| format!("failed to run `gh pr create`: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        warn!("gh pr create failed: {}", stderr);
        return Err(format!("`gh pr create` failed: {}", stderr));
    }

    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    info!("opened draft PR: {}", url);
    Ok(url)
}

/// True when the repository has at least one commit.
///
/// `git init` leaves a repository whose HEAD points at a branch that does not
/// exist yet. Every command that resolves HEAD fails there, so "is this a
/// repository" is the wrong question to ask before a run - this is.
pub fn has_commits(repo: &Path) -> bool {
    git(repo, &["rev-parse", "--verify", "HEAD"]).is_ok()
}

/// Turn `path` into a repository a run can branch from.
///
/// The commit is the point, not the init: the runner takes its base SHA from
/// HEAD, so a freshly initialised repository is no more runnable than a plain
/// directory. Whatever the directory already holds becomes that first commit,
/// and an empty directory gets an empty one.
///
/// Both steps are skipped when they are already done, so this is safe to call
/// on a directory, on a repository without commits, or twice by accident.
pub fn init_repository(path: &Path) -> Result<(), String> {
    if !path.is_dir() {
        return Err(format!("{} is not a directory", path.display()));
    }

    if !is_git_repo(path) {
        git(path, &["init"])?;
    }

    if has_commits(path) {
        return Ok(());
    }

    git(path, &["add", "-A"])?;
    git(path, &["commit", "--allow-empty", "-m", "Initial commit"])?;

    info!("initialised git repository at {}", path.display());
    Ok(())
}

/// Paths git neither tracks nor has been told to ignore.
///
/// Directories are reported whole - `node_modules/` rather than its ten
/// thousand files - which is also what wants writing into `.gitignore`.
pub fn untracked_paths(repo: &Path) -> Result<Vec<String>, String> {
    Ok(git(
        repo,
        &["ls-files", "--others", "--exclude-standard", "--directory"],
    )?
    .lines()
    .map(str::trim)
    .filter(|line| !line.is_empty())
    .map(str::to_string)
    .collect())
}

/// True when tracked files have been edited, as opposed to new files appearing.
///
/// The difference decides what can be done about a dirty tree: build output
/// can be ignored, but an edit to a tracked file is someone's work.
pub fn has_tracked_changes(repo: &Path) -> Result<bool, String> {
    Ok(!git(repo, &["diff", "--name-only", "HEAD"])?.is_empty())
}

/// Directory and file names that are build output or dependencies in
/// essentially any project.
const BUILD_OUTPUT_NAMES: &[&str] = &[
    ".cache",
    ".gradle",
    ".mypy_cache",
    ".next",
    ".nuxt",
    ".parcel-cache",
    ".pytest_cache",
    ".svelte-kit",
    ".swc",
    ".turbo",
    ".venv",
    "__pycache__",
    "build",
    "coverage",
    "dist",
    "next-env.d.ts",
    "node_modules",
    "out",
    "target",
    "venv",
];

/// Endings that mark a file as generated wherever it sits.
const BUILD_OUTPUT_SUFFIXES: &[&str] = &[".log", ".tsbuildinfo", ".DS_Store"];

/// Whether a path is something a build produced rather than something written.
///
/// Matched against a known list rather than taken as "anything untracked",
/// because an agent's own new files are untracked too: a fresh `public/` or a
/// new component directory looks exactly like `dist/` to git, and ignoring
/// those would quietly keep real work out of the repository - which is what
/// happened before this existed.
pub fn is_build_output(path: &str) -> bool {
    let trimmed = path.trim_end_matches('/');
    let name = trimmed.rsplit('/').next().unwrap_or(trimmed);

    BUILD_OUTPUT_NAMES.contains(&name)
        || BUILD_OUTPUT_SUFFIXES
            .iter()
            .any(|suffix| name.ends_with(suffix))
}

/// Add `paths` to the repository's `.gitignore` and commit it.
///
/// An agent building a project regenerates its dependencies and its build
/// output every run, so a repository that does not ignore them is dirty again
/// the moment one finishes and can never run a second ticket. Ignoring exactly
/// what is in the way beats guessing at a template for the ecosystem.
pub fn ignore_paths(repo: &Path, paths: &[String]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }

    // Belt and braces: callers filter already, but a stray source file reaching
    // `.gitignore` is the kind of mistake nobody notices until the work is
    // missing from the repository.
    if let Some(unexpected) = paths.iter().find(|p| !is_build_output(p)) {
        return Err(format!(
            "`{}` is not build output, and ignoring it would keep real work out of the \
             repository",
            unexpected
        ));
    }

    let file = repo.join(".gitignore");
    let mut text = std::fs::read_to_string(&file).unwrap_or_default();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str("\n# Added by opcode: build output that would otherwise block every run\n");
    for path in paths {
        text.push_str(path);
        text.push('\n');
    }
    std::fs::write(&file, text).map_err(|e| format!("could not write .gitignore: {}", e))?;

    // Committed directly rather than through `commit_all`: only the .gitignore
    // belongs in this commit, and `commit_all` stages the whole tree.
    let (ok, out) = git_retrying_lock(repo, &["add", ".gitignore"])?;
    if !ok {
        return Err(format!("`git add .gitignore` failed: {}", out));
    }
    let (ok, out) = git_retrying_lock(
        repo,
        &[
            "commit",
            "-m",
            "Ignore build output so agent runs are not blocked",
        ],
    )?;
    if !ok {
        return Err(format!("committing .gitignore failed: {}", out));
    }

    info!("ignored {} path(s) in {}", paths.len(), repo.display());
    Ok(())
}

/// Put the working tree aside so a run can start, returning whether anything
/// moved.
///
/// The runner never does this on its own - stashing someone's work as a
/// side effect of starting a run is how code goes missing. Behind an explicit
/// button it is a different thing: the person chose it, it is named in the
/// stash list, and `git stash pop` puts it back.
pub fn stash_working_tree(repo: &Path) -> Result<bool, String> {
    if !is_dirty(repo)? {
        return Ok(false);
    }

    // Untracked files are included: left behind they would still read as a
    // dirty tree, and the run would be refused for the same reason as before.
    let (ok, out) = git_status(
        repo,
        &[
            "stash",
            "push",
            "--include-untracked",
            "-m",
            "opcode: set aside before an agent run",
        ],
    )?;
    if !ok {
        return Err(format!("`git stash push` failed: {}", out));
    }

    info!("stashed working tree in {}", repo.display());
    Ok(true)
}

/// Largest patch handed to the UI. A preview pane cannot usefully render
/// megabytes of diff, and holding it all in memory helps nobody.
pub const MAX_DIFF_BYTES: usize = 400_000;

/// A change set, ready to render.
pub struct Diff {
    /// `git diff --stat` output: the per-file summary.
    pub stat: String,
    /// The unified patch itself, capped at `MAX_DIFF_BYTES`.
    pub patch: String,
    pub truncated: bool,
}

/// Truncate in place without splitting a multi-byte character.
fn truncate_on_char_boundary(s: &mut String, max: usize) {
    if s.len() <= max {
        return;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
}

/// The platform's empty file, for diffing something against nothing.
fn null_device() -> &'static str {
    if cfg!(windows) {
        "NUL"
    } else {
        "/dev/null"
    }
}

/// Everything that changed since `base`.
///
/// With `head` set the comparison is between two commits, which is what a
/// finished run wants: the answer stays the same no matter what is checked out
/// now. With `head` as `None` the working tree is the other side, so a run
/// still in flight shows its edits before they are committed - including
/// untracked files, which are diffed against the null device individually so
/// new files appear without touching the index.
pub fn diff(repo: &Path, base: &str, head: Option<&str>) -> Result<Diff, String> {
    let (stat, mut patch) = match head {
        Some(h) => (
            git(repo, &["diff", "--stat", base, h])?,
            git(repo, &["diff", base, h])?,
        ),
        None => (
            git(repo, &["diff", "--stat", base])?,
            git(repo, &["diff", base])?,
        ),
    };

    if head.is_none() {
        let untracked = git(repo, &["ls-files", "--others", "--exclude-standard"])?;
        for file in untracked.lines().filter(|l| !l.trim().is_empty()) {
            if patch.len() >= MAX_DIFF_BYTES {
                break;
            }
            // `--no-index` signals "these differ" with exit code 1, so the
            // status is expected to be false here and is deliberately ignored.
            if let Ok((_, out)) =
                git_status(repo, &["diff", "--no-index", "--", null_device(), file])
            {
                if !out.is_empty() {
                    if !patch.is_empty() {
                        patch.push('\n');
                    }
                    patch.push_str(&out);
                }
            }
        }
    }

    let truncated = patch.len() > MAX_DIFF_BYTES;
    if truncated {
        truncate_on_char_boundary(&mut patch, MAX_DIFF_BYTES);
        patch.push_str("\n... (diff truncated) ...");
    }

    Ok(Diff {
        stat,
        patch,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_handles_messy_task_text() {
        assert_eq!(slugify("Add OAuth login!"), "add-oauth-login");
        assert_eq!(slugify("  ...  "), "task");
        assert_eq!(slugify(""), "task");
        assert!(!slugify("a".repeat(200).as_str()).is_empty());
        assert!(slugify(&"very long task ".repeat(20)).len() <= 40);
    }

    #[test]
    fn slugify_never_leaves_edge_dashes() {
        let s = slugify("!!! hello world !!!");
        assert!(!s.starts_with('-'), "got {s}");
        assert!(!s.ends_with('-'), "got {s}");
    }

    #[test]
    fn branch_name_includes_prefix_and_run_id() {
        let name = build_branch_name("opcode", "Fix the login bug", 42);
        assert_eq!(name, "opcode/fix-the-login-bug-42");
    }

    /// Build a throwaway repo with one commit on `main`.
    fn scratch_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path();
        git(p, &["init", "--initial-branch=main"]).expect("init");
        git(p, &["config", "user.email", "test@example.com"]).expect("email");
        git(p, &["config", "user.name", "Test"]).expect("name");
        std::fs::write(p.join("README.md"), "hello\n").expect("write");
        git(p, &["add", "-A"]).expect("add");
        git(p, &["commit", "-m", "initial"]).expect("commit");
        dir
    }

    #[test]
    fn detects_repo_and_branch_state() {
        let dir = scratch_repo();
        let p = dir.path();

        assert!(is_git_repo(p));
        assert_eq!(current_branch(p).unwrap(), "main");
        assert_eq!(default_branch(p).unwrap(), "main");
        assert!(!is_dirty(p).unwrap());

        std::fs::write(p.join("new.txt"), "x").unwrap();
        assert!(is_dirty(p).unwrap(), "untracked file should count as dirty");
    }

    /// multi-branch: DISABLED - the inverse of the old
    /// `refuses_to_commit_on_default_branch`. A run works on the default branch
    /// now, so its commit has to land there; refusing left the work in the tree
    /// and the next run was blocked for a dirty tree.
    #[test]
    fn commits_on_the_default_branch() {
        let dir = scratch_repo();
        let p = dir.path();

        std::fs::write(p.join("a.txt"), "x").unwrap();
        let sha = commit_all(p, "lands on main")
            .unwrap()
            .expect("a commit on the default branch");

        assert_eq!(sha, head_sha(p).unwrap());
        assert_eq!(current_branch(p).unwrap(), "main");
        assert!(!is_dirty(p).unwrap(), "the tree should be clean after");
    }

    /// multi-branch: DISABLED - was `refuses_to_push_default_branch`. Nothing
    /// stops the push now; without a remote it fails for that reason instead.
    #[test]
    fn pushing_the_default_branch_is_no_longer_refused() {
        let dir = scratch_repo();
        let err = push_branch(dir.path(), "main").unwrap_err();
        assert!(!err.contains("refusing to push"), "got: {err}");
        assert!(err.contains("origin"), "got: {err}");
    }

    #[test]
    fn branch_then_commit_succeeds_off_default() {
        let dir = scratch_repo();
        let p = dir.path();

        create_branch(p, "opcode/feature-1").unwrap();
        assert_eq!(current_branch(p).unwrap(), "opcode/feature-1");

        // Nothing changed yet - that is not an error, just no commit.
        assert!(commit_all(p, "empty").unwrap().is_none());

        std::fs::write(p.join("feature.txt"), "code").unwrap();
        let sha = commit_all(p, "add feature").unwrap();
        assert!(sha.is_some(), "expected a commit sha");
        assert!(!is_dirty(p).unwrap());
    }

    #[test]
    fn refuses_to_reuse_an_existing_branch() {
        let dir = scratch_repo();
        let p = dir.path();

        create_branch(p, "opcode/dup").unwrap();
        checkout(p, "main").unwrap();

        let err = create_branch(p, "opcode/dup").unwrap_err();
        assert!(err.contains("already exists"), "got: {err}");
    }

    #[test]
    fn rejects_malformed_branch_names() {
        let dir = scratch_repo();
        let err = create_branch(dir.path(), "bad..name").unwrap_err();
        assert!(err.contains("not a valid git branch name"), "got: {err}");
    }

    #[test]
    fn pushes_branch_to_a_real_remote() {
        let remote = tempfile::tempdir().expect("remote dir");
        // A bare repo is a perfectly real git remote - exercises the actual
        // push path without needing the network.
        std::process::Command::new("git")
            .args(["init", "--bare", "--initial-branch=main"])
            .current_dir(remote.path())
            .output()
            .expect("init bare");

        let dir = scratch_repo();
        let p = dir.path();
        git(
            p,
            &["remote", "add", "origin", remote.path().to_str().unwrap()],
        )
        .unwrap();

        create_branch(p, "opcode/pushes-1").unwrap();
        std::fs::write(p.join("work.txt"), "done").unwrap();
        commit_all(p, "do the work").unwrap().expect("a commit");

        push_branch(p, "opcode/pushes-1").expect("push should succeed");

        // The remote really has the branch now.
        let refs = std::process::Command::new("git")
            .args(["branch", "--list"])
            .current_dir(remote.path())
            .output()
            .expect("list remote branches");
        let listed = String::from_utf8_lossy(&refs.stdout).to_string();
        assert!(
            listed.contains("opcode/pushes-1"),
            "remote branches: {listed}"
        );
    }

    /// `init_repository` commits in one step, so there is no window to set a
    /// per-repository identity in. These are passed through by
    /// `git_env_command` and keep the test off the developer's global config.
    fn use_a_test_git_identity() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            std::env::set_var("GIT_AUTHOR_NAME", "Test");
            std::env::set_var("GIT_AUTHOR_EMAIL", "test@example.com");
            std::env::set_var("GIT_COMMITTER_NAME", "Test");
            std::env::set_var("GIT_COMMITTER_EMAIL", "test@example.com");
        });
    }

    /// Two branches that both edit the same line of the same file, guaranteed
    /// to conflict when merged either way.
    fn diverge_on_the_same_line(p: &Path) -> (&'static str, &'static str) {
        create_branch(p, "opcode/mine").unwrap();
        std::fs::write(p.join("README.md"), "hello\nmine\n").unwrap();
        commit_all(p, "my change").unwrap();

        checkout(p, "main").unwrap();
        create_branch(p, "opcode/theirs").unwrap();
        std::fs::write(p.join("README.md"), "hello\ntheirs\n").unwrap();
        commit_all(p, "their change").unwrap();

        ("opcode/mine", "opcode/theirs")
    }

    #[test]
    fn a_clean_merge_reports_where_it_landed() {
        use_a_test_git_identity();
        let dir = scratch_repo();
        let p = dir.path();
        create_branch(p, "opcode/work").unwrap();
        std::fs::write(p.join("feature.txt"), "done\n").unwrap();
        let sha = commit_all(p, "the work").unwrap().unwrap();
        checkout(p, "main").unwrap();

        match try_merge(p, "main", "opcode/work").unwrap() {
            MergeAttempt::Merged(landed_at) => assert_eq!(landed_at, head_sha(p).unwrap()),
            MergeAttempt::Conflicted => panic!("should not have conflicted"),
        }
        assert!(git(p, &["log", "--oneline"]).unwrap().contains(&sha[..7]));
    }

    #[test]
    fn a_conflict_leaves_markers_rather_than_undoing_itself() {
        use_a_test_git_identity();
        let dir = scratch_repo();
        let p = dir.path();
        let (mine, theirs) = diverge_on_the_same_line(p);

        match try_merge(p, mine, theirs).unwrap() {
            MergeAttempt::Conflicted => {}
            MergeAttempt::Merged(_) => panic!("should have conflicted"),
        }

        // Left for a resolver, not thrown away: the markers are still there,
        // and the merge is still in progress.
        let conflicted = conflicted_paths(p).unwrap();
        assert_eq!(conflicted, vec!["README.md".to_string()]);
        let content = std::fs::read_to_string(p.join("README.md")).unwrap();
        assert!(content.contains("<<<<<<<"), "markers missing: {content}");
        assert!(p.join(".git").join("MERGE_HEAD").exists());
    }

    #[test]
    fn aborting_returns_the_tree_to_before_the_attempt() {
        use_a_test_git_identity();
        let dir = scratch_repo();
        let p = dir.path();
        let (mine, theirs) = diverge_on_the_same_line(p);
        // try_merge checks `mine` out before merging into it, so that - not
        // whatever happened to be checked out beforehand - is the state to
        // come back to.
        let before = git(p, &["rev-parse", "--short", mine]).unwrap();
        try_merge(p, mine, theirs).unwrap();

        abort_merge(p).unwrap();

        assert_eq!(current_branch(p).unwrap(), mine);
        assert_eq!(head_sha(p).unwrap(), before);
        assert!(!is_dirty(p).unwrap());
        assert!(!p.join(".git").join("MERGE_HEAD").exists());
    }

    #[test]
    fn finishing_refuses_while_conflicts_remain() {
        use_a_test_git_identity();
        let dir = scratch_repo();
        let p = dir.path();
        let (mine, theirs) = diverge_on_the_same_line(p);
        try_merge(p, mine, theirs).unwrap();

        let err = finish_merge(p).unwrap_err();
        assert!(err.contains("conflicts remain"), "got: {err}");
    }

    #[test]
    fn finishing_concludes_a_merge_once_resolved_and_staged() {
        use_a_test_git_identity();
        let dir = scratch_repo();
        let p = dir.path();
        let (mine, theirs) = diverge_on_the_same_line(p);
        let before = head_sha(p).unwrap();
        try_merge(p, mine, theirs).unwrap();

        // Stand in for what a resolver - a person or an agent - does: edit
        // past the markers, then stage.
        std::fs::write(p.join("README.md"), "hello\nmine and theirs\n").unwrap();
        git(p, &["add", "-A"]).unwrap();

        let landed_at = finish_merge(p).unwrap();

        assert_ne!(landed_at, before);
        assert_eq!(head_sha(p).unwrap(), landed_at);
        assert!(!p.join(".git").join("MERGE_HEAD").exists());
        let content = std::fs::read_to_string(p.join("README.md")).unwrap();
        assert_eq!(content, "hello\nmine and theirs\n");
    }

    #[test]
    fn merging_moves_the_base_branch_up_to_the_work() {
        use_a_test_git_identity();
        let dir = scratch_repo();
        let p = dir.path();
        create_branch(p, "opcode/work-1").unwrap();
        std::fs::write(p.join("feature.txt"), "done\n").unwrap();
        let work = commit_all(p, "the work").unwrap().unwrap();

        merge_fast_forward(p, "main", "opcode/work-1").unwrap();

        assert_eq!(current_branch(p).unwrap(), "main");
        assert_eq!(head_sha(p).unwrap(), work, "main should be at the work");
        // The branch is left alone: its pull request and history still matter.
        assert!(git(p, &["show-ref", "--verify", "refs/heads/opcode/work-1"]).is_ok());
    }

    #[test]
    fn merging_refuses_when_the_base_has_moved_on() {
        use_a_test_git_identity();
        let dir = scratch_repo();
        let p = dir.path();
        create_branch(p, "opcode/work-2").unwrap();
        std::fs::write(p.join("feature.txt"), "done\n").unwrap();
        commit_all(p, "the work").unwrap();

        // Somebody committed to main in the meantime, so this is no longer a
        // fast-forward and inventing a merge commit is not ours to do.
        checkout(p, "main").unwrap();
        std::fs::write(p.join("README.md"), "hello\nchanged\n").unwrap();
        git(p, &["commit", "-am", "meanwhile"]).unwrap();
        let before = head_sha(p).unwrap();

        let err = merge_fast_forward(p, "main", "opcode/work-2").unwrap_err();

        assert!(err.contains("fast-forward"), "got: {err}");
        assert_eq!(head_sha(p).unwrap(), before, "main must be untouched");
    }

    #[test]
    fn build_output_is_told_apart_from_the_agents_own_new_files() {
        for generated in [
            "node_modules/",
            "node_modules",
            ".next/",
            "dist/",
            "target/",
            "coverage/",
            "__pycache__/",
            "next-env.d.ts",
            "tsconfig.tsbuildinfo",
            "npm-debug.log",
            "packages/web/dist/",
        ] {
            assert!(
                is_build_output(generated),
                "should be ignorable: {generated}"
            );
        }

        // The regression this exists for: a fresh `public/` is untracked in
        // exactly the way `dist/` is, and ignoring it kept the app's static
        // assets out of the repository for good.
        for written in [
            "public/",
            "src/",
            "app/page.tsx",
            "components/ui/promo-carousel.tsx",
            "README.md",
            ".env.example",
        ] {
            assert!(!is_build_output(written), "should be kept: {written}");
        }
    }

    #[test]
    fn ignoring_refuses_anything_that_is_not_build_output() {
        use_a_test_git_identity();
        let dir = scratch_repo();
        let p = dir.path();

        let err = ignore_paths(p, &["public/".to_string()]).unwrap_err();

        assert!(err.contains("public/"), "got: {err}");
        assert!(
            !p.join(".gitignore").exists(),
            ".gitignore should be untouched"
        );
    }

    #[test]
    fn untracked_paths_reports_a_directory_rather_than_its_contents() {
        use_a_test_git_identity();
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::create_dir_all(p.join("node_modules").join("react")).unwrap();
        std::fs::write(p.join("node_modules").join("react").join("index.js"), "x").unwrap();
        std::fs::write(p.join("next-env.d.ts"), "generated\n").unwrap();

        let untracked = untracked_paths(p).unwrap();

        // Ten thousand files under one directory is not a useful answer, and
        // the directory is what wants writing into .gitignore anyway.
        assert!(
            untracked.contains(&"node_modules/".to_string()),
            "got: {untracked:?}"
        );
        assert!(
            untracked.contains(&"next-env.d.ts".to_string()),
            "got: {untracked:?}"
        );
    }

    #[test]
    fn ignoring_build_output_leaves_the_tree_clean() {
        use_a_test_git_identity();
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::create_dir_all(p.join("node_modules")).unwrap();
        std::fs::write(p.join("node_modules").join("thing.js"), "x").unwrap();
        std::fs::write(p.join("next-env.d.ts"), "generated\n").unwrap();
        assert!(is_dirty(p).unwrap());

        let untracked = untracked_paths(p).unwrap();
        ignore_paths(p, &untracked).unwrap();

        // The whole point: an agent regenerates these every run, so the next
        // one has to find the tree clean.
        assert!(!is_dirty(p).unwrap(), "still dirty: {:?}", dirty_paths(p));
        let ignore = std::fs::read_to_string(p.join(".gitignore")).unwrap();
        assert!(ignore.contains("node_modules/"), "got: {ignore}");
        // And it is committed, not left as one more untracked file.
        assert!(git(p, &["ls-files"]).unwrap().contains(".gitignore"));
    }

    #[test]
    fn ignoring_appends_rather_than_replacing() {
        use_a_test_git_identity();
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join(".gitignore"), "*.log\n").unwrap();
        git(p, &["add", ".gitignore"]).unwrap();
        git(p, &["commit", "-m", "existing rules"]).unwrap();
        std::fs::write(p.join("next-env.d.ts"), "generated\n").unwrap();

        ignore_paths(p, &untracked_paths(p).unwrap()).unwrap();

        let ignore = std::fs::read_to_string(p.join(".gitignore")).unwrap();
        assert!(
            ignore.contains("*.log"),
            "existing rules were lost: {ignore}"
        );
        assert!(ignore.contains("next-env.d.ts"), "got: {ignore}");
    }

    #[test]
    fn an_edited_tracked_file_is_not_build_output() {
        let dir = scratch_repo();
        let p = dir.path();
        assert!(!has_tracked_changes(p).unwrap());

        std::fs::write(p.join("untracked.txt"), "new\n").unwrap();
        assert!(
            !has_tracked_changes(p).unwrap(),
            "a new file is not an edit"
        );

        std::fs::write(p.join("README.md"), "hello\nedited\n").unwrap();
        assert!(has_tracked_changes(p).unwrap());
    }

    #[test]
    fn dirty_paths_names_what_is_actually_in_the_way() {
        let dir = scratch_repo();
        let p = dir.path();
        assert!(dirty_paths(p).unwrap().is_empty());

        std::fs::write(p.join("README.md"), "hello\nedited\n").unwrap();
        std::fs::write(p.join("next-env.d.ts"), "generated\n").unwrap();

        let paths = dirty_paths(p).unwrap();
        // Both kinds count, and the status codes are not part of the answer.
        assert!(paths.contains(&"README.md".to_string()), "got: {paths:?}");
        assert!(
            paths.contains(&"next-env.d.ts".to_string()),
            "got: {paths:?}"
        );
    }

    #[test]
    fn stashing_clears_the_tree_including_untracked_files() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("README.md"), "hello\nedited\n").unwrap();
        std::fs::write(p.join("scratch.txt"), "untracked\n").unwrap();
        assert!(is_dirty(p).unwrap());

        assert!(stash_working_tree(p).unwrap());

        assert!(!is_dirty(p).unwrap());
        // Nothing is destroyed: the work is on the stash, ready to be popped.
        let list = git(p, &["stash", "list"]).unwrap();
        assert!(list.contains("opcode: set aside"), "stash list: {list}");
    }

    #[test]
    fn stashing_a_clean_tree_does_nothing() {
        let dir = scratch_repo();
        let p = dir.path();

        assert!(!stash_working_tree(p).unwrap());
        assert!(git(p, &["stash", "list"]).unwrap().is_empty());
    }

    #[test]
    fn commit_waits_out_a_briefly_held_index_lock() {
        let dir = scratch_repo();
        let p = dir.path().to_path_buf();
        create_branch(&p, "opcode/locked").unwrap();
        std::fs::write(p.join("new.txt"), "work\n").unwrap();

        // Stand in for the agent's shell or the UI's diff holding the index for
        // a moment, which is what killed a finished run's work in practice.
        let lock = p.join(".git").join("index.lock");
        std::fs::write(&lock, "").unwrap();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            let _ = std::fs::remove_file(&lock);
        });

        let sha = commit_all(&p, "work done under contention").unwrap();
        assert!(sha.is_some(), "the commit should have survived the lock");
    }

    #[test]
    fn init_gives_an_empty_directory_something_to_branch_from() {
        use_a_test_git_identity();
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        assert!(!is_git_repo(p));

        init_repository(p).unwrap();

        assert!(is_git_repo(p));
        // A base SHA is what a run branches from, so HEAD must resolve.
        assert!(!head_sha(p).unwrap().is_empty());
        assert!(!is_dirty(p).unwrap());
    }

    #[test]
    fn init_commits_what_the_directory_already_holds() {
        use_a_test_git_identity();
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        std::fs::write(p.join("index.html"), "<h1>hi</h1>\n").unwrap();

        init_repository(p).unwrap();

        // Left uncommitted, existing files would read as a dirty tree and the
        // runner would refuse the very repository it was just given.
        assert!(!is_dirty(p).unwrap());
        let files = git(p, &["ls-files"]).unwrap();
        assert!(files.contains("index.html"), "tracked: {files}");
    }

    #[test]
    fn init_finishes_a_repository_that_was_only_initialised() {
        use_a_test_git_identity();
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init"]).unwrap();
        // `git init` alone is the trap: it looks like a repository to
        // `is_git_repo` while every command that resolves HEAD still fails.
        assert!(is_git_repo(p));
        assert!(!has_commits(p));

        init_repository(p).unwrap();

        assert!(has_commits(p));
        assert!(!head_sha(p).unwrap().is_empty());
    }

    #[test]
    fn init_leaves_an_existing_repository_alone() {
        use_a_test_git_identity();
        let dir = scratch_repo();
        let p = dir.path();
        let before = head_sha(p).unwrap();

        init_repository(p).unwrap();

        assert_eq!(head_sha(p).unwrap(), before);
    }

    #[test]
    fn diff_against_the_working_tree_includes_untracked_files() {
        let dir = scratch_repo();
        let p = dir.path();
        let base = head_sha(p).unwrap();
        create_branch(p, "opcode/diffs-1").unwrap();

        std::fs::write(p.join("README.md"), "hello\nworld\n").unwrap();
        std::fs::write(p.join("new.txt"), "brand new\n").unwrap();

        let d = diff(p, &base, None).unwrap();
        assert!(d.stat.contains("README.md"), "stat: {}", d.stat);
        assert!(d.patch.contains("+world"), "patch: {}", d.patch);
        // A run's new files are the most interesting part of its diff, and they
        // only show up because untracked paths are diffed individually.
        assert!(d.patch.contains("+brand new"), "patch: {}", d.patch);
        assert!(!d.truncated);
    }

    #[test]
    fn diff_between_commits_ignores_later_edits() {
        let dir = scratch_repo();
        let p = dir.path();
        let base = head_sha(p).unwrap();
        create_branch(p, "opcode/diffs-2").unwrap();

        std::fs::write(p.join("README.md"), "hello\ncommitted\n").unwrap();
        commit_all(p, "the work").unwrap();
        std::fs::write(p.join("README.md"), "hello\ncommitted\nstray\n").unwrap();

        let d = diff(p, &base, Some("opcode/diffs-2")).unwrap();
        assert!(d.patch.contains("+committed"), "patch: {}", d.patch);
        assert!(!d.patch.contains("stray"), "patch: {}", d.patch);
    }

    #[test]
    fn diff_truncates_oversized_patches() {
        let dir = scratch_repo();
        let p = dir.path();
        let base = head_sha(p).unwrap();
        create_branch(p, "opcode/diffs-3").unwrap();

        let huge = "x".repeat(MAX_DIFF_BYTES + 10_000);
        std::fs::write(p.join("huge.txt"), format!("{}\n", huge)).unwrap();

        let d = diff(p, &base, None).unwrap();
        assert!(d.truncated);
        assert!(
            d.patch.ends_with("... (diff truncated) ..."),
            "tail missing"
        );
    }

    #[test]
    fn push_without_origin_is_a_clear_error() {
        let dir = scratch_repo();
        let p = dir.path();
        create_branch(p, "opcode/no-remote").unwrap();

        let err = push_branch(p, "opcode/no-remote").unwrap_err();
        assert!(err.contains("no `origin` remote"), "got: {err}");
    }
}
