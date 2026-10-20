//! CLI wire serializers shared by all hosts.
use super::transport_types::{Content, ContentPart, Message, Role, ToolDefinition};
use serde_json::{Value, json};

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

pub(crate) fn anthropic_messages(messages: &[Message]) -> (Option<String>, Vec<Value>) {
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

pub(crate) fn to_gemma_message(message: &Message) -> gemma_chat::Message {
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

pub(crate) fn to_gemma_tools(tools: &[ToolDefinition]) -> Vec<gemma_chat::ToolDefinition> {
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
