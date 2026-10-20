//! Shared CLI model types; UI and OS hosts must not be dependencies of parsing.
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    pub function: FunctionCall,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum BlockType {
    Text,
    ProviderError,
    User,
    Thought,
    Markdown,
    ToolCall,
    ToolResult,
    ToolError,
    Divider,
    Formulating,
}
