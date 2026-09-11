//! Ticket board: per-project kanban tracking of implementation work.
//!
//! A ticket is the unit a human approves and an agent then executes. Approving
//! is deliberately a separate step from starting: the board is the gate where
//! someone decides an autonomous run is allowed to touch the repository.
//!
//! Tickets link to a `workflow_runs` row once started, which is where the
//! branch, commit and pull-request details come from.

use crate::commands::agents::AgentDb;
use log::info;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tauri::State;

/// The ticket board for one repository.
///
/// There is exactly one per project path, created on first use rather than by
/// hand: a board is how a project's work is tracked, not a separate thing to
/// set up and keep in sync with the project it belongs to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoardProject {
    pub id: i64,
    pub name: String,
    pub repo_path: String,
    /// Shell command an agent's work must pass before the run counts as done.
    /// Without one the runner implements and stops, verifying nothing.
    pub test_command: Option<String>,
    /// Push the run's branch to `origin` once its tests pass.
    pub auto_push: bool,
    /// Open a draft pull request for the pushed branch. Needs `auto_push`,
    /// since there is nothing to open a request against otherwise.
    pub open_pr: bool,
    /// Start the next ticket by itself once a run finishes, so a queue of work
    /// runs through without being nudged along one ticket at a time.
    pub auto_approve: bool,
    /// Fast-forward the default branch onto each ticket's work once its tests
    /// pass, so the next ticket builds on it rather than starting where this
    /// one did.
    pub merge_to_main: bool,
    pub created_at: String,
    /// Number of tickets on this board, for the tab badge.
    pub ticket_count: i64,
}

/// One unit of implementation work.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ticket {
    pub id: i64,
    pub project_id: i64,
    /// Grouping label, shown under the title (e.g. "Search & Explore Places").
    pub epic: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub priority: String,
    pub status: String,
    pub position: i64,
    /// Chosen from the task when the ticket is created, not picked by hand.
    pub agent_id: Option<i64>,
    pub workflow_run_id: Option<i64>,
    pub issue_number: Option<i64>,
    pub issue_url: Option<String>,
    pub pr_number: Option<i64>,
    pub pr_url: Option<String>,
    pub pr_state: Option<String>,
    pub branch: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub approved_at: Option<String>,
    pub completed_at: Option<String>,
    /// Runs started for this ticket. A queue gives up on a ticket that keeps
    /// failing rather than working through it forever.
    pub attempts: i64,
}

/// Per-column counts for the board header.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoardSummary {
    pub pending: i64,
    pub approved: i64,
    pub in_progress: i64,
    pub completed: i64,
    pub cancelled: i64,
    pub total: i64,
}

/// The board columns, in display order.
pub const STATUSES: &[&str] = &[
    "pending",
    "approved",
    "in_progress",
    "completed",
    "cancelled",
];

/// Valid priorities, lowest first.
pub const PRIORITIES: &[&str] = &["low", "medium", "high", "critical"];

/// Runs a queue will start for one ticket before moving on.
///
/// One retry: a run can fail for a reason that will not happen again - a
/// flaky test, a network hiccup - but a ticket the agent cannot do is not
/// worth grinding the whole board against.
pub const MAX_TICKET_ATTEMPTS: i64 = 2;

/// The order work is done in: the order it was written down.
///
/// Not priority. A plan is laid out as a sequence - set the project up, then
/// the design system, then the features that stand on it - and reordering it by
/// urgency takes tickets out of the order that makes them possible.
const QUEUE_ORDER: &str = "position, id";

/// Tickets a queue is allowed to pick up.
const QUEUE_ELIGIBLE: &str =
    "status IN ('pending', 'approved') AND agent_id IS NOT NULL AND attempts < ?2";

/// The board columns as selected, aliased `p`, with the ticket count last.
const BOARD_COLUMNS: &str = "p.id, p.name, p.repo_path, p.test_command, p.auto_push,
     p.open_pr, p.auto_approve, p.merge_to_main, p.created_at,
     (SELECT COUNT(*) FROM tickets t WHERE t.project_id = p.id)";

fn row_to_board(row: &rusqlite::Row) -> rusqlite::Result<BoardProject> {
    Ok(BoardProject {
        id: row.get(0)?,
        name: row.get(1)?,
        repo_path: row.get(2)?,
        test_command: row.get(3)?,
        auto_push: row.get::<_, i64>(4)? != 0,
        open_pr: row.get::<_, i64>(5)? != 0,
        auto_approve: row.get::<_, i64>(6)? != 0,
        merge_to_main: row.get::<_, i64>(7)? != 0,
        created_at: row.get(8)?,
        ticket_count: row.get(9)?,
    })
}

/// Name a board after the directory it tracks, the way the rest of the app
/// labels a project.
pub fn board_name_from_path(repo_path: &str) -> String {
    repo_path
        .split(['/', '\\'])
        .rev()
        .find(|seg| !seg.trim().is_empty())
        .unwrap_or(repo_path)
        .to_string()
}

/// Fetch the board for `repo_path`, creating it the first time it is asked for.
///
/// Split out from the command so the get-or-create behaviour can be tested
/// against a plain connection.
pub fn get_or_create_board(
    conn: &Connection,
    repo_path: &str,
    name: Option<&str>,
) -> Result<BoardProject, String> {
    let sql = format!(
        "SELECT {} FROM board_projects p WHERE p.repo_path = ?1 ORDER BY p.id LIMIT 1",
        BOARD_COLUMNS
    );
    if let Some(board) = conn.query_row(&sql, params![repo_path], row_to_board).ok() {
        return Ok(board);
    }

    let preferred = name
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| board_name_from_path(repo_path));

    // Names are unique, and two checkouts can easily share a directory name, so
    // the full path is the fallback - it is unique by construction.
    let mut inserted = conn.execute(
        "INSERT INTO board_projects (name, repo_path) VALUES (?1, ?2)",
        params![preferred, repo_path],
    );
    if inserted.is_err() {
        inserted = conn.execute(
            "INSERT INTO board_projects (name, repo_path) VALUES (?1, ?2)",
            params![repo_path, repo_path],
        );
    }
    inserted.map_err(|e| e.to_string())?;

    let id = conn.last_insert_rowid();
    info!("created ticket board {} for {}", id, repo_path);

    let sql = format!(
        "SELECT {} FROM board_projects p WHERE p.id = ?1",
        BOARD_COLUMNS
    );
    conn.query_row(&sql, params![id], row_to_board)
        .map_err(|e| e.to_string())
}

/// Words that mark a ticket as a particular kind of work, and who does it.
///
/// Implementing is deliberately absent. Its words - add, build, create - turn
/// up in nearly every ticket, including the ones that are plainly about
/// something else: "add tests for the search endpoint" would score two hits
/// for implementing against one for testing and route to the wrong agent.
/// Implementation is what a ticket is when nothing more particular applies, so
/// it is the fallback rather than a competitor.
///
/// Ordered most specific first, which decides a tie.
const AGENT_HINTS: &[(&str, &[&str])] = &[
    (
        "Security Scanner",
        &[
            "security",
            "vulnerability",
            "vulnerabilities",
            "exploit",
            "xss",
            "csrf",
            "injection",
            "sanitize",
            "escaping",
        ],
    ),
    (
        "Tester",
        &[
            "test",
            "tests",
            "testing",
            "coverage",
            "spec",
            "specs",
            "e2e",
            "regression",
        ],
    ),
    (
        "Documenter",
        &[
            "doc",
            "docs",
            "document",
            "documentation",
            "readme",
            "changelog",
            "guide",
            "comment",
            "comments",
        ],
    ),
    (
        "Debugger",
        &[
            "bug", "debug", "crash", "crashes", "broken", "fails", "failing", "error", "errors",
            "leak", "hang", "flaky",
        ],
    ),
    (
        "Code Reviewer",
        &[
            "review",
            "audit",
            "refactor",
            "cleanup",
            "tidy",
            "simplify",
            "duplication",
        ],
    ),
    (
        "Planner",
        &[
            "plan",
            "planning",
            "design",
            "architecture",
            "investigate",
            "research",
            "spike",
            "evaluate",
        ],
    ),
];

/// Split text into lowercased words, so `Docs:` and `docs` are the same word
/// and `documentation` does not count as a hit for `doc`.
fn words_of(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .collect()
}

/// Choose the agent whose kind of work a task most looks like.
///
/// Deliberately a table of words rather than a model call: picking an agent
/// happens every time a ticket is created, and an import of thirty rows should
/// not cost thirty round trips, take seconds, or produce a different answer
/// for the same sentence on a different day.
///
/// `agents` is what is actually installed, as `(id, name)`. A suggestion for an
/// agent nobody has is passed over for the next best, and a task that suggests
/// nothing falls back to whoever implements - which is what most tickets are.
///
/// Matching is on whole words, so `contest` is not a ticket about tests.
pub fn pick_agent(task: &str, agents: &[(i64, String)]) -> Option<i64> {
    let installed = |name: &str| -> Option<i64> {
        agents
            .iter()
            .find(|(_, n)| n.eq_ignore_ascii_case(name))
            .map(|(id, _)| *id)
    };

    let words = words_of(task);

    let mut scored: Vec<(usize, usize, &str)> = AGENT_HINTS
        .iter()
        .enumerate()
        .map(|(rank, (name, keywords))| {
            let hits = words
                .iter()
                .filter(|w| keywords.contains(&w.as_str()))
                .count();
            (hits, rank, *name)
        })
        .filter(|(hits, _, _)| *hits > 0)
        .collect();

    // Most hits wins; an equal count goes to the more specific kind, which is
    // the one listed first.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));

    scored
        .iter()
        .find_map(|(_, _, name)| installed(name))
        .or_else(|| installed("Implementer"))
        .or_else(|| agents.first().map(|(id, _)| *id))
}

/// Installed agents as `(id, name)`.
fn installed_agents(conn: &Connection) -> Result<Vec<(i64, String)>, String> {
    let mut stmt = conn
        .prepare("SELECT id, name FROM agents ORDER BY id")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

pub fn is_valid_status(s: &str) -> bool {
    STATUSES.contains(&s)
}

pub fn is_valid_priority(p: &str) -> bool {
    PRIORITIES.contains(&p)
}

/// Whether a ticket may move directly from one column to another.
///
/// The board is a workflow, not a free-for-all: work cannot jump from
/// `pending` straight to `completed` without someone approving it, which is
/// what keeps the approval gate meaningful.
pub fn can_transition(from: &str, to: &str) -> bool {
    if from == to {
        return true;
    }
    match (from, to) {
        ("pending", "approved") | ("pending", "cancelled") => true,
        ("approved", "in_progress") | ("approved", "pending") | ("approved", "cancelled") => true,
        // A run can finish, fail back to approved for another attempt, or be
        // abandoned mid-flight.
        ("in_progress", "completed")
        | ("in_progress", "approved")
        | ("in_progress", "cancelled") => true,
        // Reopening finished or abandoned work puts it back in the queue.
        ("completed", "approved") | ("cancelled", "pending") => true,
        _ => false,
    }
}

/// Create the board tables if they are absent, so an existing agents.db
/// upgrades in place.
pub fn ensure_tables(conn: &Connection) -> Result<(), String> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS board_projects (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE,
            repo_path TEXT NOT NULL,
            test_command TEXT,
            auto_push INTEGER NOT NULL DEFAULT 0,
            open_pr INTEGER NOT NULL DEFAULT 0,
            auto_approve INTEGER NOT NULL DEFAULT 0,
            merge_to_main INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        )",
        [],
    )
    .map_err(|e| e.to_string())?;

    // Boards created before test commands existed are upgraded in place. The
    // error when the column is already there is the expected steady state, so
    // it is discarded rather than reported.
    let _ = conn.execute(
        "ALTER TABLE board_projects ADD COLUMN test_command TEXT",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE board_projects ADD COLUMN auto_push INTEGER NOT NULL DEFAULT 0",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE board_projects ADD COLUMN open_pr INTEGER NOT NULL DEFAULT 0",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE board_projects ADD COLUMN auto_approve INTEGER NOT NULL DEFAULT 0",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE board_projects ADD COLUMN merge_to_main INTEGER NOT NULL DEFAULT 0",
        [],
    );

    conn.execute(
        "CREATE TABLE IF NOT EXISTS tickets (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            project_id INTEGER NOT NULL REFERENCES board_projects(id) ON DELETE CASCADE,
            epic TEXT,
            title TEXT NOT NULL,
            description TEXT,
            priority TEXT NOT NULL DEFAULT 'medium',
            status TEXT NOT NULL DEFAULT 'pending',
            position INTEGER NOT NULL DEFAULT 0,
            agent_id INTEGER,
            workflow_run_id INTEGER,
            issue_number INTEGER,
            issue_url TEXT,
            pr_number INTEGER,
            pr_url TEXT,
            pr_state TEXT,
            branch TEXT,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            approved_at TEXT,
            completed_at TEXT,
            attempts INTEGER NOT NULL DEFAULT 0
        )",
        [],
    )
    .map_err(|e| e.to_string())?;

    let _ = conn.execute(
        "ALTER TABLE tickets ADD COLUMN attempts INTEGER NOT NULL DEFAULT 0",
        [],
    );

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_tickets_project ON tickets(project_id, status)",
        [],
    )
    .map_err(|e| e.to_string())?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_tickets_run ON tickets(workflow_run_id)",
        [],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

const TICKET_COLUMNS: &str = "id, project_id, epic, title, description, priority, status,
     position, agent_id, workflow_run_id, issue_number, issue_url,
     pr_number, pr_url, pr_state, branch, created_at, updated_at, approved_at, completed_at,
     attempts";

fn row_to_ticket(row: &rusqlite::Row) -> rusqlite::Result<Ticket> {
    Ok(Ticket {
        id: row.get(0)?,
        project_id: row.get(1)?,
        epic: row.get(2)?,
        title: row.get(3)?,
        description: row.get(4)?,
        priority: row.get(5)?,
        status: row.get(6)?,
        position: row.get(7)?,
        agent_id: row.get(8)?,
        workflow_run_id: row.get(9)?,
        issue_number: row.get(10)?,
        issue_url: row.get(11)?,
        pr_number: row.get(12)?,
        pr_url: row.get(13)?,
        pr_state: row.get(14)?,
        branch: row.get(15)?,
        created_at: row.get(16)?,
        updated_at: row.get(17)?,
        approved_at: row.get(18)?,
        completed_at: row.get(19)?,
        attempts: row.get(20)?,
    })
}

// ---------------------------------------------------------------------------
// Board projects
// ---------------------------------------------------------------------------

/// The board for a project, created the first time the project is opened.
#[tauri::command]
pub async fn board_for_project(
    db: State<'_, AgentDb>,
    repo_path: String,
    name: Option<String>,
) -> Result<BoardProject, String> {
    let repo_path = repo_path.trim().to_string();
    if repo_path.is_empty() {
        return Err("A project path is required".to_string());
    }

    let conn = db.0.lock().map_err(|e| e.to_string())?;
    ensure_tables(&conn)?;
    get_or_create_board(&conn, &repo_path, name.as_deref())
}

/// Edit a board. Omitted fields are left as they are.
///
/// Exists mainly so a board created before test commands were a thing can be
/// given one without being recreated from scratch.
#[tauri::command]
pub async fn update_board_project(
    db: State<'_, AgentDb>,
    id: i64,
    name: Option<String>,
    repo_path: Option<String>,
    test_command: Option<String>,
    auto_push: Option<bool>,
    open_pr: Option<bool>,
    auto_approve: Option<bool>,
    merge_to_main: Option<bool>,
) -> Result<BoardProject, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    ensure_tables(&conn)?;

    if let Some(name) = name {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err("Project name cannot be empty".to_string());
        }
        conn.execute(
            "UPDATE board_projects SET name = ?1 WHERE id = ?2",
            params![name, id],
        )
        .map_err(|e| e.to_string())?;
    }

    if let Some(repo_path) = repo_path {
        conn.execute(
            "UPDATE board_projects SET repo_path = ?1 WHERE id = ?2",
            params![repo_path.trim(), id],
        )
        .map_err(|e| e.to_string())?;
    }

    // An empty string clears the command; `None` leaves it untouched.
    if let Some(test_command) = test_command {
        let value = Some(test_command.trim().to_string()).filter(|c| !c.is_empty());
        conn.execute(
            "UPDATE board_projects SET test_command = ?1 WHERE id = ?2",
            params![value, id],
        )
        .map_err(|e| e.to_string())?;
    }

    if let Some(auto_push) = auto_push {
        conn.execute(
            "UPDATE board_projects SET auto_push = ?1 WHERE id = ?2",
            params![auto_push as i32, id],
        )
        .map_err(|e| e.to_string())?;
    }

    // A pull request needs a pushed branch, so switching pushing off takes the
    // request with it rather than leaving a setting that can never fire.
    let open_pr = match (open_pr, auto_push) {
        (_, Some(false)) => Some(false),
        (requested, _) => requested,
    };
    if let Some(open_pr) = open_pr {
        conn.execute(
            "UPDATE board_projects SET open_pr = ?1 WHERE id = ?2",
            params![open_pr as i32, id],
        )
        .map_err(|e| e.to_string())?;
    }

    if let Some(auto_approve) = auto_approve {
        conn.execute(
            "UPDATE board_projects SET auto_approve = ?1 WHERE id = ?2",
            params![auto_approve as i32, id],
        )
        .map_err(|e| e.to_string())?;
    }

    if let Some(merge_to_main) = merge_to_main {
        conn.execute(
            "UPDATE board_projects SET merge_to_main = ?1 WHERE id = ?2",
            params![merge_to_main as i32, id],
        )
        .map_err(|e| e.to_string())?;
    }

    let sql = format!(
        "SELECT {} FROM board_projects p WHERE p.id = ?1",
        BOARD_COLUMNS
    );
    conn.query_row(&sql, params![id], row_to_board)
        .map_err(|e| format!("board {} not found: {}", id, e))
}

// ---------------------------------------------------------------------------
// Tickets
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn list_tickets(db: State<'_, AgentDb>, project_id: i64) -> Result<Vec<Ticket>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    ensure_tables(&conn)?;

    // The board reads in the order things will actually run, so what comes
    // next is visible rather than something to work out from the badges.
    let sql = format!(
        "SELECT {} FROM tickets WHERE project_id = ?1 ORDER BY {}",
        TICKET_COLUMNS, QUEUE_ORDER
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![project_id], row_to_ticket)
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;

    Ok(rows)
}

#[tauri::command]
pub async fn board_summary(
    db: State<'_, AgentDb>,
    project_id: i64,
) -> Result<BoardSummary, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    ensure_tables(&conn)?;

    let count = |status: &str| -> Result<i64, String> {
        conn.query_row(
            "SELECT COUNT(*) FROM tickets WHERE project_id = ?1 AND status = ?2",
            params![project_id, status],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())
    };

    let summary = BoardSummary {
        pending: count("pending")?,
        approved: count("approved")?,
        in_progress: count("in_progress")?,
        completed: count("completed")?,
        cancelled: count("cancelled")?,
        total: conn
            .query_row(
                "SELECT COUNT(*) FROM tickets WHERE project_id = ?1",
                params![project_id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?,
    };

    Ok(summary)
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn create_ticket(
    db: State<'_, AgentDb>,
    project_id: i64,
    title: String,
    epic: Option<String>,
    description: Option<String>,
    priority: Option<String>,
    issue_number: Option<i64>,
    issue_url: Option<String>,
) -> Result<Ticket, String> {
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err("Ticket title cannot be empty".to_string());
    }

    let priority = priority.unwrap_or_else(|| "medium".to_string());
    if !is_valid_priority(&priority) {
        return Err(format!(
            "Unknown priority `{}` (expected one of {})",
            priority,
            PRIORITIES.join(", ")
        ));
    }

    let conn = db.0.lock().map_err(|e| e.to_string())?;
    ensure_tables(&conn)?;

    let next_position: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM tickets WHERE project_id = ?1",
            params![project_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;

    // The agent follows from what the ticket asks for, so it is decided here
    // rather than being one more field to fill in.
    let agent_id = pick_agent(
        &format!("{} {}", title, description.as_deref().unwrap_or_default()),
        &installed_agents(&conn)?,
    );

    conn.execute(
        "INSERT INTO tickets (project_id, epic, title, description, priority,
         position, agent_id, issue_number, issue_url)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            project_id,
            epic,
            title,
            description,
            priority,
            next_position,
            agent_id,
            issue_number,
            issue_url
        ],
    )
    .map_err(|e| e.to_string())?;

    let id = conn.last_insert_rowid();
    let sql = format!("SELECT {} FROM tickets WHERE id = ?1", TICKET_COLUMNS);
    conn.query_row(&sql, params![id], row_to_ticket)
        .map_err(|e| e.to_string())
}

/// Move a ticket between columns, enforcing the allowed transitions.
#[tauri::command]
pub async fn set_ticket_status(
    db: State<'_, AgentDb>,
    id: i64,
    status: String,
) -> Result<Ticket, String> {
    if !is_valid_status(&status) {
        return Err(format!(
            "Unknown status `{}` (expected one of {})",
            status,
            STATUSES.join(", ")
        ));
    }

    let conn = db.0.lock().map_err(|e| e.to_string())?;
    ensure_tables(&conn)?;

    let current: String = conn
        .query_row(
            "SELECT status FROM tickets WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .map_err(|_| format!("Ticket {} not found", id))?;

    if !can_transition(&current, &status) {
        return Err(format!(
            "Cannot move a ticket from `{}` to `{}`",
            current, status
        ));
    }

    // Stamp the moment a ticket enters a milestone column, and clear it again
    // when the ticket is moved back out so the timestamps never lie.
    conn.execute(
        "UPDATE tickets SET status = ?1,
           approved_at  = CASE WHEN ?1 = 'pending'   THEN NULL
                               WHEN approved_at IS NULL THEN CURRENT_TIMESTAMP
                               ELSE approved_at END,
           completed_at = CASE WHEN ?1 = 'completed' THEN CURRENT_TIMESTAMP
                               ELSE NULL END,
           updated_at = CURRENT_TIMESTAMP
         WHERE id = ?2",
        params![status, id],
    )
    .map_err(|e| e.to_string())?;

    info!("ticket {} moved {} -> {}", id, current, status);

    let sql = format!("SELECT {} FROM tickets WHERE id = ?1", TICKET_COLUMNS);
    conn.query_row(&sql, params![id], row_to_ticket)
        .map_err(|e| e.to_string())
}

/// Edit a ticket's editable fields. `None` leaves a field unchanged.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn update_ticket(
    db: State<'_, AgentDb>,
    id: i64,
    title: Option<String>,
    epic: Option<String>,
    description: Option<String>,
    priority: Option<String>,
) -> Result<Ticket, String> {
    let text_changed = title.is_some() || description.is_some();

    if let Some(p) = &priority {
        if !is_valid_priority(p) {
            return Err(format!("Unknown priority `{}`", p));
        }
    }
    if let Some(t) = &title {
        if t.trim().is_empty() {
            return Err("Ticket title cannot be empty".to_string());
        }
    }

    let conn = db.0.lock().map_err(|e| e.to_string())?;
    ensure_tables(&conn)?;

    conn.execute(
        "UPDATE tickets SET
           title       = COALESCE(?1, title),
           epic        = COALESCE(?2, epic),
           description = COALESCE(?3, description),
           priority    = COALESCE(?4, priority),
           updated_at  = CURRENT_TIMESTAMP
         WHERE id = ?5",
        params![title, epic, description, priority, id],
    )
    .map_err(|e| e.to_string())?;

    let sql = format!("SELECT {} FROM tickets WHERE id = ?1", TICKET_COLUMNS);
    let ticket = conn
        .query_row(&sql, params![id], row_to_ticket)
        .map_err(|_| format!("Ticket {} not found", id))?;

    // Reword a ticket and it may well be different work now, so the agent is
    // chosen again rather than left pointing at what the old wording implied.
    if !text_changed {
        return Ok(ticket);
    }

    let agent_id = pick_agent(
        &format!(
            "{} {}",
            ticket.title,
            ticket.description.as_deref().unwrap_or_default()
        ),
        &installed_agents(&conn)?,
    );
    if agent_id == ticket.agent_id {
        return Ok(ticket);
    }

    conn.execute(
        "UPDATE tickets SET agent_id = ?1 WHERE id = ?2",
        params![agent_id, id],
    )
    .map_err(|e| e.to_string())?;

    Ok(Ticket { agent_id, ..ticket })
}

#[tauri::command]
pub async fn delete_ticket(db: State<'_, AgentDb>, id: i64) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    ensure_tables(&conn)?;
    conn.execute("DELETE FROM tickets WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Attach a workflow run to a ticket and move it into `in_progress`.
/// The board for a repository path, if one exists.
///
/// Used by the runner to find out what a finished run should do next, which it
/// cannot ask the UI about - by then nobody may be looking.
pub fn board_by_repo_path(
    conn: &Connection,
    repo_path: &str,
) -> Result<Option<BoardProject>, String> {
    ensure_tables(conn)?;
    let sql = format!(
        "SELECT {} FROM board_projects p WHERE p.repo_path = ?1 ORDER BY p.id LIMIT 1",
        BOARD_COLUMNS
    );
    Ok(conn.query_row(&sql, params![repo_path], row_to_board).ok())
}

/// A module of work: an epic and the tickets waiting under it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Module {
    /// The epic these tickets share. Empty for tickets that name none.
    pub name: String,
    pub tickets: Vec<Ticket>,
}

/// The module to take on next, approving its tickets on the way.
///
/// Work arrives grouped: a header, a hero and a footer are one module and are
/// worth doing together, on one branch, verified once. Which module comes first
/// is the order the plan was written in - the first ticket still waiting names
/// it.
pub fn take_next_module(conn: &Connection, project_id: i64) -> Result<Option<Module>, String> {
    ensure_tables(conn)?;

    // The epic of the first ticket still waiting: whichever module the plan
    // reaches next, rather than whichever happens to be largest or loudest.
    let name: Option<String> = conn
        .query_row(
            &format!(
                "SELECT COALESCE(epic, '') FROM tickets
                 WHERE project_id = ?1 AND {} ORDER BY {} LIMIT 1",
                QUEUE_ELIGIBLE, QUEUE_ORDER
            ),
            params![project_id, MAX_TICKET_ATTEMPTS],
            |r| r.get(0),
        )
        .ok();

    let Some(name) = name else {
        return Ok(None);
    };

    let sql = format!(
        "SELECT {} FROM tickets
         WHERE project_id = ?1 AND {} AND COALESCE(epic, '') = ?3
         ORDER BY {}",
        TICKET_COLUMNS, QUEUE_ELIGIBLE, QUEUE_ORDER
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let tickets = stmt
        .query_map(
            params![project_id, MAX_TICKET_ATTEMPTS, name],
            row_to_ticket,
        )
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;

    if tickets.is_empty() {
        return Ok(None);
    }

    for ticket in &tickets {
        if ticket.status == "pending" {
            conn.execute(
                "UPDATE tickets SET status = 'approved', approved_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
                params![ticket.id],
            )
            .map_err(|e| e.to_string())?;
        }
    }

    info!("module `{}` has {} ticket(s) waiting", name, tickets.len());

    Ok(Some(Module {
        name,
        tickets: tickets
            .into_iter()
            .map(|t| Ticket {
                status: "approved".to_string(),
                ..t
            })
            .collect(),
    }))
}

/// Whether a run is already under way on this board.
///
/// One at a time: a board has one working tree, and two runs in it would fight
/// over the branch and the index.
pub fn has_ticket_in_progress(conn: &Connection, project_id: i64) -> Result<bool, String> {
    ensure_tables(conn)?;
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tickets WHERE project_id = ?1 AND status = 'in_progress'",
            params![project_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    Ok(count > 0)
}

/// The ticket a queue should pick up next, approving it on the way if needed.
///
/// Most urgent first. Only tickets with an agent are eligible: one without
/// cannot run, and skipping it beats stalling the queue on it. Cancelled and
/// completed work is left alone, and so is anything already in progress - as
/// is a ticket that has used up its attempts, so one that cannot be done does
/// not hold up everything behind it.
pub fn take_next_queued_ticket(
    conn: &Connection,
    project_id: i64,
) -> Result<Option<Ticket>, String> {
    ensure_tables(conn)?;

    let sql = format!(
        "SELECT {} FROM tickets WHERE project_id = ?1 AND {} ORDER BY {} LIMIT 1",
        TICKET_COLUMNS, QUEUE_ELIGIBLE, QUEUE_ORDER
    );

    let Ok(ticket) = conn.query_row(
        &sql,
        params![project_id, MAX_TICKET_ATTEMPTS],
        row_to_ticket,
    ) else {
        return Ok(None);
    };

    if ticket.status == "pending" {
        conn.execute(
            "UPDATE tickets SET status = 'approved', approved_at = CURRENT_TIMESTAMP,
             updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
            params![ticket.id],
        )
        .map_err(|e| e.to_string())?;
    }

    Ok(Some(Ticket {
        status: "approved".to_string(),
        ..ticket
    }))
}

/// Take back the attempt charged to a ticket whose run never started.
///
/// Preflight refusing a run says nothing about the ticket, and counting it
/// would use up a ticket's chances on something it had no part in.
pub fn refund_attempt(conn: &Connection, run_id: i64) -> Result<(), String> {
    ensure_tables(conn)?;
    conn.execute(
        "UPDATE tickets SET attempts = MAX(attempts - 1, 0), updated_at = CURRENT_TIMESTAMP
         WHERE workflow_run_id = ?1",
        params![run_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Give the tickets a queue has passed over another go.
///
/// Returns how many were reset, so the caller can say what it did.
#[tauri::command]
pub async fn reset_skipped_tickets(
    db: State<'_, AgentDb>,
    project_id: i64,
) -> Result<usize, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    ensure_tables(&conn)?;
    let reset = conn
        .execute(
            "UPDATE tickets SET attempts = 0, updated_at = CURRENT_TIMESTAMP
             WHERE project_id = ?1 AND attempts >= ?2 AND status IN ('pending', 'approved')",
            params![project_id, MAX_TICKET_ATTEMPTS],
        )
        .map_err(|e| e.to_string())?;
    info!("reset {} skipped ticket(s) on board {}", reset, project_id);
    Ok(reset)
}

pub fn link_run(conn: &Connection, ticket_id: i64, run_id: i64) -> Result<(), String> {
    ensure_tables(conn)?;
    conn.execute(
        "UPDATE tickets SET workflow_run_id = ?1, status = 'in_progress',
         attempts = attempts + 1,
         approved_at = COALESCE(approved_at, CURRENT_TIMESTAMP),
         updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
        params![run_id, ticket_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Fold a finished workflow run back into whichever ticket started it.
///
/// A run that produced no pull request still moves the ticket on; the board
/// distinguishes "done" from "done and reviewable" through the PR badge.
pub fn apply_run_result(
    conn: &Connection,
    run_id: i64,
    succeeded: bool,
    branch: Option<&str>,
    pr_url: Option<&str>,
) -> Result<(), String> {
    ensure_tables(conn)?;

    let pr_number = pr_url.and_then(parse_pr_number);
    let status = if succeeded { "completed" } else { "approved" };

    conn.execute(
        "UPDATE tickets SET
           status       = ?1,
           branch       = COALESCE(?2, branch),
           pr_url       = COALESCE(?3, pr_url),
           pr_number    = COALESCE(?4, pr_number),
           pr_state     = CASE WHEN ?3 IS NOT NULL THEN 'open' ELSE pr_state END,
           completed_at = CASE WHEN ?1 = 'completed' THEN CURRENT_TIMESTAMP ELSE NULL END,
           updated_at   = CURRENT_TIMESTAMP
         WHERE workflow_run_id = ?5",
        params![status, branch, pr_url, pr_number, run_id],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

/// Pull the numeric id out of a GitHub pull-request URL.
pub fn parse_pr_number(url: &str) -> Option<i64> {
    let tail = url.rsplit('/').next()?;
    tail.parse::<i64>().ok()
}

// ---------------------------------------------------------------------------
// Spreadsheet import
// ---------------------------------------------------------------------------

/// The example sheet, compiled in rather than read from disk.
///
/// A packaged app has no repository next to it, so a file path would work in
/// development and break everywhere else. Building it into the binary also
/// keeps it the same file the tests check.
pub const TICKET_TEMPLATE_CSV: &str = include_str!("../../../templates/tickets-template.csv");

/// Write the example sheet somewhere the person can open and edit it.
#[tauri::command]
pub async fn save_ticket_template(file_path: String) -> Result<(), String> {
    std::fs::write(&file_path, TICKET_TEMPLATE_CSV)
        .map_err(|e| format!("Could not write {}: {}", file_path, e))
}

/// One spreadsheet row, read as the fields a ticket needs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportRow {
    /// Row number as the spreadsheet shows it, so a problem can be pointed at.
    pub row: usize,
    pub title: String,
    pub epic: Option<String>,
    pub description: Option<String>,
    pub priority: String,
    /// Why this row cannot become a ticket, when it cannot.
    pub problem: Option<String>,
}

/// What a file would import, shown before anything is created.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportPreview {
    pub sheet: String,
    /// Headings that were understood.
    pub columns: Vec<String>,
    /// Headings that were not. A misspelt one drops a whole column, so it is
    /// named rather than silently ignored.
    pub ignored_columns: Vec<String>,
    pub rows: Vec<ImportRow>,
}

/// What an import actually did.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportResult {
    pub created: usize,
    /// Rows left out, each carrying the reason.
    pub skipped: Vec<ImportRow>,
}

/// The ticket fields a column can supply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Column {
    Title,
    Epic,
    Description,
    Priority,
}

/// Reduce a heading to its letters and digits, so `Estimate (hours)`,
/// `estimate_hours` and `Estimate Hours` are one column rather than three.
fn normalise_heading(heading: &str) -> String {
    heading
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn column_for(heading: &str) -> Option<Column> {
    match normalise_heading(heading).as_str() {
        "title" | "task" | "summary" => Some(Column::Title),
        "epic" | "feature" => Some(Column::Epic),
        "description" | "details" | "notes" => Some(Column::Description),
        "priority" => Some(Column::Priority),
        _ => None,
    }
}

fn cell(row: &[String], index: Option<usize>) -> Option<String> {
    let value = index.and_then(|i| row.get(i))?.trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// Turn a sheet's cells into rows ready to become tickets.
///
/// Kept separate from reading the file so the mapping - which is where the
/// surprises live - can be tested without a spreadsheet on disk.
///
/// Returns the headings understood, those that were not, and one entry per
/// data row. A row that cannot become a ticket is returned carrying its reason
/// rather than dropped: a spreadsheet with one bad line in thirty should say
/// which line, not refuse the other twenty-nine.
pub fn rows_from_cells(cells: &[Vec<String>]) -> (Vec<String>, Vec<String>, Vec<ImportRow>) {
    let Some(header_index) = cells
        .iter()
        .position(|row| row.iter().any(|c| !c.trim().is_empty()))
    else {
        return (Vec::new(), Vec::new(), Vec::new());
    };

    let header = &cells[header_index];
    let mut mapping: Vec<(Column, usize)> = Vec::new();
    let mut understood: Vec<String> = Vec::new();
    let mut ignored: Vec<String> = Vec::new();

    for (index, heading) in header.iter().enumerate() {
        let heading = heading.trim();
        if heading.is_empty() {
            continue;
        }
        match column_for(heading) {
            Some(column) => {
                mapping.push((column, index));
                understood.push(heading.to_string());
            }
            None => ignored.push(heading.to_string()),
        }
    }

    let at = |column: Column| -> Option<usize> {
        mapping.iter().find(|(c, _)| *c == column).map(|(_, i)| *i)
    };

    let mut rows = Vec::new();

    for (offset, raw) in cells.iter().enumerate().skip(header_index + 1) {
        if raw.iter().all(|c| c.trim().is_empty()) {
            continue;
        }

        // Spreadsheets number rows from one, and so does everyone reading one.
        let row_number = offset + 1;
        let title = cell(raw, at(Column::Title)).unwrap_or_default();

        let priority_raw = cell(raw, at(Column::Priority));
        let priority = priority_raw
            .as_deref()
            .map(|p| p.trim().to_lowercase())
            .unwrap_or_else(|| "medium".to_string());

        let problem = if title.is_empty() {
            Some("No title".to_string())
        } else if !is_valid_priority(&priority) {
            Some(format!(
                "Unknown priority `{}` (expected one of {})",
                priority_raw.unwrap_or_default(),
                PRIORITIES.join(", ")
            ))
        } else {
            None
        };

        rows.push(ImportRow {
            row: row_number,
            title,
            epic: cell(raw, at(Column::Epic)),
            description: cell(raw, at(Column::Description)),
            priority,
            problem,
        });
    }

    (understood, ignored, rows)
}

/// Read a delimited text file as rows of plain strings.
///
/// `flexible` matters: a sheet exported to CSV often has a short first line or
/// a ragged tail, and refusing the whole file over a missing trailing comma
/// would be no help to anyone.
fn read_delimited(file_path: &str, delimiter: u8) -> Result<Vec<Vec<String>>, String> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter)
        .from_path(file_path)
        .map_err(|e| format!("Could not open {}: {}", file_path, e))?;

    reader
        .records()
        .map(|record| {
            record
                .map(|r| r.iter().map(str::to_string).collect())
                .map_err(|e| format!("Could not read {}: {}", file_path, e))
        })
        .collect()
}

/// Read the first sheet of a spreadsheet, or a delimited text file, as rows of
/// plain strings.
fn read_sheet(file_path: &str) -> Result<(String, Vec<Vec<String>>), String> {
    use calamine::{open_workbook_auto, Data, Reader};

    let extension = Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_lowercase();

    // A workbook has named sheets; a text file has only itself, so it is named
    // after the file rather than pretending to a sheet it does not have.
    if let Some(delimiter) = match extension.as_str() {
        "csv" => Some(b','),
        "tsv" | "tab" => Some(b'\t'),
        _ => None,
    } {
        let name = Path::new(file_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(file_path)
            .to_string();
        return Ok((name, read_delimited(file_path, delimiter)?));
    }

    let mut workbook = open_workbook_auto(file_path)
        .map_err(|e| format!("Could not open {}: {}", file_path, e))?;

    let sheet = workbook
        .sheet_names()
        .first()
        .cloned()
        .ok_or_else(|| "The file has no sheets".to_string())?;

    let range = workbook
        .worksheet_range(&sheet)
        .map_err(|e| format!("Could not read sheet `{}`: {}", sheet, e))?;

    let cells = range
        .rows()
        .map(|row| {
            row.iter()
                .map(|c| match c {
                    Data::Empty => String::new(),
                    // Whole numbers arrive as floats; `3` should not read `3.0`.
                    Data::Float(f) if f.fract() == 0.0 => format!("{}", *f as i64),
                    other => other.to_string(),
                })
                .collect()
        })
        .collect();

    Ok((sheet, cells))
}

/// What a spreadsheet would import, without importing anything.
#[tauri::command]
pub async fn preview_ticket_import(file_path: String) -> Result<ImportPreview, String> {
    let (sheet, cells) = read_sheet(&file_path)?;
    let (columns, ignored_columns, rows) = rows_from_cells(&cells);

    if !columns.iter().any(|c| column_for(c) == Some(Column::Title)) {
        return Err(
            "No `title` column found. The first non-empty row is read as headings.".to_string(),
        );
    }

    Ok(ImportPreview {
        sheet,
        columns,
        ignored_columns,
        rows,
    })
}

/// Create tickets from a spreadsheet, skipping the rows that cannot be.
///
/// The file is read again rather than taking the caller's parsed rows: what
/// gets created should be what the file says, not what a client sent back.
#[tauri::command]
pub async fn import_tickets(
    db: State<'_, AgentDb>,
    project_id: i64,
    file_path: String,
) -> Result<ImportResult, String> {
    let (_, cells) = read_sheet(&file_path)?;
    let (_, _, rows) = rows_from_cells(&cells);

    let conn = db.0.lock().map_err(|e| e.to_string())?;
    ensure_tables(&conn)?;

    // Read once for the whole sheet rather than per row: thirty tickets should
    // not be thirty identical queries.
    let agents = installed_agents(&conn)?;
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;

    let mut position: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM tickets WHERE project_id = ?1",
            params![project_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;

    let mut created = 0usize;
    let mut skipped = Vec::new();

    for row in rows {
        if row.problem.is_some() {
            skipped.push(row);
            continue;
        }

        let agent_id = pick_agent(
            &format!(
                "{} {}",
                row.title,
                row.description.as_deref().unwrap_or_default()
            ),
            &agents,
        );

        tx.execute(
            "INSERT INTO tickets (project_id, epic, title, description, priority,
             position, agent_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                project_id,
                row.epic,
                row.title,
                row.description,
                row.priority,
                position,
                agent_id
            ],
        )
        .map_err(|e| e.to_string())?;

        position += 1;
        created += 1;
    }

    tx.commit().map_err(|e| e.to_string())?;
    info!(
        "imported {} ticket(s) into project {} ({} skipped)",
        created,
        project_id,
        skipped.len()
    );

    Ok(ImportResult { created, skipped })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A board created before test commands existed must gain the column rather
    /// than leaving every query on that table broken.
    #[test]
    fn ensure_tables_upgrades_a_board_from_the_old_schema() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE board_projects (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                repo_path TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO board_projects (name, repo_path) VALUES ('Old', '/tmp/old')",
            [],
        )
        .unwrap();

        ensure_tables(&conn).expect("first upgrade");
        // Running again is the steady state and must stay quiet.
        ensure_tables(&conn).expect("second call is a no-op");

        let (command, auto_push, open_pr): (Option<String>, i64, i64) = conn
            .query_row(
                "SELECT test_command, auto_push, open_pr FROM board_projects WHERE name = 'Old'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .expect("columns exist after upgrade");
        assert_eq!(command, None);
        assert_eq!(auto_push, 0);
        assert_eq!(open_pr, 0);
    }

    #[test]
    fn board_names_come_from_the_directory() {
        assert_eq!(board_name_from_path("/Users/me/Projects/dotsquares-ai"), "dotsquares-ai");
        // A trailing separator must not name the board after nothing.
        assert_eq!(board_name_from_path("/Users/me/Projects/dotsquares-ai/"), "dotsquares-ai");
        assert_eq!(board_name_from_path("C:\\work\\app"), "app");
        assert_eq!(board_name_from_path("dotsquares-ai"), "dotsquares-ai");
    }

    #[test]
    fn a_project_gets_one_board_however_often_it_is_opened() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_tables(&conn).unwrap();

        let first = get_or_create_board(&conn, "/tmp/app", None).unwrap();
        let again = get_or_create_board(&conn, "/tmp/app", None).unwrap();
        assert_eq!(first.id, again.id);
        assert_eq!(first.name, "app");
        // Pushing to a shared remote is opt-in: a new board keeps work local.
        assert!(!first.auto_push);
        assert!(!first.open_pr);

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM board_projects", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn projects_sharing_a_directory_name_still_get_their_own_board() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_tables(&conn).unwrap();

        let one = get_or_create_board(&conn, "/tmp/one/app", None).unwrap();
        let two = get_or_create_board(&conn, "/tmp/two/app", None).unwrap();

        assert_ne!(one.id, two.id);
        assert_eq!(one.name, "app");
        // Names are unique, so the loser falls back to its path rather than
        // silently reusing the first board.
        assert_eq!(two.name, "/tmp/two/app");
    }

    /// Build sheet cells the way calamine hands them over.
    fn sheet(rows: &[&[&str]]) -> Vec<Vec<String>> {
        rows.iter()
            .map(|r| r.iter().map(|c| c.to_string()).collect())
            .collect()
    }

    #[test]
    fn headings_are_matched_however_they_are_written() {
        let cells = sheet(&[
            &["Title", "Epic", "  DETAILS  ", "Priority"],
            &["Build the grid", "Search", "Cards, not rows", "high"],
        ]);
        let (columns, ignored, rows) = rows_from_cells(&cells);

        assert_eq!(columns.len(), 4, "columns: {columns:?}");
        assert!(ignored.is_empty(), "ignored: {ignored:?}");
        assert_eq!(rows[0].title, "Build the grid");
        assert_eq!(rows[0].epic.as_deref(), Some("Search"));
        assert_eq!(rows[0].description.as_deref(), Some("Cards, not rows"));
        assert_eq!(rows[0].priority, "high");
        assert_eq!(rows[0].problem, None);
    }

    #[test]
    fn a_misspelt_heading_is_named_rather_than_dropped_in_silence() {
        let cells = sheet(&[&["Title", "Prioriti"], &["Ship it", "high"]]);
        let (_, ignored, rows) = rows_from_cells(&cells);

        assert_eq!(ignored, vec!["Prioriti"]);
        // The column was not read, so the row falls back to the default rather
        // than quietly taking the value from a heading nobody understood.
        assert_eq!(rows[0].priority, "medium");
    }

    #[test]
    fn bad_rows_carry_their_reason_instead_of_sinking_the_import() {
        let cells = sheet(&[
            &["Title", "Priority"],
            &["Fine", "low"],
            &["", "high"],
            &["Bad priority", "urgent"],
            &["Also fine", ""],
        ]);
        let (_, _, rows) = rows_from_cells(&cells);

        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].problem, None);
        assert_eq!(rows[1].problem.as_deref(), Some("No title"));
        assert!(rows[2].problem.as_deref().unwrap().contains("urgent"));
        // An empty priority cell is not an error; it means "the usual".
        assert_eq!(rows[3].priority, "medium");
        assert_eq!(rows[3].problem, None);
    }

    #[test]
    fn row_numbers_match_what_the_spreadsheet_shows() {
        let cells = sheet(&[&["", ""], &["Title"], &["First"], &["", ""], &["Second"]]);
        let (_, _, rows) = rows_from_cells(&cells);

        // Headings on sheet row 2, so the data starts at 3; the blank row is
        // skipped without shifting the numbering of what follows.
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].row, 3);
        assert_eq!(rows[1].row, 5);
    }

    #[test]
    fn an_empty_sheet_imports_nothing_rather_than_failing() {
        let (columns, ignored, rows) = rows_from_cells(&sheet(&[&["", ""], &[""]]));
        assert!(columns.is_empty());
        assert!(ignored.is_empty());
        assert!(rows.is_empty());
    }

    #[test]
    fn a_csv_reads_back_as_tickets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tickets.csv");
        // A quoted comma and a quoted newline are what hand-rolled splitting
        // gets wrong, and a description is exactly where they turn up.
        std::fs::write(
            &path,
            "Title,Epic,Priority,Description\n\
             Build the grid,Search,high,\"Cards, not rows\"\n\
             Wire the filters,Search,,\"Line one\nline two\"\n",
        )
        .unwrap();

        let (sheet, cells) = read_sheet(path.to_str().unwrap()).unwrap();
        let (_, ignored, rows) = rows_from_cells(&cells);

        assert_eq!(sheet, "tickets.csv");
        assert!(ignored.is_empty(), "ignored: {ignored:?}");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].description.as_deref(), Some("Cards, not rows"));
        assert_eq!(rows[0].priority, "high");
        assert_eq!(rows[1].description.as_deref(), Some("Line one\nline two"));
        assert_eq!(rows[1].priority, "medium");
    }

    #[test]
    fn a_ragged_csv_still_imports_what_it_can() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ragged.csv");
        // Exports routinely drop trailing empty fields; refusing the file over
        // a missing comma would help nobody.
        std::fs::write(
            &path,
            "Title,Epic,Priority\nOnly a title\nWith epic,Search\n",
        )
        .unwrap();

        let (_, cells) = read_sheet(path.to_str().unwrap()).unwrap();
        let (_, _, rows) = rows_from_cells(&cells);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].title, "Only a title");
        assert_eq!(rows[0].epic, None);
        assert_eq!(rows[1].epic.as_deref(), Some("Search"));
        assert!(rows.iter().all(|r| r.problem.is_none()));
    }

    #[test]
    fn a_tsv_is_read_with_tabs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tickets.tsv");
        std::fs::write(&path, "Title\tEpic\nBuild, carefully\tSearch\n").unwrap();

        let (_, cells) = read_sheet(path.to_str().unwrap()).unwrap();
        let (_, _, rows) = rows_from_cells(&cells);

        // The comma is part of the title here, not a separator.
        assert_eq!(rows[0].title, "Build, carefully");
        assert_eq!(rows[0].epic.as_deref(), Some("Search"));
    }

    /// A template that does not import teaches the wrong shape, and nobody
    /// finds out until they have edited it into their own sheet.
    #[test]
    fn the_bundled_template_imports_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tickets-template.csv");
        // The compiled-in copy is what the app hands out, so that is what gets
        // checked - not the file that happens to sit in the repository.
        std::fs::write(&path, TICKET_TEMPLATE_CSV).unwrap();

        let (_, cells) = read_sheet(path.to_str().expect("template path")).unwrap();
        let (columns, ignored, rows) = rows_from_cells(&cells);

        assert!(
            ignored.is_empty(),
            "the template uses headings the importer ignores: {ignored:?}"
        );
        assert!(columns.iter().any(|c| column_for(c) == Some(Column::Title)));
        assert!(!rows.is_empty(), "the template has no example rows");

        for row in &rows {
            assert_eq!(row.problem, None, "template row {}", row.row);
        }
    }

    /// Read a real workbook, not just the cell mapping.
    #[test]
    fn a_real_workbook_reads_back_as_tickets() {
        use rust_xlsxwriter::Workbook;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tickets.xlsx");

        let mut workbook = Workbook::new();
        let sheet = workbook.add_worksheet();
        sheet.write_string(0, 0, "Title").unwrap();
        sheet.write_string(0, 1, "Epic").unwrap();
        sheet.write_string(0, 2, "Priority").unwrap();
        sheet.write_string(1, 0, "Build the results grid").unwrap();
        sheet.write_string(1, 1, "Search").unwrap();
        sheet.write_string(1, 2, "high").unwrap();
        workbook.save(&path).unwrap();

        let (_, cells) = read_sheet(path.to_str().unwrap()).unwrap();
        let (columns, ignored, rows) = rows_from_cells(&cells);

        assert_eq!(columns.len(), 3, "columns: {columns:?}");
        assert!(ignored.is_empty(), "ignored: {ignored:?}");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "Build the results grid");
        assert_eq!(rows[0].epic.as_deref(), Some("Search"));
        assert_eq!(rows[0].problem, None);
    }

    /// Add a ticket straight to the database, past the command layer.
    fn queue_ticket(
        conn: &Connection,
        title: &str,
        status: &str,
        position: i64,
        agent: Option<i64>,
    ) -> i64 {
        queue_ticket_at(conn, title, status, "medium", position, agent)
    }

    fn queue_ticket_at(
        conn: &Connection,
        title: &str,
        status: &str,
        priority: &str,
        position: i64,
        agent: Option<i64>,
    ) -> i64 {
        conn.execute(
            "INSERT INTO tickets (project_id, title, priority, status, position, agent_id)
             VALUES (1, ?1, ?2, ?3, ?4, ?5)",
            params![title, priority, status, position, agent],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    #[test]
    fn the_queue_takes_the_first_ticket_and_approves_it() {
        let conn = scratch_db();
        queue_ticket(&conn, "Second", "pending", 1, Some(7));
        let first = queue_ticket(&conn, "First", "pending", 0, Some(7));

        let taken = take_next_queued_ticket(&conn, 1)
            .unwrap()
            .expect("a ticket");

        assert_eq!(taken.id, first, "position decides, not insertion order");
        assert_eq!(taken.status, "approved");
        // Approving is what the queue does on the way past, so it must stick.
        let stored: String = conn
            .query_row(
                "SELECT status FROM tickets WHERE id = ?1",
                params![first],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored, "approved");
    }

    #[test]
    fn the_queue_leaves_finished_and_abandoned_work_alone() {
        let conn = scratch_db();
        queue_ticket(&conn, "Done", "completed", 0, Some(7));
        queue_ticket(&conn, "Dropped", "cancelled", 1, Some(7));
        queue_ticket(&conn, "Running", "in_progress", 2, Some(7));
        let next = queue_ticket(&conn, "Waiting", "approved", 3, Some(7));

        let taken = take_next_queued_ticket(&conn, 1)
            .unwrap()
            .expect("a ticket");
        assert_eq!(taken.id, next);
    }

    #[test]
    fn the_queue_skips_a_ticket_it_could_not_run_anyway() {
        let conn = scratch_db();
        // No agent means no run, and stalling the whole queue on it would be
        // worse than passing it over.
        queue_ticket(&conn, "No agent", "pending", 0, None);
        let runnable = queue_ticket(&conn, "Has one", "pending", 1, Some(7));

        let taken = take_next_queued_ticket(&conn, 1)
            .unwrap()
            .expect("a ticket");
        assert_eq!(taken.id, runnable);
    }

    #[test]
    fn the_queue_follows_the_order_the_plan_was_written_in() {
        let conn = scratch_db();
        let first = queue_ticket_at(&conn, "Set up the project", "pending", "low", 0, Some(7));
        queue_ticket_at(
            &conn,
            "An urgent feature",
            "pending",
            "critical",
            1,
            Some(7),
        );

        // Priority must not reorder a plan: the urgent feature cannot be built
        // before the project it belongs to exists.
        let taken = take_next_queued_ticket(&conn, 1)
            .unwrap()
            .expect("a ticket");
        assert_eq!(taken.id, first);
    }

    fn module_ticket(conn: &Connection, title: &str, epic: Option<&str>, position: i64) -> i64 {
        conn.execute(
            "INSERT INTO tickets (project_id, title, epic, priority, status, position, agent_id)
             VALUES (1, ?1, ?2, 'medium', 'pending', ?3, 7)",
            params![title, epic, position],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    #[test]
    fn a_module_is_taken_whole() {
        let conn = scratch_db();
        module_ticket(&conn, "Header", Some("Design System"), 0);
        module_ticket(&conn, "Hero", Some("Homepage"), 1);
        module_ticket(&conn, "Buttons", Some("Design System"), 2);
        module_ticket(&conn, "Footer", Some("Homepage"), 3);

        let module = take_next_module(&conn, 1).unwrap().expect("a module");

        // The first ticket still waiting names the module, and everything under
        // that name comes with it - not just the tickets that happen to sit
        // next to each other.
        assert_eq!(module.name, "Design System");
        let titles: Vec<&str> = module.tickets.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, vec!["Header", "Buttons"]);
    }

    #[test]
    fn taking_a_module_approves_everything_in_it() {
        let conn = scratch_db();
        module_ticket(&conn, "Header", Some("Design System"), 0);
        module_ticket(&conn, "Buttons", Some("Design System"), 1);

        let module = take_next_module(&conn, 1).unwrap().unwrap();

        assert!(module.tickets.iter().all(|t| t.status == "approved"));
        let pending: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM tickets WHERE status = 'pending'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pending, 0, "approving must stick, not just be reported");
    }

    #[test]
    fn tickets_without_an_epic_are_a_module_of_their_own() {
        let conn = scratch_db();
        module_ticket(&conn, "Odd job", None, 0);
        module_ticket(&conn, "Header", Some("Design System"), 1);

        let module = take_next_module(&conn, 1).unwrap().unwrap();

        assert_eq!(module.name, "");
        assert_eq!(module.tickets.len(), 1);
        // And crucially not swept in with the next module's work.
        assert_eq!(module.tickets[0].title, "Odd job");
    }

    #[test]
    fn a_module_leaves_out_what_the_queue_cannot_run() {
        let conn = scratch_db();
        module_ticket(&conn, "Fine", Some("Design System"), 0);
        let no_agent = module_ticket(&conn, "No agent", Some("Design System"), 1);
        conn.execute(
            "UPDATE tickets SET agent_id = NULL WHERE id = ?1",
            params![no_agent],
        )
        .unwrap();
        let exhausted = module_ticket(&conn, "Given up on", Some("Design System"), 2);
        conn.execute(
            "UPDATE tickets SET attempts = ?1 WHERE id = ?2",
            params![MAX_TICKET_ATTEMPTS, exhausted],
        )
        .unwrap();

        let module = take_next_module(&conn, 1).unwrap().unwrap();

        let titles: Vec<&str> = module.tickets.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, vec!["Fine"]);
    }

    #[test]
    fn an_empty_board_has_no_module_to_take() {
        let conn = scratch_db();
        assert!(take_next_module(&conn, 1).unwrap().is_none());
    }

    #[test]
    fn a_board_with_a_run_going_is_busy() {
        let conn = scratch_db();
        assert!(!has_ticket_in_progress(&conn, 1).unwrap());

        queue_ticket(&conn, "Running", "in_progress", 0, Some(7));
        assert!(has_ticket_in_progress(&conn, 1).unwrap());

        // Another board's run says nothing about this one.
        assert!(!has_ticket_in_progress(&conn, 2).unwrap());
    }

    #[test]
    fn a_ticket_that_keeps_failing_stops_holding_up_the_board() {
        let conn = scratch_db();
        let stuck = queue_ticket_at(&conn, "Cannot be done", "approved", "high", 0, Some(7));
        let next = queue_ticket_at(&conn, "Perfectly fine", "approved", "high", 1, Some(7));

        // First run, then a failure puts it back in Approved.
        link_run(&conn, stuck, 1).unwrap();
        apply_run_result(&conn, 1, false, None, None).unwrap();
        assert_eq!(
            take_next_queued_ticket(&conn, 1).unwrap().unwrap().id,
            stuck,
            "one retry is worth having"
        );

        // Second failure exhausts it, and the queue moves on rather than
        // working through the same ticket for ever.
        link_run(&conn, stuck, 2).unwrap();
        apply_run_result(&conn, 2, false, None, None).unwrap();
        assert_eq!(take_next_queued_ticket(&conn, 1).unwrap().unwrap().id, next);
    }

    #[test]
    fn starting_a_run_counts_as_an_attempt() {
        let conn = scratch_db();
        let id = queue_ticket(&conn, "Work", "approved", 0, Some(7));

        link_run(&conn, id, 1).unwrap();
        let attempts: i64 = conn
            .query_row(
                "SELECT attempts FROM tickets WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(attempts, 1);
    }

    fn attempts_of(conn: &Connection, id: i64) -> i64 {
        conn.query_row(
            "SELECT attempts FROM tickets WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn a_run_that_never_started_does_not_cost_the_ticket_a_chance() {
        let conn = scratch_db();
        let id = queue_ticket(&conn, "Work", "approved", 0, Some(7));

        // Preflight refusing a run - a dirty tree, say - says nothing about
        // this ticket, and it was the whole board being burnt through on one.
        link_run(&conn, id, 1).unwrap();
        assert_eq!(attempts_of(&conn, id), 1);

        refund_attempt(&conn, 1).unwrap();
        assert_eq!(attempts_of(&conn, id), 0);
    }

    #[test]
    fn refunding_never_goes_below_zero() {
        let conn = scratch_db();
        let id = queue_ticket(&conn, "Work", "approved", 0, Some(7));
        link_run(&conn, id, 1).unwrap();

        refund_attempt(&conn, 1).unwrap();
        refund_attempt(&conn, 1).unwrap();
        assert_eq!(attempts_of(&conn, id), 0);
    }

    #[test]
    fn resetting_gives_skipped_tickets_another_go() {
        let conn = scratch_db();
        let skipped = queue_ticket(&conn, "Skipped", "approved", 0, Some(7));
        let running = queue_ticket(&conn, "Running", "in_progress", 1, Some(7));
        let done = queue_ticket(&conn, "Done", "completed", 2, Some(7));

        for id in [skipped, running, done] {
            conn.execute(
                "UPDATE tickets SET attempts = ?1 WHERE id = ?2",
                params![MAX_TICKET_ATTEMPTS, id],
            )
            .unwrap();
        }

        // Mirrors what the command does, which cannot be called without a
        // Tauri State in a unit test.
        let reset = conn
            .execute(
                "UPDATE tickets SET attempts = 0, updated_at = CURRENT_TIMESTAMP
                 WHERE project_id = ?1 AND attempts >= ?2 AND status IN ('pending', 'approved')",
                params![1, MAX_TICKET_ATTEMPTS],
            )
            .unwrap();

        assert_eq!(reset, 1, "only work still waiting is reset");
        assert_eq!(attempts_of(&conn, skipped), 0);
        // A run under way and finished work are left as they are.
        assert_eq!(attempts_of(&conn, running), MAX_TICKET_ATTEMPTS);
        assert_eq!(attempts_of(&conn, done), MAX_TICKET_ATTEMPTS);

        assert_eq!(
            take_next_queued_ticket(&conn, 1).unwrap().unwrap().id,
            skipped
        );
    }

    #[test]
    fn an_empty_queue_is_not_an_error() {
        let conn = scratch_db();
        queue_ticket(&conn, "Done", "completed", 0, Some(7));

        assert!(take_next_queued_ticket(&conn, 1).unwrap().is_none());
    }

    #[test]
    fn a_board_is_found_by_the_path_it_tracks() {
        let conn = scratch_db();
        let found = board_by_repo_path(&conn, "/tmp/demo")
            .unwrap()
            .expect("the board");
        assert_eq!(found.id, 1);
        // Continuing a queue is opt-in, so a board that never asked stays put.
        assert!(!found.auto_approve);

        assert!(board_by_repo_path(&conn, "/tmp/elsewhere")
            .unwrap()
            .is_none());
    }

    /// The agents `cc_agents/` ships, as the picker sees them.
    fn all_agents() -> Vec<(i64, String)> {
        [
            "Planner",
            "Implementer",
            "Tester",
            "Code Reviewer",
            "Documenter",
            "Debugger",
            "Security Scanner",
        ]
        .iter()
        .enumerate()
        .map(|(i, name)| (i as i64 + 1, name.to_string()))
        .collect()
    }

    fn picked(task: &str) -> String {
        let agents = all_agents();
        let id = pick_agent(task, &agents).expect("an agent");
        agents
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, n)| n.clone())
            .unwrap()
    }

    #[test]
    fn tasks_go_to_the_agent_whose_work_they_describe() {
        assert_eq!(picked("Add tests for the search endpoint"), "Tester");
        assert_eq!(picked("Fix the crash on an empty query"), "Debugger");
        assert_eq!(picked("Write the search API documentation"), "Documenter");
        assert_eq!(
            picked("Review the auth module for duplication"),
            "Code Reviewer"
        );
        assert_eq!(
            picked("Investigate how we should design the sync layer"),
            "Planner"
        );
        assert_eq!(
            picked("Check for XSS in the comment renderer"),
            "Security Scanner"
        );
        assert_eq!(picked("Build the results grid"), "Implementer");
    }

    #[test]
    fn a_task_that_suggests_nothing_falls_back_to_implementing() {
        // Most tickets read like this, and implementation is the safe default.
        assert_eq!(picked("Results grid"), "Implementer");
        assert_eq!(picked(""), "Implementer");
    }

    #[test]
    fn the_more_particular_kind_of_work_wins_a_tie() {
        // One word each for testing and debugging; the more specific kind is
        // listed first and takes it.
        assert_eq!(picked("Add tests for the crash"), "Tester");
    }

    #[test]
    fn weight_of_evidence_decides_between_particular_kinds() {
        // Two debugging words against one for testing.
        assert_eq!(
            picked("The flaky test crashes on an empty query"),
            "Debugger"
        );
    }

    #[test]
    fn everyday_building_words_do_not_out_vote_the_real_subject() {
        // "Add" and "endpoint" would win on raw count if implementing competed
        // on keywords, which is exactly why it does not.
        assert_eq!(picked("Add tests for the search endpoint"), "Tester");
        assert_eq!(picked("Build the docs page"), "Documenter");
    }

    #[test]
    fn an_agent_nobody_has_installed_is_passed_over() {
        let without_tester: Vec<(i64, String)> = all_agents()
            .into_iter()
            .filter(|(_, n)| n != "Tester")
            .collect();

        let id = pick_agent("Add tests for the search endpoint", &without_tester).unwrap();
        let name = &without_tester.iter().find(|(i, _)| *i == id).unwrap().1;
        assert_eq!(name, "Implementer", "should fall through, not fail");
    }

    #[test]
    fn with_no_agents_at_all_there_is_nothing_to_pick() {
        assert_eq!(pick_agent("Add tests", &[]), None);
    }

    #[test]
    fn only_whole_words_count() {
        let agents = all_agents();
        // "contest" contains "test" but is not about testing, and "adding"
        // is not "add".
        let id = pick_agent("Run the contest page", &agents).unwrap();
        let name = &agents.iter().find(|(i, _)| *i == id).unwrap().1;
        assert_eq!(name, "Implementer");
    }

    fn scratch_db() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory db");
        ensure_tables(&conn).expect("tables");
        conn.execute(
            "INSERT INTO board_projects (id, name, repo_path) VALUES (1, 'Demo', '/tmp/demo')",
            [],
        )
        .unwrap();
        conn
    }

    fn add_ticket(conn: &Connection, title: &str, status: &str) -> i64 {
        conn.execute(
            "INSERT INTO tickets (project_id, title, status) VALUES (1, ?1, ?2)",
            params![title, status],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn status_of(conn: &Connection, id: i64) -> String {
        conn.query_row(
            "SELECT status FROM tickets WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn board_columns_match_the_ui() {
        assert_eq!(
            STATUSES,
            &[
                "pending",
                "approved",
                "in_progress",
                "completed",
                "cancelled"
            ]
        );
    }

    #[test]
    fn approval_gate_cannot_be_skipped() {
        // The whole point of the board is that nothing runs unreviewed.
        assert!(!can_transition("pending", "in_progress"));
        assert!(!can_transition("pending", "completed"));
        assert!(can_transition("pending", "approved"));
        assert!(can_transition("approved", "in_progress"));
    }

    #[test]
    fn transitions_allow_retry_and_reopen() {
        assert!(
            can_transition("in_progress", "approved"),
            "failed run retries"
        );
        assert!(
            can_transition("completed", "approved"),
            "reopen finished work"
        );
        assert!(
            can_transition("cancelled", "pending"),
            "revive cancelled work"
        );
        assert!(can_transition("pending", "pending"), "no-op is fine");
    }

    #[test]
    fn transitions_reject_nonsense() {
        assert!(!can_transition("completed", "in_progress"));
        assert!(!can_transition("cancelled", "completed"));
        assert!(!can_transition("completed", "pending"));
    }

    #[test]
    fn anything_open_can_be_cancelled() {
        for from in ["pending", "approved", "in_progress"] {
            assert!(can_transition(from, "cancelled"), "{from} should cancel");
        }
    }

    #[test]
    fn validates_statuses_and_priorities() {
        assert!(is_valid_status("in_progress"));
        assert!(!is_valid_status("done"));
        assert!(is_valid_priority("critical"));
        assert!(!is_valid_priority("urgent"));
    }

    #[test]
    fn parses_pr_numbers_from_urls() {
        assert_eq!(
            parse_pr_number("https://github.com/acme/repo/pull/72"),
            Some(72)
        );
        assert_eq!(parse_pr_number("not a url"), None);
        assert_eq!(parse_pr_number("https://github.com/acme/repo/pull/"), None);
    }

    #[test]
    fn linking_a_run_moves_the_ticket_into_progress() {
        let conn = scratch_db();
        let id = add_ticket(&conn, "Build sorting controls", "approved");

        link_run(&conn, id, 99).unwrap();

        assert_eq!(status_of(&conn, id), "in_progress");
        let run: i64 = conn
            .query_row(
                "SELECT workflow_run_id FROM tickets WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(run, 99);
    }

    #[test]
    fn a_successful_run_completes_the_ticket_with_pr_details() {
        let conn = scratch_db();
        let id = add_ticket(&conn, "Build results grid", "approved");
        link_run(&conn, id, 7).unwrap();

        apply_run_result(
            &conn,
            7,
            true,
            Some("dotsquares-ai/build-results-grid-7"),
            Some("https://github.com/acme/repo/pull/72"),
        )
        .unwrap();

        let (status, branch, pr_num, pr_state, completed): (
            String,
            String,
            i64,
            String,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT status, branch, pr_number, pr_state, completed_at
                 FROM tickets WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();

        assert_eq!(status, "completed");
        assert_eq!(branch, "dotsquares-ai/build-results-grid-7");
        assert_eq!(pr_num, 72);
        assert_eq!(pr_state, "open");
        assert!(completed.is_some(), "completed_at should be stamped");
    }

    #[test]
    fn a_failed_run_sends_the_ticket_back_for_another_attempt() {
        let conn = scratch_db();
        let id = add_ticket(&conn, "Build booking wizard", "approved");
        link_run(&conn, id, 8).unwrap();

        apply_run_result(&conn, 8, false, Some("dotsquares-ai/wizard-8"), None).unwrap();

        assert_eq!(status_of(&conn, id), "approved");
        let completed: Option<String> = conn
            .query_row(
                "SELECT completed_at FROM tickets WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(completed.is_none(), "a failed run must not look finished");
    }

    #[test]
    fn a_run_result_only_touches_its_own_ticket() {
        let conn = scratch_db();
        let mine = add_ticket(&conn, "Mine", "approved");
        let theirs = add_ticket(&conn, "Theirs", "approved");
        link_run(&conn, mine, 1).unwrap();
        link_run(&conn, theirs, 2).unwrap();

        apply_run_result(&conn, 1, true, None, None).unwrap();

        assert_eq!(status_of(&conn, mine), "completed");
        assert_eq!(status_of(&conn, theirs), "in_progress");
    }

    #[test]
    fn ticket_positions_increment_per_project() {
        let conn = scratch_db();
        conn.execute(
            "INSERT INTO board_projects (id, name, repo_path) VALUES (2, 'Other', '/tmp/other')",
            [],
        )
        .unwrap();

        let next = |project: i64| -> i64 {
            conn.query_row(
                "SELECT COALESCE(MAX(position), -1) + 1 FROM tickets WHERE project_id = ?1",
                params![project],
                |r| r.get(0),
            )
            .unwrap()
        };

        assert_eq!(next(1), 0);
        conn.execute(
            "INSERT INTO tickets (project_id, title, position) VALUES (1, 'a', 0)",
            [],
        )
        .unwrap();
        assert_eq!(next(1), 1);
        // A second board starts its own numbering.
        assert_eq!(next(2), 0);
    }
}
