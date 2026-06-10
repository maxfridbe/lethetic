use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
pub struct ModelServer {
    pub name: String,
    /// May be supplied by `config.local.yml` (e.g. private endpoints kept out of git).
    /// A server with an empty url is shown but unreachable.
    #[serde(default)]
    pub url: String,
    pub model: String,
    /// Parser dialect: "gemma4" | "qwen3" | "default"
    /// Controls initial parser state and which token markers to expect.
    #[serde(default = "default_parser")]
    pub parser: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub context_size: Option<usize>,
    #[serde(default)]
    pub input_cost_per_1m: Option<f64>,
    #[serde(default)]
    pub output_cost_per_1m: Option<f64>,
    #[serde(default)]
    pub thinking: Option<bool>,
    #[serde(default)]
    pub extra_body: Option<serde_json::Value>,
    #[serde(default)]
    pub context_mode: Option<crate::context::ContextMode>,
    #[serde(default)]
    pub theme: Option<String>,
}

fn default_parser() -> String { "gemma4".to_string() }

/// `context_size` values treated as "unset" when merging server-specific settings.
pub const CONTEXT_SIZE_UNSET: usize = 0;
pub const CONTEXT_SIZE_LEGACY_DEFAULT: usize = 262_144;

#[derive(Deserialize, Debug, Clone, Default)]
pub struct Config {
    pub server_url: String,
    pub model: String,
    pub context_size: usize,
    pub tool_wrapper: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub estimate_cost: Option<bool>,
    #[serde(default)]
    pub input_cost_per_1m: Option<f64>,
    #[serde(default)]
    pub output_cost_per_1m: Option<f64>,
    #[serde(default)]
    pub enable_image_processing_tool: bool,
    #[serde(default)]
    pub theme: Option<String>,
    #[serde(default)]
    pub model_servers: Vec<ModelServer>,
    #[serde(default)]
    pub thinking: Option<bool>,
    #[serde(default)]
    pub extra_body: Option<serde_json::Value>,
    #[serde(default)]
    pub context_mode: Option<crate::context::ContextMode>,
}

impl Config {
    /// Load a config file, overlaying a sibling `config.local.yml` if present.
    /// The local file is for machine-specific secrets (API keys) kept out of git:
    /// mappings merge recursively, `model_servers` entries merge by `name`,
    /// scalars in the local file win.
    pub fn load(path: impl AsRef<std::path::Path>) -> Result<Self, String> {
        let path = path.as_ref();
        let base_text = std::fs::read_to_string(path)
            .map_err(|e| format!("Could not read {}: {}", path.display(), e))?;
        let mut value: serde_yaml::Value = serde_yaml::from_str(&base_text)
            .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))?;

        let local_path = path.with_file_name("config.local.yml");
        if let Ok(local_text) = std::fs::read_to_string(&local_path) {
            let overlay: serde_yaml::Value = serde_yaml::from_str(&local_text)
                .map_err(|e| format!("Failed to parse {}: {}", local_path.display(), e))?;
            merge_yaml(&mut value, overlay);
        }

        serde_yaml::from_value(value).map_err(|e| format!("Invalid config: {}", e))
    }

    /// Merges server-specific settings from the matching server in `model_servers` if they are not already set.
    pub fn merge_matching_server_settings(&mut self) {
        if let Some(matching_server) = self.model_servers.iter().find(|s| s.url == self.server_url) {
            if self.api_key.is_none() {
                self.api_key = matching_server.api_key.clone();
            }
            if self.input_cost_per_1m.is_none() {
                self.input_cost_per_1m = matching_server.input_cost_per_1m;
            }
            if self.output_cost_per_1m.is_none() {
                self.output_cost_per_1m = matching_server.output_cost_per_1m;
            }
            if self.thinking.is_none() {
                self.thinking = matching_server.thinking;
            }
            if self.extra_body.is_none() {
                self.extra_body = matching_server.extra_body.clone();
            }
            if self.context_mode.is_none() {
                self.context_mode = matching_server.context_mode;
            }
            if self.theme.is_none() {
                self.theme = matching_server.theme.clone();
            }
            if let Some(sz) = matching_server.context_size
                && (self.context_size == CONTEXT_SIZE_UNSET || self.context_size == CONTEXT_SIZE_LEGACY_DEFAULT) {
                    self.context_size = sz;
                }
        }
    }
}

/// Overlay `overlay` onto `base`: mappings merge recursively; sequence items
/// that are mappings with a matching `name` merge in place (others are appended);
/// any other value in the overlay replaces the base value.
fn merge_yaml(base: &mut serde_yaml::Value, overlay: serde_yaml::Value) {
    use serde_yaml::Value;
    match (base, overlay) {
        (Value::Mapping(b), Value::Mapping(o)) => {
            for (k, v) in o {
                if let Some(bv) = b.get_mut(&k) {
                    merge_yaml(bv, v);
                } else {
                    b.insert(k, v);
                }
            }
        }
        (Value::Sequence(b), Value::Sequence(o)) => {
            for item in o {
                let matching = item.get("name").and_then(|n| {
                    b.iter_mut().find(|existing| existing.get("name") == Some(n))
                });
                match matching {
                    Some(existing) => merge_yaml(existing, item),
                    None => b.push(item),
                }
            }
        }
        (b, o) => *b = o,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_merge_matching_server_settings() {
        let mut config = Config {
            server_url: "http://example.com/api".to_string(),
            model: "my-model".to_string(),
            context_size: 0,
            model_servers: vec![
                ModelServer {
                    name: "Test Server".to_string(),
                    url: "http://example.com/api".to_string(),
                    model: "my-model".to_string(),
                    parser: "qwen3".to_string(),
                    api_key: Some("secret-key".to_string()),
                    context_size: Some(8192),
                    input_cost_per_1m: Some(1.5),
                    output_cost_per_1m: Some(2.5),
                    thinking: Some(true),
                    extra_body: Some(json!({"reasoning_effort": "high"})),
                    context_mode: None,
                    theme: Some("Monokai".to_string()),
                }
            ],
            ..Default::default()
        };

        config.merge_matching_server_settings();

        assert_eq!(config.api_key.as_deref(), Some("secret-key"));
        assert_eq!(config.input_cost_per_1m, Some(1.5));
        assert_eq!(config.output_cost_per_1m, Some(2.5));
        assert_eq!(config.thinking, Some(true));
        assert_eq!(config.context_size, 8192);
        assert_eq!(config.extra_body.as_ref().unwrap()["reasoning_effort"], "high");
        assert_eq!(config.theme.as_deref(), Some("Monokai"));
    }

    #[test]
    fn test_load_overlays_config_local() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("config.yml");
        std::fs::write(&base, "\
server_url: http://base/v1
model: base-model
context_size: 1000
tool_wrapper: null
model_servers:
  - name: Azure
    model: deepseek
  - name: Local
    url: http://local/v1
    model: gemma
").unwrap();
        std::fs::write(dir.path().join("config.local.yml"), "\
api_key: top-level-key
model_servers:
  - name: Azure
    url: http://azure/v1
    api_key: azure-secret
  - name: Extra
    url: http://extra/v1
    model: extra-model
").unwrap();

        let config = Config::load(&base).unwrap();
        assert_eq!(config.api_key.as_deref(), Some("top-level-key"));
        assert_eq!(config.model, "base-model");
        assert_eq!(config.model_servers.len(), 3);
        let azure = config.model_servers.iter().find(|s| s.name == "Azure").unwrap();
        assert_eq!(azure.api_key.as_deref(), Some("azure-secret"));
        assert_eq!(azure.url, "http://azure/v1"); // url supplied entirely by the overlay
        assert_eq!(azure.model, "deepseek");      // base fields survive the overlay
        assert!(config.model_servers.iter().any(|s| s.name == "Extra"));
    }

    #[test]
    fn test_load_without_local_file() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("config.yml");
        std::fs::write(&base, "server_url: http://x\nmodel: m\ncontext_size: 1\ntool_wrapper: null\n").unwrap();
        let config = Config::load(&base).unwrap();
        assert_eq!(config.model, "m");
    }
}



