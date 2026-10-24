//! CLI Anthropic parsing and accounting, independent of networking and executors.
use super::transport_types::{StreamEvent, ToolCall};
use crate::accounting::Usage;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};

#[cfg(feature = "native")]
#[derive(Default)]
pub(crate) struct SseDecoder {
    buffer: Vec<u8>,
}

#[cfg(feature = "native")]
impl SseDecoder {
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Vec<Result<String, String>> {
        self.buffer.extend_from_slice(bytes);
        let mut data = Vec::new();
        while let Some(position) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let line = self.buffer.drain(..=position).collect::<Vec<_>>();
            if let Some(decoded) = decode_sse_line(&line) {
                data.push(decoded);
            }
        }
        data
    }

    pub(crate) fn finish(&mut self) -> Vec<Result<String, String>> {
        if self.buffer.is_empty() {
            return Vec::new();
        }
        let line = std::mem::take(&mut self.buffer);
        decode_sse_line(&line).into_iter().collect()
    }
}

#[cfg(feature = "native")]
pub(crate) fn decode_sse_line(line: &[u8]) -> Option<Result<String, String>> {
    let line = line
        .strip_suffix(b"\n")
        .unwrap_or(line)
        .strip_suffix(b"\r")
        .unwrap_or(line);
    let data = line.strip_prefix(b"data:")?;
    let data = data.strip_prefix(b" ").unwrap_or(data);
    if data.is_empty() || data == b"[DONE]" {
        return None;
    }
    Some(
        String::from_utf8(data.to_vec())
            .map_err(|error| format!("Invalid UTF-8 in claude-code-proxy stream: {error}")),
    )
}

#[derive(Default)]
pub(crate) struct UsageTracker {
    pub(crate) usage: Usage,
    saw_uncached_input: bool,
    saw_cache_creation: bool,
    saw_cache_read: bool,
    saw_output: bool,
    last_emitted: Option<Usage>,
}

impl UsageTracker {
    pub(crate) fn update(&mut self, value: &Value) {
        if let Some(tokens) = value.get("input_tokens").and_then(Value::as_u64) {
            self.usage.uncached_input_tokens = tokens;
            self.saw_uncached_input = true;
        }
        if let Some(tokens) = value
            .get("cache_creation_input_tokens")
            .and_then(Value::as_u64)
        {
            self.usage.cache_creation_input_tokens = tokens;
            self.saw_cache_creation = true;
        }
        if let Some(tokens) = value.get("cache_read_input_tokens").and_then(Value::as_u64) {
            self.usage.cache_read_input_tokens = tokens;
            self.saw_cache_read = true;
        }
        if let Some(tokens) = value.get("output_tokens").and_then(Value::as_u64) {
            self.usage.output_tokens = tokens;
            self.saw_output = true;
        }
        self.refresh_derived();
    }

    fn refresh_derived(&mut self) {
        let saw_input = self.saw_uncached_input || self.saw_cache_creation || self.saw_cache_read;
        self.usage.total_input_tokens = saw_input
            .then(|| {
                self.usage
                    .uncached_input_tokens
                    .checked_add(self.usage.cache_creation_input_tokens)?
                    .checked_add(self.usage.cache_read_input_tokens)
            })
            .flatten();
        self.usage.breakdown_complete = self.saw_uncached_input
            && self.saw_cache_creation
            && self.saw_cache_read
            && self.saw_output
            && self.usage.total_input_tokens.is_some();
    }

    pub(crate) fn snapshot(&self) -> Option<Usage> {
        (self.saw_uncached_input
            || self.saw_cache_creation
            || self.saw_cache_read
            || self.saw_output)
            .then_some(self.usage)
    }

    pub(crate) fn update_event(&mut self) -> Option<StreamEvent> {
        let snapshot = self.snapshot()?;
        if self.last_emitted == Some(snapshot) {
            return None;
        }
        self.last_emitted = Some(snapshot);
        Some(StreamEvent::UsageUpdate(snapshot))
    }
}

#[cfg(feature = "native")]
pub(crate) fn anthropic_usage(value: &Value) -> Option<Usage> {
    let mut tracker = UsageTracker::default();
    tracker.update(value);
    tracker.snapshot()
}

#[derive(Default)]
pub(crate) struct AnthropicStreamParser {
    blocks: BTreeMap<usize, Value>,
    started_blocks: HashSet<usize>,
    seen_blocks: HashSet<usize>,
    partial_inputs: HashMap<usize, String>,
    tool_calls: BTreeMap<usize, ToolCall>,
    pub(crate) usage: UsageTracker,
    pub(crate) stop_reason: Option<String>,
    pub(crate) pending_error: Option<String>,
    pub(crate) stopped: bool,
}

impl AnthropicStreamParser {
    pub(crate) fn record_error(&mut self, error: impl Into<String>) {
        if self.pending_error.is_none() {
            self.pending_error = Some(error.into());
        }
        self.tool_calls.clear();
    }

    pub(crate) fn process_data(&mut self, data: &str) -> Vec<StreamEvent> {
        let event: Value = match serde_json::from_str(data) {
            Ok(event) => event,
            Err(error) => {
                self.record_error(format!("Invalid claude-code-proxy stream JSON: {error}"));
                return Vec::new();
            }
        };
        let mut events = Vec::new();
        let event_type = event["type"].as_str().unwrap_or_default();
        if self.stop_reason.is_some() && !matches!(event_type, "message_stop" | "ping" | "error") {
            self.record_error(format!(
                "claude-code-proxy sent '{event_type}' after its stop reason"
            ));
            return events;
        }
        match event_type {
            "message_start" => {
                if let Some(usage) = event.pointer("/message/usage") {
                    self.usage.update(usage);
                    if let Some(update) = self.usage.update_event() {
                        events.push(update);
                    }
                }
                if let Some(content) = event.pointer("/message/content").and_then(Value::as_array) {
                    for (index, block) in content.iter().enumerate() {
                        self.seen_blocks.insert(index);
                        self.blocks.insert(index, block.clone());
                    }
                }
            }
            "content_block_start" => {
                let Some(index) = event["index"]
                    .as_u64()
                    .and_then(|value| usize::try_from(value).ok())
                else {
                    self.record_error("claude-code-proxy content block is missing an index");
                    return events;
                };
                if !self.seen_blocks.insert(index) {
                    self.record_error(format!(
                        "claude-code-proxy reused content block index {index}"
                    ));
                    return events;
                }
                self.started_blocks.insert(index);
                let block = event["content_block"].clone();
                if block["type"] == "tool_use" {
                    events.push(StreamEvent::ToolCallStart {
                        id: block["id"].as_str().unwrap_or_default().to_string(),
                        index,
                        name: block["name"].as_str().unwrap_or_default().to_string(),
                    });
                }
                self.blocks.insert(index, block);
            }
            "content_block_delta" => {
                let Some(index) = event["index"]
                    .as_u64()
                    .and_then(|value| usize::try_from(value).ok())
                else {
                    self.record_error("claude-code-proxy content delta is missing an index");
                    return events;
                };
                if !self.started_blocks.contains(&index) {
                    self.record_error(format!(
                        "claude-code-proxy sent a delta before starting content block {index}"
                    ));
                    return events;
                }
                let delta = &event["delta"];
                match delta["type"].as_str().unwrap_or_default() {
                    "text_delta" => {
                        if let Some(text) = delta["text"].as_str() {
                            append_field(self.blocks.get_mut(&index), "text", text);
                            events.push(StreamEvent::TextDelta(text.to_string()));
                        }
                    }
                    "thinking_delta" => {
                        if let Some(thinking) = delta["thinking"].as_str() {
                            append_field(self.blocks.get_mut(&index), "thinking", thinking);
                            events.push(StreamEvent::ReasoningDelta {
                                text: thinking.to_string(),
                                segment_index: Some(index),
                            });
                        }
                    }
                    "signature_delta" => {
                        if let Some(signature) = delta["signature"].as_str() {
                            append_field(self.blocks.get_mut(&index), "signature", signature);
                        }
                    }
                    "input_json_delta" => {
                        if !self
                            .blocks
                            .get(&index)
                            .is_some_and(|block| block["type"] == "tool_use")
                        {
                            self.record_error(format!(
                                "claude-code-proxy sent tool input for non-tool content block {index}"
                            ));
                            return events;
                        }
                        if let Some(fragment) = delta["partial_json"].as_str() {
                            self.partial_inputs
                                .entry(index)
                                .or_default()
                                .push_str(fragment);
                            events.push(StreamEvent::ToolCallDelta {
                                index,
                                args_fragment: fragment.to_string(),
                            });
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                let Some(index) = event["index"]
                    .as_u64()
                    .and_then(|value| usize::try_from(value).ok())
                else {
                    self.record_error("claude-code-proxy content block stop is missing an index");
                    return events;
                };
                if !self.started_blocks.remove(&index) {
                    self.record_error(format!(
                        "claude-code-proxy stopped content block {index} before it was started"
                    ));
                    return events;
                }
                if self
                    .blocks
                    .get(&index)
                    .is_some_and(|block| block["type"] == "tool_use")
                {
                    let arguments = match self.partial_inputs.remove(&index) {
                        Some(input) if !input.is_empty() => {
                            match serde_json::from_str::<Value>(&input) {
                                Ok(arguments) if arguments.is_object() => Some(arguments),
                                Ok(_) => {
                                    self.record_error(
                                        "Invalid tool input from claude-code-proxy: expected a JSON object",
                                    );
                                    None
                                }
                                Err(error) => {
                                    self.record_error(format!(
                                        "Invalid tool input from claude-code-proxy: {error}"
                                    ));
                                    None
                                }
                            }
                        }
                        _ => self
                            .blocks
                            .get(&index)
                            .and_then(|block| block.get("input"))
                            .cloned(),
                    };
                    if let Some(arguments) = arguments {
                        if !arguments.is_object() {
                            self.record_error(
                                "Invalid tool input from claude-code-proxy: expected a JSON object",
                            );
                        } else if let Some(block) = self.blocks.get_mut(&index) {
                            block["input"] = arguments.clone();
                            let id = block["id"].as_str().unwrap_or_default().to_string();
                            let name = block["name"].as_str().unwrap_or_default().to_string();
                            if !id.is_empty() && !name.is_empty() {
                                self.tool_calls.insert(
                                    index,
                                    ToolCall {
                                        id,
                                        name,
                                        arguments,
                                    },
                                );
                            } else {
                                self.record_error(
                                    "claude-code-proxy returned an incomplete native tool identity",
                                );
                            }
                        }
                    }
                }
            }
            "message_delta" => {
                self.stop_reason = event
                    .pointer("/delta/stop_reason")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if let Some(usage) = event.get("usage") {
                    self.usage.update(usage);
                    if let Some(update) = self.usage.update_event() {
                        events.push(update);
                    }
                }
            }
            "message_stop" => events.extend(self.finish()),
            "error" => {
                if let Some(update) = self.usage.update_event() {
                    events.push(update);
                }
                events.push(StreamEvent::Error(
                    gemma_chat::provider_error::response_error(&event)
                        .unwrap_or_else(|| "Provider error: No error message supplied.".into()),
                ));
            }
            "ping" => {}
            _ => {}
        }
        events
    }

    pub(crate) fn finish(&mut self) -> Vec<StreamEvent> {
        if self.stopped {
            return Vec::new();
        }
        self.stopped = true;
        let mut events = Vec::new();
        if let Some(update) = self.usage.update_event() {
            events.push(update);
        }
        if !self.started_blocks.is_empty() {
            self.record_error("claude-code-proxy sent message_stop with unfinished content blocks");
        }
        if let Some(error) =
            gemma_chat::provider_error::finish_error(self.stop_reason.as_deref(), None)
        {
            self.pending_error = Some(error);
        }
        if let Some(error) = self.pending_error.take() {
            self.tool_calls.clear();
            self.partial_inputs.clear();
            events.push(StreamEvent::Error(error));
            return events;
        }
        let provider_content =
            (!self.blocks.is_empty()).then(|| self.blocks.values().cloned().collect::<Vec<_>>());
        let reason = self.stop_reason.as_deref().unwrap_or("missing");
        let error = match (reason, self.tool_calls.is_empty()) {
            ("tool_use", false) => None,
            ("tool_use", true) => Some(
                "claude-code-proxy ended with stop reason 'tool_use' but returned no complete native tool call"
                    .to_string(),
            ),
            ("end_turn" | "stop_sequence", true) => None,
            ("end_turn" | "stop_sequence", false) => Some(format!(
                "claude-code-proxy returned native tool calls with stop reason '{reason}'; refusing to execute a non-tool turn"
            )),
            _ => Some(format!(
                "claude-code-proxy ended with non-complete stop reason '{reason}'; partial or refused output was not accepted"
            )),
        };
        if let Some(error) = error {
            events.push(StreamEvent::Error(error));
            return events;
        }

        if !self.tool_calls.is_empty() {
            events.push(StreamEvent::ToolCalls {
                calls: self.tool_calls.values().cloned().collect(),
                provider_content: provider_content.clone(),
            });
        }
        let usage = self.usage.snapshot();
        let completion_tokens = self
            .usage
            .saw_output
            .then(|| usage.and_then(|usage| u32::try_from(usage.output_tokens).ok()))
            .flatten();
        let prompt_tokens = usage
            .and_then(|usage| usage.total_input_tokens)
            .and_then(|tokens| u32::try_from(tokens).ok());
        events.push(StreamEvent::Done {
            completion_tokens,
            prompt_tokens,
            usage,
            tg_per_s: None,
            pp_per_s: None,
            stop_reason: self.stop_reason.clone(),
            provider_content,
        });
        events
    }
}

fn append_field(block: Option<&mut Value>, field: &str, delta: &str) {
    let Some(block) = block.and_then(Value::as_object_mut) else {
        return;
    };
    match block.get_mut(field) {
        Some(Value::String(value)) => value.push_str(delta),
        _ => {
            block.insert(field.to_string(), Value::String(delta.to_string()));
        }
    }
}
