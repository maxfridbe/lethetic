//! Skills: folders of instructions the model loads when a task calls for them.
//!
//! A skill is a directory holding `SKILL.md`, whose YAML frontmatter gives a
//! `name` and a `description` (the Agent Skills format, so skills written for
//! Claude Code work unchanged). Only enabled skills' names and descriptions
//! reach the model, listed in the `skill` tool; the body is loaded when the
//! model calls that tool.
//!
//! Skills are found in this order, and the first skill with a given name wins:
//! 1. `<project>/.lethetic/skills/`
//! 2. `~/.config/lethetic/skills/`
//! 3. `<project>/.claude/skills/`
//! 4. `~/.claude/skills/`
//!
//! Lethetic's own folders are enabled by default; Claude's folders belong to
//! another tool, so their skills start disabled. Choices made in the Skills
//! menu are saved in `~/.config/lethetic/skills.yml`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub mod catalog;

/// Largest `SKILL.md` Lethetic reads.
const MAX_SKILL_FILE_BYTES: u64 = 256 * 1024;
/// Longest description listed for the model.
const MAX_DESCRIPTION_CHARS: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SkillSource {
    LetheticProject,
    LetheticUser,
    ClaudeProject,
    ClaudeUser,
}

impl SkillSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::LetheticProject => ".lethetic/skills",
            Self::LetheticUser => "~/.config/lethetic/skills",
            Self::ClaudeProject => ".claude/skills",
            Self::ClaudeUser => "~/.claude/skills",
        }
    }

    pub fn enabled_by_default(self) -> bool {
        matches!(self, Self::LetheticProject | Self::LetheticUser)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub dir: PathBuf,
    pub source: SkillSource,
    pub enabled: bool,
}

impl Skill {
    pub fn skill_file(&self) -> PathBuf {
        self.dir.join("SKILL.md")
    }
}

/// Menu choices, saved in `~/.config/lethetic/skills.yml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillSettings {
    /// Skill name → enabled, for skills whose default was changed.
    #[serde(default)]
    pub enabled: BTreeMap<String, bool>,
}

impl SkillSettings {
    pub fn path() -> PathBuf {
        crate::platform::lethetic_config_dir().join("skills.yml")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|text| serde_yaml::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let text = serde_yaml::to_string(self).map_err(|error| error.to_string())?;
        let temporary = path.with_extension("yml.tmp");
        std::fs::write(&temporary, text).map_err(|error| error.to_string())?;
        std::fs::rename(&temporary, &path).map_err(|error| error.to_string())
    }
}

/// Where skills are looked for, in priority order.
pub fn search_roots(project: &Path) -> Vec<(PathBuf, SkillSource)> {
    let mut roots = vec![
        (project.join(".lethetic").join("skills"), SkillSource::LetheticProject),
        (user_skill_root(), SkillSource::LetheticUser),
        (project.join(".claude").join("skills"), SkillSource::ClaudeProject),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        roots.push((
            PathBuf::from(home).join(".claude").join("skills"),
            SkillSource::ClaudeUser,
        ));
    }
    roots
}

/// Lethetic's user skill folder, where catalog installs go.
pub fn user_skill_root() -> PathBuf {
    crate::platform::lethetic_config_dir().join("skills")
}

/// Every skill visible from `project`, the first of each name, sorted.
pub fn discover(project: &Path) -> Vec<Skill> {
    discover_with(project, &search_roots(project), &SkillSettings::load())
}

pub fn discover_with(
    _project: &Path,
    roots: &[(PathBuf, SkillSource)],
    settings: &SkillSettings,
) -> Vec<Skill> {
    let mut found: BTreeMap<String, Skill> = BTreeMap::new();
    for (root, source) in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        let mut directories: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        directories.sort();
        for dir in directories {
            let Some((name, description)) = read_frontmatter(&dir) else {
                continue;
            };
            if found.contains_key(&name) {
                continue;
            }
            let enabled = settings
                .enabled
                .get(&name)
                .copied()
                .unwrap_or_else(|| source.enabled_by_default());
            found.insert(
                name.clone(),
                Skill {
                    name,
                    description,
                    dir,
                    source: *source,
                    enabled,
                },
            );
        }
    }
    found.into_values().collect()
}

/// Enabled skills visible from the launch directory.
pub fn enabled_here() -> Vec<Skill> {
    let project = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    discover(&project)
        .into_iter()
        .filter(|skill| skill.enabled)
        .collect()
}

/// Turns a skill on or off and saves the choice.
pub fn set_enabled(name: &str, enabled: bool) -> Result<(), String> {
    let mut settings = SkillSettings::load();
    settings.enabled.insert(name.to_string(), enabled);
    settings.save()
}

#[derive(Deserialize)]
struct Frontmatter {
    name: Option<String>,
    description: Option<String>,
}

/// The `name` and `description` from `dir/SKILL.md`, or `None` when the file
/// is missing, too large, or has no description.
fn read_frontmatter(dir: &Path) -> Option<(String, String)> {
    let file = dir.join("SKILL.md");
    if std::fs::metadata(&file).ok()?.len() > MAX_SKILL_FILE_BYTES {
        return None;
    }
    let text = std::fs::read_to_string(&file).ok()?;
    let (yaml, _) = split_frontmatter(&text)?;
    let front: Frontmatter = serde_yaml::from_str(yaml).ok()?;
    let fallback = dir.file_name()?.to_str()?.to_string();
    let name = front
        .name
        .map(|name| name.trim().to_string())
        .filter(|name| valid_name(name))
        .unwrap_or(fallback);
    if !valid_name(&name) {
        return None;
    }
    let description: String = front
        .description?
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_DESCRIPTION_CHARS)
        .collect();
    (!description.is_empty()).then_some((name, description))
}

/// `(frontmatter, body)` when `text` opens with a `---` fenced YAML block.
pub fn split_frontmatter(text: &str) -> Option<(&str, &str)> {
    let rest = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))?;
    let end = rest.find("\n---")?;
    let yaml = &rest[..end];
    let after = &rest[end + 4..];
    let body = after.split_once('\n').map_or("", |(_, body)| body);
    Some((yaml, body))
}

/// Skill names are lowercase letters, digits and hyphens, as in the format.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// The text the `skill` tool returns: where the skill lives, its files, and
/// its instructions.
pub fn load_for_model(skill: &Skill) -> Result<String, String> {
    let text = std::fs::read_to_string(skill.skill_file())
        .map_err(|error| format!("could not read {}: {error}", skill.skill_file().display()))?;
    let body = split_frontmatter(&text).map_or(text.as_str(), |(_, body)| body);
    let mut files = Vec::new();
    collect_files(&skill.dir, &skill.dir, &mut files, 0);
    files.sort();
    let listing = if files.len() > 60 {
        format!("{}\n… {} more", files[..60].join("\n"), files.len() - 60)
    } else {
        files.join("\n")
    };
    Ok(format!(
        "# Skill: {}\nDirectory: {}\nPaths in these instructions are relative to that directory; read them with read_file and run its scripts from there.\n\nFiles:\n{listing}\n\n---\n\n{}",
        skill.name,
        skill.dir.display(),
        body.trim()
    ))
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<String>, depth: usize) {
    if depth > 4 || out.len() > 500 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, out, depth + 1);
        } else if let Ok(relative) = path.strip_prefix(root) {
            out.push(relative.display().to_string());
        }
    }
}

#[cfg(test)]
mod tests;
