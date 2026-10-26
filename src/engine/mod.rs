//! Browser-portable conversation state. The host owns networking and tool execution.
//! No filesystem, process, credentials, or native runtime dependencies are included.

pub(crate) mod anthropic;
pub mod compaction;
mod context;
pub(crate) mod messages;
mod provider;
mod streaming;
pub(crate) mod tool_errors;
pub mod transport_types;
pub use streaming::ProviderStream;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_MESSAGES: usize = 512;

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Tool {
    name: String,
    description: String,
    parameters: Value,
}

/// A validated request for a tool explicitly registered by the embedding host.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Clone, Debug, Serialize)]
struct Turn {
    text: String,
    thinking: String,
    calls: Vec<ToolCall>,
}

/// Stateful, transactional model conversation exported to JavaScript.
/// Call `begin`, then alternate `request`, `accept`, and `tool_result`.
/// `rollback` retains the user's prompt while discarding incomplete model output.
pub struct Conversation {
    system: String,
    tools: Vec<Tool>,
    messages: Vec<Value>,
    pending: Vec<String>,
    checkpoint: Option<usize>,
    awaiting_response: bool,
    context_limit: usize,
    output_limit: u32,
    last_request_tokens: usize,
}

impl Conversation {
    /// Create a runtime with a system prompt and JSON array of tool definitions.
    /// Returns an error for malformed, duplicate, or oversized definitions.
    pub fn new(system: &str, tools_json: &str) -> Result<Conversation, String> {
        bounded(system)?;
        bounded(tools_json)?;
        let tools: Vec<Tool> = serde_json::from_str(tools_json).map_err(|e| e.to_string())?;
        if tools.len() > 64 {
            return Err("at most 64 tools may be registered".into());
        }
        for (index, tool) in tools.iter().enumerate() {
            if tool.name.is_empty()
                || tool.name.len() > 64
                || !tool
                    .name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                || tools[..index].iter().any(|other| other.name == tool.name)
                || tool.parameters.get("type").and_then(Value::as_str) != Some("object")
            {
                return Err("tools need unique names and object parameter schemas".into());
            }
        }
        Ok(Self {
            system: system.into(),
            tools,
            messages: Vec::new(),
            pending: Vec::new(),
            checkpoint: None,
            awaiting_response: false,
            context_limit: 32_768,
            output_limit: 4096,
            last_request_tokens: 0,
        })
    }

    /// Use the same input budget and context projection as the native CLI.
    pub fn set_limits(&mut self, tokens: usize, output: u32) -> Result<(), String> {
        if self.checkpoint.is_some() {
            return Err("stop the turn before changing context limits".into());
        }
        if !(256..=2_000_000).contains(&tokens) {
            return Err("context budget must be 256–2000000 tokens".into());
        }
        if !(1..=2_000_000).contains(&output) {
            return Err("output budget must be 1–2000000 tokens".into());
        }
        self.context_limit = tokens;
        self.output_limit = output;
        Ok(())
    }

    /// Start a user turn. Concurrent turns and empty prompts are rejected.
    pub fn begin(&mut self, prompt: &str) -> Result<(), String> {
        if self.checkpoint.is_some() {
            return Err("a turn is already active".into());
        }
        if prompt.trim().is_empty() {
            return Err("enter a message".into());
        }
        bounded(prompt)?;
        self.checkpoint = Some(self.messages.len());
        self.messages
            .push(json!({"role": "user", "content": prompt}));
        Ok(())
    }

    /// Serialize an OpenAI Chat Completions or Anthropic Messages request.
    pub fn request(&mut self, provider: &str, model: &str) -> Result<String, String> {
        if self.checkpoint.is_none() || !self.pending.is_empty() || self.awaiting_response {
            return Err("the conversation is not ready for a model request".into());
        }
        if model.trim().is_empty() {
            return Err("a model is required".into());
        }
        if self.messages.len() > MAX_MESSAGES {
            return Err("conversation is full; start a new chat".into());
        }
        let context = context::project(&self.system, &self.messages, self.context_limit, provider)?;
        self.last_request_tokens = context.estimated_tokens();
        let body = provider::request(
            provider,
            model,
            &self.tools,
            self.output_limit,
            context.messages(),
        )?;
        let encoded = body.to_string();
        bounded(&encoded)?;
        self.awaiting_response = true;
        Ok(encoded)
    }

    /// CLI estimate for the most recently prepared request, before provider usage arrives.
    pub fn last_request_tokens(&self) -> usize {
        self.last_request_tokens
    }

    /// Validate a model response before committing it to conversation history.
    /// Unknown tools, duplicate IDs, and malformed argument objects are rejected.
    pub fn accept(&mut self, provider: &str, response: &str) -> Result<String, String> {
        self.accept_response(provider, response, true)
    }

    fn accept_response(
        &mut self,
        provider: &str,
        response: &str,
        require_registered: bool,
    ) -> Result<String, String> {
        if !self.awaiting_response {
            return Err("no model response is expected".into());
        }
        bounded(response)?;
        let value: Value = serde_json::from_str(response).map_err(|e| e.to_string())?;
        let (message, turn) = provider::response(provider, &value)?;
        if turn.calls.len() > 64 {
            return Err("model requested too many tools".into());
        }
        for (index, call) in turn.calls.iter().enumerate() {
            if call.id.is_empty()
                || turn.calls[..index].iter().any(|other| other.id == call.id)
                || !call.arguments.is_object()
                || call.name.is_empty()
                || call.name.len() > 64
                || !call
                    .name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
                || (require_registered && !self.tools.iter().any(|tool| tool.name == call.name))
            {
                return Err("model returned an invalid or unregistered tool call".into());
            }
        }
        self.pending = turn.calls.iter().map(|call| call.id.clone()).collect();
        self.messages.push(message);
        self.awaiting_response = false;
        if self.pending.is_empty() {
            self.checkpoint = None;
        }
        serde_json::to_string(&turn).map_err(|e| e.to_string())
    }

    /// Add exactly one result for a pending call. Tool output is plain text.
    pub fn tool_result(&mut self, id: &str, output: &str) -> Result<(), String> {
        bounded(output)?;
        let position = self
            .pending
            .iter()
            .position(|pending| pending == id)
            .ok_or("tool call is unknown or already completed")?;
        self.pending.remove(position);
        self.messages
            .push(json!({"role":"tool", "tool_call_id":id, "content":output}));
        Ok(())
    }

    /// Discard unfinished model/tool messages but retain the user's instruction
    /// so the next turn can resume it. This does not undo host tool side effects.
    pub fn rollback(&mut self) {
        if let Some(checkpoint) = self.checkpoint.take() {
            self.messages.truncate(checkpoint + 1);
        }
        self.pending.clear();
        self.awaiting_response = false;
    }

    /// Clear the conversation, retaining the system prompt and registered tools.
    pub fn clear(&mut self) {
        self.messages.clear();
        self.rollback();
    }

    /// Export conversation messages as JSON. Credentials never enter the runtime.
    pub fn history(&self) -> String {
        Value::Array(self.messages.clone()).to_string()
    }

    /// Restore completed turns and unanswered user prompts without executing tools
    /// or accepting system messages. Incomplete tool exchanges remain invalid.
    pub fn restore(&mut self, history: &str) -> Result<(), String> {
        if self.checkpoint.is_some() {
            return Err("stop the active turn before restoring history".into());
        }
        bounded(history)?;
        let messages: Vec<Value> = serde_json::from_str(history).map_err(|e| e.to_string())?;
        if messages.len() > MAX_MESSAGES {
            return Err("saved conversation exceeds the message limit".into());
        }
        let definitions = serde_json::to_string(&self.tools).map_err(|e| e.to_string())?;
        let mut candidate = Self::new(&self.system, &definitions)?;
        for message in messages {
            match message["role"].as_str() {
                Some("user") => {
                    candidate.finish_unanswered_prompt();
                    candidate.begin(message["content"].as_str().ok_or("invalid user message")?)?
                }
                Some("assistant") => {
                    let is_anthropic = message.get("anthropic_content").is_some();
                    let has_calls = message["tool_calls"]
                        .as_array()
                        .is_some_and(|calls| !calls.is_empty());
                    let (provider, response) = if is_anthropic {
                        (
                            "anthropic",
                            json!({"role":"assistant", "content":message["anthropic_content"],
                            "stop_reason":if has_calls { "tool_use" } else { "end_turn" }}),
                        )
                    } else {
                        (
                            "openai",
                            json!({"choices":[{"message":message,
                            "finish_reason":if has_calls { "tool_calls" } else { "stop" }}]}),
                        )
                    };
                    candidate.request(provider, "restore")?;
                    // Saved tool results are inert history, even after changing
                    // host capabilities. Live responses still require registration.
                    candidate.accept_response(provider, &response.to_string(), false)?;
                }
                Some("tool") => candidate.tool_result(
                    message["tool_call_id"]
                        .as_str()
                        .ok_or("invalid tool result id")?,
                    message["content"]
                        .as_str()
                        .ok_or("invalid tool result content")?,
                )?,
                _ => return Err("unsupported saved message role".into()),
            }
        }
        candidate.finish_unanswered_prompt();
        if candidate.checkpoint.is_some() {
            return Err("saved conversation contains an incomplete turn".into());
        }
        self.messages = candidate.messages;
        Ok(())
    }

    // A failed turn can leave just its user prompt. It is safe to restore this
    // as idle history, but never silently accept a partial tool exchange.
    fn finish_unanswered_prompt(&mut self) {
        if self
            .checkpoint
            .is_some_and(|start| self.messages.len() == start + 1)
        {
            self.checkpoint = None;
        }
    }
}

fn bounded(value: &str) -> Result<(), String> {
    if value.len() > MAX_BYTES {
        Err("conversation data exceeds the 4 MiB limit".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;

/// CLI retry classification for browser-owned network requests.
pub fn retryable_error(error: &str) -> bool {
    gemma_chat::retry::is_retryable(error)
}

/// CLI exponential backoff, in milliseconds.
pub fn retry_delay_ms(attempt: u32) -> u32 {
    u32::try_from(gemma_chat::retry::delay(attempt).as_millis()).unwrap_or(30_000)
}

/// Bounded public provider error details for browser HTTP failures.
pub fn provider_error_message(body: &str) -> Option<String> {
    serde_json::from_str(body)
        .ok()
        .and_then(|value| gemma_chat::provider_error::response_error(&value))
}
