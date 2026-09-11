//! Slack bridge for agent questions and run outcomes.
//!
//! A question goes out as a channel message and the answer comes back as a
//! threaded reply, so a waiting run can be unblocked from a phone without the
//! app in front of you. Reading the reply is why this needs a bot token rather
//! than an incoming webhook: a webhook can only send.
//!
//! The channel is looked after here too - a name nobody has used yet is
//! created, and an existing public one is joined - so setting this up is a
//! token and nothing else.

use log::{debug, info, warn};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_notification::NotificationExt;
use tokio::time::Duration;

use crate::commands::agents::AgentDb;

const API: &str = "https://slack.com/api";

/// The channel dotsquares-ai posts to unless told otherwise.
const DEFAULT_CHANNEL: &str = "dotsquares-ai";

/// How often a question's thread is checked for a reply.
const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Consecutive failed reads before the thread is given up on. A flaky network
/// should not cost the answer, but nor should polling spin for the whole
/// answer timeout once Slack has stopped talking to us.
const MAX_POLL_FAILURES: u32 = 5;

/// How long any single Slack call may take.
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);

/// Pages of `conversations.list` walked while looking for the channel. A
/// workspace with more channels than this has them, and the answer is to
/// paste the channel id instead of the name.
const MAX_CHANNEL_PAGES: u32 = 10;

/// Where questions and run notices are sent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackSettings {
    pub enabled: bool,
    /// Bot token (`xoxb-...`). See `templates/slack-app-manifest.json` for the
    /// scopes, each of which is load-bearing.
    pub bot_token: String,
    /// Channel name (with or without the leading `#`) or channel id. A name
    /// that does not exist yet is created.
    pub channel: String,
    /// Also post when a run finishes or fails. Those want no answer.
    pub notify_runs: bool,
}

impl Default for SlackSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            bot_token: String::new(),
            channel: DEFAULT_CHANNEL.to_string(),
            notify_runs: true,
        }
    }
}

/// Whether a channel setting is an id rather than a name.
///
/// Slack ids for conversations start with C (public), G (private group) or D
/// (a DM), and carry no lowercase letters - which is what separates `C0A1B2C3D`
/// from a channel someone called `coding`.
fn looks_like_id(channel: &str) -> bool {
    channel.len() >= 9
        && matches!(channel.chars().next(), Some('C' | 'G' | 'D'))
        && channel
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

impl SlackSettings {
    /// The channel as configured, without the decorative `#`.
    fn channel_ref(&self) -> &str {
        self.channel.trim().trim_start_matches('#')
    }

    /// The channel as Slack will accept it at creation: lowercase, and only
    /// letters, digits, hyphens and underscores. Slack rejects anything else,
    /// and rejecting it here instead would only mean a worse error message.
    fn channel_name(&self) -> String {
        self.channel_ref()
            .to_lowercase()
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .trim_matches('-')
            .chars()
            .take(80)
            .collect()
    }

    /// How the channel reads to a person.
    pub fn channel_label(&self) -> String {
        let c = self.channel_ref();
        if looks_like_id(c) {
            c.to_string()
        } else {
            format!("#{}", c)
        }
    }

    /// Whether this is complete enough to post with.
    fn usable(&self) -> bool {
        self.enabled && !self.bot_token.trim().is_empty() && !self.channel_ref().is_empty()
    }
}

fn read(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM app_settings WHERE key = ?1",
        params![key],
        |row| row.get::<_, String>(0),
    )
    .ok()
}

fn load(conn: &Connection) -> SlackSettings {
    let mut s = SlackSettings::default();
    if let Some(v) = read(conn, "slack_enabled") {
        s.enabled = v == "true";
    }
    if let Some(v) = read(conn, "slack_bot_token") {
        s.bot_token = v;
    }
    if let Some(v) = read(conn, "slack_channel") {
        if !v.trim().is_empty() {
            s.channel = v;
        }
    }
    if let Some(v) = read(conn, "slack_notify_runs") {
        s.notify_runs = v == "true";
    }
    s
}

/// The settings, when they are complete enough to post with.
///
/// Read synchronously and handed back by value: `AgentDb`'s guard must not be
/// held across the awaits that follow.
pub fn configured(app: &AppHandle) -> Option<SlackSettings> {
    let db = app.state::<AgentDb>();
    let conn = db.0.lock().ok()?;
    let settings = load(&conn);
    drop(conn);
    settings.usable().then_some(settings)
}

#[tauri::command]
pub async fn get_slack_settings(db: State<'_, AgentDb>) -> Result<SlackSettings, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    Ok(load(&conn))
}

#[tauri::command]
pub async fn save_slack_settings(
    db: State<'_, AgentDb>,
    settings: SlackSettings,
) -> Result<(), String> {
    {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let values = [
            ("slack_enabled", settings.enabled.to_string()),
            ("slack_bot_token", settings.bot_token.trim().to_string()),
            ("slack_channel", settings.channel.trim().to_string()),
            ("slack_notify_runs", settings.notify_runs.to_string()),
        ];
        for (key, value) in values {
            conn.execute(
                "INSERT OR REPLACE INTO app_settings (key, value) VALUES (?1, ?2)",
                params![key, value],
            )
            .map_err(|e| format!("Failed to save {}: {}", key, e))?;
        }
    }

    // A new token or a new channel name means the id behind the old one is no
    // longer the answer.
    forget_resolved();

    info!(
        "Slack settings saved (enabled={}, channel={})",
        settings.enabled,
        settings.channel_label()
    );
    Ok(())
}

/// Set the channel up and post a test message, so a misconfiguration is found
/// here rather than in the middle of a run that is holding a repository.
#[tauri::command]
pub async fn test_slack_connection(db: State<'_, AgentDb>) -> Result<String, String> {
    let settings = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        load(&conn)
    };
    if settings.bot_token.trim().is_empty() {
        return Err("Add a bot token first.".to_string());
    }
    if settings.channel_ref().is_empty() {
        return Err("Name the channel to post in.".to_string());
    }

    let who = auth_test(&settings).await?;
    let posted = post(&settings, "dotsquares-ai is connected to this channel.").await?;
    // Reading the message back proves the history scope is there too - without
    // it questions would go out and no answer could ever be read.
    let history = match fetch_thread_reply(&settings, &posted.channel, &posted.ts).await {
        Replies::Refused(e) => format!("\nReplies cannot be read yet: {}", e),
        _ => String::new(),
    };

    Ok(format!(
        "Posted to {} as {}.{}",
        settings.channel_label(),
        who,
        history
    ))
}

/// Bot name behind the token.
async fn auth_test(s: &SlackSettings) -> Result<String, String> {
    let body = call(s, "auth.test", Method::Post(json!({})))
        .await
        .map_err(|f| f.message)?;
    Ok(body["user"]
        .as_str()
        .or_else(|| body["bot_id"].as_str())
        .unwrap_or("the bot")
        .to_string())
}

enum Method {
    Post(Value),
    Get(Vec<(&'static str, String)>),
}

/// A Slack call that did not work.
struct Failed {
    /// Slack's own error code, when Slack was the one refusing. `None` means
    /// the request never got an answer, which is worth retrying.
    code: Option<String>,
    /// What to show someone.
    message: String,
}

impl Failed {
    fn unreachable(message: String) -> Self {
        Self {
            code: None,
            message,
        }
    }

    fn is(&self, code: &str) -> bool {
        self.code.as_deref() == Some(code)
    }
}

/// One Slack API call, with `ok: false` turned into a sentence.
async fn call(s: &SlackSettings, method: &str, how: Method) -> Result<Value, Failed> {
    let client = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|e| Failed::unreachable(format!("Could not build an HTTP client: {}", e)))?;
    let url = format!("{}/{}", API, method);
    let request = match how {
        Method::Post(body) => client.post(&url).json(&body),
        Method::Get(query) => client.get(&url).query(&query),
    };

    let body: Value = request
        .bearer_auth(s.bot_token.trim())
        .send()
        .await
        .map_err(|e| Failed::unreachable(format!("Slack could not be reached: {}", e)))?
        .json()
        .await
        .map_err(|e| Failed::unreachable(format!("Slack sent something unreadable: {}", e)))?;

    if body["ok"].as_bool() == Some(true) {
        Ok(body)
    } else {
        Err(explain(s, &body))
    }
}

/// Turn a Slack error code into something worth showing someone.
fn explain(s: &SlackSettings, body: &Value) -> Failed {
    let code = body["error"].as_str().unwrap_or("unknown_error");
    let message = match code {
        "invalid_auth" | "not_authed" | "token_revoked" | "account_inactive" => {
            "Slack rejected the bot token.".to_string()
        }
        "channel_not_found" => format!(
            "Slack has no channel {} that this bot can see.",
            s.channel_label()
        ),
        "not_in_channel" => format!(
            "The bot is not in {} and could not join it.",
            s.channel_label()
        ),
        "missing_scope" => {
            let needed = body["needed"].as_str().unwrap_or("chat:write");
            format!(
                "The bot token is missing the {} scope - add it and reinstall the app \
                 (see templates/slack-app-manifest.json).",
                needed
            )
        }
        "restricted_action" | "user_is_restricted" => format!(
            "This workspace does not let the app create {}. Create it by hand and \
             invite the bot.",
            s.channel_label()
        ),
        "invalid_name" | "invalid_name_specials" | "invalid_name_maxlength"
        | "invalid_name_punctuation" | "invalid_name_required" => format!(
            "Slack will not accept {} as a channel name - lowercase letters, digits, \
             hyphens and underscores only.",
            s.channel_label()
        ),
        "is_archived" => format!(
            "{} is archived. Un-archive it, or point dotsquares-ai at another channel.",
            s.channel_label()
        ),
        "ratelimited" => "Slack is rate-limiting this token.".to_string(),
        other => format!("Slack refused the request: {}", other),
    };
    Failed {
        code: Some(code.to_string()),
        message,
    }
}

/// Channel ids already worked out, keyed by the setting they came from.
///
/// Finding a channel by name costs a `conversations.list` walk, and the answer
/// does not change while dotsquares-ai is running - so it is worked out once per
/// token/name and then reused. Cleared whenever the settings are saved, and
/// whenever a post suggests the id has stopped being right.
static RESOLVED: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

fn resolved() -> &'static Mutex<HashMap<String, String>> {
    RESOLVED.get_or_init(|| Mutex::new(HashMap::new()))
}

fn forget_resolved() {
    if let Ok(mut cache) = resolved().lock() {
        cache.clear();
    }
}

/// The most recent question asked in each channel, by channel id.
///
/// Only that question is allowed to claim an answer typed into the channel
/// instead of into a thread - see `fetch_answer`.
static LATEST_QUESTION: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

fn latest_question() -> &'static Mutex<HashMap<String, String>> {
    LATEST_QUESTION.get_or_init(|| Mutex::new(HashMap::new()))
}

fn remember_question(posted: &Posted) {
    if let Ok(mut latest) = latest_question().lock() {
        latest.insert(posted.channel.clone(), posted.ts.clone());
    }
}

fn is_latest_question(posted: &Posted) -> bool {
    latest_question()
        .lock()
        .ok()
        .and_then(|latest| latest.get(&posted.channel).cloned())
        .map(|ts| ts == posted.ts)
        .unwrap_or(false)
}

/// The channel id to post to, creating or joining the channel if that is what
/// it takes.
async fn channel_id(s: &SlackSettings) -> Result<String, String> {
    let key = s.channel_ref().to_string();
    if let Ok(cache) = resolved().lock() {
        if let Some(id) = cache.get(&key) {
            return Ok(id.clone());
        }
    }

    let id = ensure_channel(s).await?;
    if let Ok(mut cache) = resolved().lock() {
        cache.insert(key, id.clone());
    }
    Ok(id)
}

/// Make sure the configured channel exists, that the bot is in it, and say
/// which id it is.
async fn ensure_channel(s: &SlackSettings) -> Result<String, String> {
    let configured = s.channel_ref();

    // An id was given: nothing to look up, but the bot may still need to join.
    if looks_like_id(configured) {
        let id = configured.to_string();
        join(s, &id).await?;
        return Ok(id);
    }

    let name = s.channel_name();
    if name.is_empty() {
        return Err(format!(
            "{} is not a name Slack will accept for a channel.",
            s.channel_label()
        ));
    }

    if let Some(id) = find_by_name(s, &name).await.map_err(|f| f.message)? {
        info!("Slack channel #{} already exists as {}", name, id);
        join(s, &id).await?;
        return Ok(id);
    }

    match call(
        s,
        "conversations.create",
        Method::Post(json!({ "name": name, "is_private": false })),
    )
    .await
    {
        Ok(body) => {
            let id = body["channel"]["id"]
                .as_str()
                .ok_or_else(|| "Slack created the channel but gave it no id.".to_string())?
                .to_string();
            // Whoever creates a channel is in it, so no join is needed here.
            info!("created Slack channel #{} as {}", name, id);
            Ok(id)
        }
        // Something took the name between the search and the create - or it
        // belongs to a private or archived channel the search cannot see.
        Err(f) if f.is("name_taken") => match find_by_name(s, &name).await {
            Ok(Some(id)) => {
                join(s, &id).await?;
                Ok(id)
            }
            _ => Err(format!(
                "A channel called #{} already exists but this bot cannot see it - if it \
                 is private, invite the bot to it.",
                name
            )),
        },
        Err(f) => Err(f.message),
    }
}

/// Join a public channel. Being in it already is a success, not an error.
///
/// A private channel cannot be joined through the API at all, so that case is
/// reported as the invite it actually needs.
async fn join(s: &SlackSettings, id: &str) -> Result<(), String> {
    match call(
        s,
        "conversations.join",
        Method::Post(json!({ "channel": id })),
    )
    .await
    {
        Ok(_) => Ok(()),
        // Slack says this for a private channel, which no scope will let an app
        // walk into.
        Err(f) if f.is("method_not_supported_for_channel_type") => {
            debug!("{} is private; the bot has to be invited to it", id);
            Ok(())
        }
        Err(f) if f.is("already_in_channel") || f.is("is_archived") => Ok(()),
        Err(f) => Err(f.message),
    }
}

/// Find a channel by name, walking as many pages as it takes.
async fn find_by_name(s: &SlackSettings, name: &str) -> Result<Option<String>, Failed> {
    let mut cursor = String::new();
    for _ in 0..MAX_CHANNEL_PAGES {
        let mut query = vec![
            ("types", "public_channel,private_channel".to_string()),
            ("exclude_archived", "true".to_string()),
            ("limit", "200".to_string()),
        ];
        if !cursor.is_empty() {
            query.push(("cursor", cursor.clone()));
        }

        let body = call(s, "conversations.list", Method::Get(query)).await?;
        for channel in body["channels"].as_array().into_iter().flatten() {
            if channel["name"].as_str() == Some(name) {
                if let Some(id) = channel["id"].as_str() {
                    return Ok(Some(id.to_string()));
                }
            }
        }

        cursor = body["response_metadata"]["next_cursor"]
            .as_str()
            .unwrap_or("")
            .to_string();
        if cursor.is_empty() {
            return Ok(None);
        }
    }
    warn!(
        "gave up looking for #{} after {} pages of channels",
        name, MAX_CHANNEL_PAGES
    );
    Ok(None)
}

/// A message that went out, and the thread it opened.
#[derive(Debug, Clone)]
pub struct Posted {
    /// Channel id the message landed in. `conversations.replies` wants an id,
    /// and a thread is only addressable alongside the channel it is in.
    pub channel: String,
    pub ts: String,
}

/// Where a message goes.
#[derive(Clone, Copy)]
enum Destination<'a> {
    /// Straight into the channel.
    Channel,
    /// A reply inside this thread. `broadcast` also shows it in the channel,
    /// which an acknowledgement needs: whoever answered in the channel would
    /// never see a thread they did not open.
    Thread { ts: &'a str, broadcast: bool },
}

async fn post_once(
    s: &SlackSettings,
    channel: &str,
    text: &str,
    to: Destination<'_>,
) -> Result<Posted, Failed> {
    let mut body = json!({ "channel": channel, "text": text });
    if let Destination::Thread { ts, broadcast } = to {
        body["thread_ts"] = json!(ts);
        body["reply_broadcast"] = json!(broadcast);
    }

    let reply = call(s, "chat.postMessage", Method::Post(body)).await?;
    let ts = reply["ts"]
        .as_str()
        .ok_or_else(|| {
            Failed::unreachable("Slack accepted the message but gave it no timestamp.".to_string())
        })?
        .to_string();
    Ok(Posted {
        channel: reply["channel"].as_str().unwrap_or(channel).to_string(),
        ts,
    })
}

/// Post to the configured channel, setting it up first if it is not there yet.
async fn post(s: &SlackSettings, text: &str) -> Result<Posted, String> {
    let channel = channel_id(s).await?;
    match post_once(s, &channel, text, Destination::Channel).await {
        Ok(posted) => Ok(posted),
        // The cached id has stopped being somewhere this bot can post - the
        // channel was deleted, archived, or the bot was removed from it. Work
        // it out again from scratch and try once more; a second failure is
        // reported rather than retried.
        Err(f) if f.is("channel_not_found") || f.is("not_in_channel") || f.is("is_archived") => {
            warn!(
                "Slack channel {} needs setting up again: {}",
                channel, f.message
            );
            forget_resolved();
            let channel = channel_id(s).await?;
            post_once(s, &channel, text, Destination::Channel)
                .await
                .map_err(|f| f.message)
        }
        Err(f) => Err(f.message),
    }
}

/// What one look at a question's thread found.
enum Replies {
    /// Somebody replied.
    Answer(String),
    /// Nothing yet; worth asking again.
    Empty,
    /// Slack refused in a way repeating will not fix - bad token, missing
    /// scope, channel gone. Polling stops.
    Refused(String),
    /// The request itself failed. Worth another try.
    Unreachable(String),
}

/// Whether a message is somebody's answer rather than one of ours.
fn is_from_a_person(message: &Value) -> bool {
    // A bot_id marks the question and its acknowledgements; a subtype marks
    // joins, topic changes and the rest of the channel's furniture.
    !message["bot_id"].is_string() && !message["subtype"].is_string()
}

fn answer_text(message: &Value) -> Option<String> {
    let text = message["text"].as_str().unwrap_or("").trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Look in the question's own thread. An answer here is unambiguous.
async fn fetch_thread_reply(s: &SlackSettings, channel: &str, ts: &str) -> Replies {
    let query = vec![
        ("channel", channel.to_string()),
        ("ts", ts.to_string()),
        ("limit", "30".to_string()),
    ];
    let body = match call(s, "conversations.replies", Method::Get(query)).await {
        Ok(body) => body,
        Err(f) if f.code.is_none() => return Replies::Unreachable(f.message),
        Err(f) => return Replies::Refused(f.message),
    };

    let empty = Vec::new();
    for message in body["messages"].as_array().unwrap_or(&empty) {
        // The question itself is the first message in its own thread.
        if message["ts"].as_str() == Some(ts) {
            continue;
        }
        if !is_from_a_person(message) {
            continue;
        }
        if let Some(text) = answer_text(message) {
            return Replies::Answer(text);
        }
    }
    Replies::Empty
}

/// Look in the channel itself, for an answer typed under the question rather
/// than into its thread.
///
/// Slack only offers the thread if you go looking for it, so a plain channel
/// message is what most people send - and reading only the thread left runs
/// waiting next to an answer that was sitting right there.
async fn fetch_channel_reply(s: &SlackSettings, posted: &Posted) -> Replies {
    let query = vec![
        ("channel", posted.channel.clone()),
        // Exclusive, so the question itself is not read back as its own answer.
        ("oldest", posted.ts.clone()),
        ("inclusive", "false".to_string()),
        ("limit", "50".to_string()),
    ];
    let body = match call(s, "conversations.history", Method::Get(query)).await {
        Ok(body) => body,
        Err(f) if f.code.is_none() => return Replies::Unreachable(f.message),
        Err(f) => return Replies::Refused(f.message),
    };

    // Slack hands these back newest first, and the answer is the first thing
    // said *after* the question - so the oldest of them.
    let empty = Vec::new();
    for message in body["messages"].as_array().unwrap_or(&empty).iter().rev() {
        if message["ts"].as_str() == Some(posted.ts.as_str()) {
            continue;
        }
        if !is_from_a_person(message) {
            continue;
        }
        if let Some(text) = answer_text(message) {
            return Replies::Answer(text);
        }
    }
    Replies::Empty
}

/// One look for the answer to a question, in the thread and then the channel.
async fn fetch_answer(s: &SlackSettings, posted: &Posted) -> Replies {
    match fetch_thread_reply(s, &posted.channel, &posted.ts).await {
        Replies::Empty => {}
        found => return found,
    }

    // A message in the channel says nothing about which question it answers,
    // so only the most recent question in that channel may claim one. Without
    // this, two runs waiting at once would both take the same reply and one of
    // them would carry on with an answer meant for the other.
    if !is_latest_question(posted) {
        return Replies::Empty;
    }
    fetch_channel_reply(s, posted).await
}

/// Send a question to Slack. `Ok` carries the thread its answer will arrive in.
pub async fn ask(
    s: &SlackSettings,
    run_id: i64,
    task: &str,
    question: &str,
    options: &[String],
) -> Result<Posted, String> {
    let mut text = format!(
        "*dotsquares-ai* · run #{} — {}\nThe agent needs a decision:\n> {}\n",
        run_id,
        task_title(task),
        question.replace('\n', "\n> ")
    );
    if options.is_empty() {
        text.push_str("\nReply here with the answer — in this thread or in the channel.");
    } else {
        text.push('\n');
        for (i, option) in options.iter().enumerate() {
            text.push_str(&format!("{}. {}\n", i + 1, option));
        }
        text.push_str(
            "\nReply here with the number, or with the answer itself — in this thread \
             or in the channel.",
        );
    }

    let posted = post(s, &text).await?;
    remember_question(&posted);
    Ok(posted)
}

/// Hold until somebody answers in Slack, or until reading it stops working.
pub async fn wait_for_reply(s: &SlackSettings, posted: &Posted) -> Option<String> {
    let mut failures = 0u32;
    loop {
        tokio::time::sleep(POLL_INTERVAL).await;
        match fetch_answer(s, posted).await {
            Replies::Answer(answer) => {
                debug!("Slack answered the question on run thread {}", posted.ts);
                return Some(answer);
            }
            Replies::Empty => failures = 0,
            Replies::Refused(e) => {
                warn!("Slack will not let the question thread be read: {}", e);
                return None;
            }
            Replies::Unreachable(e) => {
                failures += 1;
                warn!(
                    "could not read the Slack thread ({}/{}): {}",
                    failures, MAX_POLL_FAILURES, e
                );
                if failures >= MAX_POLL_FAILURES {
                    return None;
                }
            }
        }
    }
}

/// Read a reply as a choice when it is one.
///
/// "2" against a two-option question means the second option, which is what
/// anyone answering from a phone will type. Anything else is taken as written.
pub fn resolve_choice(reply: &str, options: &[String]) -> String {
    let trimmed = reply.trim();
    if let Ok(n) = trimmed.trim_end_matches(['.', ')']).parse::<usize>() {
        if n >= 1 && n <= options.len() {
            return options[n - 1].clone();
        }
    }
    trimmed.to_string()
}

/// Say in the thread that the answer landed, so the person knows the run moved.
pub async fn acknowledge(s: &SlackSettings, posted: &Posted, answer: &str) {
    let text = format!("Got it — carrying on with: {}", answer);
    // Straight to the thread it belongs in, rather than through `post`: the
    // channel is plainly there, since the question is sitting in it. Broadcast,
    // because an answer given in the channel came from someone who never opened
    // the thread and would otherwise see no sign the run moved.
    let to = Destination::Thread {
        ts: &posted.ts,
        broadcast: true,
    };
    if let Err(f) = post_once(s, &posted.channel, &text, to).await {
        warn!("could not acknowledge the Slack answer: {}", f.message);
    }
}

/// Post a run notice, if run notices are wanted. Never fails a run.
pub async fn notify_run(app: &AppHandle, text: String) {
    let Some(s) = configured(app) else {
        return;
    };
    if !s.notify_runs {
        return;
    }
    if let Err(e) = post(&s, &text).await {
        warn!("could not post the run notice to Slack: {}", e);
    }
}

/// Tell the person, outside the window, that a run has stopped to ask
/// something - and where to answer it.
///
/// Sent whether or not Slack took the question: a blocked run holds a
/// repository, and it is exactly the thing nobody is looking at the app to
/// notice. `channel` is where it went, or `None` when Slack is not set up and
/// the app is the only place it can be answered.
pub fn notify_question(app: &AppHandle, run_id: i64, channel: Option<&str>) {
    let body = match channel {
        Some(channel) => format!(
            "Reply in the {} thread and the run carries on.",
            channel
        ),
        None => "Answer it in dotsquares-ai. Set up Slack in Settings → Slack and these come to \
                 you there instead."
            .to_string(),
    };

    let result = app
        .notification()
        .builder()
        .title(format!("Run #{} needs a decision", run_id))
        .body(body)
        .show();
    if let Err(e) = result {
        warn!("could not show the question notification: {}", e);
    }
}

/// First line of a task, short enough for a message header.
pub fn task_title(task: &str) -> String {
    let line = task.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let line = line.trim();
    if line.chars().count() > 80 {
        format!("{}…", line.chars().take(79).collect::<String>())
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_channel(channel: &str) -> SlackSettings {
        SlackSettings {
            channel: channel.into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_number_picks_the_option_it_names() {
        let options = vec!["Drop".to_string(), "Keep".to_string()];
        assert_eq!(resolve_choice(" 2 ", &options), "Keep");
        assert_eq!(resolve_choice("1.", &options), "Drop");
    }

    #[test]
    fn anything_else_is_the_answer_as_written() {
        let options = vec!["Drop".to_string(), "Keep".to_string()];
        assert_eq!(resolve_choice("keep it for now", &options), "keep it for now");
        // Out of range is text, not a panic.
        assert_eq!(resolve_choice("7", &options), "7");
        assert_eq!(resolve_choice("3", &[]), "3");
    }

    #[test]
    fn a_name_gets_its_hash_back_and_an_id_does_not() {
        let named = with_channel("#dotsquares-ai");
        assert_eq!(named.channel_ref(), "dotsquares-ai");
        assert_eq!(named.channel_label(), "#dotsquares-ai");

        let by_id = with_channel("C0123456789");
        assert_eq!(by_id.channel_label(), "C0123456789");
    }

    #[test]
    fn an_id_is_told_apart_from_a_name_that_starts_with_c() {
        assert!(looks_like_id("C0123456789"));
        assert!(looks_like_id("GABCDEF12"));
        // Lowercase means somebody typed a name.
        assert!(!looks_like_id("coding-help"));
        assert!(!looks_like_id("Csomething"));
        // Too short to be an id.
        assert!(!looks_like_id("C123"));
        assert!(!looks_like_id("dotsquares-ai"));
    }

    #[test]
    fn a_channel_name_is_made_legal_before_slack_sees_it() {
        assert_eq!(with_channel("dotsquares-ai").channel_name(), "dotsquares-ai");
        assert_eq!(with_channel("#Dotsquares AI").channel_name(), "dotsquares-ai");
        assert_eq!(with_channel("opcode_runs").channel_name(), "opcode_runs");
        // Leading and trailing filler is dropped rather than sent as hyphens.
        assert_eq!(with_channel("  ai!!  ").channel_name(), "ai");
        assert_eq!(with_channel("###").channel_name(), "");
        assert_eq!(with_channel(&"x".repeat(120)).channel_name().len(), 80);
    }

    #[test]
    fn settings_are_unusable_without_a_token() {
        let mut s = SlackSettings::default();
        assert!(!s.usable(), "off by default");
        s.enabled = true;
        assert!(!s.usable(), "no token");
        s.bot_token = "xoxb-test".into();
        assert!(s.usable());
        s.channel = "  ".into();
        assert!(!s.usable(), "no channel");
    }

    #[test]
    fn only_the_newest_question_may_claim_a_channel_reply() {
        // A channel of its own, so this does not fight other tests over the
        // shared map.
        let first = Posted {
            channel: "CTESTONE".into(),
            ts: "100.0".into(),
        };
        let second = Posted {
            channel: "CTESTONE".into(),
            ts: "200.0".into(),
        };

        remember_question(&first);
        assert!(is_latest_question(&first));

        remember_question(&second);
        assert!(is_latest_question(&second));
        assert!(
            !is_latest_question(&first),
            "an older question must not take a reply meant for the newer one"
        );

        // A channel nobody has asked anything in claims nothing.
        assert!(!is_latest_question(&Posted {
            channel: "CTESTTWO".into(),
            ts: "100.0".into(),
        }));
    }

    #[test]
    fn our_own_messages_are_not_read_as_answers() {
        assert!(is_from_a_person(&json!({ "text": "go with 1" })));
        assert!(!is_from_a_person(&json!({ "text": "…", "bot_id": "B123" })));
        assert!(!is_from_a_person(
            &json!({ "text": "joined", "subtype": "channel_join" })
        ));
        assert_eq!(answer_text(&json!({ "text": "  1  " })), Some("1".into()));
        assert_eq!(answer_text(&json!({ "text": "   " })), None);
        assert_eq!(answer_text(&json!({})), None);
    }

    #[test]
    fn a_long_task_is_cut_down_to_a_header() {
        assert_eq!(task_title("\n\nFix the login bug\nmore detail"), "Fix the login bug");
        let long = "x".repeat(200);
        assert_eq!(task_title(&long).chars().count(), 80);
    }
}
