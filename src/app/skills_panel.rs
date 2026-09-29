//! The Skills menu: turn found skills on or off, and install catalog skills.

use super::*;
use crate::skills::catalog::{self, CatalogEntry};
use crate::skills::Skill;
use crossterm::event::{self, KeyCode};

#[derive(Debug, Clone)]
pub enum SkillRow {
    Installed(Skill),
    Catalog {
        entry: &'static CatalogEntry,
        installed: bool,
    },
}

#[derive(Debug, Clone, Default)]
pub struct SkillsPanel {
    pub rows: Vec<SkillRow>,
    pub selected: usize,
    /// The catalog skill being downloaded, if any.
    pub installing: Option<String>,
    pub message: Option<String>,
}

impl SkillsPanel {
    pub fn open() -> Self {
        let mut panel = Self::default();
        panel.refresh();
        panel
    }

    /// Re-reads the skill folders and the settings file.
    pub fn refresh(&mut self) {
        let project = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let skills = crate::skills::discover(&project);
        let mut rows: Vec<SkillRow> = skills.iter().cloned().map(SkillRow::Installed).collect();
        rows.extend(catalog::ENTRIES.iter().map(|entry| SkillRow::Catalog {
            entry,
            installed: skills.iter().any(|skill| skill.name == entry.name),
        }));
        self.rows = rows;
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
    }

    pub fn installed_count(&self) -> usize {
        self.rows
            .iter()
            .filter(|row| matches!(row, SkillRow::Installed(_)))
            .count()
    }
}

pub(super) fn handle_skills_key(app: &mut App, key: event::KeyEvent) -> AppEventOutcome {
    let Some(panel) = app.skills_panel.as_mut() else {
        return AppEventOutcome::Continue;
    };
    app.should_redraw = true;
    let last = panel.rows.len().saturating_sub(1);
    match key.code {
        KeyCode::Esc => app.skills_panel = None,
        KeyCode::Up => panel.selected = panel.selected.saturating_sub(1),
        KeyCode::Down => panel.selected = (panel.selected + 1).min(last),
        KeyCode::PageUp => panel.selected = panel.selected.saturating_sub(10),
        KeyCode::PageDown => panel.selected = (panel.selected + 10).min(last),
        KeyCode::Home => panel.selected = 0,
        KeyCode::End => panel.selected = last,
        KeyCode::Char('r') => {
            panel.refresh();
            panel.message = Some("Rescanned skill folders".to_string());
        }
        KeyCode::Char('c') => {
            if let Some(SkillRow::Catalog { entry, .. }) = panel.rows.get(panel.selected) {
                let url = entry.url();
                tokio::spawn(async move {
                    let _ = tokio::process::Command::new("wl-copy")
                        .arg(url)
                        .status()
                        .await;
                });
                panel.message = Some(format!("Copied the link to {}", entry.name));
            }
        }
        KeyCode::Enter | KeyCode::Char(' ') => match panel.rows.get(panel.selected).cloned() {
            Some(SkillRow::Installed(skill)) => {
                let enabled = !skill.enabled;
                panel.message = Some(match crate::skills::set_enabled(&skill.name, enabled) {
                    Ok(()) => format!(
                        "{} {}",
                        skill.name,
                        if enabled { "enabled" } else { "disabled" }
                    ),
                    Err(error) => format!("✗ Could not save the choice: {error}"),
                });
                panel.refresh();
            }
            Some(SkillRow::Catalog { entry, installed }) => {
                if installed {
                    panel.message = Some(format!("{} is already installed", entry.name));
                } else if let Some(busy) = &panel.installing {
                    panel.message = Some(format!("Still installing {busy}…"));
                } else {
                    panel.installing = Some(entry.name.to_string());
                    panel.message = Some(format!("Installing {}…", entry.name));
                    return AppEventOutcome::InstallSkill {
                        name: entry.name.to_string(),
                    };
                }
            }
            None => {}
        },
        _ => {}
    }
    AppEventOutcome::Continue
}

impl App {
    /// Called when a catalog install finishes, whether or not the menu is open.
    pub fn finish_skill_install(&mut self, name: &str, result: Result<String, String>) {
        let message = match &result {
            Ok(path) => format!("✓ Installed {name} to {path}; it is enabled"),
            Err(error) => format!("✗ Could not install {name}: {error}"),
        };
        if let Some(panel) = self.skills_panel.as_mut() {
            panel.installing = None;
            panel.message = Some(message.clone());
            panel.refresh();
        }
        self.stop_reason = message;
        self.should_redraw = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menu_lists_the_catalog_and_starts_an_install() {
        let mut app = App::new(&crate::config::Config::default());
        let mut panel = SkillsPanel::open();
        let index = panel
            .rows
            .iter()
            .position(|row| matches!(row, SkillRow::Catalog { entry, installed: false } if entry.name == "mcp-builder"));
        let Some(index) = index else {
            return; // already installed on this machine
        };
        panel.selected = index;
        app.skills_panel = Some(panel);
        let key = event::KeyEvent::new(KeyCode::Enter, event::KeyModifiers::NONE);
        let outcome = handle_skills_key(&mut app, key);
        assert!(matches!(outcome, AppEventOutcome::InstallSkill { ref name } if name == "mcp-builder"));
        let panel = app.skills_panel.as_ref().unwrap();
        assert_eq!(panel.installing.as_deref(), Some("mcp-builder"));

        // A second install waits for the first.
        let again = handle_skills_key(&mut app, key);
        assert!(matches!(again, AppEventOutcome::Continue));

        app.finish_skill_install("mcp-builder", Err("offline".to_string()));
        let panel = app.skills_panel.as_ref().unwrap();
        assert!(panel.installing.is_none());
        assert!(panel.message.as_deref().unwrap().contains("offline"));
    }
}
