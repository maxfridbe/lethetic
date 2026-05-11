use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
pub struct ModelServer {
    pub name: String,
    pub url: String,
    pub model: String,
    /// Parser dialect: "gemma4" | "qwen3" | "default"
    /// Controls initial parser state and which token markers to expect.
    #[serde(default = "default_parser")]
    pub parser: String,
}

fn default_parser() -> String { "gemma4".to_string() }

#[derive(Deserialize, Debug, Clone)]
pub struct Config {
    pub server_url: String,
    pub model: String,
    pub context_size: usize,
    pub tool_wrapper: Option<String>,
    #[serde(default)]
    pub enable_image_processing_tool: bool,
    #[serde(default)]
    pub theme: Option<String>,
    #[serde(default)]
    pub model_servers: Vec<ModelServer>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server_url: String::new(),
            model: String::new(),
            context_size: 0,
            tool_wrapper: None,
            enable_image_processing_tool: false,
            theme: None,
            model_servers: Vec::new(),
        }
    }
}
