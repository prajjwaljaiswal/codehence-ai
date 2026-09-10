//! The test documentation a Tester run leaves behind, and its export to Excel.
//!
//! The Tester agent writes one machine-readable file, `.opcode/test-report.json`,
//! alongside whatever it says in the chat. Prose in a transcript cannot be
//! filtered, sorted or handed to someone who does not have opcode open, so the
//! structured file is what this module turns into a workbook: one row per test
//! case, per device checked, per defect found.
//!
//! Everything here is deliberately forgiving. The file is written by a language
//! model, so a missing field, a `steps` that came out as one string instead of a
//! list, or `automated: "yes"` must not lose the other two hundred rows. What
//! cannot be read is reported as a *problem* next to the export rather than
//! raised as an error - the same bargain the ticket importer makes.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Where the Tester agent is told to leave its report.
pub const REPORT_RELATIVE_PATH: &str = ".opcode/test-report.json";

/// Names that are accepted as well, in order.
///
/// The report is written by an agent following a prompt, and a prompt is not a
/// schema: runs do occasionally land on the obvious neighbouring name. Reading
/// those costs nothing and saves a person from re-running a long test pass to
/// fix a filename.
const REPORT_CANDIDATES: &[&str] = &[
    ".opcode/test-report.json",
    ".opcode/test_report.json",
    ".opcode/tests.json",
    "test-report.json",
];

/// The example report, compiled in rather than read from disk.
///
/// A packaged app has no repository next to it, so a file path would work in
/// development and break everywhere else.
pub const TEST_REPORT_TEMPLATE_JSON: &str =
    include_str!("../../../templates/test-report.example.json");

// ---------------------------------------------------------------------------
// The report as it arrives
// ---------------------------------------------------------------------------

/// A field that may arrive as one string or as a list of them.
///
/// `steps` is the usual offender: both `"1. Open the board 2. ..."` and
/// `["Open the board", "..."]` are natural things to write, and both have to
/// read back the same way.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Lines {
    One(String),
    Many(Vec<String>),
}

impl Default for Lines {
    fn default() -> Self {
        Lines::One(String::new())
    }
}

impl Lines {
    /// The parts, with blank ones dropped.
    pub fn parts(&self) -> Vec<String> {
        match self {
            Lines::One(s) => s
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect(),
            Lines::Many(v) => v
                .iter()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect(),
        }
    }

    /// One cell's worth of text, numbered when there is more than one part.
    ///
    /// Numbering matters in a spreadsheet: a reviewer reading a failed case
    /// needs to say "it broke at step 3", and a wrapped cell of unnumbered
    /// lines gives them nothing to point at.
    pub fn numbered(&self) -> String {
        let parts = self.parts();
        if parts.len() <= 1 {
            return parts.into_iter().next().unwrap_or_default();
        }
        parts
            .iter()
            .enumerate()
            .map(|(i, p)| {
                // Leave a list that already numbered itself alone.
                if starts_with_number(p) {
                    p.clone()
                } else {
                    format!("{}. {}", i + 1, p)
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The parts joined for a one-line cell, such as a list of case ids.
    pub fn inline(&self) -> String {
        self.parts().join(", ")
    }

    pub fn is_empty(&self) -> bool {
        self.parts().is_empty()
    }
}

fn starts_with_number(line: &str) -> bool {
    let mut chars = line.chars();
    match chars.next() {
        Some(c) if c.is_ascii_digit() => {}
        _ => return false,
    }
    chars
        .skip_while(|c| c.is_ascii_digit())
        .next()
        .map(|c| c == '.' || c == ')' || c == ':')
        .unwrap_or(false)
}

/// A yes/no that may arrive as a bool, a string or a number.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Flag {
    Bool(bool),
    Text(String),
    Number(i64),
}

impl Flag {
    fn truth(&self) -> Option<bool> {
        match self {
            Flag::Bool(b) => Some(*b),
            Flag::Number(n) => Some(*n != 0),
            Flag::Text(s) => match s.trim().to_ascii_lowercase().as_str() {
                "true" | "yes" | "y" | "1" | "automated" => Some(true),
                "false" | "no" | "n" | "0" | "manual" => Some(false),
                "" => None,
                _ => None,
            },
        }
    }

    fn cell(&self) -> String {
        match self.truth() {
            Some(true) => "Yes".to_string(),
            Some(false) => "No".to_string(),
            None => match self {
                Flag::Text(s) => s.trim().to_string(),
                _ => String::new(),
            },
        }
    }
}

/// Somewhere the feature was exercised: a browser, a phone, a simulator.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Environment {
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub device: String,
    #[serde(default, alias = "os")]
    pub os_version: String,
    #[serde(default)]
    pub browser: String,
    #[serde(default, alias = "screen")]
    pub viewport: String,
    #[serde(default)]
    pub notes: String,
}

/// One test case, run or planned.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TestCase {
    #[serde(default)]
    pub id: String,
    #[serde(default, alias = "module", alias = "component")]
    pub area: String,
    #[serde(default, alias = "scenario", alias = "name")]
    pub title: String,
    #[serde(default, alias = "type")]
    pub category: String,
    #[serde(default)]
    pub priority: String,
    #[serde(default)]
    pub platform: String,
    #[serde(default, alias = "setup")]
    pub preconditions: String,
    #[serde(default)]
    pub steps: Lines,
    #[serde(default, alias = "data")]
    pub test_data: String,
    #[serde(default, alias = "expected_result")]
    pub expected: String,
    #[serde(default, alias = "actual_result")]
    pub actual: String,
    #[serde(default, alias = "result")]
    pub status: String,
    #[serde(default)]
    pub severity: String,
    #[serde(default)]
    pub automated: Option<Flag>,
    /// Where the automated test lives, so a row can be traced to real code.
    #[serde(default, alias = "test", alias = "test_file")]
    pub test_ref: String,
    #[serde(default, alias = "bug_id")]
    pub defect_id: String,
    #[serde(default)]
    pub notes: String,
}

/// One device-or-viewport check, which is a different shape from a test case:
/// the same scenario repeated across a matrix rather than a scenario of its own.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MobileCheck {
    #[serde(default, alias = "scenario", alias = "title")]
    pub check: String,
    #[serde(default)]
    pub device: String,
    #[serde(default, alias = "os")]
    pub os_version: String,
    #[serde(default, alias = "screen")]
    pub viewport: String,
    #[serde(default)]
    pub orientation: String,
    #[serde(default, alias = "connection")]
    pub network: String,
    #[serde(default, alias = "status")]
    pub result: String,
    #[serde(default)]
    pub notes: String,
}

/// Something the tests found that is wrong with the code.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Defect {
    #[serde(default)]
    pub id: String,
    #[serde(default, alias = "summary")]
    pub title: String,
    #[serde(default)]
    pub severity: String,
    #[serde(default, alias = "test_case", alias = "case")]
    pub case_id: String,
    #[serde(default, alias = "repro")]
    pub steps: Lines,
    #[serde(default)]
    pub expected: String,
    #[serde(default)]
    pub actual: String,
    #[serde(default, alias = "env")]
    pub environment: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub notes: String,
}

/// A requirement and the cases that cover it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CoverageRow {
    #[serde(default, alias = "behaviour", alias = "behavior")]
    pub requirement: String,
    #[serde(default, alias = "cases", alias = "case_id")]
    pub case_ids: Lines,
    #[serde(default)]
    pub covered: Option<Flag>,
    #[serde(default)]
    pub gap: String,
    #[serde(default)]
    pub notes: String,
}

/// The whole report.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TestReport {
    #[serde(default, alias = "title")]
    pub feature: String,
    #[serde(default, alias = "scope")]
    pub summary: String,
    #[serde(default)]
    pub tested_by: String,
    #[serde(default, alias = "date")]
    pub generated_at: String,
    #[serde(default)]
    pub commit: String,
    #[serde(default)]
    pub branch: String,
    /// The commands that were actually run, so a reader can repeat the pass.
    #[serde(default, alias = "commands")]
    pub test_commands: Lines,
    #[serde(default, alias = "environment")]
    pub environments: Vec<Environment>,
    #[serde(default, alias = "test_cases")]
    pub cases: Vec<TestCase>,
    #[serde(default, alias = "mobile_matrix", alias = "devices")]
    pub mobile_checks: Vec<MobileCheck>,
    #[serde(default, alias = "bugs")]
    pub defects: Vec<Defect>,
    #[serde(default, alias = "traceability")]
    pub coverage: Vec<CoverageRow>,
    /// What was left untested, and why. An honest report has some.
    #[serde(default, alias = "gaps")]
    pub not_covered: Lines,
}

// ---------------------------------------------------------------------------
// Reading it
// ---------------------------------------------------------------------------

/// How a status word maps onto the few outcomes a workbook colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    Fail,
    Blocked,
    Skipped,
    NotRun,
}

impl Outcome {
    /// Read a status the way a person wrote it.
    ///
    /// Runs write `pass`, `Passed`, `PASS ✅` and `ok` for the same thing, and a
    /// report whose statuses do not colour is a report nobody trusts.
    pub fn parse(raw: &str) -> Option<Outcome> {
        let cleaned: String = raw
            .chars()
            .filter(|c| c.is_ascii_alphabetic() || c.is_whitespace() || *c == '-' || *c == '_')
            .collect();
        let s = cleaned.trim().to_ascii_lowercase().replace(['-', '_'], " ");
        let s = s.trim();
        match s {
            "pass" | "passed" | "passing" | "ok" | "green" | "success" => Some(Outcome::Pass),
            "fail" | "failed" | "failing" | "red" | "error" | "broken" => Some(Outcome::Fail),
            "blocked" | "block" | "cannot test" | "couldnt test" => Some(Outcome::Blocked),
            "skip" | "skipped" | "n a" | "na" | "not applicable" => Some(Outcome::Skipped),
            "" | "not run" | "notrun" | "pending" | "todo" | "planned" | "untested" => {
                Some(Outcome::NotRun)
            }
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Outcome::Pass => "Pass",
            Outcome::Fail => "Fail",
            Outcome::Blocked => "Blocked",
            Outcome::Skipped => "Skipped",
            Outcome::NotRun => "Not run",
        }
    }
}

/// The tallies the summary sheet and the button's label are built from.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReportCounts {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub blocked: usize,
    pub skipped: usize,
    pub not_run: usize,
    /// Cases whose category reads as an edge, boundary or negative case.
    pub edge_cases: usize,
    /// Cases and matrix rows that name a mobile platform or device.
    pub mobile_cases: usize,
    pub defects: usize,
    pub open_defects: usize,
    /// Passed over everything that was actually run, as a fraction.
    pub pass_rate: f64,
}

fn is_edge_category(category: &str) -> bool {
    let c = category.to_ascii_lowercase();
    ["edge", "boundary", "negative", "error", "security", "stress", "concurrency"]
        .iter()
        .any(|k| c.contains(k))
}

fn is_mobile_platform(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    [
        "mobile", "ios", "android", "iphone", "ipad", "phone", "tablet", "responsive", "touch",
        "safari mobile", "pixel", "galaxy",
    ]
    .iter()
    .any(|k| t.contains(k))
}

impl TestReport {
    pub fn counts(&self) -> ReportCounts {
        let mut c = ReportCounts::default();
        c.total = self.cases.len();
        for case in &self.cases {
            match Outcome::parse(&case.status).unwrap_or(Outcome::NotRun) {
                Outcome::Pass => c.passed += 1,
                Outcome::Fail => c.failed += 1,
                Outcome::Blocked => c.blocked += 1,
                Outcome::Skipped => c.skipped += 1,
                Outcome::NotRun => c.not_run += 1,
            }
            if is_edge_category(&case.category) {
                c.edge_cases += 1;
            }
            if is_mobile_platform(&case.platform) || is_mobile_platform(&case.category) {
                c.mobile_cases += 1;
            }
        }
        c.mobile_cases += self.mobile_checks.len();
        c.defects = self.defects.len();
        c.open_defects = self
            .defects
            .iter()
            .filter(|d| {
                let s = d.status.to_ascii_lowercase();
                !(s.contains("fixed") || s.contains("closed") || s.contains("wont") || s.contains("won't"))
            })
            .count();

        let ran = c.passed + c.failed;
        c.pass_rate = if ran == 0 {
            0.0
        } else {
            c.passed as f64 / ran as f64
        };
        c
    }

    /// What is wrong with the report itself, in the reader's words.
    ///
    /// A test document is only worth exporting if it can be audited, so the
    /// things that make it unauditable - two cases sharing an id, a case with
    /// no expected result, a defect pointing at a case that does not exist -
    /// are surfaced next to the download rather than buried in a sheet.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();

        if self.cases.is_empty() {
            problems.push("No test cases in the report.".to_string());
        }
        if self.feature.trim().is_empty() {
            problems.push("No feature name, so the workbook has nothing to title itself with.".to_string());
        }

        let mut seen: Vec<&str> = Vec::new();
        let mut duplicates: Vec<String> = Vec::new();
        let mut unnamed = 0usize;
        let mut without_expected = 0usize;
        let mut unknown_status: Vec<String> = Vec::new();

        for case in &self.cases {
            let id = case.id.trim();
            if id.is_empty() {
                unnamed += 1;
            } else if seen.contains(&id) {
                if !duplicates.iter().any(|d| d == id) {
                    duplicates.push(id.to_string());
                }
            } else {
                seen.push(id);
            }

            if case.expected.trim().is_empty() {
                without_expected += 1;
            }
            if Outcome::parse(&case.status).is_none() && !unknown_status.contains(&case.status) {
                unknown_status.push(case.status.clone());
            }
        }

        if unnamed > 0 {
            problems.push(format!(
                "{} case{} without an id, so nothing can refer to {}.",
                unnamed,
                if unnamed == 1 { "" } else { "s" },
                if unnamed == 1 { "it" } else { "them" }
            ));
        }
        if !duplicates.is_empty() {
            problems.push(format!("Ids used more than once: {}.", duplicates.join(", ")));
        }
        if without_expected > 0 {
            problems.push(format!(
                "{} case{} with no expected result, which cannot pass or fail.",
                without_expected,
                if without_expected == 1 { "" } else { "s" }
            ));
        }
        if !unknown_status.is_empty() {
            problems.push(format!(
                "Statuses not understood, counted as not run: {}.",
                unknown_status.join(", ")
            ));
        }

        for defect in &self.defects {
            let case_id = defect.case_id.trim();
            if !case_id.is_empty() && !seen.iter().any(|s| *s == case_id) {
                problems.push(format!(
                    "Defect {} points at case {}, which is not in the report.",
                    if defect.id.trim().is_empty() { "(no id)" } else { defect.id.trim() },
                    case_id
                ));
            }
        }

        let counts = self.counts();
        if counts.mobile_cases == 0 {
            problems.push(
                "Nothing mobile was checked: no case names a mobile platform and the device matrix is empty."
                    .to_string(),
            );
        }
        if counts.edge_cases == 0 && !self.cases.is_empty() {
            problems.push("No edge, boundary or negative cases - only happy paths.".to_string());
        }
        if counts.failed > 0 && counts.defects == 0 {
            problems.push(format!(
                "{} case{} failed but no defect was raised.",
                counts.failed,
                if counts.failed == 1 { "" } else { "s" }
            ));
        }

        problems
    }
}

/// Find the report under a project, if one has been written.
pub fn report_path(project_path: &str) -> Option<PathBuf> {
    let root = Path::new(project_path);
    REPORT_CANDIDATES
        .iter()
        .map(|name| root.join(name))
        .find(|p| p.is_file())
}

/// Read and parse the report, naming the file that failed when it will not parse.
pub fn load_report(project_path: &str) -> Result<(PathBuf, TestReport), String> {
    let path = report_path(project_path).ok_or_else(|| {
        format!(
            "No test report in this project. The Tester agent writes {} - run it first.",
            REPORT_RELATIVE_PATH
        )
    })?;

    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("Could not read {}: {}", path.display(), e))?;

    let report: TestReport = serde_json::from_str(&text)
        .map_err(|e| format!("{} is not a test report opcode can read: {}", path.display(), e))?;

    Ok((path, report))
}

/// What the UI needs to decide whether to offer the download, and to warn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestReportStatus {
    pub found: bool,
    /// The file that was read, when one was.
    pub path: Option<String>,
    /// Where the Tester agent is told to write it, for the empty state to name.
    pub expected_path: String,
    pub feature: String,
    pub generated_at: String,
    pub counts: ReportCounts,
    /// Things wrong with the report, shown before it is exported.
    pub problems: Vec<String>,
    /// Set when a file exists but could not be parsed at all.
    pub error: Option<String>,
}

/// Report on the test documentation in a project without exporting anything.
#[tauri::command]
pub async fn test_report_status(project_path: String) -> Result<TestReportStatus, String> {
    let path = report_path(&project_path);
    let Some(path) = path else {
        return Ok(TestReportStatus {
            found: false,
            path: None,
            expected_path: REPORT_RELATIVE_PATH.to_string(),
            feature: String::new(),
            generated_at: String::new(),
            counts: ReportCounts::default(),
            problems: Vec::new(),
            error: None,
        });
    };

    match load_report(&project_path) {
        Ok((path, report)) => Ok(TestReportStatus {
            found: true,
            path: Some(path.display().to_string()),
            expected_path: REPORT_RELATIVE_PATH.to_string(),
            feature: report.feature.clone(),
            generated_at: report.generated_at.clone(),
            counts: report.counts(),
            problems: report.problems(),
            error: None,
        }),
        // A file that is there but unreadable is a different state from no file
        // at all: the first is a bad run to re-do, the second is a run to start.
        Err(error) => Ok(TestReportStatus {
            found: true,
            path: Some(path.display().to_string()),
            expected_path: REPORT_RELATIVE_PATH.to_string(),
            feature: String::new(),
            generated_at: String::new(),
            counts: ReportCounts::default(),
            problems: Vec::new(),
            error: Some(error),
        }),
    }
}

// ---------------------------------------------------------------------------
// Writing the workbook
// ---------------------------------------------------------------------------

use rust_xlsxwriter::{Color, Format, FormatAlign, FormatBorder, Workbook, Worksheet};

/// One cell as the table writer sees it.
enum Cell {
    /// Short text, one line.
    Text(String),
    /// Long text that should wrap in the cell.
    Wrap(String),
    /// A status word, coloured by the outcome it reads as.
    Status(String),
}

fn text(s: impl Into<String>) -> Cell {
    Cell::Text(s.into())
}

fn wrap(s: impl Into<String>) -> Cell {
    Cell::Wrap(s.into())
}

/// The formats every sheet shares, built once.
struct Styles {
    title: Format,
    header: Format,
    cell: Format,
    wrap: Format,
    label: Format,
    pass: Format,
    fail: Format,
    blocked: Format,
    skipped: Format,
    not_run: Format,
    percent: Format,
    number: Format,
}

impl Styles {
    fn new() -> Styles {
        let base = || {
            Format::new()
                .set_border(FormatBorder::Thin)
                .set_border_color(Color::RGB(0xD8DEE9))
                .set_align(FormatAlign::Top)
        };
        let status = |bg: u32, fg: u32| {
            base()
                .set_bold()
                .set_background_color(Color::RGB(bg))
                .set_font_color(Color::RGB(fg))
                .set_align(FormatAlign::Center)
        };

        Styles {
            title: Format::new().set_bold().set_font_size(14),
            header: Format::new()
                .set_bold()
                .set_font_color(Color::RGB(0xFFFFFF))
                .set_background_color(Color::RGB(0x1F2937))
                .set_border(FormatBorder::Thin)
                .set_border_color(Color::RGB(0x1F2937))
                .set_align(FormatAlign::Left)
                .set_text_wrap(),
            cell: base(),
            wrap: base().set_text_wrap(),
            label: Format::new().set_bold().set_align(FormatAlign::Top),
            pass: status(0xDCFCE7, 0x166534),
            fail: status(0xFEE2E2, 0x991B1B),
            blocked: status(0xFEF3C7, 0x92400E),
            skipped: status(0xF1F5F9, 0x475569),
            not_run: status(0xF8FAFC, 0x64748B),
            percent: Format::new().set_bold().set_num_format("0.0%"),
            number: Format::new().set_align(FormatAlign::Top),
        }
    }

    fn for_outcome(&self, outcome: Option<Outcome>) -> &Format {
        match outcome {
            Some(Outcome::Pass) => &self.pass,
            Some(Outcome::Fail) => &self.fail,
            Some(Outcome::Blocked) => &self.blocked,
            Some(Outcome::Skipped) => &self.skipped,
            // An unrecognised word keeps its own text but gets the neutral fill,
            // so it stands out as something to correct rather than as a pass.
            Some(Outcome::NotRun) | None => &self.not_run,
        }
    }
}

/// One line of a spreadsheet row, in points.
const LINE_HEIGHT: f64 = 14.0;

/// The most lines a row is grown to. A 40-line reproduction would otherwise
/// push every other row off the screen; the cell still holds all of it.
const MAX_WRAPPED_LINES: usize = 10;

/// Roughly how many lines `text` takes in a column `width` characters wide.
///
/// Excel only auto-fits a wrapped row when someone edits it, so a document
/// that is opened and read - which is all anyone does with this one - shows a
/// single clipped line unless the height is set here. An estimate is better
/// than that, and erring tall costs nothing.
fn estimated_lines(text: &str, width: f64) -> usize {
    let per_line = (width - 2.0).max(8.0);
    text.split('\n')
        .map(|line| {
            let chars = line.chars().count() as f64;
            ((chars / per_line).ceil() as usize).max(1)
        })
        .sum::<usize>()
        .max(1)
}

/// Lay out one table: a heading row that stays put, filterable, sized columns.
fn write_table(
    sheet: &mut Worksheet,
    styles: &Styles,
    columns: &[(&str, f64)],
    rows: Vec<Vec<Cell>>,
) -> Result<(), String> {
    let err = |e: rust_xlsxwriter::XlsxError| format!("Could not write the sheet: {}", e);

    for (col, (heading, width)) in columns.iter().enumerate() {
        let col = col as u16;
        sheet
            .write_string_with_format(0, col, *heading, &styles.header)
            .map_err(err)?;
        sheet.set_column_width(col, *width).map_err(err)?;
    }

    for (row_index, row) in rows.iter().enumerate() {
        let row_number = row_index as u32 + 1;

        let lines = row
            .iter()
            .enumerate()
            .filter_map(|(col, cell)| match cell {
                Cell::Wrap(s) => Some(estimated_lines(s, columns.get(col)?.1)),
                _ => None,
            })
            .max()
            .unwrap_or(1)
            .min(MAX_WRAPPED_LINES);
        if lines > 1 {
            sheet
                .set_row_height(row_number, lines as f64 * LINE_HEIGHT)
                .map_err(err)?;
        }

        for (col, cell) in row.iter().enumerate() {
            let col = col as u16;
            match cell {
                Cell::Text(s) => {
                    sheet
                        .write_string_with_format(row_number, col, s, &styles.cell)
                        .map_err(err)?;
                }
                Cell::Wrap(s) => {
                    sheet
                        .write_string_with_format(row_number, col, s, &styles.wrap)
                        .map_err(err)?;
                }
                Cell::Status(s) => {
                    let outcome = Outcome::parse(s);
                    // Print the canonical word when it was understood, so a
                    // sheet of "ok"/"passed"/"PASS" sorts and filters as one.
                    let shown = match outcome {
                        Some(o) if !s.trim().is_empty() => o.label().to_string(),
                        Some(_) => Outcome::NotRun.label().to_string(),
                        None => s.trim().to_string(),
                    };
                    sheet
                        .write_string_with_format(row_number, col, shown, styles.for_outcome(outcome))
                        .map_err(err)?;
                }
            }
        }
    }

    sheet.set_freeze_panes(1, 0).map_err(err)?;
    if !rows.is_empty() && !columns.is_empty() {
        sheet
            .autofilter(0, 0, rows.len() as u32, columns.len() as u16 - 1)
            .map_err(err)?;
    }
    Ok(())
}

/// The sheets a workbook always has, in order, whether or not they have rows.
///
/// A fixed set of tabs is what makes two exports comparable, and an empty
/// "Defects" tab says something ("nothing was found") that a missing tab does
/// not ("was anything looked for?").
pub const SHEET_NAMES: &[&str] = &[
    "Summary",
    "Test Cases",
    "Mobile Matrix",
    "Defects",
    "Coverage",
    "Environments",
];

/// Build the whole workbook in memory.
///
/// Bytes rather than a file because the same workbook is served two ways: saved
/// through a native dialog on the desktop, and streamed over HTTP to a phone
/// browser driving opcode remotely.
pub fn build_workbook(report: &TestReport) -> Result<Vec<u8>, String> {
    let styles = Styles::new();
    let counts = report.counts();
    let mut workbook = Workbook::new();
    let err = |e: rust_xlsxwriter::XlsxError| format!("Could not build the workbook: {}", e);

    // --- Summary -----------------------------------------------------------
    {
        let sheet = workbook.add_worksheet();
        sheet.set_name("Summary").map_err(err)?;
        sheet.set_column_width(0, 26.0).map_err(err)?;
        sheet.set_column_width(1, 78.0).map_err(err)?;

        let feature = if report.feature.trim().is_empty() {
            "Test report".to_string()
        } else {
            report.feature.trim().to_string()
        };
        sheet
            .write_string_with_format(0, 0, feature, &styles.title)
            .map_err(err)?;

        let mut row = 2u32;
        let mut pair = |sheet: &mut Worksheet, label: &str, value: String| -> Result<(), String> {
            if value.trim().is_empty() {
                return Ok(());
            }
            sheet
                .write_string_with_format(row, 0, label, &styles.label)
                .map_err(err)?;
            sheet
                .write_string_with_format(row, 1, value.trim(), &styles.wrap)
                .map_err(err)?;
            let lines = estimated_lines(value.trim(), 78.0).min(MAX_WRAPPED_LINES);
            if lines > 1 {
                sheet
                    .set_row_height(row, lines as f64 * LINE_HEIGHT)
                    .map_err(err)?;
            }
            row += 1;
            Ok(())
        };

        pair(sheet, "Scope", report.summary.clone())?;
        pair(sheet, "Tested by", report.tested_by.clone())?;
        pair(sheet, "Generated", report.generated_at.clone())?;
        pair(sheet, "Branch", report.branch.clone())?;
        pair(sheet, "Commit", report.commit.clone())?;
        pair(sheet, "Commands run", report.test_commands.numbered())?;
        pair(sheet, "Not covered", report.not_covered.numbered())?;

        row += 1;
        sheet
            .write_string_with_format(row, 0, "Results", &styles.title)
            .map_err(err)?;
        row += 1;

        let tallies: [(&str, usize); 9] = [
            ("Test cases", counts.total),
            ("Passed", counts.passed),
            ("Failed", counts.failed),
            ("Blocked", counts.blocked),
            ("Skipped", counts.skipped),
            ("Not run", counts.not_run),
            ("Edge / negative cases", counts.edge_cases),
            ("Mobile checks", counts.mobile_cases),
            ("Defects (open)", counts.open_defects),
        ];
        for (label, value) in tallies {
            sheet
                .write_string_with_format(row, 0, label, &styles.label)
                .map_err(err)?;
            sheet
                .write_number_with_format(row, 1, value as f64, &styles.number)
                .map_err(err)?;
            row += 1;
        }

        sheet
            .write_string_with_format(row, 0, "Pass rate", &styles.label)
            .map_err(err)?;
        sheet
            .write_number_with_format(row, 1, counts.pass_rate, &styles.percent)
            .map_err(err)?;
        row += 2;

        let problems = report.problems();
        if !problems.is_empty() {
            sheet
                .write_string_with_format(row, 0, "Gaps in this report", &styles.title)
                .map_err(err)?;
            row += 1;
            for problem in problems {
                sheet
                    .write_string_with_format(row, 1, problem, &styles.wrap)
                    .map_err(err)?;
                row += 1;
            }
        }
    }

    // --- Test Cases --------------------------------------------------------
    {
        let sheet = workbook.add_worksheet();
        sheet.set_name("Test Cases").map_err(err)?;
        let columns: &[(&str, f64)] = &[
            ("ID", 10.0),
            ("Area", 18.0),
            ("Scenario", 40.0),
            ("Category", 16.0),
            ("Priority", 9.0),
            ("Platform", 14.0),
            ("Preconditions", 28.0),
            ("Steps", 46.0),
            ("Test data", 24.0),
            ("Expected result", 40.0),
            ("Actual result", 34.0),
            ("Status", 11.0),
            ("Severity", 11.0),
            ("Automated", 11.0),
            ("Test reference", 34.0),
            ("Defect", 11.0),
            ("Notes", 30.0),
        ];
        let rows = report
            .cases
            .iter()
            .map(|c| {
                vec![
                    text(c.id.trim()),
                    text(c.area.trim()),
                    wrap(c.title.trim()),
                    text(c.category.trim()),
                    text(c.priority.trim()),
                    text(c.platform.trim()),
                    wrap(c.preconditions.trim()),
                    wrap(c.steps.numbered()),
                    wrap(c.test_data.trim()),
                    wrap(c.expected.trim()),
                    wrap(c.actual.trim()),
                    Cell::Status(c.status.clone()),
                    text(c.severity.trim()),
                    text(c.automated.as_ref().map(|f| f.cell()).unwrap_or_default()),
                    wrap(c.test_ref.trim()),
                    text(c.defect_id.trim()),
                    wrap(c.notes.trim()),
                ]
            })
            .collect();
        write_table(sheet, &styles, columns, rows)?;
    }

    // --- Mobile Matrix -----------------------------------------------------
    {
        let sheet = workbook.add_worksheet();
        sheet.set_name("Mobile Matrix").map_err(err)?;
        let columns: &[(&str, f64)] = &[
            ("Check", 44.0),
            ("Device", 22.0),
            ("OS / version", 18.0),
            ("Viewport", 14.0),
            ("Orientation", 13.0),
            ("Network", 14.0),
            ("Result", 11.0),
            ("Notes", 40.0),
        ];
        let rows = report
            .mobile_checks
            .iter()
            .map(|m| {
                vec![
                    wrap(m.check.trim()),
                    text(m.device.trim()),
                    text(m.os_version.trim()),
                    text(m.viewport.trim()),
                    text(m.orientation.trim()),
                    text(m.network.trim()),
                    Cell::Status(m.result.clone()),
                    wrap(m.notes.trim()),
                ]
            })
            .collect();
        write_table(sheet, &styles, columns, rows)?;
    }

    // --- Defects -----------------------------------------------------------
    {
        let sheet = workbook.add_worksheet();
        sheet.set_name("Defects").map_err(err)?;
        let columns: &[(&str, f64)] = &[
            ("ID", 10.0),
            ("Title", 44.0),
            ("Severity", 11.0),
            ("Case", 11.0),
            ("Steps to reproduce", 46.0),
            ("Expected", 34.0),
            ("Actual", 34.0),
            ("Environment", 24.0),
            ("Status", 12.0),
            ("Notes", 30.0),
        ];
        let rows = report
            .defects
            .iter()
            .map(|d| {
                vec![
                    text(d.id.trim()),
                    wrap(d.title.trim()),
                    text(d.severity.trim()),
                    text(d.case_id.trim()),
                    wrap(d.steps.numbered()),
                    wrap(d.expected.trim()),
                    wrap(d.actual.trim()),
                    wrap(d.environment.trim()),
                    text(d.status.trim()),
                    wrap(d.notes.trim()),
                ]
            })
            .collect();
        write_table(sheet, &styles, columns, rows)?;
    }

    // --- Coverage ----------------------------------------------------------
    {
        let sheet = workbook.add_worksheet();
        sheet.set_name("Coverage").map_err(err)?;
        let columns: &[(&str, f64)] = &[
            ("Requirement / behaviour", 56.0),
            ("Covered by", 24.0),
            ("Covered", 11.0),
            ("Gap", 40.0),
            ("Notes", 30.0),
        ];
        let rows = report
            .coverage
            .iter()
            .map(|c| {
                vec![
                    wrap(c.requirement.trim()),
                    wrap(c.case_ids.inline()),
                    text(
                        c.covered
                            .as_ref()
                            .map(|f| f.cell())
                            // No flag, but cases listed and no gap named, reads
                            // as covered rather than as unknown.
                            .unwrap_or_else(|| {
                                if !c.case_ids.is_empty() && c.gap.trim().is_empty() {
                                    "Yes".to_string()
                                } else {
                                    String::new()
                                }
                            }),
                    ),
                    wrap(c.gap.trim()),
                    wrap(c.notes.trim()),
                ]
            })
            .collect();
        write_table(sheet, &styles, columns, rows)?;
    }

    // --- Environments ------------------------------------------------------
    {
        let sheet = workbook.add_worksheet();
        sheet.set_name("Environments").map_err(err)?;
        let columns: &[(&str, f64)] = &[
            ("Platform", 16.0),
            ("Device", 24.0),
            ("OS / version", 18.0),
            ("Browser", 22.0),
            ("Viewport", 14.0),
            ("Notes", 40.0),
        ];
        let rows = report
            .environments
            .iter()
            .map(|e| {
                vec![
                    text(e.platform.trim()),
                    text(e.device.trim()),
                    text(e.os_version.trim()),
                    text(e.browser.trim()),
                    text(e.viewport.trim()),
                    wrap(e.notes.trim()),
                ]
            })
            .collect();
        write_table(sheet, &styles, columns, rows)?;
    }

    workbook.save_to_buffer().map_err(err)
}

/// What an export actually produced.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedReport {
    pub file_path: String,
    pub sheets: Vec<String>,
    pub counts: ReportCounts,
    /// Carried through so the confirmation can repeat what is missing.
    pub problems: Vec<String>,
}

/// Turn a project's test report into a workbook on disk.
#[tauri::command]
pub async fn export_test_report(
    project_path: String,
    file_path: String,
) -> Result<ExportedReport, String> {
    let (_, report) = load_report(&project_path)?;
    let bytes = build_workbook(&report)?;

    std::fs::write(&file_path, &bytes)
        .map_err(|e| format!("Could not write {}: {}", file_path, e))?;

    Ok(ExportedReport {
        file_path,
        sheets: SHEET_NAMES.iter().map(|s| s.to_string()).collect(),
        counts: report.counts(),
        problems: report.problems(),
    })
}

/// The same workbook as bytes, for a caller that is not writing to this disk.
pub fn export_test_report_bytes(project_path: &str) -> Result<(String, Vec<u8>), String> {
    let (_, report) = load_report(project_path)?;
    let bytes = build_workbook(&report)?;
    Ok((suggested_file_name(&report), bytes))
}

/// A file name that says which feature and when, safe on every platform.
pub fn suggested_file_name(report: &TestReport) -> String {
    let stem: String = report
        .feature
        .trim()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let stem = stem.trim_matches('-').to_ascii_lowercase();
    let mut stem = stem.split('-').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("-");
    if stem.is_empty() {
        stem = "test-report".to_string();
    }
    // Long feature names make a name no dialog can show; the sheet has the rest.
    if stem.chars().count() > 60 {
        stem = stem.chars().take(60).collect::<String>().trim_end_matches('-').to_string();
    }
    format!("{}-test-doc.xlsx", stem)
}

/// Write the example report somewhere it can be opened and edited.
///
/// The schema is otherwise something you learn by having a run get it wrong.
#[tauri::command]
pub async fn save_test_report_template(file_path: String) -> Result<(), String> {
    std::fs::write(&file_path, TEST_REPORT_TEMPLATE_JSON)
        .map_err(|e| format!("Could not write {}: {}", file_path, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report_from(json: &str) -> TestReport {
        serde_json::from_str(json).expect("should parse")
    }

    #[test]
    fn statuses_are_read_however_they_are_written() {
        for raw in ["pass", "Passed", "PASS ✅", "ok", "  green  "] {
            assert_eq!(Outcome::parse(raw), Some(Outcome::Pass), "{raw}");
        }
        for raw in ["fail", "Failed", "❌ FAIL", "broken"] {
            assert_eq!(Outcome::parse(raw), Some(Outcome::Fail), "{raw}");
        }
        assert_eq!(Outcome::parse("blocked"), Some(Outcome::Blocked));
        assert_eq!(Outcome::parse("N/A"), Some(Outcome::Skipped));
        assert_eq!(Outcome::parse(""), Some(Outcome::NotRun));
    }

    #[test]
    fn a_status_nobody_recognises_is_reported_rather_than_counted_as_a_pass() {
        assert_eq!(Outcome::parse("mostly fine"), None);

        let report = report_from(
            r#"{ "feature": "F", "cases": [{ "id": "TC-1", "expected": "x", "status": "mostly fine" }] }"#,
        );
        assert_eq!(report.counts().passed, 0);
        assert_eq!(report.counts().not_run, 1);
        assert!(report
            .problems()
            .iter()
            .any(|p| p.contains("not understood") && p.contains("mostly fine")));
    }

    #[test]
    fn steps_read_the_same_as_one_string_or_as_a_list() {
        let as_list = report_from(
            r#"{ "cases": [{ "steps": ["Open the board", "Press Import"] }] }"#,
        );
        let as_text = report_from(
            r#"{ "cases": [{ "steps": "Open the board\nPress Import" }] }"#,
        );
        assert_eq!(
            as_list.cases[0].steps.numbered(),
            "1. Open the board\n2. Press Import"
        );
        assert_eq!(
            as_text.cases[0].steps.numbered(),
            as_list.cases[0].steps.numbered()
        );
    }

    #[test]
    fn a_list_that_numbered_itself_is_not_numbered_twice() {
        let report = report_from(r#"{ "cases": [{ "steps": ["1. Open it", "2. Close it"] }] }"#);
        assert_eq!(report.cases[0].steps.numbered(), "1. Open it\n2. Close it");
    }

    #[test]
    fn a_single_step_gets_no_number() {
        let report = report_from(r#"{ "cases": [{ "steps": ["Open it"] }] }"#);
        assert_eq!(report.cases[0].steps.numbered(), "Open it");
    }

    #[test]
    fn automated_reads_from_a_bool_or_the_word() {
        let report = report_from(
            r#"{ "cases": [{ "automated": true }, { "automated": "yes" }, { "automated": "manual" }, {}] }"#,
        );
        assert_eq!(report.cases[0].automated.as_ref().unwrap().cell(), "Yes");
        assert_eq!(report.cases[1].automated.as_ref().unwrap().cell(), "Yes");
        assert_eq!(report.cases[2].automated.as_ref().unwrap().cell(), "No");
        assert!(report.cases[3].automated.is_none());
    }

    #[test]
    fn the_headings_an_agent_is_likely_to_use_instead_are_accepted() {
        let report = report_from(
            r#"{
                 "title": "Import",
                 "scope": "the whole importer",
                 "test_cases": [{ "scenario": "empty file", "type": "edge case", "expected_result": "says so", "result": "pass" }],
                 "bugs": [{ "summary": "crash" }],
                 "devices": [{ "scenario": "at 320px", "status": "pass" }]
               }"#,
        );
        assert_eq!(report.feature, "Import");
        assert_eq!(report.summary, "the whole importer");
        assert_eq!(report.cases[0].title, "empty file");
        assert_eq!(report.cases[0].category, "edge case");
        assert_eq!(report.cases[0].expected, "says so");
        assert_eq!(report.counts().passed, 1);
        assert_eq!(report.defects[0].title, "crash");
        assert_eq!(report.mobile_checks[0].check, "at 320px");
    }

    #[test]
    fn a_report_with_only_a_feature_name_still_parses() {
        let report = report_from(r#"{ "feature": "Something" }"#);
        assert_eq!(report.counts().total, 0);
        assert!(report.problems().iter().any(|p| p.contains("No test cases")));
    }

    #[test]
    fn the_pass_rate_ignores_what_was_never_run() {
        let report = report_from(
            r#"{ "cases": [
                 { "status": "pass" }, { "status": "pass" }, { "status": "fail" },
                 { "status": "blocked" }, { "status": "not run" }
               ] }"#,
        );
        let counts = report.counts();
        assert_eq!(counts.total, 5);
        assert_eq!(counts.passed, 2);
        assert_eq!(counts.failed, 1);
        assert_eq!(counts.blocked, 1);
        assert_eq!(counts.not_run, 1);
        // Two of the three that ran, not two of five.
        assert!((counts.pass_rate - 2.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn a_report_with_nothing_run_has_no_pass_rate_rather_than_a_division_by_zero() {
        let report = report_from(r#"{ "cases": [{ "status": "not run" }] }"#);
        assert_eq!(report.counts().pass_rate, 0.0);
    }

    #[test]
    fn mobile_coverage_counts_both_the_cases_and_the_device_matrix() {
        let report = report_from(
            r#"{ "cases": [
                   { "platform": "iOS Safari" },
                   { "platform": "Desktop", "category": "responsive" },
                   { "platform": "Desktop" }
                 ],
                 "mobile_checks": [{ "device": "iPhone SE" }] }"#,
        );
        assert_eq!(report.counts().mobile_cases, 3);
    }

    #[test]
    fn a_report_that_never_looked_at_a_phone_says_so() {
        let report = report_from(
            r#"{ "feature": "F", "cases": [{ "id": "TC-1", "platform": "Desktop", "category": "edge", "expected": "x", "status": "pass" }] }"#,
        );
        assert!(report
            .problems()
            .iter()
            .any(|p| p.contains("Nothing mobile was checked")));
    }

    #[test]
    fn a_report_of_only_happy_paths_says_so() {
        let report = report_from(
            r#"{ "feature": "F",
                 "cases": [{ "id": "TC-1", "category": "happy path", "platform": "Android", "expected": "x", "status": "pass" }] }"#,
        );
        assert!(report
            .problems()
            .iter()
            .any(|p| p.contains("only happy paths")));
    }

    #[test]
    fn ids_used_twice_are_named() {
        let report = report_from(
            r#"{ "cases": [{ "id": "TC-1" }, { "id": "TC-1" }, { "id": "TC-2" }, { "id": "" }] }"#,
        );
        let problems = report.problems();
        assert!(problems.iter().any(|p| p.contains("more than once") && p.contains("TC-1")));
        assert!(problems.iter().any(|p| p.contains("without an id")));
    }

    #[test]
    fn a_defect_pointing_at_a_case_that_does_not_exist_is_named() {
        let report = report_from(
            r#"{ "cases": [{ "id": "TC-1" }], "defects": [{ "id": "BUG-1", "case_id": "TC-9" }] }"#,
        );
        assert!(report
            .problems()
            .iter()
            .any(|p| p.contains("BUG-1") && p.contains("TC-9")));
    }

    #[test]
    fn a_failure_with_no_defect_raised_is_named() {
        let report = report_from(
            r#"{ "feature": "F", "cases": [{ "id": "TC-1", "platform": "iOS", "category": "edge", "expected": "x", "status": "fail" }] }"#,
        );
        assert!(report
            .problems()
            .iter()
            .any(|p| p.contains("failed but no defect")));
    }

    #[test]
    fn a_fixed_defect_is_not_counted_as_open() {
        let report = report_from(
            r#"{ "defects": [
                   { "id": "B1", "status": "Open" },
                   { "id": "B2", "status": "Fixed" },
                   { "id": "B3", "status": "Won't fix" }
                 ] }"#,
        );
        let counts = report.counts();
        assert_eq!(counts.defects, 3);
        assert_eq!(counts.open_defects, 1);
    }

    #[test]
    fn a_good_report_has_nothing_to_complain_about() {
        let report = report_from(
            r#"{
                 "feature": "Ticket import",
                 "cases": [
                   { "id": "TC-1", "category": "happy path", "platform": "Web", "expected": "imports", "status": "pass" },
                   { "id": "TC-2", "category": "edge case", "platform": "iOS Safari", "expected": "says so", "status": "pass" }
                 ]
               }"#,
        );
        assert_eq!(report.problems(), Vec::<String>::new());
    }

    #[test]
    fn a_missing_report_names_the_file_the_agent_should_write() {
        let dir = tempfile::tempdir().unwrap();
        let error = load_report(dir.path().to_str().unwrap()).unwrap_err();
        assert!(error.contains(REPORT_RELATIVE_PATH), "{error}");
    }

    #[test]
    fn the_report_is_found_under_the_neighbouring_names_too() {
        for name in [".opcode/test-report.json", ".opcode/test_report.json", "test-report.json"] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, r#"{ "feature": "F" }"#).unwrap();

            let (found, report) = load_report(dir.path().to_str().unwrap()).unwrap();
            assert_eq!(found, path, "{name}");
            assert_eq!(report.feature, "F");
        }
    }

    #[test]
    fn a_report_that_is_there_but_unreadable_is_a_different_state_from_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".opcode/test-report.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "not json at all").unwrap();

        let status = tokio_block(test_report_status(dir.path().to_str().unwrap().to_string()));
        let status = status.unwrap();
        assert!(status.found);
        assert!(status.error.is_some());
        assert_eq!(status.path, Some(path.display().to_string()));
    }

    #[test]
    fn no_report_at_all_reports_where_one_should_go() {
        let dir = tempfile::tempdir().unwrap();
        let status =
            tokio_block(test_report_status(dir.path().to_str().unwrap().to_string())).unwrap();
        assert!(!status.found);
        assert!(status.error.is_none());
        assert_eq!(status.expected_path, REPORT_RELATIVE_PATH);
    }

    #[test]
    fn the_bundled_example_parses_and_is_a_clean_report() {
        let report: TestReport =
            serde_json::from_str(TEST_REPORT_TEMPLATE_JSON).expect("the example should parse");
        assert!(report.counts().total > 0);
        assert!(report.counts().mobile_cases > 0);
        assert!(report.counts().edge_cases > 0);
        assert_eq!(
            report.problems(),
            Vec::<String>::new(),
            "the example a person copies should not itself be flagged"
        );
    }

    #[test]
    fn the_workbook_has_every_sheet_even_when_the_report_is_thin() {
        let report = report_from(r#"{ "feature": "F", "cases": [{ "id": "TC-1" }] }"#);
        let bytes = build_workbook(&report).expect("should build");

        // A real .xlsx is a zip, and every named sheet has to be in it.
        assert_eq!(&bytes[..2], b"PK", "should be a zip");
        let names = sheet_names_in(&bytes);
        for expected in SHEET_NAMES {
            assert!(names.iter().any(|n| n == expected), "missing {expected}: {names:?}");
        }
    }

    #[test]
    fn the_workbook_reads_back_with_the_rows_and_statuses_it_was_given() {
        let report = report_from(
            r#"{
                 "feature": "Ticket import",
                 "cases": [
                   { "id": "TC-1", "title": "a csv imports", "expected": "tickets appear", "status": "ok" },
                   { "id": "TC-2", "title": "an empty file", "expected": "says so", "status": "Failed" }
                 ]
               }"#,
        );
        let bytes = build_workbook(&report).expect("should build");
        let rows = read_sheet(&bytes, "Test Cases");

        assert_eq!(rows[0][0], "ID");
        assert_eq!(rows[1][0], "TC-1");
        assert_eq!(rows[1][2], "a csv imports");
        // "ok" and "Failed" are stored as the canonical words, so the column filters.
        assert_eq!(rows[1][11], "Pass");
        assert_eq!(rows[2][11], "Fail");
    }

    #[test]
    fn the_whole_path_works_from_a_project_directory_to_a_saved_workbook() {
        let project = tempfile::tempdir().unwrap();
        let report_path = project.path().join(REPORT_RELATIVE_PATH);
        std::fs::create_dir_all(report_path.parent().unwrap()).unwrap();
        std::fs::write(&report_path, TEST_REPORT_TEMPLATE_JSON).unwrap();

        let out = project.path().join("doc.xlsx");
        let exported = tokio_block(export_test_report(
            project.path().to_str().unwrap().to_string(),
            out.to_str().unwrap().to_string(),
        ))
        .expect("should export");

        assert_eq!(exported.sheets, SHEET_NAMES);
        assert!(exported.counts.total > 0);
        assert!(out.is_file());

        let bytes = std::fs::read(&out).unwrap();
        let cases = read_sheet(&bytes, "Test Cases");
        // The heading row plus one row per case, and nothing lost in between.
        assert_eq!(cases.len(), exported.counts.total + 1);

        let mobile = read_sheet(&bytes, "Mobile Matrix");
        assert!(mobile.len() > 1, "the device matrix should have rows");

        let summary = read_sheet(&bytes, "Summary");
        assert_eq!(summary[0][0], "Ticket import from a spreadsheet");
    }

    #[test]
    fn exporting_without_a_report_fails_before_it_writes_anything() {
        let project = tempfile::tempdir().unwrap();
        let out = project.path().join("doc.xlsx");

        let error = tokio_block(export_test_report(
            project.path().to_str().unwrap().to_string(),
            out.to_str().unwrap().to_string(),
        ))
        .unwrap_err();

        assert!(error.contains(REPORT_RELATIVE_PATH), "{error}");
        assert!(!out.exists(), "a failed export should leave no half-written file");
    }

    #[test]
    fn the_bytes_the_web_server_sends_are_the_same_workbook_under_a_named_file() {
        let project = tempfile::tempdir().unwrap();
        let report_path = project.path().join(REPORT_RELATIVE_PATH);
        std::fs::create_dir_all(report_path.parent().unwrap()).unwrap();
        std::fs::write(&report_path, TEST_REPORT_TEMPLATE_JSON).unwrap();

        let (file_name, bytes) =
            export_test_report_bytes(project.path().to_str().unwrap()).expect("should build");

        assert_eq!(file_name, "ticket-import-from-a-spreadsheet-test-doc.xlsx");
        assert_eq!(&bytes[..2], b"PK");
        for expected in SHEET_NAMES {
            assert!(sheet_names_in(&bytes).iter().any(|n| n == expected), "missing {expected}");
        }
    }

    #[test]
    fn a_wrapped_row_is_grown_to_fit_and_a_short_one_is_left_alone() {
        // Excel shows one clipped line for a wrapped cell of default height,
        // so the estimate is what makes a long expected-result readable.
        assert_eq!(estimated_lines("short", 40.0), 1);
        assert_eq!(estimated_lines("a\nb\nc", 40.0), 3);
        assert!(estimated_lines(&"x".repeat(200), 40.0) >= 5);
        // A narrow column does not divide by zero or go negative.
        assert!(estimated_lines("some text", 1.0) >= 1);
        assert_eq!(estimated_lines("", 40.0), 1);
    }

    #[test]
    fn the_file_name_says_which_feature_it_is() {
        let report = report_from(r#"{ "feature": "Ticket import / board" }"#);
        assert_eq!(suggested_file_name(&report), "ticket-import-board-test-doc.xlsx");

        let unnamed = report_from(r#"{}"#);
        assert_eq!(suggested_file_name(&unnamed), "test-report-test-doc.xlsx");
    }

    #[test]
    fn a_very_long_feature_name_is_cut_to_something_a_dialog_can_show() {
        let report = report_from(&format!(r#"{{ "feature": "{}" }}"#, "word ".repeat(40)));
        let name = suggested_file_name(&report);
        assert!(name.chars().count() <= 60 + "-test-doc.xlsx".len(), "{name}");
        assert!(name.ends_with("-test-doc.xlsx"));
    }

    // -- helpers ------------------------------------------------------------

    /// Run one of the async commands from a synchronous test.
    fn tokio_block<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(f)
    }

    fn sheet_names_in(bytes: &[u8]) -> Vec<String> {
        use calamine::Reader;
        let cursor = std::io::Cursor::new(bytes.to_vec());
        let workbook = calamine::open_workbook_auto_from_rs(cursor).expect("should open");
        workbook.sheet_names().to_vec()
    }

    fn read_sheet(bytes: &[u8], name: &str) -> Vec<Vec<String>> {
        use calamine::{Data, Reader};
        let cursor = std::io::Cursor::new(bytes.to_vec());
        let mut workbook = calamine::open_workbook_auto_from_rs(cursor).expect("should open");
        let range = workbook.worksheet_range(name).expect("sheet should exist");
        range
            .rows()
            .map(|row| {
                row.iter()
                    .map(|cell| match cell {
                        Data::Empty => String::new(),
                        Data::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .collect()
            })
            .collect()
    }
}
