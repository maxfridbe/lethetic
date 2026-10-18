use serde_json::{Value, json};

use crate::{Tool, ToolCall, Turn};

pub(crate) fn request(
    provider: &str,
    model: &str,
    system: &str,
    tools: &[Tool],
    history: &[Value],
) -> Result<Value, String> {
    match provider {
        "openai" => {
            let mut messages = vec![json!({"role":"system", "content":system})];
            messages.extend(history.iter().map(|message| {
                let mut message = message.clone();
                if let Some(object) = message.as_object_mut() {
                    object.remove("anthropic_content");
                }
                message
            }));
            let mut body = json!({"model":model, "messages":messages, "stream":false});
            if !tools.is_empty() {
                body["tools"] = Value::Array(
                    tools
                        .iter()
                        .map(|tool| {
                            json!({
                                "type":"function", "function":tool
                            })
                        })
                        .collect(),
                );
            }
            Ok(body)
        }
        "anthropic" => {
            let mut messages: Vec<Value> = Vec::new();
            for message in history {
                if message["role"] == "tool" {
                    let block = json!({"type":"tool_result", "tool_use_id":message["tool_call_id"],
                        "content":message["content"]});
                    if let Some(last) = messages.last_mut().filter(|m| m["role"] == "user")
                        && let Some(content) = last["content"].as_array_mut()
                    {
                        content.push(block);
                        continue;
                    }
                    messages.push(json!({"role":"user", "content":[block]}));
                } else if let Some(blocks) = message.get("anthropic_content") {
                    messages.push(json!({"role":"assistant", "content":blocks}));
                } else {
                    messages.push(json!({"role":message["role"], "content":message["content"]}));
                }
            }
            let mut body = json!({"model":model, "system":system, "messages":messages,
                "max_tokens":4096, "stream":false});
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
                    Some("thinking" | "redacted_thinking") => {}
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
    Ok((message, Turn { text, calls }))
}

fn required_string<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value[key]
        .as_str()
        .ok_or_else(|| format!("missing string field: {key}"))
}
