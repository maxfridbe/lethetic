//! How many tool calls one model turn may carry.
//!
//! `single` keeps the historical contract: providers are asked for one call
//! and a batch is rejected with an explanation to the model. `sequential`
//! lets the provider return several calls; Lethetic runs them one after
//! another, each with its own approval, and asks the model again only when
//! the whole batch has results. Python-only mode always uses `single`.

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallMode {
    #[default]
    Single,
    Sequential,
}

impl ToolCallMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Single => "one per turn",
            Self::Sequential => "several per turn",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Single => Self::Sequential,
            Self::Sequential => Self::Single,
        }
    }
}

/// The mode in force for `config`: Python-only mode is always single-call.
pub fn effective(config: &crate::config::Config) -> ToolCallMode {
    if config.tool_profile == crate::config::ToolProfile::PythonOnly {
        ToolCallMode::Single
    } else {
        config.tool_calls
    }
}

/// True when a turn may carry more than one tool call under `config`.
pub fn allows_batches(config: &crate::config::Config) -> bool {
    effective(config) == ToolCallMode::Sequential
}

/// Text-format calls take their id from the model's `tool_call_id`, which may
/// repeat or be missing; results must name each call exactly once.
pub fn with_unique_ids(
    mut calls: Vec<crate::context::ToolCall>,
) -> Vec<crate::context::ToolCall> {
    let mut seen = std::collections::HashSet::new();
    for (index, call) in calls.iter_mut().enumerate() {
        if !seen.insert(call.id.clone()) {
            let mut suffix = index + 1;
            while !seen.insert(format!("{}-{suffix}", call.id)) {
                suffix += 1;
            }
            call.id = format!("{}-{suffix}", call.id);
        }
    }
    calls
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_only_is_always_single() {
        let config = crate::config::Config {
            tool_profile: crate::config::ToolProfile::PythonOnly,
            ..Default::default()
        };
        assert_eq!(effective(&config), ToolCallMode::Single);
        assert_eq!(ToolCallMode::Single.next(), ToolCallMode::Sequential);
        let parsed: ToolCallMode = serde_yaml::from_str("sequential").unwrap();
        assert_eq!(parsed, ToolCallMode::Sequential);
    }
}
