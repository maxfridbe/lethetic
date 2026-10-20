//! Transport-neutral CLI messages and events, shared by native and WASM hosts.
use crate::accounting::Usage;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Content {
    Text(String),
    Parts(Vec<ContentPart>),
}

impl Content {
    pub fn text(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::Parts(parts) => parts
                .iter()
                .filter_map(|part| match part {
                    ContentPart::Text(text) => Some(text.as_str()),
                    ContentPart::Image { .. } => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Self::Text(text) => text.is_empty(),
            Self::Parts(parts) => parts.is_empty(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ContentPart {
    Text(String),
    Image { media_type: String, data: String },
}

#[derive(Debug, Clone)]
pub struct Message {
    pub role: Role,
    pub content: Content,
    pub tool_call_id: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub tool_result_is_error: bool,
    /// Provider-native assistant blocks that must be replayed unchanged.
    pub provider_content: Option<Vec<Value>>,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self::text(Role::System, content)
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::text(Role::User, content)
    }

    pub fn user_with_pngs(content: impl Into<String>, images: &[String]) -> Self {
        let mut parts = images
            .iter()
            .map(|data| ContentPart::Image {
                media_type: "image/png".to_string(),
                data: data.clone(),
            })
            .collect::<Vec<_>>();
        parts.push(ContentPart::Text(content.into()));
        Self {
            role: Role::User,
            content: Content::Parts(parts),
            tool_call_id: None,
            tool_calls: Vec::new(),
            tool_result_is_error: false,
            provider_content: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::text(Role::Assistant, content)
    }

    pub fn assistant_with_tools(
        content: impl Into<String>,
        tool_calls: Vec<ToolCall>,
        provider_content: Option<Vec<Value>>,
    ) -> Self {
        Self {
            role: Role::Assistant,
            content: Content::Text(content.into()),
            tool_call_id: None,
            tool_calls,
            tool_result_is_error: false,
            provider_content,
        }
    }

    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self::tool_result_with_status(tool_call_id, content, false)
    }

    pub fn tool_result_with_status(
        tool_call_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self {
            role: Role::Tool,
            content: Content::Text(content.into()),
            tool_call_id: Some(tool_call_id.into()),
            tool_calls: Vec::new(),
            tool_result_is_error: is_error,
            provider_content: None,
        }
    }

    fn text(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: Content::Text(content.into()),
            tool_call_id: None,
            tool_calls: Vec::new(),
            tool_result_is_error: false,
            provider_content: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone)]
pub enum StreamEvent {
    ReasoningDelta {
        text: String,
        segment_index: Option<usize>,
    },
    TextDelta(String),
    ToolCallStart {
        id: String,
        index: usize,
        name: String,
    },
    ToolCallDelta {
        index: usize,
        args_fragment: String,
    },
    ToolCalls {
        calls: Vec<ToolCall>,
        provider_content: Option<Vec<Value>>,
    },
    /// Latest cumulative usage snapshot for the current provider request.
    UsageUpdate(Usage),
    Done {
        completion_tokens: Option<u32>,
        prompt_tokens: Option<u32>,
        usage: Option<Usage>,
        tg_per_s: Option<f64>,
        pp_per_s: Option<f64>,
        stop_reason: Option<String>,
        provider_content: Option<Vec<Value>>,
    },
    Error(String),
}
