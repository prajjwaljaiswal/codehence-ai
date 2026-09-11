use anyhow::Result;
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
/// Shared module for detecting Claude Code binary installations
/// Supports NVM installations, aliased paths, and version-based selection
use std::path::PathBuf;
use std::process::Command;
use tauri::Manager;

// ---------------------------------------------------------------------------
// Platform differences that decide whether a spawn works at all
// ---------------------------------------------------------------------------

/// The character PATH is joined with on this platform.
#[cfg(windows)]
const PATH_SEPARATOR: &str = ";";
#[cfg(not(windows))]
const PATH_SEPARATOR: &str = ":";

/// The extensions Windows will start a file under, when PATHEXT is unreadable.
#[cfg_attr(not(windows), allow(dead_code))]
const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

/// Whether Windows can actually start a file with this name.
///
/// `npm i -g @anthropic-ai/claude-code` leaves three shims side by side in
/// `%APPDATA%\npm`: `claude` (a POSIX sh script), `claude.ps1`, and
/// `claude.cmd`. Only the last is launchable, and `where claude` lists the
/// extensionless script *first* - so taking the first line hands back a file
/// that fails to spawn with "%1 is not a valid Win32 application" (os error
/// 193). That is what an agent run which dies in 0.00s with no output hit.
///
/// Takes PATHEXT as an argument so it can be tested away from Windows, which
/// is where this logic is hardest to exercise and easiest to get wrong.
#[cfg_attr(all(not(windows), not(test)), allow(dead_code))]
fn has_launchable_extension(path: &str, pathext: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    pathext
        .to_ascii_lowercase()
        .split(';')
        .map(str::trim)
        .filter(|ext| !ext.is_empty())
        .any(|ext| lower.ends_with(ext))
}

/// Pick the first line of `where` output that Windows could start, and report
/// the ones passed over so the log says why.
#[cfg_attr(all(not(windows), not(test)), allow(dead_code))]
fn first_launchable(where_output: &str, pathext: &str) -> (Option<String>, Vec<String>) {
    let mut skipped = Vec::new();
    for line in where_output.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if has_launchable_extension(line, pathext) {
            return (Some(line.to_string()), skipped);
        }
        skipped.push(line.to_string());
    }
    (None, skipped)
}

/// Whether Windows would have to go through cmd.exe to start this program.
fn is_batch_shim(program: &str) -> bool {
    let lower = program.to_ascii_lowercase();
    lower.ends_with(".cmd") || lower.ends_with(".bat")
}

/// Whether this token is already a full path rather than one relative to the
/// shim. Spelled out by hand because `Path::is_absolute` does not recognise a
/// `C:\` prefix anywhere except Windows, and this has to be testable on macOS.
#[cfg_attr(all(not(windows), not(test)), allow(dead_code))]
fn looks_absolute(token: &str) -> bool {
    let bytes = token.as_bytes();
    token.starts_with('\\')
        || token.starts_with('/')
        || (bytes.len() > 2 && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/'))
}

/// Resolve a path as written inside a batch shim against the shim's directory,
/// expanding the `%dp0%` / `%~dp0` the shim uses to mean "next to me".
#[cfg_attr(all(not(windows), not(test)), allow(dead_code))]
fn expand_shim_path(token: &str, shim_dir: &std::path::Path) -> PathBuf {
    let lower = token.to_ascii_lowercase();
    for marker in ["%dp0%", "%~dp0"] {
        if let Some(at) = lower.find(marker) {
            let rest = token[at + marker.len()..].trim_start_matches(['\\', '/']);
            return shim_dir.join(rest.replace('\\', std::path::MAIN_SEPARATOR_STR));
        }
    }
    if looks_absolute(token) {
        PathBuf::from(token)
    } else {
        shim_dir.join(token.replace('\\', std::path::MAIN_SEPARATOR_STR))
    }
}

/// The `.js` entry points a Windows batch shim could be handing to Node.
///
/// npm, yarn and pnpm all write the same shape of `.cmd` shim, ending in a
/// line like `"%_prog%" "%dp0%\node_modules\@anthropic-ai\claude-code\cli.js"
/// %*`. Pulling that path back out lets Node be started with it directly.
#[cfg_attr(all(not(windows), not(test)), allow(dead_code))]
fn js_entries_in_shim(shim_text: &str, shim_dir: &std::path::Path) -> Vec<PathBuf> {
    shim_text
        .split(['"', '\'', ' ', '\t', '\r', '\n'])
        .map(str::trim)
        .filter(|token| token.to_ascii_lowercase().ends_with(".js"))
        .map(|token| expand_shim_path(token, shim_dir))
        .collect()
}

/// The directory a program lives in, looking it up on PATH when it was given
/// as a bare name (`find_standard_installations` reports `claude.cmd` that
/// way when it is only known to be somewhere on PATH).
#[cfg(windows)]
fn program_directory(program: &str) -> Option<PathBuf> {
    if let Some(parent) = std::path::Path::new(program).parent() {
        if !parent.as_os_str().is_empty() {
            return Some(parent.to_path_buf());
        }
    }
    std::env::var("PATH")
        .ok()?
        .split(PATH_SEPARATOR)
        .filter(|dir| !dir.is_empty())
        .map(std::path::Path::new)
        .find(|dir| dir.join(program).is_file())
        .map(|dir| dir.to_path_buf())
}

/// A `node.exe` that can run the shim's script.
#[cfg(windows)]
fn find_node_exe(shim_dir: &std::path::Path) -> Option<PathBuf> {
    // nvm-windows and the official installer put node.exe in the same
    // directory as the global npm shims, which is the first place the shim
    // itself looks.
    let beside = shim_dir.join("node.exe");
    if beside.is_file() {
        return Some(beside);
    }
    std::env::var("PATH")
        .ok()?
        .split(PATH_SEPARATOR)
        .filter(|dir| !dir.is_empty())
        .map(|dir| std::path::Path::new(dir).join("node.exe"))
        .find(|candidate| candidate.is_file())
}

/// Node plus the script to hand it, when Claude is installed as a batch shim.
///
/// Rust refuses to build a command line for a `.cmd`/`.bat` program if any
/// argument contains a newline: the spawn fails with "batch file arguments are
/// invalid" and nothing starts. Every agent system prompt is multi-line (and
/// the Tester's is ~30 KB), so every agent run through the npm `claude.cmd`
/// died there in 0.00s - as does any chat prompt typed across more than one
/// line. cmd.exe would also truncate at its 8191-character command line limit
/// and expand any `%VAR%` inside the prompt.
///
/// The shim exists only to call `node cli.js`, so call that directly and none
/// of the above applies - CreateProcess passes arguments through verbatim.
#[cfg(windows)]
fn node_launcher(program: &str) -> Option<(PathBuf, PathBuf)> {
    if !is_batch_shim(program) {
        return None;
    }
    let dir = program_directory(program)?;
    let shim = dir.join(std::path::Path::new(program).file_name()?);
    let script = std::fs::read_to_string(&shim)
        .ok()
        .and_then(|text| js_entries_in_shim(&text, &dir).into_iter().find(|p| p.is_file()))
        .or_else(|| {
            // A shim that does not spell the path out the usual way still has
            // the package in one of the two standard places.
            [
                "node_modules/@anthropic-ai/claude-code/cli.js",
                "../node_modules/@anthropic-ai/claude-code/cli.js",
            ]
            .iter()
            .map(|rel| dir.join(rel))
            .find(|p| p.is_file())
        })?;
    let node = find_node_exe(&dir)?;
    Some((node, script))
}

#[cfg(not(windows))]
fn node_launcher(_program: &str) -> Option<(PathBuf, PathBuf)> {
    None
}

/// Whether this environment variable should reach the child process.
///
/// The child is Claude Code, which needs to find Node, its own config, and the
/// network. The lists differ per platform and getting the Windows one wrong is
/// not a degraded experience but a hard failure: a process started without
/// `SystemRoot` cannot initialise winsock, and one without `APPDATA` /
/// `USERPROFILE` cannot find `~/.claude`.
fn should_inherit(key: &str) -> bool {
    const SHARED: &[&str] = &[
        "PATH",
        "LANG",
        "LC_ALL",
        "NODE_PATH",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "NO_PROXY",
        "ALL_PROXY",
    ];
    #[cfg(windows)]
    const PLATFORM: &[&str] = &[
        // Without these two, CreateProcess and winsock fail in ways that look
        // like Claude itself crashing.
        "SystemRoot",
        "windir",
        "ComSpec",
        // Resolving `claude.cmd` and `~/.claude` needs all of these.
        "PATHEXT",
        "APPDATA",
        "LOCALAPPDATA",
        "USERPROFILE",
        "HOMEDRIVE",
        "HOMEPATH",
        "TEMP",
        "TMP",
        "PROGRAMFILES",
        "PROGRAMFILES(X86)",
        "PROGRAMDATA",
        "NUMBER_OF_PROCESSORS",
        "PROCESSOR_ARCHITECTURE",
        "OS",
    ];
    #[cfg(not(windows))]
    const PLATFORM: &[&str] = &[
        "HOME",
        "USER",
        "SHELL",
        "NVM_DIR",
        "NVM_BIN",
        "HOMEBREW_PREFIX",
        "HOMEBREW_CELLAR",
    ];

    // Windows environment names are case-insensitive, so `Path` and `PATH` are
    // the same variable and either spelling has to match.
    SHARED.iter().chain(PLATFORM).any(|known| known.eq_ignore_ascii_case(key))
        || key.starts_with("LC_")
}

/// A `Command` for `program` that this platform can actually start.
///
/// On Windows this also suppresses the console window that would otherwise
/// flash up for every spawn, including the version probes done at startup.
pub fn command_for(program: &str) -> Command {
    // `mut` is only needed for the Windows creation flag below.
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        /// Do not allocate a console for the child.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Type of Claude installation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum InstallationType {
    /// System-installed binary
    System,
    /// Custom path specified by user
    Custom,
}

/// Represents a Claude installation with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeInstallation {
    /// Full path to the Claude binary
    pub path: String,
    /// Version string if available
    pub version: Option<String>,
    /// Source of discovery (e.g., "nvm", "system", "homebrew", "which")
    pub source: String,
    /// Type of installation
    pub installation_type: InstallationType,
}

/// Main function to find the Claude binary
/// Checks database first for stored path and preference, then prioritizes accordingly
pub fn find_claude_binary(app_handle: &tauri::AppHandle) -> Result<String, String> {
    info!("Searching for claude binary...");

    // First check if we have a stored path and preference in the database
    if let Ok(app_data_dir) = app_handle.path().app_data_dir() {
        let db_path = app_data_dir.join("agents.db");
        if db_path.exists() {
            if let Ok(conn) = rusqlite::Connection::open(&db_path) {
                // Check for stored path first
                if let Ok(stored_path) = conn.query_row(
                    "SELECT value FROM app_settings WHERE key = 'claude_binary_path'",
                    [],
                    |row| row.get::<_, String>(0),
                ) {
                    info!("Found stored claude path in database: {}", stored_path);

                    // Check if the path still exists
                    let path_buf = PathBuf::from(&stored_path);
                    if path_buf.exists() && path_buf.is_file() {
                        return Ok(stored_path);
                    } else {
                        warn!("Stored claude path no longer exists: {}", stored_path);
                    }
                }

                // Check user preference
                let preference = conn.query_row(
                    "SELECT value FROM app_settings WHERE key = 'claude_installation_preference'",
                    [],
                    |row| row.get::<_, String>(0),
                ).unwrap_or_else(|_| "system".to_string());

                info!("User preference for Claude installation: {}", preference);
            }
        }
    }

    // Discover all available system installations
    let installations = discover_system_installations();

    if installations.is_empty() {
        error!("Could not find claude binary in any location");
        // The searched locations differ per platform, so listing the Unix
        // ones on Windows just sends the user looking in the wrong places.
        #[cfg(windows)]
        return Err("Claude Code not found. Install it (npm i -g @anthropic-ai/claude-code) so that `claude` is on your PATH, or set the binary path in Settings.".to_string());
        #[cfg(not(windows))]
        return Err("Claude Code not found. Please ensure it's installed in one of these locations: PATH, /usr/local/bin, /opt/homebrew/bin, ~/.nvm/versions/node/*/bin, ~/.claude/local, ~/.local/bin — or set the binary path in Settings.".to_string());
    }

    // Log all found installations
    for installation in &installations {
        info!("Found Claude installation: {:?}", installation);
    }

    // Select the best installation (highest version)
    if let Some(best) = select_best_installation(installations) {
        info!(
            "Selected Claude installation: path={}, version={:?}, source={}",
            best.path, best.version, best.source
        );
        Ok(best.path)
    } else {
        Err("No valid Claude installation found".to_string())
    }
}

/// Discovers all available Claude installations and returns them for selection
/// This allows UI to show a version selector
pub fn discover_claude_installations() -> Vec<ClaudeInstallation> {
    info!("Discovering all Claude installations...");

    let mut installations = discover_system_installations();

    // Sort by version (highest first), then by source preference
    installations.sort_by(|a, b| {
        match (&a.version, &b.version) {
            (Some(v1), Some(v2)) => {
                // Compare versions in descending order (newest first)
                match compare_versions(v2, v1) {
                    Ordering::Equal => {
                        // If versions are equal, prefer by source
                        source_preference(a).cmp(&source_preference(b))
                    }
                    other => other,
                }
            }
            (Some(_), None) => Ordering::Less, // Version comes before no version
            (None, Some(_)) => Ordering::Greater,
            (None, None) => source_preference(a).cmp(&source_preference(b)),
        }
    });

    installations
}

/// Returns a preference score for installation sources (lower is better)
fn source_preference(installation: &ClaudeInstallation) -> u8 {
    match installation.source.as_str() {
        "which" => 1,
        "homebrew" => 2,
        "system" => 3,
        "nvm-active" => 4,
        source if source.starts_with("nvm") => 5,
        "local-bin" => 6,
        "claude-local" => 7,
        "npm-global" => 8,
        "yarn" | "yarn-global" => 9,
        "bun" => 10,
        "node-modules" => 11,
        "home-bin" => 12,
        "PATH" => 13,
        _ => 14,
    }
}

/// Discovers all Claude installations on the system
fn discover_system_installations() -> Vec<ClaudeInstallation> {
    let mut installations = Vec::new();

    // 1. Try 'which' command first (now works in production)
    if let Some(installation) = try_which_command() {
        installations.push(installation);
    }

    // 2. Check NVM paths (includes current active NVM)
    installations.extend(find_nvm_installations());

    // 3. Check standard paths
    installations.extend(find_standard_installations());

    // Remove duplicates by path
    let mut unique_paths = std::collections::HashSet::new();
    installations.retain(|install| unique_paths.insert(install.path.clone()));

    installations
}

/// Try using the 'which' command to find Claude
#[cfg(unix)]
fn try_which_command() -> Option<ClaudeInstallation> {
    debug!("Trying 'which claude' to find binary...");

    match Command::new("which").arg("claude").output() {
        Ok(output) if output.status.success() => {
            let output_str = String::from_utf8_lossy(&output.stdout).trim().to_string();

            if output_str.is_empty() {
                return None;
            }

            // Parse aliased output: "claude: aliased to /path/to/claude"
            let path = if output_str.starts_with("claude:") && output_str.contains("aliased to") {
                output_str
                    .split("aliased to")
                    .nth(1)
                    .map(|s| s.trim().to_string())
            } else {
                Some(output_str)
            }?;

            debug!("'which' found claude at: {}", path);

            // Verify the path exists
            if !PathBuf::from(&path).exists() {
                warn!("Path from 'which' does not exist: {}", path);
                return None;
            }

            // Get version
            let version = get_claude_version(&path).ok().flatten();

            Some(ClaudeInstallation {
                path,
                version,
                source: "which".to_string(),
                installation_type: InstallationType::System,
            })
        }
        _ => None,
    }
}

#[cfg(windows)]
fn try_which_command() -> Option<ClaudeInstallation> {
    debug!("Trying 'where claude' to find binary...");

    match Command::new("where").arg("claude").output() {
        Ok(output) if output.status.success() => {
            let output_str = String::from_utf8_lossy(&output.stdout).trim().to_string();

            if output_str.is_empty() {
                return None;
            }

            // `where` returns every match, newline-separated, and lists the
            // extensionless npm shim before the `.cmd` that can actually be
            // launched - so this takes the first launchable line, not the
            // first line.
            let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| DEFAULT_PATHEXT.to_string());
            let (path, skipped) = first_launchable(&output_str, &pathext);

            if !skipped.is_empty() {
                debug!(
                    "'where' returned shims Windows cannot start, passing over them: {:?}",
                    skipped
                );
            }

            let path = path?;

            debug!("'where' found claude at: {}", path);

            // Verify the path exists
            if !PathBuf::from(&path).exists() {
                warn!("Path from 'where' does not exist: {}", path);
                return None;
            }

            // Get version
            let version = get_claude_version(&path).ok().flatten();

            Some(ClaudeInstallation {
                path,
                version,
                source: "where".to_string(),
                installation_type: InstallationType::System,
            })
        }
        _ => None,
    }
}

/// Find Claude installations in NVM directories
#[cfg(unix)]
fn find_nvm_installations() -> Vec<ClaudeInstallation> {
    let mut installations = Vec::new();

    // First check NVM_BIN environment variable (current active NVM)
    if let Ok(nvm_bin) = std::env::var("NVM_BIN") {
        let claude_path = PathBuf::from(&nvm_bin).join("claude");
        if claude_path.exists() && claude_path.is_file() {
            debug!("Found Claude via NVM_BIN: {:?}", claude_path);
            let version = get_claude_version(&claude_path.to_string_lossy())
                .ok()
                .flatten();
            installations.push(ClaudeInstallation {
                path: claude_path.to_string_lossy().to_string(),
                version,
                source: "nvm-active".to_string(),
                installation_type: InstallationType::System,
            });
        }
    }

    // Then check all NVM directories
    if let Ok(home) = std::env::var("HOME") {
        let nvm_dir = PathBuf::from(&home)
            .join(".nvm")
            .join("versions")
            .join("node");

        debug!("Checking NVM directory: {:?}", nvm_dir);

        if let Ok(entries) = std::fs::read_dir(&nvm_dir) {
            for entry in entries.flatten() {
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    let claude_path = entry.path().join("bin").join("claude");

                    if claude_path.exists() && claude_path.is_file() {
                        let path_str = claude_path.to_string_lossy().to_string();
                        let node_version = entry.file_name().to_string_lossy().to_string();

                        debug!("Found Claude in NVM node {}: {}", node_version, path_str);

                        // Get Claude version
                        let version = get_claude_version(&path_str).ok().flatten();

                        installations.push(ClaudeInstallation {
                            path: path_str,
                            version,
                            source: format!("nvm ({})", node_version),
                            installation_type: InstallationType::System,
                        });
                    }
                }
            }
        }
    }

    installations
}

#[cfg(windows)]
fn find_nvm_installations() -> Vec<ClaudeInstallation> {
    let mut installations = Vec::new();

    if let Ok(nvm_home) = std::env::var("NVM_HOME") {
        debug!("Checking NVM_HOME directory: {:?}", nvm_home);

        if let Ok(entries) = std::fs::read_dir(&nvm_home) {
            for entry in entries.flatten() {
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    let claude_path = entry.path().join("claude.exe");

                    if claude_path.exists() && claude_path.is_file() {
                        let path_str = claude_path.to_string_lossy().to_string();
                        let node_version = entry.file_name().to_string_lossy().to_string();

                        debug!("Found Claude in NVM node {}: {}", node_version, path_str);

                        // Get Claude version
                        let version = get_claude_version(&path_str).ok().flatten();

                        installations.push(ClaudeInstallation {
                            path: path_str,
                            version,
                            source: format!("nvm ({})", node_version),
                            installation_type: InstallationType::System,
                        });
                    }
                }
            }
        }
    }

    installations
}

/// Check standard installation paths
#[cfg(unix)]
fn find_standard_installations() -> Vec<ClaudeInstallation> {
    let mut installations = Vec::new();

    // Common installation paths for claude
    let mut paths_to_check: Vec<(String, String)> = vec![
        ("/usr/local/bin/claude".to_string(), "system".to_string()),
        (
            "/opt/homebrew/bin/claude".to_string(),
            "homebrew".to_string(),
        ),
        ("/usr/bin/claude".to_string(), "system".to_string()),
        ("/bin/claude".to_string(), "system".to_string()),
    ];

    // Also check user-specific paths
    if let Ok(home) = std::env::var("HOME") {
        paths_to_check.extend(vec![
            (
                format!("{}/.claude/local/claude", home),
                "claude-local".to_string(),
            ),
            (
                format!("{}/.local/bin/claude", home),
                "local-bin".to_string(),
            ),
            (
                format!("{}/.npm-global/bin/claude", home),
                "npm-global".to_string(),
            ),
            (format!("{}/.yarn/bin/claude", home), "yarn".to_string()),
            (format!("{}/.bun/bin/claude", home), "bun".to_string()),
            (format!("{}/bin/claude", home), "home-bin".to_string()),
            // Check common node_modules locations
            (
                format!("{}/node_modules/.bin/claude", home),
                "node-modules".to_string(),
            ),
            (
                format!("{}/.config/yarn/global/node_modules/.bin/claude", home),
                "yarn-global".to_string(),
            ),
        ]);
    }

    // Check each path
    for (path, source) in paths_to_check {
        let path_buf = PathBuf::from(&path);
        if path_buf.exists() && path_buf.is_file() {
            debug!("Found claude at standard path: {} ({})", path, source);

            // Get version
            let version = get_claude_version(&path).ok().flatten();

            installations.push(ClaudeInstallation {
                path,
                version,
                source,
                installation_type: InstallationType::System,
            });
        }
    }

    // Also check if claude is available in PATH (without full path)
    if let Ok(output) = command_for("claude").arg("--version").output() {
        if output.status.success() {
            debug!("claude is available in PATH");
            let version = extract_version_from_output(&output.stdout);

            installations.push(ClaudeInstallation {
                path: "claude".to_string(),
                version,
                source: "PATH".to_string(),
                installation_type: InstallationType::System,
            });
        }
    }

    installations
}

#[cfg(windows)]
fn find_standard_installations() -> Vec<ClaudeInstallation> {
    let mut installations = Vec::new();

    // Common installation paths for claude on Windows
    let mut paths_to_check: Vec<(String, String)> = vec![];

    // Check user-specific paths
    if let Ok(user_profile) = std::env::var("USERPROFILE") {
        paths_to_check.extend(vec![
            (
                format!("{}\\.claude\\local\\claude.exe", user_profile),
                "claude-local".to_string(),
            ),
            (
                format!("{}\\.local\\bin\\claude.exe", user_profile),
                "local-bin".to_string(),
            ),
            (
                format!("{}\\AppData\\Roaming\\npm\\claude.cmd", user_profile),
                "npm-global".to_string(),
            ),
            (
                format!("{}\\.yarn\\bin\\claude.cmd", user_profile),
                "yarn".to_string(),
            ),
            (
                format!("{}\\.bun\\bin\\claude.exe", user_profile),
                "bun".to_string(),
            ),
        ]);
    }

    // Check each path
    for (path, source) in paths_to_check {
        let path_buf = PathBuf::from(&path);
        if path_buf.exists() && path_buf.is_file() {
            debug!("Found claude at standard path: {} ({})", path, source);

            // Get version
            let version = get_claude_version(&path).ok().flatten();

            installations.push(ClaudeInstallation {
                path,
                version,
                source,
                installation_type: InstallationType::System,
            });
        }
    }

    // Also check whether claude is on PATH under either name it ships as. An
    // npm global install has no `claude.exe` at all - only `claude.cmd` - so
    // probing for the executable alone misses the most common install.
    for name in ["claude.cmd", "claude.exe"] {
        if let Ok(output) = command_for(name).arg("--version").output() {
            if output.status.success() {
                debug!("{} is available in PATH", name);
                let version = extract_version_from_output(&output.stdout);

                installations.push(ClaudeInstallation {
                    path: name.to_string(),
                    version,
                    source: "PATH".to_string(),
                    installation_type: InstallationType::System,
                });
            }
        }
    }

    installations
}

/// Get Claude version by running --version command
fn get_claude_version(path: &str) -> Result<Option<String>, String> {
    match command_for(path).arg("--version").output() {
        Ok(output) => {
            if output.status.success() {
                Ok(extract_version_from_output(&output.stdout))
            } else {
                Ok(None)
            }
        }
        Err(e) => {
            warn!("Failed to get version for {}: {}", path, e);
            Ok(None)
        }
    }
}

/// Extract version string from command output
fn extract_version_from_output(stdout: &[u8]) -> Option<String> {
    let output_str = String::from_utf8_lossy(stdout);

    // Debug log the raw output
    debug!("Raw version output: {:?}", output_str);

    // Use regex to directly extract version pattern (e.g., "1.0.41")
    // This pattern matches:
    // - One or more digits, followed by
    // - A dot, followed by
    // - One or more digits, followed by
    // - A dot, followed by
    // - One or more digits
    // - Optionally followed by pre-release/build metadata
    let version_regex =
        regex::Regex::new(r"(\d+\.\d+\.\d+(?:-[a-zA-Z0-9.-]+)?(?:\+[a-zA-Z0-9.-]+)?)").ok()?;

    if let Some(captures) = version_regex.captures(&output_str) {
        if let Some(version_match) = captures.get(1) {
            let version = version_match.as_str().to_string();
            debug!("Extracted version: {:?}", version);
            return Some(version);
        }
    }

    debug!("No version found in output");
    None
}

/// Select the best installation based on version
fn select_best_installation(installations: Vec<ClaudeInstallation>) -> Option<ClaudeInstallation> {
    // In production builds, version information may not be retrievable because
    // spawning external processes can be restricted. We therefore no longer
    // discard installations that lack a detected version – the mere presence
    // of a readable binary on disk is enough to consider it valid. We still
    // prefer binaries with version information when it is available so that
    // in development builds we keep the previous behaviour of picking the
    // most recent version.
    installations.into_iter().max_by(|a, b| {
        match (&a.version, &b.version) {
            // If both have versions, compare them semantically.
            (Some(v1), Some(v2)) => compare_versions(v1, v2),
            // Prefer the entry that actually has version information.
            (Some(_), None) => Ordering::Greater,
            (None, Some(_)) => Ordering::Less,
            // Neither have version info: prefer the one that is not just
            // the bare "claude" lookup from PATH, because that may fail
            // at runtime if PATH is modified.
            (None, None) => {
                if a.path == "claude" && b.path != "claude" {
                    Ordering::Less
                } else if a.path != "claude" && b.path == "claude" {
                    Ordering::Greater
                } else {
                    Ordering::Equal
                }
            }
        }
    })
}

/// Compare two version strings
fn compare_versions(a: &str, b: &str) -> Ordering {
    // Simple semantic version comparison
    let a_parts: Vec<u32> = a
        .split('.')
        .filter_map(|s| {
            // Handle versions like "1.0.17-beta" by taking only numeric part
            s.chars()
                .take_while(|c| c.is_numeric())
                .collect::<String>()
                .parse()
                .ok()
        })
        .collect();

    let b_parts: Vec<u32> = b
        .split('.')
        .filter_map(|s| {
            s.chars()
                .take_while(|c| c.is_numeric())
                .collect::<String>()
                .parse()
                .ok()
        })
        .collect();

    // Compare each part
    for i in 0..std::cmp::max(a_parts.len(), b_parts.len()) {
        let a_val = a_parts.get(i).unwrap_or(&0);
        let b_val = b_parts.get(i).unwrap_or(&0);
        match a_val.cmp(b_val) {
            Ordering::Equal => continue,
            other => return other,
        }
    }

    Ordering::Equal
}

/// Build a `Command` for the Claude binary with an environment it can work in.
///
/// A GUI app does not inherit the shell's environment, so this puts back the
/// parts Claude needs. It is also the one place that knows how to extend PATH,
/// which has to be done with the platform's own separator - joining with `:`
/// on Windows produces a single unusable entry and silently breaks every
/// lookup the child makes.
pub fn create_command_with_env(program: &str) -> Command {
    // On Windows an npm-installed Claude is a `.cmd` shim, which cannot be
    // given multi-line arguments at all - see `node_launcher`.
    let launcher = node_launcher(program);
    let mut cmd = match &launcher {
        Some((node, script)) => {
            info!(
                "Starting Claude as `{} {}` instead of through {} - a batch shim cannot carry multi-line arguments",
                node.display(),
                script.display(),
                program
            );
            let mut cmd = command_for(&node.to_string_lossy());
            cmd.arg(script);
            cmd
        }
        None => {
            if is_batch_shim(program) {
                warn!(
                    "Could not resolve {} to `node cli.js`; multi-line arguments (every agent system prompt) will fail to spawn",
                    program
                );
            }
            command_for(program)
        }
    };

    info!("Creating command for: {}", program);

    for (key, value) in std::env::vars() {
        if should_inherit(&key) {
            debug!("Inheriting env var: {}={}", key, value);
            cmd.env(&key, &value);
        }
    }

    for var in ["HTTP_PROXY", "HTTPS_PROXY"] {
        if let Ok(value) = std::env::var(var) {
            info!("Command will use {}={}", var, value);
        }
    }

    // Directories that must be on the child's PATH: the one the binary itself
    // lives in, so a version manager's `node` is found next to its `claude`,
    // plus the usual places a Unix GUI app cannot see.
    let mut wanted: Vec<String> = Vec::new();
    for dir in [
        std::path::Path::new(program).parent(),
        // When Node is started directly its own directory has to be on PATH
        // too, or the child cannot spawn `node`/`npm` for itself.
        launcher.as_ref().and_then(|(node, _)| node.parent()),
    ]
    .into_iter()
    .flatten()
    {
        if !dir.as_os_str().is_empty() {
            wanted.push(dir.to_string_lossy().to_string());
        }
    }
    #[cfg(not(windows))]
    wanted.extend(
        ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"]
            .iter()
            .map(|s| s.to_string()),
    );

    if !wanted.is_empty() {
        let existing = std::env::var("PATH").unwrap_or_default();
        let mut entries: Vec<String> = existing
            .split(PATH_SEPARATOR)
            .filter(|e| !e.is_empty())
            .map(|e| e.to_string())
            .collect();

        for dir in wanted {
            // Windows paths are case-insensitive, so a case-sensitive contains
            // check would add C:\Foo next to c:\foo.
            let already = entries.iter().any(|e| e.eq_ignore_ascii_case(&dir));
            if !already {
                debug!("Adding to the child's PATH: {}", dir);
                entries.insert(0, dir);
            }
        }

        cmd.env("PATH", entries.join(PATH_SEPARATOR));
    }

    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATHEXT: &str = ".COM;.EXE;.BAT;.CMD;.VBS;.JS;.MSC";

    #[test]
    fn the_npm_shim_windows_cannot_start_is_not_mistaken_for_the_binary() {
        // The three files `npm i -g` actually leaves behind.
        assert!(!has_launchable_extension(r"C:\Users\x\AppData\Roaming\npm\claude", PATHEXT));
        assert!(!has_launchable_extension(r"C:\Users\x\AppData\Roaming\npm\claude.ps1", PATHEXT));
        assert!(has_launchable_extension(r"C:\Users\x\AppData\Roaming\npm\claude.cmd", PATHEXT));
    }

    #[test]
    fn extensions_match_however_they_are_cased() {
        assert!(has_launchable_extension(r"C:\bin\CLAUDE.CMD", PATHEXT));
        assert!(has_launchable_extension(r"C:\bin\claude.Exe", ".com;.exe"));
    }

    /// Verbatim `%APPDATA%\npm\claude.cmd` from `npm i -g
    /// @anthropic-ai/claude-code`.
    const NPM_SHIM: &str = concat!(
        "@ECHO off\r\n",
        "GOTO start\r\n",
        ":find_dp0\r\n",
        "SET dp0=%~dp0\r\n",
        "EXIT /b\r\n",
        ":start\r\n",
        "SETLOCAL\r\n",
        "CALL :find_dp0\r\n",
        "IF EXIST \"%dp0%\\node.exe\" (\r\n",
        "  SET \"_prog=%dp0%\\node.exe\"\r\n",
        ") ELSE (\r\n",
        "  SET \"_prog=node\"\r\n",
        "  SET PATHEXT=%PATHEXT:;.JS;=;%\r\n",
        ")\r\n",
        "endLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & ",
        "\"%_prog%\"  \"%dp0%\\node_modules\\@anthropic-ai\\claude-code\\cli.js\" %*\r\n",
    );

    #[test]
    fn the_npm_shim_gives_up_the_script_it_would_have_run() {
        let dir = std::path::Path::new("C:\\Users\\x\\AppData\\Roaming\\npm");
        let entries = js_entries_in_shim(NPM_SHIM, dir);
        assert_eq!(
            entries,
            vec![dir.join("node_modules/@anthropic-ai/claude-code/cli.js")]
        );
    }

    #[test]
    fn a_shim_pointing_somewhere_else_entirely_is_left_where_it_points() {
        let dir = std::path::Path::new("C:\\tools");
        let entries = js_entries_in_shim(
            "@\"%_prog%\" \"C:\\other\\claude-code\\cli.js\" %*",
            dir,
        );
        assert_eq!(entries, vec![PathBuf::from("C:\\other\\claude-code\\cli.js")]);
    }

    #[test]
    fn only_batch_programs_go_looking_for_node() {
        assert!(is_batch_shim("C:\\npm\\claude.cmd"));
        assert!(is_batch_shim("C:\\npm\\CLAUDE.CMD"));
        assert!(is_batch_shim("claude.bat"));
        assert!(!is_batch_shim("C:\\npm\\claude.exe"));
        assert!(!is_batch_shim("/usr/local/bin/claude"));
    }

    #[test]
    fn a_shim_with_no_script_in_it_yields_nothing_to_run() {
        assert!(js_entries_in_shim("@echo off\r\nnode --version\r\n", std::path::Path::new("C:\\npm")).is_empty());
    }

    #[test]
    fn where_output_gives_up_the_cmd_rather_than_the_first_line() {
        // Exactly the order `where claude` prints for an npm install.
        let output = "C:\\Users\\x\\AppData\\Roaming\\npm\\claude\n                      C:\\Users\\x\\AppData\\Roaming\\npm\\claude.cmd\n                      C:\\Users\\x\\AppData\\Roaming\\npm\\claude.ps1\n";
        let (path, skipped) = first_launchable(output, PATHEXT);
        assert_eq!(path.unwrap(), r"C:\Users\x\AppData\Roaming\npm\claude.cmd");
        assert_eq!(skipped, vec![r"C:\Users\x\AppData\Roaming\npm\claude"]);
    }

    #[test]
    fn an_exe_on_the_first_line_is_taken_as_is() {
        let (path, skipped) = first_launchable("C:\\tools\\claude.exe\n", PATHEXT);
        assert_eq!(path.unwrap(), r"C:\tools\claude.exe");
        assert!(skipped.is_empty());
    }

    #[test]
    fn output_with_nothing_launchable_in_it_finds_nothing() {
        let (path, skipped) = first_launchable("C:\\npm\\claude\nC:\\npm\\claude.ps1\n", PATHEXT);
        assert!(path.is_none());
        assert_eq!(skipped.len(), 2, "both should be reported as passed over");
    }

    #[test]
    fn empty_and_blank_where_output_is_not_a_path() {
        assert!(first_launchable("", PATHEXT).0.is_none());
        assert!(first_launchable("   \n\n  \n", PATHEXT).0.is_none());
    }

    #[test]
    fn the_child_gets_path_and_the_proxy_settings_and_not_the_whole_environment() {
        assert!(should_inherit("PATH"));
        assert!(should_inherit("HTTPS_PROXY"));
        assert!(should_inherit("NODE_PATH"));
        assert!(should_inherit("LC_CTYPE"));

        assert!(!should_inherit("AWS_SECRET_ACCESS_KEY"));
        assert!(!should_inherit("SOME_LOCAL_THING"));
    }

    #[test]
    fn environment_names_match_however_they_are_cased() {
        // Windows spells it `Path`, and treating that as a different variable
        // would send the child out with no PATH at all.
        assert!(should_inherit("Path"));
        assert!(should_inherit("path"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_gets_the_variables_a_process_cannot_start_without() {
        for key in ["SystemRoot", "ComSpec", "PATHEXT", "APPDATA", "USERPROFILE", "TEMP"] {
            assert!(should_inherit(key), "{key} must reach the child");
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_gets_the_variables_a_version_manager_needs() {
        for key in ["HOME", "SHELL", "NVM_BIN", "HOMEBREW_PREFIX"] {
            assert!(should_inherit(key), "{key} must reach the child");
        }
    }

    #[test]
    fn path_is_joined_with_the_separator_this_platform_uses() {
        // Joining with ':' on Windows makes one unusable entry out of every
        // directory, and every lookup the child does then fails.
        if cfg!(windows) {
            assert_eq!(PATH_SEPARATOR, ";");
        } else {
            assert_eq!(PATH_SEPARATOR, ":");
        }
    }

    #[test]
    fn the_binarys_own_directory_is_put_on_the_childs_path() {
        // A version manager keeps `node` next to `claude`; without the parent
        // directory on PATH the shim runs and then cannot find its runtime.
        let program = if cfg!(windows) {
            r"C:\opcode-test-not-on-path\claude.cmd"
        } else {
            "/opcode-test-not-on-path/claude"
        };
        let path = child_path(program).expect("PATH should be set on the child");
        let parent = parent_of(program);

        assert!(path.starts_with(&parent), "expected {parent} first in {path}");
    }

    #[test]
    fn a_directory_already_on_path_is_not_added_a_second_time() {
        // Re-adding it would grow PATH on every spawn and, on Windows, do it
        // under whichever casing this call happened to see.
        let existing = std::env::var("PATH").unwrap_or_default();
        let first = existing
            .split(PATH_SEPARATOR)
            .find(|e| !e.is_empty())
            .expect("the test process should have a PATH")
            .to_string();

        let program = format!("{first}{}claude", std::path::MAIN_SEPARATOR);
        let path = child_path(&program).expect("PATH should be set on the child");

        let occurrences = path
            .split(PATH_SEPARATOR)
            .filter(|e| e.eq_ignore_ascii_case(&first))
            .count();
        assert_eq!(occurrences, 1, "{first} should appear once in {path}");
    }

    /// The PATH `create_command_with_env` would hand to a child.
    fn child_path(program: &str) -> Option<String> {
        create_command_with_env(program)
            .get_envs()
            .find(|(k, _)| k.to_string_lossy().eq_ignore_ascii_case("PATH"))
            .and_then(|(_, v)| v)
            .map(|v| v.to_string_lossy().to_string())
    }

    fn parent_of(program: &str) -> String {
        std::path::Path::new(program)
            .parent()
            .unwrap()
            .to_string_lossy()
            .to_string()
    }
}
