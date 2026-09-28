use futures_util::StreamExt;
use reqwest::Client;
use serde_json::{Value, json};

use crate::accounting::Usage;
use crate::config::Config;
use crate::transport::{
    Completion, Content, ContentPart, EventStream, Message, ModelInfo, Role, StreamEvent, ToolCall,
    ToolDefinition,
};

pub fn base_url(server_url: &str) -> String {
    for suffix in &["/v1/responses", "/v1/chat/completions", "/completion"] {
        if let Some(base) = server_url.strip_suffix(suffix) {
            return format!("{base}/v1");
        }
    }
    server_url.to_string()
}

fn to_gemma_message(message: &Message) -> gemma_chat::Message {
    let role = match message.role {
        Role::System => gemma_chat::Role::System,
        Role::User => gemma_chat::Role::User,
        Role::Assistant => gemma_chat::Role::Assistant,
        Role::Tool => gemma_chat::Role::Tool,
    };
    let content = match &message.content {
        Content::Text(text) => Value::String(text.clone()),
        Content::Parts(parts) => Value::Array(
            parts
                .iter()
                .map(|part| match part {
                    ContentPart::Text(text) => json!({"type": "text", "text": text}),
                    ContentPart::Image { media_type, data } => json!({
                        "type": "image_url",
                        "image_url": {
                            "url": format!("data:{media_type};base64,{data}")
                        }
                    }),
                })
                .collect(),
        ),
    };
    let tool_calls = (!message.tool_calls.is_empty()).then(|| {
        message
            .tool_calls
            .iter()
            .map(|call| gemma_chat::AssistantToolCall {
                id: call.id.clone(),
                kind: "function".to_string(),
                function: gemma_chat::FunctionCall {
                    name: call.name.clone(),
                    arguments: serde_json::to_string(&call.arguments).unwrap_or_default(),
                },
            })
            .collect()
    });

    gemma_chat::Message {
        role,
        content,
        tool_call_id: message.tool_call_id.clone(),
        tool_calls,
        reasoning_content: None,
    }
}

fn to_gemma_tools(tools: &[ToolDefinition]) -> Vec<gemma_chat::ToolDefinition> {
    tools
        .iter()
        .map(|tool| {
            gemma_chat::ToolDefinition::new(
                tool.name.clone(),
                tool.description.clone(),
                tool.input_schema.clone(),
            )
        })
        .collect()
}

pub fn build_request(
    config: &Config,
    messages: &[Message],
    tools: &[ToolDefinition],
    max_tokens: u32,
    stream: bool,
) -> Result<Value, String> {
    let messages = messages.iter().map(to_gemma_message).collect::<Vec<_>>();
    let tools = to_gemma_tools(tools);
    let mut body = gemma_chat::build_request(
        &config.model,
        &messages,
        &tools,
        max_tokens,
        config.thinking,
        config.extra_body.as_ref(),
    )
    .map_err(|error| format!("Invalid OpenAI-compatible request config: {error}"))?;
    body["stream"] = Value::Bool(stream);
    if !tools.is_empty() && crate::tool_call_mode::allows_batches(config) {
        body["parallel_tool_calls"] = Value::Bool(true);
    }
    if !stream && let Some(object) = body.as_object_mut() {
        object.remove("stream_options");
    }
    crate::transport::request_policy::validate_final_agent_body(config, &body)?;
    Ok(body)
}

fn convert_usage(raw: &gemma_chat::Usage) -> Usage {
    let total_input = raw.prompt_tokens;
    let reported_cache_read = raw.cache_read_input_tokens.or_else(|| {
        raw.prompt_tokens_details
            .as_ref()
            .and_then(|details| details.cached_tokens)
    });

    let mut cache_read = reported_cache_read.unwrap_or(0);
    let mut cache_creation = raw.cache_creation_input_tokens.unwrap_or(0);
    let mut uncached = raw.uncached_input_tokens.unwrap_or_else(|| {
        total_input
            .and_then(|total| total.checked_sub(cache_read)?.checked_sub(cache_creation))
            .unwrap_or_else(|| total_input.unwrap_or(0))
    });

    let reported_component_total = uncached
        .checked_add(cache_read)
        .and_then(|value| value.checked_add(cache_creation));
    let reported_components_consistent = match (total_input, reported_component_total) {
        (Some(total), Some(components)) => total == components,
        (None, Some(_)) => true,
        (_, None) => false,
    };
    if let Some(total) = total_input
        && reported_component_total != Some(total)
    {
        // Keep the provider total usable without double-counting malformed or
        // ambiguous component extensions.
        cache_read = 0;
        cache_creation = 0;
        uncached = total;
    }

    let component_total = uncached
        .checked_add(cache_read)
        .and_then(|value| value.checked_add(cache_creation));
    let all_components_reported = raw.uncached_input_tokens.is_some()
        && reported_cache_read.is_some()
        && raw.cache_creation_input_tokens.is_some();

    Usage {
        uncached_input_tokens: uncached,
        cache_read_input_tokens: cache_read,
        cache_creation_input_tokens: cache_creation,
        output_tokens: raw.completion_tokens.unwrap_or(0),
        total_input_tokens: total_input.or(component_total),
        breakdown_complete: all_components_reported
            && raw.completion_tokens.is_some()
            && reported_components_consistent,
        reported_cost_nanos: raw.cost_nanos,
    }
}

pub async fn stream(
    client: &Client,
    config: &Config,
    messages: &[Message],
    tools: &[ToolDefinition],
    max_tokens: u32,
) -> Result<EventStream, String> {
    let body = build_request(config, messages, tools, max_tokens, true)?;
    let encoded = serde_json::to_vec(&body)
        .map_err(|error| format!("Could not encode OpenAI-compatible request: {error}"))?;
    stream_encoded(client, config, encoded).await
}

pub(crate) async fn stream_encoded(
    client: &Client,
    config: &Config,
    body: Vec<u8>,
) -> Result<EventStream, String> {
    let stream = gemma_chat::stream_chat_with_encoded_body(
        client,
        &base_url(&config.server_url),
        config.api_key.as_deref(),
        body,
    )
    .await?
    .map(|event| match event {
        gemma_chat::StreamEvent::ReasoningDelta(text) => StreamEvent::ReasoningDelta {
            text,
            segment_index: None,
        },
        gemma_chat::StreamEvent::TextDelta(text) => StreamEvent::TextDelta(text),
        gemma_chat::StreamEvent::ToolCallStart { id, index, name } => {
            StreamEvent::ToolCallStart { id, index, name }
        }
        gemma_chat::StreamEvent::ToolCallDelta {
            index,
            args_fragment,
        } => StreamEvent::ToolCallDelta {
            index,
            args_fragment,
        },
        gemma_chat::StreamEvent::ToolCallComplete {
            id,
            name,
            arguments,
            ..
        } => StreamEvent::ToolCalls {
            calls: vec![ToolCall {
                id,
                name,
                arguments,
            }],
            provider_content: None,
        },
        gemma_chat::StreamEvent::UsageUpdate(usage) => {
            StreamEvent::UsageUpdate(convert_usage(&usage))
        }
        gemma_chat::StreamEvent::Done {
            completion_tokens,
            prompt_tokens,
            usage,
            tg_per_s,
            pp_per_s,
            stop_reason,
        } => StreamEvent::Done {
            completion_tokens,
            prompt_tokens,
            usage: usage.as_ref().map(convert_usage),
            tg_per_s,
            pp_per_s,
            stop_reason,
            provider_content: None,
        },
        gemma_chat::StreamEvent::Error(error) => StreamEvent::Error(error),
    });

    Ok(Box::pin(stream))
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
    gemma_chat::validate_extra_body(config.extra_body.as_ref())
        .map_err(|error| format!("Invalid OpenAI-compatible request config: {error}"))?;
    let messages = messages.iter().map(to_gemma_message).collect::<Vec<_>>();
    let completion = gemma_chat::complete_with_usage(
        client,
        &base_url(&config.server_url),
        &config.model,
        &messages,
        max_tokens,
        config.api_key.as_deref(),
        config.thinking,
        config.extra_body.as_ref(),
    )
    .await?;
    Ok(Completion {
        text: completion.text,
        usage: completion.usage.as_ref().map(convert_usage),
    })
}

pub async fn discover_models(
    client: &Client,
    server_url: &str,
    api_key: Option<&str>,
) -> Result<Vec<ModelInfo>, String> {
    let base = base_url(server_url);
    let models_url = if base.ends_with("/v1") {
        format!("{base}/models")
    } else if let Some((prefix, _)) = base.split_once("/chat/completions") {
        let prefix = prefix.trim_end_matches('/');
        if prefix.ends_with("/v1") {
            format!("{prefix}/models")
        } else {
            format!("{prefix}/v1/models")
        }
    } else {
        format!("{}/v1/models", base.trim_end_matches('/'))
    };

    let mut request = client.get(&models_url);
    if let Some(key) = api_key {
        request = request.bearer_auth(key);
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("Request failed: {error}"))?;
    if matches!(
        response.status(),
        reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::METHOD_NOT_ALLOWED
    ) {
        return Ok(Vec::new());
    }
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(format!("Server {status}: {text}"));
    }

    let body: Value = response
        .json()
        .await
        .map_err(|error| format!("Invalid model response: {error}"))?;
    Ok(body["data"]
        .as_array()
        .map(|models| {
            models
                .iter()
                .filter_map(|model| {
                    let id = model["id"].as_str()?;
                    let price = |field: &str| {
                        model["pricing"][field]
                            .as_str()
                            .filter(|value| !value.is_empty())
                            .map(str::to_string)
                    };
                    let pricing =
                        price("prompt")
                            .zip(price("completion"))
                            .map(|(prompt, completion)| crate::transport::CatalogPricing {
                                prompt,
                                completion,
                                cache_read: price("input_cache_read"),
                                cache_write: price("input_cache_write"),
                            });
                    Some(ModelInfo {
                        id: id.to_string(),
                        display_name: model["display_name"]
                            .as_str()
                            .or_else(|| model["name"].as_str())
                            .unwrap_or(id)
                            .to_string(),
                        pricing,
                    })
                })
                .collect()
        })
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConnectionKind;

    fn config() -> Config {
        Config {
            server_url: "http://localhost:8000/v1".to_string(),
            model: "model".to_string(),
            context_size: 1000,
            connection_kind: ConnectionKind::OpenAiChatCompletions,
            ..Default::default()
        }
    }

    #[test]
    fn preserves_cache_details_but_marks_standard_usage_incomplete() {
        let raw: gemma_chat::Usage = serde_json::from_value(json!({
            "prompt_tokens": 100,
            "completion_tokens": 7,
            "prompt_tokens_details": {"cached_tokens": 80}
        }))
        .unwrap();
        let usage = convert_usage(&raw);
        assert_eq!(usage.uncached_input_tokens, 20);
        assert_eq!(usage.cache_read_input_tokens, 80);
        assert_eq!(usage.cache_creation_input_tokens, 0);
        assert_eq!(usage.output_tokens, 7);
        assert_eq!(usage.total_input_tokens, Some(100));
        assert!(!usage.breakdown_complete);
    }

    #[test]
    fn complete_vendor_breakdown_is_marked_complete() {
        let raw: gemma_chat::Usage = serde_json::from_value(json!({
            "prompt_tokens": 100,
            "completion_tokens": 7,
            "uncached_input_tokens": 10,
            "cache_read_input_tokens": 85,
            "cache_creation_input_tokens": 5
        }))
        .unwrap();
        let usage = convert_usage(&raw);
        assert_eq!(usage.uncached_input_tokens, 10);
        assert_eq!(usage.cache_read_input_tokens, 85);
        assert_eq!(usage.cache_creation_input_tokens, 5);
        assert!(usage.breakdown_complete);
    }

    #[test]
    fn malformed_cache_breakdown_falls_back_without_double_counting() {
        let raw: gemma_chat::Usage = serde_json::from_value(json!({
            "prompt_tokens": 10,
            "completion_tokens": 1,
            "prompt_tokens_details": {"cached_tokens": 20}
        }))
        .unwrap();
        let usage = convert_usage(&raw);
        assert_eq!(usage.uncached_input_tokens, 10);
        assert_eq!(usage.cache_read_input_tokens, 0);
        assert_eq!(usage.total_input_tokens, Some(10));
        assert!(!usage.breakdown_complete);
    }

    #[test]
    fn fully_reported_inconsistent_breakdown_is_not_marked_complete() {
        let raw: gemma_chat::Usage = serde_json::from_value(json!({
            "prompt_tokens": 100,
            "completion_tokens": 7,
            "uncached_input_tokens": 10,
            "cache_read_input_tokens": 85,
            "cache_creation_input_tokens": 10
        }))
        .unwrap();
        let usage = convert_usage(&raw);
        assert_eq!(usage.uncached_input_tokens, 100);
        assert_eq!(usage.cache_read_input_tokens, 0);
        assert_eq!(usage.cache_creation_input_tokens, 0);
        assert_eq!(usage.total_input_tokens, Some(100));
        assert!(!usage.breakdown_complete);
    }

    #[test]
    fn preserves_openai_request_shape() {
        let body = build_request(&config(), &[Message::user("hello")], &[], 100, true).unwrap();
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"], "hello");
        assert_eq!(body["stream_options"]["include_usage"], true);
    }

    #[test]
    fn converts_pngs_to_data_urls() {
        let body = build_request(
            &config(),
            &[Message::user_with_pngs("inspect", &["abc".to_string()])],
            &[],
            100,
            false,
        )
        .unwrap();
        assert_eq!(
            body["messages"][0]["content"][0]["image_url"]["url"],
            "data:image/png;base64,abc"
        );
        assert!(body.get("stream_options").is_none());
    }
}
