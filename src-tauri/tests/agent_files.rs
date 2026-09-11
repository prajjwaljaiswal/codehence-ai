//! Validates the bundled agent definitions in `cc_agents/` against the struct
//! the importer actually deserializes into, so a malformed file is caught here
//! rather than by a user hitting "Import" and getting a parse error.

use opcode_lib::commands::agents::AgentExport;
use std::fs;
use std::path::PathBuf;

fn cc_agents_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root")
        .join("cc_agents")
}

fn agent_files() -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(cc_agents_dir())
        .expect("cc_agents directory should exist")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.to_string_lossy().ends_with(".dotsquares-ai.json"))
        .collect();
    files.sort();
    files
}

/// Icon names the picker knows about. An icon outside this set silently falls
/// back to the default robot, which looks like a bug to the user.
const VALID_ICONS: &[&str] = &[
    "home",
    "menu",
    "settings",
    "user",
    "users",
    "log-out",
    "bell",
    "bookmark",
    "calendar",
    "clock",
    "eye",
    "eye-off",
    "hash",
    "heart",
    "info",
    "link",
    "lock",
    "map",
    "message-square",
    "mic",
    "music",
    "paperclip",
    "phone",
    "pin",
    "plus",
    "save",
    "share",
    "star",
    "tag",
    "trash",
    "upload",
    "download",
    "edit",
    "copy",
    "bot",
    "brain",
    "code",
    "terminal",
    "cpu",
    "database",
    "git-branch",
    "github",
    "globe",
    "hard-drive",
    "laptop",
    "monitor",
    "server",
    "wifi",
    "cloud",
    "command",
    "file-code",
    "file-json",
    "folder",
    "folder-open",
    "bug",
    "coffee",
    "briefcase",
    "building",
    "credit-card",
    "dollar-sign",
    "trending-up",
    "trending-down",
    "bar-chart",
    "pie-chart",
    "calculator",
    "receipt",
    "wallet",
    "palette",
    "brush",
    "camera",
    "film",
    "image",
    "layers",
    "layout",
    "pen-tool",
    "scissors",
    "type",
    "zap",
    "sparkles",
    "wand-2",
    "beaker",
    "atom",
    "dna",
    "flame",
    "leaf",
    "mountain",
    "sun",
    "moon",
    "cloud-rain",
    "snowflake",
    "tree-pine",
    "waves",
    "wind",
    "gamepad-2",
    "dice-1",
    "trophy",
    "medal",
    "crown",
    "rocket",
    "target",
    "swords",
    "shield",
    "mail",
    "send",
    "message-circle",
    "video",
    "voicemail",
    "radio",
    "podcast",
    "megaphone",
    "activity",
    "anchor",
    "award",
    "battery",
    "bluetooth",
    "compass",
    "crosshair",
    "flag",
    "flashlight",
    "gift",
    "headphones",
    "key",
    "lightbulb",
    "package",
    "puzzle",
    "search",
    "smile",
    "thumbs-up",
    "umbrella",
    "watch",
    "wrench",
];

#[test]
fn every_bundled_agent_parses() {
    let files = agent_files();
    assert!(!files.is_empty(), "expected bundled agent files");

    for path in files {
        let raw = fs::read_to_string(&path).expect("readable");
        let parsed: AgentExport = serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("{} failed to parse: {e}", path.display()));

        assert_eq!(parsed.version, 1, "{} has wrong version", path.display());
        assert!(
            !parsed.agent.name.trim().is_empty(),
            "{} has an empty name",
            path.display()
        );
        assert!(
            !parsed.agent.system_prompt.trim().is_empty(),
            "{} has an empty system prompt",
            path.display()
        );
    }
}

#[test]
fn every_bundled_agent_uses_a_real_icon() {
    for path in agent_files() {
        let raw = fs::read_to_string(&path).expect("readable");
        let parsed: AgentExport = serde_json::from_str(&raw).expect("parses");
        assert!(
            VALID_ICONS.contains(&parsed.agent.icon.as_str()),
            "{} uses unknown icon `{}`",
            path.display(),
            parsed.agent.icon
        );
    }
}

#[test]
fn every_bundled_agent_uses_a_supported_model() {
    for path in agent_files() {
        let raw = fs::read_to_string(&path).expect("readable");
        let parsed: AgentExport = serde_json::from_str(&raw).expect("parses");
        assert!(
            matches!(parsed.agent.model.as_str(), "sonnet" | "opus" | "haiku"),
            "{} uses unexpected model `{}`",
            path.display(),
            parsed.agent.model
        );
    }
}

#[test]
fn agent_names_are_unique() {
    let mut seen: Vec<String> = Vec::new();
    for path in agent_files() {
        let raw = fs::read_to_string(&path).expect("readable");
        let parsed: AgentExport = serde_json::from_str(&raw).expect("parses");
        assert!(
            !seen.contains(&parsed.agent.name),
            "duplicate agent name `{}` in {}",
            parsed.agent.name,
            path.display()
        );
        seen.push(parsed.agent.name);
    }
}

#[test]
fn the_development_team_is_present() {
    let names: Vec<String> = agent_files()
        .iter()
        .map(|p| {
            let raw = fs::read_to_string(p).expect("readable");
            let parsed: AgentExport = serde_json::from_str(&raw).expect("parses");
            parsed.agent.name
        })
        .collect();

    for role in [
        "Planner",
        "Implementer",
        "Tester",
        "Code Reviewer",
        "Debugger",
        "Documenter",
    ] {
        assert!(
            names.iter().any(|n| n == role),
            "missing team role `{role}`; have {names:?}"
        );
    }
}

#[test]
fn team_agents_share_the_handoff_contract() {
    // The roles coordinate through files under .dotsquares-ai/. If a prompt loses that
    // section the agents stop being a team and become six unrelated bots.
    let team = [
        "Planner",
        "Implementer",
        "Tester",
        "Code Reviewer",
        "Debugger",
        "Documenter",
    ];

    for path in agent_files() {
        let raw = fs::read_to_string(&path).expect("readable");
        let parsed: AgentExport = serde_json::from_str(&raw).expect("parses");
        if !team.contains(&parsed.agent.name.as_str()) {
            continue;
        }
        assert!(
            parsed.agent.system_prompt.contains("handoff_contract"),
            "{} is missing the handoff contract",
            path.display()
        );
        assert!(
            parsed.agent.system_prompt.contains(".dotsquares-ai/progress.md"),
            "{} does not reference the shared progress file",
            path.display()
        );
    }
}
