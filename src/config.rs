use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
pub struct Config {
    pub server_url: String,
    pub model: String,
    pub context_size: usize,
    pub tool_wrapper: Option<String>,
}
