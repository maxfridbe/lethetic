use serde::Deserialize;

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
        }
    }
}
