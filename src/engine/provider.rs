use serde_json::{Value, json};

use super::{Tool, ToolCall, Turn};

pub(crate) fn request(
    provider: &str,
    model: &str,
    tools: &[Tool],
    max_tokens: u32,
    history: &[super::transport_types::Message],
) -> Result<Value, String> {
    let definitions = tools
        .iter()
        .map(|tool| super::transport_types::ToolDefinition {
            name: tool.name.clone(),
            description: tool.description.clone(),
            input_schema: tool.parameters.clone(),
        })
        .collect::<Vec<_>>();
    match provider {
        "openai" => {
            let messages = history
                .iter()
                .map(super::messages::to_gemma_message)
                .collect::<Vec<_>>();
            let mut body = gemma_chat::build_request(
                model,
                &messages,
                &super::messages::to_gemma_tools(&definitions),
                max_tokens,
                None,
                None,
            )?;
            // OpenRouter requires signed/opaque reasoning to be replayed exactly.
            for (source, target) in history
                .iter()
                .zip(body["messages"].as_array_mut().ok_or("invalid messages")?)
            {
                if let Some(metadata) = source
                    .provider_content
                    .as_ref()
                    .and_then(|blocks| blocks.first())
                    .and_then(Value::as_object)
                {
                    for key in [
                        "reasoning",
                        "reasoning_content",
                        "reasoning_text",
                        "reasoning_details",
                    ] {
                        if let Some(value) = metadata.get(key) {
                            target[key] = value.clone();
                        }
                    }
                }
            }
            Ok(body)
        }
        "anthropic" => {
            let (system, messages) = super::messages::anthropic_messages(history);
            let mut body =
                json!({"model":model, "messages":messages, "max_tokens":max_tokens, "stream":true});
            if let Some(system) = system {
                body["system"] = system.into();
            }
            if !tools.is_empty() {
                body["tools"] = Value::Array(tools.iter().map(|tool| json!({
                    "name":tool.name, "description":tool.description, "input_schema":tool.parameters
                })).collect());
            }
            Ok(body)
        }
        _ => Err("unsupported provider; use openai or anthropic".into()),
    }
}

pub(crate) fn response(provider: &str, value: &Value) -> Result<(Value, Turn), String> {
    if value.get("error").is_some() {
        return Err("provider returned an error response".into());
    }
    let mut thinking = String::new();
    let (text, calls, blocks) = match provider {
        "openai" => {
            let choice = &value["choices"][0];
            if !matches!(
                choice["finish_reason"].as_str(),
                Some("stop" | "tool_calls")
            ) {
                return Err("model response was incomplete or filtered".into());
            }
            let message = &choice["message"];
            if message["role"] != "assistant" {
                return Err("expected an assistant message".into());
            }
            // Providers may return the same reasoning in both forms. Prefer the
            // plain-text field for display; preserve structured details verbatim.
            thinking = message["reasoning"]
                .as_str()
                .filter(|text| !text.is_empty())
                .or_else(|| message["reasoning_content"].as_str())
                .or_else(|| message["reasoning_text"].as_str())
                .unwrap_or_default()
                .to_owned();
            if let Some(details) = message.get("reasoning_details").filter(|v| !v.is_null()) {
                let details = details.as_array().ok_or("invalid reasoning_details")?;
                if thinking.is_empty() {
                    for detail in details {
                        let text = match detail["type"].as_str() {
                            Some("reasoning.text") => detail["text"].as_str(),
                            Some("reasoning.summary") => detail["summary"].as_str(),
                            _ => None, // Encrypted reasoning must never be rendered.
                        };
                        if let Some(text) = text {
                            if !thinking.is_empty() {
                                thinking.push('\n');
                            }
                            thinking.push_str(text);
                        }
                    }
                }
            }
            let text = match &message["content"] {
                Value::String(text) => text.clone(),
                Value::Null => String::new(),
                _ => return Err("unsupported assistant content".into()),
            };
            let mut calls = Vec::new();
            if let Some(tool_calls) = message.get("tool_calls").filter(|v| !v.is_null()) {
                for call in tool_calls.as_array().ok_or("invalid tool_calls")? {
                    if call["type"] != "function" {
                        return Err("unsupported tool type".into());
                    }
                    calls.push(ToolCall {
                        id: required_string(call, "id")?.into(),
                        name: required_string(&call["function"], "name")?.into(),
                        arguments: serde_json::from_str(required_string(
                            &call["function"],
                            "arguments",
                        )?)
                        .map_err(|_| "tool arguments must be valid JSON")?,
                    });
                }
            }
            if (choice["finish_reason"] == "tool_calls") != !calls.is_empty() {
                return Err("tool calls do not match the model finish reason".into());
            }
            (text, calls, None)
        }
        "anthropic" => {
            if value["role"] != "assistant"
                || !matches!(
                    value["stop_reason"].as_str(),
                    Some("end_turn" | "tool_use" | "stop_sequence")
                )
            {
                return Err("model response was incomplete or filtered".into());
            }
            let blocks = value["content"]
                .as_array()
                .ok_or("expected content blocks")?;
            let mut text = String::new();
            let mut calls = Vec::new();
            for block in blocks {
                match block["type"].as_str() {
                    Some("text") => text.push_str(required_string(block, "text")?),
                    Some("tool_use") => calls.push(ToolCall {
                        id: required_string(block, "id")?.into(),
                        name: required_string(block, "name")?.into(),
                        arguments: block["input"].clone(),
                    }),
                    Some("thinking") => thinking.push_str(required_string(block, "thinking")?),
                    Some("redacted_thinking") => {}
                    _ => return Err("unsupported model content block".into()),
                }
            }
            if (value["stop_reason"] == "tool_use") != !calls.is_empty() {
                return Err("tool calls do not match the model stop reason".into());
            }
            (text, calls, Some(Value::Array(blocks.clone())))
        }
        _ => return Err("unsupported provider".into()),
    };
    if text.is_empty() && calls.is_empty() {
        return Err("model returned an empty response".into());
    }
    let mut message = json!({"role":"assistant", "content":text});
    if provider == "openai" {
        let source = &value["choices"][0]["message"];
        for key in [
            "reasoning",
            "reasoning_content",
            "reasoning_text",
            "reasoning_details",
        ] {
            if let Some(value) = source.get(key).filter(|v| !v.is_null()) {
                if key != "reasoning_details" && !value.is_string() {
                    return Err(format!("invalid {key}"));
                }
                message[key] = value.clone();
            }
        }
    }
    if !calls.is_empty() {
        message["tool_calls"] = Value::Array(
            calls
                .iter()
                .map(|call| {
                    json!({
                        "id":call.id, "type":"function", "function":{
                            "name":call.name, "arguments":call.arguments.to_string()
                        }
                    })
                })
                .collect(),
        );
    }
    if let Some(blocks) = blocks {
        message["anthropic_content"] = blocks;
    }
    Ok((
        message,
        Turn {
            text,
            thinking,
            calls,
        },
    ))
}

fn required_string<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value[key]
        .as_str()
        .ok_or_else(|| format!("missing string field: {key}"))
}
