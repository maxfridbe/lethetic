//! "Scan for more" in the model picker: the connection's full `/v1/models`
//! catalog with a type-to-filter list. Enter saves a model to the picker.

use super::*;
use crossterm::event::{self, KeyCode, KeyModifiers};

/// Rows are capped so a catalog of hundreds stays responsive to render.
const MAX_ROWS: usize = 500;

pub struct ModelCatalogState {
    pub connection_id: String,
    pub connection_name: String,
    /// `None` while the scan is in flight.
    pub models: Option<Vec<crate::transport::ModelInfo>>,
    pub error: Option<String>,
    pub filter: String,
    pub list_state: ListState,
}

impl ModelCatalogState {
    pub fn new(connection_id: String, connection_name: String) -> Self {
        Self {
            connection_id,
            connection_name,
            models: None,
            error: None,
            filter: String::new(),
            list_state: ListState::default(),
        }
    }

    /// Case-insensitive; every whitespace-separated term must match the id
    /// or the display name.
    pub fn filtered(&self) -> Vec<&crate::transport::ModelInfo> {
        let terms: Vec<String> = self
            .filter
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        self.models
            .iter()
            .flatten()
            .filter(|model| {
                let haystack = format!("{} {}", model.id, model.display_name).to_lowercase();
                terms.iter().all(|term| haystack.contains(term))
            })
            .take(MAX_ROWS)
            .collect()
    }

    pub fn total(&self) -> usize {
        self.models.as_ref().map_or(0, Vec::len)
    }

    fn clamp_selection(&mut self) {
        let count = self.filtered().len();
        if count == 0 {
            self.list_state.select(None);
        } else {
            let selected = self.list_state.selected().unwrap_or(0).min(count - 1);
            self.list_state.select(Some(selected));
        }
    }

    pub fn set_models(&mut self, result: Result<Vec<crate::transport::ModelInfo>, String>) {
        match result {
            Ok(mut models) => {
                models.sort_by(|left, right| left.id.cmp(&right.id));
                self.models = Some(models);
                self.error = None;
            }
            Err(error) => {
                self.models = Some(Vec::new());
                self.error = Some(error);
            }
        }
        self.clamp_selection();
    }
}

pub(super) fn handle_model_catalog_key(app: &mut App, key: event::KeyEvent) -> AppEventOutcome {
    let Some(catalog) = app.model_catalog.as_mut() else {
        return AppEventOutcome::Continue;
    };
    app.should_redraw = true;
    match key.code {
        KeyCode::Esc => {
            app.model_catalog = None;
            app.show_model_switcher = true;
        }
        KeyCode::Down => {
            let count = catalog.filtered().len();
            if count > 0 {
                let next = catalog.list_state.selected().map_or(0, |i| (i + 1) % count);
                catalog.list_state.select(Some(next));
            }
        }
        KeyCode::Up => {
            let count = catalog.filtered().len();
            if count > 0 {
                let next = catalog
                    .list_state
                    .selected()
                    .map_or(0, |i| if i == 0 { count - 1 } else { i - 1 });
                catalog.list_state.select(Some(next));
            }
        }
        KeyCode::PageDown => {
            let count = catalog.filtered().len();
            if count > 0 {
                let next = (catalog.list_state.selected().unwrap_or(0) + 15).min(count - 1);
                catalog.list_state.select(Some(next));
            }
        }
        KeyCode::PageUp => {
            let next = catalog
                .list_state
                .selected()
                .unwrap_or(0)
                .saturating_sub(15);
            catalog.list_state.select(Some(next));
        }
        KeyCode::Enter => {
            let selected = catalog
                .list_state
                .selected()
                .and_then(|index| catalog.filtered().get(index).map(|model| model.id.clone()));
            if let Some(model_id) = selected {
                let connection_id = catalog.connection_id.clone();
                app.model_catalog = None;
                return AppEventOutcome::SaveModel {
                    connection_id,
                    model_id,
                };
            }
        }
        KeyCode::Backspace => {
            catalog.filter.pop();
            catalog.list_state.select(Some(0));
            catalog.clamp_selection();
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            catalog.filter.clear();
            catalog.list_state.select(Some(0));
            catalog.clamp_selection();
        }
        KeyCode::Char(character)
            if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
        {
            catalog.filter.push(character);
            catalog.list_state.select(Some(0));
            catalog.clamp_selection();
        }
        _ => {}
    }
    AppEventOutcome::Continue
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::ModelInfo;

    fn model(id: &str, name: &str) -> ModelInfo {
        ModelInfo {
            id: id.to_string(),
            display_name: name.to_string(),
            pricing: None,
        }
    }

    #[test]
    fn filter_matches_every_term_case_insensitively() {
        let mut catalog = ModelCatalogState::new("openrouter".into(), "OpenRouter".into());
        catalog.set_models(Ok(vec![
            model("z-ai/glm-5.3-flash", "GLM 5.3 Flash"),
            model("meta/muse-spark-1.3", "Muse Spark"),
            model("google/gemma-4-31b-it:free", "Gemma 4 31B (free)"),
        ]));
        catalog.filter = "GEMMA free".into();
        let ids: Vec<_> = catalog.filtered().iter().map(|m| m.id.clone()).collect();
        assert_eq!(ids, ["google/gemma-4-31b-it:free"]);
        catalog.filter.clear();
        assert_eq!(catalog.filtered().len(), 3);
        assert_eq!(catalog.filtered()[0].id, "google/gemma-4-31b-it:free");
    }
}
