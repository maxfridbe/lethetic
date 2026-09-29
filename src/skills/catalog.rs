//! A short list of skills from Anthropic's public skills repository that the
//! Skills menu can install into `~/.config/lethetic/skills/`.

use std::path::{Component, Path, PathBuf};

pub const REPOSITORY: &str = "anthropics/skills";
pub const BRANCH: &str = "main";
/// Install refuses skills larger than this.
const MAX_FILES: usize = 200;
const MAX_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogEntry {
    pub name: &'static str,
    pub summary: &'static str,
    /// True when the skill's own license restricts use (source-available).
    pub proprietary: bool,
}

impl CatalogEntry {
    /// The skill's folder in the repository, for the menu to link to.
    pub fn url(&self) -> String {
        format!(
            "https://github.com/{REPOSITORY}/tree/{BRANCH}/skills/{}",
            self.name
        )
    }
}

pub const ENTRIES: &[CatalogEntry] = &[
    CatalogEntry {
        name: "skill-creator",
        summary: "Write, improve and test new skills",
        proprietary: false,
    },
    CatalogEntry {
        name: "mcp-builder",
        summary: "Build MCP servers that expose tools to models",
        proprietary: false,
    },
    CatalogEntry {
        name: "webapp-testing",
        summary: "Test local web apps with Playwright: screenshots, logs, UI checks",
        proprietary: false,
    },
    CatalogEntry {
        name: "frontend-design",
        summary: "Intentional visual design for new or reworked UI",
        proprietary: false,
    },
    CatalogEntry {
        name: "doc-coauthoring",
        summary: "Structured workflow for specs, proposals and design docs",
        proprietary: false,
    },
    CatalogEntry {
        name: "pdf",
        summary: "Read, merge, split, fill and create PDF files",
        proprietary: true,
    },
    CatalogEntry {
        name: "docx",
        summary: "Create and edit Word documents",
        proprietary: true,
    },
    CatalogEntry {
        name: "xlsx",
        summary: "Read and build spreadsheets, formulas and charts",
        proprietary: true,
    },
    CatalogEntry {
        name: "pptx",
        summary: "Create and edit slide decks",
        proprietary: true,
    },
];

pub fn entry(name: &str) -> Option<&'static CatalogEntry> {
    ENTRIES.iter().find(|entry| entry.name == name)
}

#[derive(serde::Deserialize)]
struct Tree {
    tree: Vec<TreeEntry>,
    #[serde(default)]
    truncated: bool,
}

#[derive(serde::Deserialize)]
struct TreeEntry {
    path: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    size: u64,
}

/// Downloads catalog skill `name` into `root/name`. Files land in a
/// temporary folder first, so a failed install leaves nothing behind.
pub async fn install(client: &reqwest::Client, name: &str, root: &Path) -> Result<PathBuf, String> {
    let entry = entry(name).ok_or_else(|| format!("{name} is not in the catalog"))?;
    let destination = root.join(entry.name);
    if destination.exists() {
        return Err(format!("{} already exists", destination.display()));
    }
    let tree: Tree = client
        .get(format!(
            "https://api.github.com/repos/{REPOSITORY}/git/trees/{BRANCH}?recursive=1"
        ))
        .header("User-Agent", "lethetic")
        .header("Accept", "application/vnd.github+json")
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| format!("could not list the skills repository: {error}"))?
        .json()
        .await
        .map_err(|error| format!("unexpected repository listing: {error}"))?;
    if tree.truncated {
        return Err("the repository listing was truncated".to_string());
    }
    let prefix = format!("skills/{}/", entry.name);
    let files: Vec<(String, PathBuf)> = tree
        .tree
        .iter()
        .filter(|item| item.kind == "blob")
        .filter_map(|item| {
            let relative = item.path.strip_prefix(&prefix)?;
            safe_relative(relative).map(|path| (item.path.clone(), path))
        })
        .collect();
    let total: u64 = tree
        .tree
        .iter()
        .filter(|item| item.kind == "blob" && item.path.starts_with(&prefix))
        .map(|item| item.size)
        .sum();
    if files.is_empty() || !files.iter().any(|(_, path)| path == Path::new("SKILL.md")) {
        return Err(format!("{} has no SKILL.md in the repository", entry.name));
    }
    if files.len() > MAX_FILES || total > MAX_BYTES {
        return Err(format!(
            "{} is too large to install ({} files, {} KB)",
            entry.name,
            files.len(),
            total / 1024
        ));
    }

    std::fs::create_dir_all(root).map_err(|error| error.to_string())?;
    let staging = root.join(format!(".installing-{}-{}", entry.name, std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    let result = async {
        for (repository_path, relative) in &files {
            let bytes = client
                .get(format!(
                    "https://raw.githubusercontent.com/{REPOSITORY}/{BRANCH}/{repository_path}"
                ))
                .header("User-Agent", "lethetic")
                .timeout(std::time::Duration::from_secs(60))
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .map_err(|error| format!("could not download {repository_path}: {error}"))?
                .bytes()
                .await
                .map_err(|error| format!("could not download {repository_path}: {error}"))?;
            let target = staging.join(relative);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            std::fs::write(&target, &bytes).map_err(|error| error.to_string())?;
        }
        std::fs::write(
            staging.join(".lethetic-source"),
            format!("{}\n", entry.url()),
        )
        .map_err(|error| error.to_string())?;
        std::fs::rename(&staging, &destination).map_err(|error| error.to_string())
    }
    .await;
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result.map(|()| destination)
}

/// A repository path confined to the skill folder: no absolute paths, `..`,
/// or empty components.
fn safe_relative(path: &str) -> Option<PathBuf> {
    let candidate = Path::new(path);
    candidate
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
        .then(|| candidate.to_path_buf())
        .filter(|path| !path.as_os_str().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_names_are_valid_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for entry in ENTRIES {
            assert!(crate::skills::valid_name(entry.name), "{}", entry.name);
            assert!(seen.insert(entry.name));
            assert!(entry.url().ends_with(&format!("/skills/{}", entry.name)));
        }
    }

    #[test]
    fn repository_paths_cannot_escape_the_skill_folder() {
        assert_eq!(safe_relative("scripts/run.py"), Some(PathBuf::from("scripts/run.py")));
        assert_eq!(safe_relative("../evil"), None);
        assert_eq!(safe_relative("/etc/passwd"), None);
        assert_eq!(safe_relative("./a"), None);
    }

    #[tokio::test]
    #[ignore = "downloads from GitHub"]
    async fn installs_a_catalog_skill_from_github() {
        let root = tempfile::tempdir().unwrap();
        let client = reqwest::Client::new();
        let path = install(&client, "webapp-testing", root.path()).await.unwrap();
        assert!(path.join("SKILL.md").is_file());
        assert!(path.join(".lethetic-source").is_file());
        let skills = crate::skills::discover_with(
            Path::new("."),
            &[(root.path().to_path_buf(), crate::skills::SkillSource::LetheticUser)],
            &crate::skills::SkillSettings::default(),
        );
        assert_eq!(skills[0].name, "webapp-testing");
        assert!(skills[0].enabled);
        let again = install(&client, "webapp-testing", root.path()).await;
        assert!(again.unwrap_err().contains("already exists"));
        assert!(
            std::fs::read_dir(root.path())
                .unwrap()
                .flatten()
                .all(|entry| !entry.file_name().to_string_lossy().starts_with(".installing")),
            "no staging folder is left behind"
        );
    }

}
