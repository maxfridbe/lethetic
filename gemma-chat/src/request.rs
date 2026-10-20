use crate::types::{Message, ToolDefinition};
use serde_json::{Value, json};

pub const RESERVED_EXTRA_BODY_FIELDS: &[&str] = &[
    "model",
    "messages",
    "system",
    "max_tokens",
    "max_completion_tokens",
    "stream",
    "stream_options",
    "tools",
    "tool_choice",
    "parallel_tool_calls",
    "functions",
    "function_call",
    "mcp_servers",
    "n",
    "thinking",
];

pub fn validate_extra_body(extra_body: Option<&Value>) -> Result<(), String> {
    let Some(extra) = extra_body else {
        return Ok(());
    };
    let object = extra
        .as_object()
        .ok_or_else(|| "extra_body must be a JSON object".to_string())?;
    if let Some(key) = object
        .keys()
        .find(|key| RESERVED_EXTRA_BODY_FIELDS.contains(&key.as_str()))
    {
        return Err(format!(
            "extra_body cannot override host-owned request field `{key}`"
        ));
    }
    Ok(())
}

/// Build the request body for `/v1/chat/completions`.
pub fn build_request(
    model: &str,
    messages: &[Message],
    tools: &[ToolDefinition],
    max_tokens: u32,
    thinking: Option<bool>,
    extra_body: Option<&serde_json::Value>,
) -> Result<Value, String> {
    validate_extra_body(extra_body)?;
    let mut body = json!({
        "model": model,
        "messages": messages,
        "max_tokens": max_tokens,
        "stream": true,
        "stream_options": {
            "include_usage": true
        },
        // llama.cpp-compatible servers only emit per-chunk `timings`
        // (prompt/predicted tokens-per-second) when this is set.
        "timings_per_token": true
    });
    if !tools.is_empty() {
        body["tools"] = json!(tools);
        body["parallel_tool_calls"] = json!(false);
    }
    if let Some(true) = thinking {
        body["thinking"] = json!({
            "type": "enabled"
        });
    }
    if let Some(extra_map) = extra_body.and_then(Value::as_object)
        && let Some(body_map) = body.as_object_mut()
    {
        for (key, value) in extra_map {
            body_map.insert(key.clone(), value.clone());
        }
    }
    Ok(body)
}
