use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Request types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Message {
    pub role: Role,
    pub content: Value, // string or array of content parts
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<AssistantToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self { role: Role::System, content: Value::String(content.into()), tool_call_id: None, tool_calls: None, reasoning_content: None }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self { role: Role::User, content: Value::String(content.into()), tool_call_id: None, tool_calls: None, reasoning_content: None }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self { role: Role::Assistant, content: Value::String(content.into()), tool_call_id: None, tool_calls: None, reasoning_content: None }
    }
    pub fn assistant_with_tools(content: impl Into<String>, tool_calls: Vec<AssistantToolCall>, reasoning: Option<String>) -> Self {
        Self { role: Role::Assistant, content: Value::String(content.into()), tool_call_id: None, tool_calls: Some(tool_calls), reasoning_content: reasoning }
    }
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self { role: Role::Tool, content: Value::String(content.into()), tool_call_id: Some(tool_call_id.into()), tool_calls: None, reasoning_content: None }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String, // JSON string
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: FunctionDefinition,
}

#[derive(Debug, Clone, Serialize)]
pub struct FunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value, // JSON Schema object
}

impl ToolDefinition {
    pub fn new(name: impl Into<String>, description: impl Into<String>, parameters: Value) -> Self {
        Self {
            kind: "function".into(),
            function: FunctionDefinition { name: name.into(), description: description.into(), parameters },
        }
    }
}

// ── Stream events ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// Model is thinking / reasoning (displayed but not the final response)
    ReasoningDelta(String),
    /// Final response text chunk
    TextDelta(String),
    /// Start of a tool call (id + function name)
    ToolCallStart { id: String, index: usize, name: String },
    /// Streamed fragment of tool call JSON arguments
    ToolCallDelta { index: usize, args_fragment: String },
    /// All chunks received; complete parsed arguments
    ToolCallComplete { index: usize, id: String, name: String, arguments: Value },
    /// Generation finished
    Done { completion_tokens: Option<u32>, prompt_tokens: Option<u32>, tg_per_s: Option<f64>, pp_per_s: Option<f64> },
    /// Server or parse error
    Error(String),
}

// ── Internal chunk schema (mirrors opencode's zod schema) ─────────────────────

#[derive(Debug, Deserialize)]
pub(crate) struct Chunk {
    pub choices: Option<Vec<ChunkChoice>>,
    pub usage: Option<UsageChunk>,
    pub timings: Option<Timings>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Timings {
    pub predicted_per_second: Option<f64>,
    pub prompt_per_second: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ChunkChoice {
    pub delta: Option<Delta>,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Delta {
    pub content: Option<String>,
    /// Gemma 4 / llama.cpp reasoning field
    pub reasoning_content: Option<String>,
    /// Copilot-style reasoning field (alias)
    pub reasoning_text: Option<String>,
    pub tool_calls: Option<Vec<ToolCallDelta>>,
}

impl Delta {
    pub fn reasoning(&self) -> Option<&str> {
        self.reasoning_content.as_deref().or(self.reasoning_text.as_deref())
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolCallDelta {
    pub index: usize,
    pub id: Option<String>,
    pub function: Option<FunctionDelta>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FunctionDelta {
    pub name: Option<String>,
    pub arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UsageChunk {
    pub completion_tokens: Option<u32>,
    pub prompt_tokens: Option<u32>,
}
