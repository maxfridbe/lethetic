use futures_util::StreamExt;
use reqwest::{Client, RequestBuilder, StatusCode};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};

use crate::accounting::Usage;
use crate::config::Config;
use crate::transport::{
    Completion, CompletionError, Content, ContentPart, EventStream, Message, ModelInfo, Role,
    StreamEvent, ToolCall, ToolDefinition,
};

const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_PROXY_TOKEN: &str = "claudecodex-local";
#[cfg(not(test))]
const MESSAGE_STOP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
#[cfg(test)]
const MESSAGE_STOP_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(250);

fn root_url(server_url: &str) -> String {
    let without_query = server_url.split('?').next().unwrap_or(server_url);
    let trimmed = without_query.trim_end_matches('/');
    for suffix in &["/v1/messages", "/v1/models", "/v1"] {
        if let Some(root) = trimmed.strip_suffix(suffix) {
            return root.trim_end_matches('/').to_string();
        }
    }
    trimmed.to_string()
}

fn messages_url(server_url: &str) -> String {
    format!("{}/v1/messages", root_url(server_url))
}

fn models_url(server_url: &str) -> String {
    format!("{}/v1/models", root_url(server_url))
}

fn authorize(request: RequestBuilder, api_key: Option<&str>) -> RequestBuilder {
    request
        .bearer_auth(api_key.unwrap_or(DEFAULT_PROXY_TOKEN))
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
}

fn native_content(content: &Content) -> Vec<Value> {
    match content {
        Content::Text(text) => vec![json!({"type": "text", "text": text})],
        Content::Parts(parts) => parts
            .iter()
            .map(|part| match part {
                ContentPart::Text(text) => json!({"type": "text", "text": text}),
                ContentPart::Image { media_type, data } => json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": media_type,
                        "data": data
                    }
                }),
            })
            .collect(),
    }
}

fn portable_assistant_content_is_empty(content: &Content) -> bool {
    match content {
        Content::Text(text) => text.is_empty(),
        Content::Parts(parts) => parts.iter().all(|part| match part {
            ContentPart::Text(text) => text.is_empty(),
            ContentPart::Image { .. } => false,
        }),
    }
}

fn native_messages(messages: &[Message]) -> (Option<String>, Vec<Value>) {
    let mut system = Vec::new();
    let mut conversation = Vec::new();

    for message in messages {
        match message.role {
            Role::System => {
                let text = message.content.text();
                if !text.is_empty() {
                    system.push(text);
                }
            }
            Role::User => conversation.push(json!({
                "role": "user",
                "content": native_content(&message.content)
            })),
            Role::Assistant => {
                let content = match message.provider_content.as_ref() {
                    Some(provider_content) if !provider_content.is_empty() => {
                        provider_content.clone()
                    }
                    _ => {
                        let mut content = if portable_assistant_content_is_empty(&message.content) {
                            Vec::new()
                        } else {
                            native_content(&message.content)
                        };
                        content.extend(message.tool_calls.iter().map(|call| {
                            json!({
                                "type": "tool_use",
                                "id": call.id,
                                "name": call.name,
                                "input": call.arguments
                            })
                        }));
                        content
                    }
                };
                if !content.is_empty() {
                    conversation.push(json!({"role": "assistant", "content": content}));
                }
            }
            Role::Tool => {
                let mut result = json!({
                    "type": "tool_result",
                    "tool_use_id": message.tool_call_id.as_deref().unwrap_or("unknown"),
                    "content": message.content.text()
                });
                if message.tool_result_is_error {
                    result["is_error"] = Value::Bool(true);
                }
                // Results of one batch share a single user turn, as the
                // Messages API requires after a multi-tool assistant turn.
                let previous_is_results = conversation.last().is_some_and(|last: &Value| {
                    last["role"] == "user"
                        && last["content"].as_array().is_some_and(|blocks| {
                            !blocks.is_empty()
                                && blocks.iter().all(|block| block["type"] == "tool_result")
                        })
                });
                if previous_is_results {
                    if let Some(blocks) = conversation
                        .last_mut()
                        .and_then(|last| last["content"].as_array_mut())
                    {
                        blocks.push(result);
                    }
                } else {
                    conversation.push(json!({
                        "role": "user",
                        "content": [result]
                    }));
                }
            }
        }
    }

    (
        (!system.is_empty()).then(|| system.join("\n\n")),
        conversation,
    )
}

pub fn build_request(
    config: &Config,
    messages: &[Message],
    tools: &[ToolDefinition],
    max_tokens: u32,
    stream: bool,
) -> Result<Value, String> {
    let (system, messages) = native_messages(messages);
    let mut body = json!({
        "model": config.model,
        "max_tokens": max_tokens,
        "messages": messages,
        "stream": stream
    });

    if let Some(system) = system {
        body["system"] = Value::String(system);
    }
    if !tools.is_empty() {
        body["tools"] = Value::Array(
            tools
                .iter()
                .map(|tool| {
                    json!({
                        "name": tool.name,
                        "description": tool.description,
                        "input_schema": crate::transport::request_policy::native_tool_schema(&tool.input_schema)
                    })
                })
                .collect(),
        );
        body["tool_choice"] = json!({
            "type": "auto",
            "disable_parallel_tool_use": !crate::tool_call_mode::allows_batches(config)
        });
    }
    match config.thinking {
        Some(true) => {
            body["thinking"] = json!({
                "type": "adaptive",
                "display": "summarized"
            });
        }
        Some(false) => {
            body["thinking"] = json!({"type": "disabled"});
        }
        None => {}
    }
    gemma_chat::validate_extra_body(config.extra_body.as_ref())
        .map_err(|error| format!("Invalid claude-code-proxy request config: {error}"))?;
    if let Some(extra) = config.extra_body.as_ref().and_then(Value::as_object)
        && let Some(object) = body.as_object_mut()
    {
        for (key, value) in extra {
            object.insert(key.clone(), value.clone());
        }
    }

    Ok(body)
}

fn connection_error(server_url: &str, error: reqwest::Error) -> String {
    if error.is_connect() {
        format!(
            "claude-code-proxy is unavailable at {}. Start and authorize it separately: {}",
            root_url(server_url),
            error
        )
    } else {
        format!("claude-code-proxy request failed: {error}")
    }
}

fn api_error(status: StatusCode, body: &str) -> String {
    if let Ok(value) = serde_json::from_str::<Value>(body) {
        let message = value
            .pointer("/error/message")
            .and_then(Value::as_str)
            .or_else(|| value.get("message").and_then(Value::as_str));
        let error_type = value
            .pointer("/error/type")
            .and_then(Value::as_str)
            .or_else(|| value.get("type").and_then(Value::as_str));
        if let Some(message) = message {
            return match error_type {
                Some(error_type) => {
                    format!("claude-code-proxy {status} ({error_type}): {message}")
                }
                None => format!("claude-code-proxy {status}: {message}"),
            };
        }
    }
    format!("claude-code-proxy {status}: {body}")
}

pub async fn stream(
    client: &Client,
    config: &Config,
    messages: &[Message],
    tools: &[ToolDefinition],
    max_tokens: u32,
) -> Result<EventStream, String> {
    let body = build_request(config, messages, tools, max_tokens, true)?;
    crate::transport::request_policy::validate_final_agent_body(config, &body)?;
    let encoded = serde_json::to_vec(&body)
        .map_err(|error| format!("Could not encode claude-code-proxy request: {error}"))?;
    stream_encoded(client, config, encoded).await
}

pub(crate) async fn stream_encoded(
    client: &Client,
    config: &Config,
    body: Vec<u8>,
) -> Result<EventStream, String> {
    let url = messages_url(&config.server_url);
    let response = authorize(client.post(&url), config.api_key.as_deref())
        .body(body)
        .send()
        .await
        .map_err(|error| connection_error(&config.server_url, error))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(api_error(status, &text));
    }

    let stream = async_stream::stream! {
        let mut bytes = response.bytes_stream();
        let mut decoder = SseDecoder::default();
        let mut parser = AnthropicStreamParser::default();
        let mut terminal_deadline = None;

        loop {
            let next = match terminal_deadline {
                Some(deadline) => match tokio::time::timeout_at(deadline, bytes.next()).await {
                    Ok(next) => next,
                    Err(_) => {
                        if let Some(update) = parser.usage.update_event() {
                            yield update;
                        }
                        let error = parser.pending_error.take().unwrap_or_else(|| {
                            "claude-code-proxy did not send message_stop after its terminal state"
                                .to_string()
                        });
                        yield StreamEvent::Error(error);
                        return;
                    }
                },
                None => bytes.next().await,
            };
            let Some(chunk) = next else { break };
            match chunk {
                Err(error) => {
                    for data in decoder.finish().into_iter().flatten() {
                        for event in parser.process_data(&data) {
                            if matches!(event, StreamEvent::UsageUpdate(_)) {
                                yield event;
                            }
                        }
                    }
                    if let Some(update) = parser.usage.update_event() {
                        yield update;
                    }
                    yield StreamEvent::Error(format!("Stream error: {error}"));
                    return;
                }
                Ok(chunk) => {
                    for data in decoder.push(&chunk) {
                        match data {
                            Ok(data) => {
                                for event in parser.process_data(&data) {
                                    let terminal = matches!(
                                        event,
                                        StreamEvent::Done { .. } | StreamEvent::Error(_)
                                    );
                                    yield event;
                                    if terminal {
                                        return;
                                    }
                                }
                            }
                            Err(error) => {
                                yield StreamEvent::Error(error);
                                return;
                            }
                        }
                    }
                    if (parser.stop_reason.is_some() || parser.pending_error.is_some())
                        && terminal_deadline.is_none()
                    {
                        terminal_deadline = Some(
                            tokio::time::Instant::now() + MESSAGE_STOP_TIMEOUT,
                        );
                    }
                }
            }
        }

        for data in decoder.finish() {
            match data {
                Ok(data) => {
                    for event in parser.process_data(&data) {
                        let terminal = matches!(
                            event,
                            StreamEvent::Done { .. } | StreamEvent::Error(_)
                        );
                        yield event;
                        if terminal {
                            return;
                        }
                    }
                }
                Err(error) => {
                    yield StreamEvent::Error(error);
                    return;
                }
            }
        }
        if !parser.stopped {
            if let Some(update) = parser.usage.update_event() {
                yield update;
            }
            yield StreamEvent::Error(
                "claude-code-proxy stream ended before message_stop".to_string(),
            );
        }
    };

    Ok(Box::pin(stream))
}

fn validate_text_completion_stop(response: &Value) -> Result<(), String> {
    let reason = response["stop_reason"].as_str().unwrap_or("missing");
    match reason {
        "end_turn" | "stop_sequence" => Ok(()),
        _ => Err(format!(
            "claude-code-proxy completion ended with non-complete stop reason '{reason}'; partial, refused, or tool output was not accepted"
        )),
    }
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
    let url = messages_url(&config.server_url);
    let body = build_request(config, messages, &[], max_tokens, false)
        .map_err(CompletionError::without_usage)?;
    let response = authorize(client.post(&url), config.api_key.as_deref())
        .json(&body)
        .send()
        .await
        .map_err(|error| {
            CompletionError::without_usage(connection_error(&config.server_url, error))
        })?;
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(CompletionError::without_usage(api_error(status, &text)));
    }

    let response: Value = response.json().await.map_err(|error| {
        CompletionError::without_usage(format!("Invalid claude-code-proxy response: {error}"))
    })?;
    let usage = anthropic_usage(&response["usage"]);
    if let Err(message) = validate_text_completion_stop(&response) {
        return Err(CompletionError { message, usage });
    }
    let text = response["content"]
        .as_array()
        .map(|content| {
            content
                .iter()
                .filter(|block| block["type"] == "text")
                .filter_map(|block| block["text"].as_str())
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    Ok(Completion { text, usage })
}

pub async fn discover_models(
    client: &Client,
    server_url: &str,
    api_key: Option<&str>,
) -> Result<Vec<ModelInfo>, String> {
    let response = authorize(client.get(models_url(server_url)), api_key)
        .send()
        .await
        .map_err(|error| connection_error(server_url, error))?;
    if matches!(
        response.status(),
        StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
    ) {
        return Ok(Vec::new());
    }
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(api_error(status, &text));
    }

    let response: Value = response
        .json()
        .await
        .map_err(|error| format!("Invalid claude-code-proxy model response: {error}"))?;
    Ok(response["data"]
        .as_array()
        .map(|models| {
            models
                .iter()
                .filter_map(|model| {
                    let id = model["id"].as_str()?;
                    Some(ModelInfo {
                        id: id.to_string(),
                        display_name: model["display_name"].as_str().unwrap_or(id).to_string(),
                        pricing: None,
                    })
                })
                .collect()
        })
        .unwrap_or_default())
}

#[derive(Default)]
struct SseDecoder {
    buffer: Vec<u8>,
}

impl SseDecoder {
    fn push(&mut self, bytes: &[u8]) -> Vec<Result<String, String>> {
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

    fn finish(&mut self) -> Vec<Result<String, String>> {
        if self.buffer.is_empty() {
            return Vec::new();
        }
        let line = std::mem::take(&mut self.buffer);
        decode_sse_line(&line).into_iter().collect()
    }
}

fn decode_sse_line(line: &[u8]) -> Option<Result<String, String>> {
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
struct UsageTracker {
    usage: Usage,
    saw_uncached_input: bool,
    saw_cache_creation: bool,
    saw_cache_read: bool,
    saw_output: bool,
    last_emitted: Option<Usage>,
}

impl UsageTracker {
    fn update(&mut self, value: &Value) {
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

    fn snapshot(&self) -> Option<Usage> {
        (self.saw_uncached_input
            || self.saw_cache_creation
            || self.saw_cache_read
            || self.saw_output)
            .then_some(self.usage)
    }

    fn update_event(&mut self) -> Option<StreamEvent> {
        let snapshot = self.snapshot()?;
        if self.last_emitted == Some(snapshot) {
            return None;
        }
        self.last_emitted = Some(snapshot);
        Some(StreamEvent::UsageUpdate(snapshot))
    }
}

fn anthropic_usage(value: &Value) -> Option<Usage> {
    let mut tracker = UsageTracker::default();
    tracker.update(value);
    tracker.snapshot()
}

#[derive(Default)]
struct AnthropicStreamParser {
    blocks: BTreeMap<usize, Value>,
    started_blocks: HashSet<usize>,
    seen_blocks: HashSet<usize>,
    partial_inputs: HashMap<usize, String>,
    tool_calls: BTreeMap<usize, ToolCall>,
    usage: UsageTracker,
    stop_reason: Option<String>,
    pending_error: Option<String>,
    stopped: bool,
}

impl AnthropicStreamParser {
    fn record_error(&mut self, error: impl Into<String>) {
        if self.pending_error.is_none() {
            self.pending_error = Some(error.into());
        }
        self.tool_calls.clear();
    }

    fn process_data(&mut self, data: &str) -> Vec<StreamEvent> {
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
                let message = event
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("Unknown stream error");
                events.push(StreamEvent::Error(format!(
                    "claude-code-proxy stream error: {message}"
                )));
            }
            "ping" => {}
            _ => {}
        }
        events
    }

    fn finish(&mut self) -> Vec<StreamEvent> {
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

#[cfg(test)]
mod tests;
