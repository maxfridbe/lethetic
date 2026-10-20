//! Browser adapter around the CLI's OpenAI parser. Transport/framing stays in the host.
use gemma_chat::{StreamEvent, StreamParser};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[path = "streaming_anthropic.rs"]
mod anthropic;
use anthropic::AnthropicStream;

pub struct ProviderStream {
    openai: StreamParser,
    anthropic: Option<AnthropicStream>,
    message: Value,
    usage: Value,
    details: BTreeMap<u64, Value>,
    stop: Option<String>,
    done: bool,
    bytes: usize,
}

impl ProviderStream {
    pub fn new(provider: &str) -> Result<Self, String> {
        if !matches!(provider, "openai" | "anthropic") {
            return Err("unsupported streaming provider".into());
        }
        Ok(Self {
            openai: StreamParser::new(),
            anthropic: (provider == "anthropic").then(AnthropicStream::default),
            message: json!({"role":"assistant","content":""}),
            usage: json!({}),
            details: BTreeMap::new(),
            stop: None,
            done: false,
            bytes: 0,
        })
    }

    /// Accept one complete SSE data field (multiline data joined by the host).
    pub fn push(&mut self, data: &str) -> Result<String, String> {
        if self.done {
            return Err("stream data after completion".into());
        }
        self.bytes = self.bytes.saturating_add(data.len());
        if self.bytes > crate::engine::MAX_BYTES {
            return Err("model stream exceeds 4 MiB".into());
        }
        let mut updates = Vec::new();
        if let Some(parser) = &mut self.anthropic {
            self.done = parser.push(data, &mut updates)?;
        } else {
            if data != "[DONE]" {
                let value: Value = serde_json::from_str(data).map_err(|e| e.to_string())?;
                if value.get("error").is_some() {
                    return Err("provider returned a streaming error".into());
                }
                if let Some(usage) = value.get("usage").filter(|v| v.is_object()) {
                    merge(&mut self.usage, usage);
                }
                let delta = &value["choices"][0]["delta"];
                for key in ["reasoning", "reasoning_content", "reasoning_text"] {
                    if let Some(text) = delta.get(key).filter(|v| !v.is_null()) {
                        append(
                            &mut self.message,
                            key,
                            text.as_str().ok_or("invalid reasoning delta")?,
                        );
                    }
                }
                if let Some(details) = delta.get("reasoning_details").filter(|v| !v.is_null()) {
                    for (position, detail) in details
                        .as_array()
                        .ok_or("invalid reasoning details")?
                        .iter()
                        .enumerate()
                    {
                        let object = detail.as_object().ok_or("invalid reasoning detail")?;
                        let index = match detail.get("index") {
                            Some(index) => index.as_u64().ok_or("invalid reasoning index")?,
                            None => u64::try_from(position).map_err(|e| e.to_string())?,
                        };
                        if self.details.len() >= 256 && !self.details.contains_key(&index) {
                            return Err("too many reasoning blocks".into());
                        }
                        let target = self.details.entry(index).or_insert_with(|| json!({}));
                        for (key, value) in object {
                            if matches!(key.as_str(), "text" | "summary" | "signature" | "data") {
                                append(
                                    target,
                                    key,
                                    value.as_str().ok_or("invalid reasoning fragment")?,
                                );
                            } else {
                                target[key] = value.clone();
                            }
                        }
                    }
                }
            }
            let events = self.openai.process_line(&format!("data: {data}"));
            self.events(events, &mut updates)?;
        }
        Ok(json!({"updates":updates,"done":self.done}).to_string())
    }

    /// Require terminal framing before the host can commit history or execute tools.
    pub fn finish(&mut self) -> Result<String, String> {
        if let Some(parser) = &self.anthropic {
            return parser.finish();
        }
        if !self.done {
            let events = self.openai.finish_clean();
            self.events(events, &mut Vec::new())?;
        }
        if !self.done {
            return Err("provider stream ended without a terminal event".into());
        }
        if !self.details.is_empty() {
            self.message["reasoning_details"] =
                Value::Array(self.details.values().cloned().collect());
        }
        Ok(json!({"choices":[{"message":self.message,"finish_reason":self.stop}],"usage":self.usage}).to_string())
    }
}

impl ProviderStream {
    fn events(&mut self, events: Vec<StreamEvent>, updates: &mut Vec<Value>) -> Result<(), String> {
        for event in events {
            match event {
                StreamEvent::TextDelta(text) => {
                    append(&mut self.message, "content", &text);
                    updates.push(json!({"type":"assistant_delta","text":text}));
                }
                StreamEvent::ReasoningDelta(text) => {
                    updates.push(json!({"type":"thinking_delta","text":text}))
                }
                StreamEvent::ToolCallComplete {
                    id,
                    name,
                    arguments,
                    ..
                } => {
                    let calls = self
                        .message
                        .as_object_mut()
                        .ok_or("invalid message")?
                        .entry("tool_calls")
                        .or_insert_with(|| json!([]))
                        .as_array_mut()
                        .ok_or("invalid calls")?;
                    calls.push(json!({"id":id,"type":"function","function":{"name":name,"arguments":arguments.to_string()}}));
                }
                StreamEvent::Done { stop_reason, .. } => {
                    self.stop = stop_reason;
                    self.done = true;
                }
                StreamEvent::Error(error) => return Err(error),
                _ => {}
            }
        }
        Ok(())
    }
}

fn append(value: &mut Value, key: &str, text: &str) {
    if let Some(Value::String(current)) = value.get_mut(key) {
        current.push_str(text);
    } else {
        value[key] = json!(text);
    }
}
fn merge(target: &mut Value, newer: &Value) {
    if let Some(object) = newer.as_object() {
        for (key, value) in object {
            if value.is_object() && target[key].is_object() {
                merge(&mut target[key], value);
            } else {
                target[key] = value.clone();
            }
        }
    }
}
