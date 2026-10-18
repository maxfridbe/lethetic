//! Browser-portable conversation state. The host owns networking and tool execution.
//! No filesystem, process, credentials, or native runtime dependencies are included.

mod provider;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;

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
    calls: Vec<ToolCall>,
}

/// Stateful, transactional model conversation exported to JavaScript.
/// Call `begin`, then alternate `request`, `accept`, and `tool_result`.
/// `rollback` discards an incomplete turn after a network error or cancellation.
#[wasm_bindgen]
pub struct BrowserRuntime {
    system: String,
    tools: Vec<Tool>,
    messages: Vec<Value>,
    pending: Vec<String>,
    checkpoint: Option<usize>,
    awaiting_response: bool,
}

#[wasm_bindgen]
impl BrowserRuntime {
    /// Create a runtime with a system prompt and JSON array of tool definitions.
    /// Returns an error for malformed, duplicate, or oversized definitions.
    #[wasm_bindgen(constructor)]
    pub fn new(system: &str, tools_json: &str) -> Result<BrowserRuntime, String> {
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
        })
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
        let body = provider::request(provider, model, &self.system, &self.tools, &self.messages)?;
        let encoded = body.to_string();
        bounded(&encoded)?;
        self.awaiting_response = true;
        Ok(encoded)
    }

    /// Validate a model response before committing it to conversation history.
    /// Unknown tools, duplicate IDs, and malformed argument objects are rejected.
    pub fn accept(&mut self, provider: &str, response: &str) -> Result<String, String> {
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
                || !self.tools.iter().any(|tool| tool.name == call.name)
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

    /// Discard an unfinished turn. This does not undo host tool side effects.
    pub fn rollback(&mut self) {
        if let Some(checkpoint) = self.checkpoint.take() {
            self.messages.truncate(checkpoint);
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
