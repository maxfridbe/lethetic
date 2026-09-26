use futures_util::Stream;
use reqwest::Client;
use serde_json::Value;
use std::pin::Pin;

use crate::accounting::Usage;
use crate::config::{Config, ConnectionKind, ToolProfile};

pub mod anthropic;
pub mod openai;
pub mod request_policy;

pub type EventStream = Pin<Box<dyn Stream<Item = StreamEvent> + Send>>;

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

#[derive(Debug)]
pub struct PreparedAgentRequest {
    connection_kind: ConnectionKind,
    server_url: String,
    model: String,
    request_output_tokens: u32,
    tool_profile: ToolProfile,
    python_guidance: Option<String>,
    encoded_body: Vec<u8>,
}

impl PreparedAgentRequest {
    pub fn body_bytes(&self) -> &[u8] {
        &self.encoded_body
    }

    fn validate_binding(&self, config: &Config) -> Result<(), String> {
        let guidance = crate::system_prompt::python_capability_guidance(config);
        if self.connection_kind != config.active_connection_kind()
            || self.server_url != config.server_url
            || self.model != config.model
            || self.request_output_tokens != config.request_output_tokens()
            || self.tool_profile != config.tool_profile
            || self.python_guidance != guidance
        {
            return Err(
                "Prepared agent request does not match the active provider configuration"
                    .to_string(),
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: String,
    /// List prices when the catalog publishes them (OpenRouter does).
    pub pricing: Option<CatalogPricing>,
}

/// Per-token USD prices as published by a model catalog, kept as the
/// original decimal strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogPricing {
    pub prompt: String,
    pub completion: String,
    pub cache_read: Option<String>,
    pub cache_write: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub text: String,
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionError {
    pub message: String,
    pub usage: Option<Usage>,
}

impl CompletionError {
    pub fn without_usage(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            usage: None,
        }
    }
}

impl std::fmt::Display for CompletionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CompletionError {}

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

pub fn prepare_agent_request(
    config: &Config,
    messages: &[Message],
    tools: &[ToolDefinition],
    max_tokens: u32,
) -> Result<PreparedAgentRequest, String> {
    if max_tokens != config.request_output_tokens() {
        return Err(
            "Agent request token limit does not match the active configuration".to_string(),
        );
    }
    request_policy::validate_agent_surface(config, messages, tools)?;
    let body = match config.active_connection_kind() {
        ConnectionKind::OpenAiChatCompletions => {
            openai::build_request(config, messages, tools, max_tokens, true)?
        }
        ConnectionKind::ClaudeCodeProxy => {
            anthropic::build_request(config, messages, tools, max_tokens, true)?
        }
    };
    request_policy::validate_final_agent_body(config, &body)?;
    let encoded_body = serde_json::to_vec_pretty(&body)
        .map_err(|error| format!("Could not encode prepared agent request: {error}"))?;
    Ok(PreparedAgentRequest {
        connection_kind: config.active_connection_kind(),
        server_url: config.server_url.clone(),
        model: config.model.clone(),
        request_output_tokens: max_tokens,
        tool_profile: config.tool_profile,
        python_guidance: crate::system_prompt::python_capability_guidance(config),
        encoded_body,
    })
}

pub async fn stream_prepared(
    client: &Client,
    config: &Config,
    request: PreparedAgentRequest,
) -> Result<EventStream, String> {
    request.validate_binding(config)?;
    match request.connection_kind {
        ConnectionKind::OpenAiChatCompletions => {
            openai::stream_encoded(client, config, request.encoded_body).await
        }
        ConnectionKind::ClaudeCodeProxy => {
            anthropic::stream_encoded(client, config, request.encoded_body).await
        }
    }
}

pub async fn stream(
    client: &Client,
    config: &Config,
    messages: &[Message],
    tools: &[ToolDefinition],
    max_tokens: u32,
) -> Result<EventStream, String> {
    let request = prepare_agent_request(config, messages, tools, max_tokens)?;
    stream_prepared(client, config, request).await
}

pub async fn complete(
    client: &Client,
    config: &Config,
    messages: &[Message],
    max_tokens: u32,
) -> Result<String, String> {
    complete_with_usage(client, config, messages, max_tokens)
        .await
        .map(|completion| completion.text)
}

pub async fn complete_with_usage(
    client: &Client,
    config: &Config,
    messages: &[Message],
    max_tokens: u32,
) -> Result<Completion, String> {
    complete_with_usage_or_error(client, config, messages, max_tokens)
        .await
        .map_err(|error| error.message)
}

pub async fn complete_with_usage_or_error(
    client: &Client,
    config: &Config,
    messages: &[Message],
    max_tokens: u32,
) -> Result<Completion, CompletionError> {
    match config.active_connection_kind() {
        ConnectionKind::OpenAiChatCompletions => {
            openai::complete_with_usage(client, config, messages, max_tokens)
                .await
                .map_err(CompletionError::without_usage)
        }
        ConnectionKind::ClaudeCodeProxy => {
            anthropic::complete_with_usage_or_error(client, config, messages, max_tokens).await
        }
    }
}

pub async fn discover_models(
    client: &Client,
    kind: ConnectionKind,
    server_url: &str,
    api_key: Option<&str>,
) -> Result<Vec<ModelInfo>, String> {
    match kind {
        ConnectionKind::OpenAiChatCompletions => {
            openai::discover_models(client, server_url, api_key).await
        }
        ConnectionKind::ClaudeCodeProxy => {
            anthropic::discover_models(client, server_url, api_key).await
        }
    }
}
