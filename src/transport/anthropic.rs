use crate::engine::anthropic::{AnthropicStreamParser, SseDecoder, anthropic_usage};
#[cfg(test)]
use crate::transport::ToolCall;
use futures_util::StreamExt;
use reqwest::{Client, RequestBuilder, StatusCode};
use serde_json::{Value, json};

use crate::config::Config;
use crate::transport::{
    Completion, CompletionError, EventStream, Message, ModelInfo, StreamEvent, ToolDefinition,
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

use crate::engine::messages::anthropic_messages as native_messages;

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

#[cfg(test)]
mod tests;
