use anyhow::{Context, Result};
use dirs;
use log::{debug, error, info};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Represents an agent skill: a `SKILL.md` file (plus any supporting files)
/// living in its own directory under `.claude/skills/<name>/`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    /// Unique identifier ("<scope>-<name>")
    pub id: String,
    /// Skill name — the directory name, which is what `/<name>` resolves to
    pub name: String,
    /// The `name:` field from frontmatter, when it disagrees with the
    /// directory name (Claude resolves skills by directory, so a mismatch is
    /// worth surfacing in the UI)
    pub frontmatter_name: Option<String>,
    /// Description from frontmatter — this is what Claude reads to decide
    /// whether a skill is relevant, so it's effectively required
    pub description: Option<String>,
    /// "project" or "user"
    pub scope: String,
    /// Absolute path to the SKILL.md file
    pub file_path: String,
    /// Absolute path to the skill's directory
    pub directory_path: String,
    /// Markdown body (frontmatter stripped)
    pub content: String,
    /// `allowed-tools` from frontmatter, if present
    pub allowed_tools: Vec<String>,
    /// Paths (relative to the skill directory) of other files bundled with
    /// the skill — references, scripts, assets and so on
    pub supporting_files: Vec<String>,
}

/// YAML frontmatter of a SKILL.md
#[derive(Debug, Deserialize)]
struct SkillFrontmatter {
    name: Option<String>,
    description: Option<String>,
    #[serde(rename = "allowed-tools")]
    allowed_tools: Option<Vec<String>>,
}

/// Split a markdown file into (frontmatter, body)
fn parse_markdown_with_frontmatter(content: &str) -> (Option<SkillFrontmatter>, String) {
    let lines: Vec<&str> = content.lines().collect();

    if lines.is_empty() || lines[0].trim() != "---" {
        return (None, content.to_string());
    }

    let frontmatter_end = lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, line)| line.trim() == "---")
        .map(|(i, _)| i);

    match frontmatter_end {
        Some(end) => {
            let frontmatter_content = lines[1..end].join("\n");
            let body = lines[(end + 1)..].join("\n").trim_start().to_string();

            match serde_yaml::from_str::<SkillFrontmatter>(&frontmatter_content) {
                Ok(frontmatter) => (Some(frontmatter), body),
                Err(e) => {
                    debug!("Failed to parse skill frontmatter: {}", e);
                    (None, content.to_string())
                }
            }
        }
        // Malformed frontmatter — treat the whole file as body
        None => (None, content.to_string()),
    }
}

/// Collect files bundled alongside SKILL.md, as paths relative to the skill dir
fn collect_supporting_files(skill_dir: &Path) -> Vec<String> {
    let mut files = Vec::new();
    collect_supporting_files_inner(skill_dir, skill_dir, &mut files);
    files.sort();
    files
}

fn collect_supporting_files_inner(dir: &Path, base: &Path, files: &mut Vec<String>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            debug!("Failed to read skill directory {:?}: {}", dir, e);
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();

        // Skip hidden files and directories
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('.'))
        {
            continue;
        }

        if path.is_dir() {
            collect_supporting_files_inner(&path, base, files);
        } else if path.is_file() {
            // SKILL.md is the skill itself, not a supporting file
            if path.file_name().and_then(|n| n.to_str()) == Some("SKILL.md") && path.parent() == Some(base) {
                continue;
            }
            if let Ok(relative) = path.strip_prefix(base) {
                files.push(relative.to_string_lossy().to_string());
            }
        }
    }
}

/// Load one skill from its directory (which must contain a SKILL.md)
fn load_skill_from_dir(skill_dir: &Path, scope: &str) -> Result<Skill> {
    let file_path = skill_dir.join("SKILL.md");
    let content = fs::read_to_string(&file_path).context("Failed to read SKILL.md")?;

    let (frontmatter, body) = parse_markdown_with_frontmatter(&content);

    let name = skill_dir
        .file_name()
        .and_then(|n| n.to_str())
        .context("Invalid skill directory name")?
        .to_string();

    let (frontmatter_name, description, allowed_tools) = match frontmatter {
        Some(fm) => (
            fm.name.filter(|n| n != &name),
            fm.description,
            fm.allowed_tools.unwrap_or_default(),
        ),
        None => (None, None, Vec::new()),
    };

    Ok(Skill {
        id: format!("{}-{}", scope, name),
        name,
        frontmatter_name,
        description,
        scope: scope.to_string(),
        file_path: file_path.to_string_lossy().to_string(),
        directory_path: skill_dir.to_string_lossy().to_string(),
        content: body,
        allowed_tools,
        supporting_files: collect_supporting_files(skill_dir),
    })
}

/// Load every skill under a `.claude/skills` directory
fn load_skills_from_base(base_dir: &Path, scope: &str, skills: &mut Vec<Skill>) {
    if !base_dir.exists() {
        return;
    }

    debug!("Scanning {} skills at: {:?}", scope, base_dir);

    let entries = match fs::read_dir(base_dir) {
        Ok(entries) => entries,
        Err(e) => {
            error!("Failed to read skills directory {:?}: {}", base_dir, e);
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if !path.is_dir() {
            continue;
        }
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('.'))
        {
            continue;
        }
        // A skill directory is only a skill if it holds a SKILL.md
        if !path.join("SKILL.md").is_file() {
            debug!("Skipping {:?}: no SKILL.md", path);
            continue;
        }

        match load_skill_from_dir(&path, scope) {
            Ok(skill) => {
                debug!("Loaded {} skill: {}", scope, skill.name);
                skills.push(skill);
            }
            Err(e) => error!("Failed to load skill from {:?}: {}", path, e),
        }
    }
}

/// Validate a skill name: lowercase kebab-case, safe to use as a directory name
fn validate_skill_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Skill name cannot be empty".to_string());
    }
    if name.len() > 64 {
        return Err("Skill name must be 64 characters or fewer".to_string());
    }
    let valid = name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit());
    if !valid {
        return Err(
            "Skill name must be lowercase letters, digits and hyphens only (e.g. \"code-review\")"
                .to_string(),
        );
    }
    Ok(())
}

/// Resolve the `.claude/skills` base directory for a scope
fn skills_base_dir(scope: &str, project_path: Option<String>) -> Result<PathBuf, String> {
    match scope {
        "project" => {
            let proj_path =
                project_path.ok_or_else(|| "Project path required for project scope".to_string())?;
            Ok(PathBuf::from(proj_path).join(".claude").join("skills"))
        }
        "user" => Ok(dirs::home_dir()
            .ok_or_else(|| "Could not find home directory".to_string())?
            .join(".claude")
            .join("skills")),
        _ => Err("Invalid scope. Must be 'project' or 'user'".to_string()),
    }
}

/// Discover all skills, from the project (if given) and the user's home
#[tauri::command]
pub async fn skills_list(project_path: Option<String>) -> Result<Vec<Skill>, String> {
    info!("Discovering skills");
    let mut skills = Vec::new();

    if let Some(proj_path) = &project_path {
        let project_skills_dir = PathBuf::from(proj_path).join(".claude").join("skills");
        load_skills_from_base(&project_skills_dir, "project", &mut skills);
    }

    if let Some(home_dir) = dirs::home_dir() {
        let user_skills_dir = home_dir.join(".claude").join("skills");
        load_skills_from_base(&user_skills_dir, "user", &mut skills);
    }

    skills.sort_by(|a, b| a.name.cmp(&b.name));

    info!("Found {} skills", skills.len());
    Ok(skills)
}

/// Get a single skill by ID
#[tauri::command]
pub async fn skill_get(skill_id: String, project_path: Option<String>) -> Result<Skill, String> {
    debug!("Getting skill: {}", skill_id);

    skills_list(project_path)
        .await?
        .into_iter()
        .find(|skill| skill.id == skill_id)
        .ok_or_else(|| format!("Skill not found: {}", skill_id))
}

/// Create or update a skill, writing `<base>/<name>/SKILL.md`
#[tauri::command]
pub async fn skill_save(
    scope: String,
    name: String,
    description: String,
    content: String,
    allowed_tools: Vec<String>,
    project_path: Option<String>,
) -> Result<Skill, String> {
    info!("Saving skill: {} in scope: {}", name, scope);

    validate_skill_name(&name)?;

    if description.trim().is_empty() {
        // Claude picks skills by reading their descriptions, so an empty one
        // makes the skill effectively undiscoverable.
        return Err("Description is required — it's what Claude uses to decide when the skill applies".to_string());
    }

    let base_dir = skills_base_dir(&scope, project_path)?;
    let skill_dir = base_dir.join(&name);

    fs::create_dir_all(&skill_dir)
        .map_err(|e| format!("Failed to create skill directory: {}", e))?;

    // Build SKILL.md: frontmatter (name + description are what Claude reads)
    // followed by the instructions body.
    let mut full_content = String::from("---\n");
    full_content.push_str(&format!("name: {}\n", name));
    full_content.push_str(&format!("description: {}\n", description.trim()));
    if !allowed_tools.is_empty() {
        full_content.push_str("allowed-tools:\n");
        for tool in &allowed_tools {
            full_content.push_str(&format!("  - {}\n", tool));
        }
    }
    full_content.push_str("---\n\n");
    full_content.push_str(content.trim_start());
    if !full_content.ends_with('\n') {
        full_content.push('\n');
    }

    let file_path = skill_dir.join("SKILL.md");
    fs::write(&file_path, &full_content).map_err(|e| format!("Failed to write SKILL.md: {}", e))?;

    load_skill_from_dir(&skill_dir, &scope).map_err(|e| format!("Failed to load saved skill: {}", e))
}

/// Delete a skill, removing its whole directory (supporting files included)
#[tauri::command]
pub async fn skill_delete(
    skill_id: String,
    project_path: Option<String>,
) -> Result<String, String> {
    info!("Deleting skill: {}", skill_id);

    if skill_id.starts_with("project-") && project_path.is_none() {
        return Err("Project path required to delete project skills".to_string());
    }

    let skill = skill_get(skill_id, project_path).await?;

    // Guard against deleting anything that isn't actually a skill directory.
    let dir = Path::new(&skill.directory_path);
    if !dir.join("SKILL.md").is_file() {
        return Err(format!(
            "Refusing to delete {:?}: it doesn't look like a skill directory",
            dir
        ));
    }

    fs::remove_dir_all(dir).map_err(|e| format!("Failed to delete skill directory: {}", e))?;

    Ok(format!("Deleted skill: {}", skill.name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "opcode-skills-test-{}-{}",
            label,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parses_frontmatter_and_body() {
        let (frontmatter, body) = parse_markdown_with_frontmatter(
            "---\nname: code-review\ndescription: Review a diff. Use when asked for a review.\nallowed-tools:\n  - Read\n  - Bash\n---\n\n# Steps\n\n1. Run git diff\n",
        );

        let fm = frontmatter.expect("frontmatter should parse");
        assert_eq!(fm.name.as_deref(), Some("code-review"));
        assert_eq!(
            fm.description.as_deref(),
            Some("Review a diff. Use when asked for a review.")
        );
        assert_eq!(fm.allowed_tools.unwrap(), vec!["Read", "Bash"]);
        assert_eq!(body, "# Steps\n\n1. Run git diff");
    }

    #[test]
    fn tolerates_missing_and_malformed_frontmatter() {
        let (fm, body) = parse_markdown_with_frontmatter("# Just markdown\n");
        assert!(fm.is_none());
        assert_eq!(body, "# Just markdown\n");

        // Opening delimiter with no closing one: whole file is the body
        let (fm, body) = parse_markdown_with_frontmatter("---\nname: x\n# no close\n");
        assert!(fm.is_none());
        assert_eq!(body, "---\nname: x\n# no close\n");
    }

    #[test]
    fn ignores_unknown_frontmatter_fields() {
        let (fm, _) = parse_markdown_with_frontmatter(
            "---\nname: x\ndescription: d\nmodel: opus\nsome-future-field: 1\n---\n\nbody\n",
        );
        let fm = fm.expect("unknown fields should not fail the parse");
        assert_eq!(fm.description.as_deref(), Some("d"));
    }

    #[test]
    fn loads_skill_with_supporting_files() {
        let base = temp_dir("load");
        let skill_dir = base.join("my-skill");
        fs::create_dir_all(skill_dir.join("references")).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: my-skill\ndescription: Does a thing.\n---\n\nInstructions here\n",
        )
        .unwrap();
        fs::write(skill_dir.join("references/notes.md"), "notes").unwrap();
        fs::write(skill_dir.join("script.sh"), "echo hi").unwrap();

        let skill = load_skill_from_dir(&skill_dir, "user").unwrap();

        assert_eq!(skill.id, "user-my-skill");
        assert_eq!(skill.name, "my-skill");
        assert_eq!(skill.description.as_deref(), Some("Does a thing."));
        assert_eq!(skill.content, "Instructions here");
        // The frontmatter name matches the directory, so nothing to warn about
        assert_eq!(skill.frontmatter_name, None);
        assert_eq!(
            skill.supporting_files,
            vec!["references/notes.md".to_string(), "script.sh".to_string()]
        );

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn flags_frontmatter_name_disagreeing_with_directory() {
        let base = temp_dir("mismatch");
        let skill_dir = base.join("actual-dir-name");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: something-else\ndescription: d\n---\n\nbody\n",
        )
        .unwrap();

        let skill = load_skill_from_dir(&skill_dir, "project").unwrap();

        // The directory name is what `/name` resolves to; the mismatch is surfaced
        assert_eq!(skill.name, "actual-dir-name");
        assert_eq!(skill.frontmatter_name.as_deref(), Some("something-else"));

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn only_directories_containing_skill_md_count_as_skills() {
        let base = temp_dir("discover");

        let good = base.join("good-skill");
        fs::create_dir_all(&good).unwrap();
        fs::write(good.join("SKILL.md"), "---\nname: good-skill\ndescription: d\n---\n\nb\n").unwrap();

        // No SKILL.md — not a skill
        fs::create_dir_all(base.join("not-a-skill")).unwrap();
        fs::write(base.join("not-a-skill/README.md"), "hi").unwrap();

        // Hidden directory — skipped
        let hidden = base.join(".hidden-skill");
        fs::create_dir_all(&hidden).unwrap();
        fs::write(hidden.join("SKILL.md"), "---\nname: h\ndescription: d\n---\n\nb\n").unwrap();

        // A loose file at the top level — skipped
        fs::write(base.join("stray.md"), "x").unwrap();

        let mut skills = Vec::new();
        load_skills_from_base(&base, "user", &mut skills);

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "good-skill");

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn validates_skill_names() {
        assert!(validate_skill_name("code-review").is_ok());
        assert!(validate_skill_name("skill2").is_ok());
        assert!(validate_skill_name("").is_err());
        assert!(validate_skill_name("Code-Review").is_err()); // uppercase
        assert!(validate_skill_name("-leading-hyphen").is_err());
        assert!(validate_skill_name("has space").is_err());
        assert!(validate_skill_name("../escape").is_err()); // path traversal
        assert!(validate_skill_name("with/slash").is_err());
    }
}
