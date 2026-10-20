//! Compatibility adapter for saved browser histories. Context policy belongs to
//! the CLI ContextManager; this module only translates the persistence format.
use super::transport_types;
use crate::context::{ContextManager, FunctionCall, Message, ToolCall};
use serde_json::Value;

pub(super) fn project(
    system: &str,
    history: &[Value],
    budget: usize,
    provider: &str,
) -> Result<Vec<transport_types::Message>, String> {
    let mut context = ContextManager::new(budget, Some(system.into()));
    for message in history {
        let role = message["role"].as_str().ok_or("missing message role")?;
        let content = message["content"].as_str().unwrap_or_default();
        if role == "tool" {
            context.add_tool_message_with_status(
                message["tool_call_id"]
                    .as_str()
                    .ok_or("missing tool id")?
                    .into(),
                "tool",
                content,
                crate::engine::tool_errors::legacy_output_is_error(content),
            );
            continue;
        }
        let calls = message["tool_calls"]
            .as_array()
            .map(|calls| {
                calls
                    .iter()
                    .map(|call| {
                        Ok(ToolCall {
                            provider_id: None,
                            id: call["id"].as_str().ok_or("missing tool id")?.into(),
                            function: FunctionCall {
                                name: call["function"]["name"]
                                    .as_str()
                                    .ok_or("missing tool name")?
                                    .into(),
                                arguments: serde_json::from_str(
                                    call["function"]["arguments"]
                                        .as_str()
                                        .ok_or("missing tool arguments")?,
                                )
                                .map_err(|error| error.to_string())?,
                            },
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()
            })
            .transpose()?;
        // Retain opaque provider replay data through the CLI's trimming. It is
        // restored only for its provider, never displayed or treated as instructions.
        let provider_content = if provider == "anthropic" {
            message["anthropic_content"].as_array().cloned()
        } else if role == "assistant" {
            let mut metadata = serde_json::Map::new();
            for key in [
                "reasoning",
                "reasoning_content",
                "reasoning_text",
                "reasoning_details",
            ] {
                if let Some(value) = message.get(key) {
                    metadata.insert(key.into(), value.clone());
                }
            }
            (!metadata.is_empty()).then(|| vec![Value::Object(metadata)])
        } else {
            None
        };
        context.add_message_raw(Message {
            role: role.into(),
            content: content.into(),
            tool_calls: calls,
            provider_content,
            tool_result_is_error: false,
        });
    }
    Ok(context
        .prepare_api_context_with_files(&|_, _| crate::context::FileRead::Unavailable)
        .into_messages())
}
