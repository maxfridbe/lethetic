//! Convert the CLI parser's events to the host-neutral JSON binding protocol.
use super::merge;
use crate::engine::{anthropic::AnthropicStreamParser, transport_types::StreamEvent};
use serde_json::{Value, json};

#[derive(Default)]
pub(super) struct AnthropicStream {
    parser: AnthropicStreamParser,
    response: Option<Value>,
    usage: Value,
}
impl AnthropicStream {
    pub fn push(&mut self, data: &str, updates: &mut Vec<Value>) -> Result<bool, String> {
        let event: Value = serde_json::from_str(data).map_err(|e| e.to_string())?;
        if !self.usage.is_object() {
            self.usage = json!({});
        }
        if let Some(usage) = event
            .pointer("/message/usage")
            .or_else(|| event.get("usage"))
        {
            merge(&mut self.usage, usage);
        }
        for event in self.parser.process_data(data) {
            match event {
                StreamEvent::TextDelta(text) => {
                    updates.push(json!({"type":"assistant_delta","text":text}))
                }
                StreamEvent::ReasoningDelta { text, .. } => {
                    updates.push(json!({"type":"thinking_delta","text":text}))
                }
                StreamEvent::Done {
                    stop_reason,
                    provider_content,
                    ..
                } => {
                    self.response = Some(
                        json!({"role":"assistant","stop_reason":stop_reason,"content":provider_content.unwrap_or_default(),"usage":self.usage}),
                    );
                }
                StreamEvent::Error(error) => return Err(error),
                _ => {}
            }
        }
        Ok(self.response.is_some())
    }
    pub fn finish(&self) -> Result<String, String> {
        self.response
            .as_ref()
            .map(Value::to_string)
            .ok_or_else(|| "provider stream ended before message_stop".into())
    }
}
