use std::collections::HashMap;
use crate::types::{Chunk, StreamEvent, AssistantToolCall, FunctionCall};
use crate::sse::parse_sse_line;

/// Stateful parser that converts raw SSE lines into `StreamEvent`s.
/// Mirrors opencode's `openai-compatible-chat-language-model.ts` stream transform.
pub struct StreamParser {
    /// Accumulated tool call state: index → (id, name, args_so_far)
    tool_calls: HashMap<usize, (String, String, String)>,
    is_reasoning: bool,
    is_text: bool,
}

impl StreamParser {
    pub fn new() -> Self {
        Self { tool_calls: HashMap::new(), is_reasoning: false, is_text: false }
    }

    /// Process one raw SSE line. Returns zero or more events.
    pub fn process_line(&mut self, line: &str) -> Vec<StreamEvent> {
        let json_str = match parse_sse_line(line) {
            Some(s) => s,
            None => return vec![],
        };

        let chunk: Chunk = match serde_json::from_str(json_str) {
            Ok(c) => c,
            Err(e) => return vec![StreamEvent::Error(format!("JSON parse error: {e}"))],
        };

        let mut events = Vec::new();

        // Usage in final chunk (may come with empty choices)
        if let Some(usage) = &chunk.usage {
            if chunk.choices.as_ref().map(|c| c.is_empty()).unwrap_or(true) {
                events.push(StreamEvent::Done {
                    completion_tokens: usage.completion_tokens,
                    prompt_tokens: usage.prompt_tokens,
                    tg_per_s: None,
                    pp_per_s: None,
                });
                return events;
            }
        }

        let choices = match chunk.choices {
            Some(c) if !c.is_empty() => c,
            _ => return events,
        };

        let choice = &choices[0];

        // Done signal
        if choice.finish_reason.as_deref().is_some_and(|r| r != "null") {
            // Flush any completed tool calls
            for (index, (id, name, args)) in self.tool_calls.drain() {
                let arguments = serde_json::from_str(&args).unwrap_or(serde_json::Value::Null);
                events.push(StreamEvent::ToolCallComplete { index, id, name, arguments });
            }
            events.push(StreamEvent::Done {
                completion_tokens: chunk.usage.as_ref().and_then(|u| u.completion_tokens),
                prompt_tokens: chunk.usage.as_ref().and_then(|u| u.prompt_tokens),
                tg_per_s: chunk.timings.as_ref().and_then(|t| t.predicted_per_second),
                pp_per_s: chunk.timings.as_ref().and_then(|t| t.prompt_per_second),
            });
            return events;
        }

        let Some(delta) = &choice.delta else { return events };

        // Reasoning delta (Gemma 4 returns reasoning_content; Copilot uses reasoning_text)
        if let Some(r) = delta.reasoning() {
            if !r.is_empty() {
                self.is_reasoning = true;
                events.push(StreamEvent::ReasoningDelta(r.to_string()));
            }
        }

        // Text delta — if reasoning was active it implicitly ends
        if let Some(text) = &delta.content {
            if !text.is_empty() {
                self.is_reasoning = false;
                self.is_text = true;
                events.push(StreamEvent::TextDelta(text.clone()));
            }
        }

        // Tool call deltas
        if let Some(tool_deltas) = &delta.tool_calls {
            self.is_reasoning = false;
            for td in tool_deltas {
                let index = td.index;
                let entry = self.tool_calls.entry(index).or_insert_with(|| ("".into(), "".into(), "".into()));

                if let Some(id) = &td.id {
                    if !id.is_empty() { entry.0 = id.clone(); }
                }

                if let Some(func) = &td.function {
                    if let Some(name) = &func.name {
                        if !name.is_empty() {
                            entry.1 = name.clone();
                            events.push(StreamEvent::ToolCallStart {
                                id: entry.0.clone(),
                                index,
                                name: name.clone(),
                            });
                        }
                    }
                    if let Some(args) = &func.arguments {
                        entry.2.push_str(args);
                        events.push(StreamEvent::ToolCallDelta {
                            index,
                            args_fragment: args.clone(),
                        });
                    }
                }
            }
        }

        events
    }

    /// Complete tool calls accumulated so far (call at end of stream if no finish_reason arrived)
    pub fn flush(&mut self) -> Vec<StreamEvent> {
        self.tool_calls.drain()
            .map(|(index, (id, name, args))| {
                let arguments = serde_json::from_str(&args).unwrap_or(serde_json::Value::Null);
                StreamEvent::ToolCallComplete { index, id, name, arguments }
            })
            .collect()
    }

    /// Collect all accumulated tool calls as `AssistantToolCall` for history
    pub fn take_tool_calls(&mut self) -> Vec<AssistantToolCall> {
        self.tool_calls.drain()
            .map(|(_, (id, name, args))| AssistantToolCall {
                id,
                kind: "function".into(),
                function: FunctionCall { name, arguments: args },
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_delta(content: &str) -> String {
        format!(r#"data: {{"choices":[{{"delta":{{"content":"{content}"}}}}]}}"#)
    }

    fn reasoning_delta(content: &str) -> String {
        format!(r#"data: {{"choices":[{{"delta":{{"reasoning_content":"{content}"}}}}]}}"#)
    }

    fn tool_start(index: usize, id: &str, name: &str) -> String {
        format!(r#"data: {{"choices":[{{"delta":{{"tool_calls":[{{"index":{index},"id":"{id}","function":{{"name":"{name}","arguments":""}}}}]}}}}]}}"#)
    }

    fn tool_args(index: usize, args: &str) -> String {
        // escape the args for JSON embedding
        let escaped = args.replace('"', "\\\"");
        format!(r#"data: {{"choices":[{{"delta":{{"tool_calls":[{{"index":{index},"function":{{"arguments":"{escaped}"}}}}]}}}}]}}"#)
    }

    fn finish(reason: &str) -> String {
        format!(r#"data: {{"choices":[{{"delta":{{}},"finish_reason":"{reason}"}}],"usage":{{"completion_tokens":10,"prompt_tokens":5}}}}"#)
    }

    #[test]
    fn parse_text_delta() {
        let mut p = StreamParser::new();
        let events = p.process_line(&text_delta("Hello"));
        assert!(matches!(&events[0], StreamEvent::TextDelta(s) if s == "Hello"));
    }

    #[test]
    fn parse_reasoning_delta() {
        let mut p = StreamParser::new();
        let events = p.process_line(&reasoning_delta("thinking..."));
        assert!(matches!(&events[0], StreamEvent::ReasoningDelta(s) if s == "thinking..."));
    }

    #[test]
    fn parse_reasoning_text_alias() {
        let mut p = StreamParser::new();
        let line = r#"data: {"choices":[{"delta":{"reasoning_text":"alt field"}}]}"#;
        let events = p.process_line(line);
        assert!(matches!(&events[0], StreamEvent::ReasoningDelta(s) if s == "alt field"));
    }

    #[test]
    fn parse_tool_call_streaming() {
        let mut p = StreamParser::new();
        let e1 = p.process_line(&tool_start(0, "call-1", "read_file"));
        assert!(matches!(&e1[0], StreamEvent::ToolCallStart { name, .. } if name == "read_file"));

        p.process_line(&tool_args(0, r#"{"path":""#));
        p.process_line(&tool_args(0, r#"src/main.rs"}"#));

        let done_events = p.process_line(&finish("stop"));
        let complete = done_events.iter().find(|e| matches!(e, StreamEvent::ToolCallComplete { .. }));
        assert!(complete.is_some(), "Expected ToolCallComplete");
        if let Some(StreamEvent::ToolCallComplete { name, arguments, .. }) = complete {
            assert_eq!(name, "read_file");
            assert_eq!(arguments["path"].as_str().unwrap(), "src/main.rs");
        }
    }

    #[test]
    fn finish_reason_emits_done() {
        let mut p = StreamParser::new();
        let events = p.process_line(&finish("stop"));
        assert!(events.iter().any(|e| matches!(e, StreamEvent::Done { completion_tokens: Some(10), .. })));
    }

    #[test]
    fn empty_and_comment_lines_produce_no_events() {
        let mut p = StreamParser::new();
        assert!(p.process_line("").is_empty());
        assert!(p.process_line(": keepalive").is_empty());
        assert!(p.process_line("event: response.output_text.delta").is_empty());
        assert!(p.process_line("data: [DONE]").is_empty());
    }

    #[test]
    fn malformed_json_emits_error() {
        let mut p = StreamParser::new();
        let events = p.process_line("data: {not json}");
        assert!(matches!(&events[0], StreamEvent::Error(_)));
    }

    #[test]
    fn multiple_tool_calls() {
        let mut p = StreamParser::new();
        p.process_line(&tool_start(0, "c1", "read_file"));
        p.process_line(&tool_args(0, r#"{"path":"a.rs"}"#));
        p.process_line(&tool_start(1, "c2", "run_shell_command"));
        p.process_line(&tool_args(1, r#"{"command":"ls"}"#));
        let done = p.process_line(&finish("tool_calls"));
        let completes: Vec<_> = done.iter().filter(|e| matches!(e, StreamEvent::ToolCallComplete { .. })).collect();
        assert_eq!(completes.len(), 2);
    }

    #[test]
    fn flush_emits_incomplete_tool_calls() {
        let mut p = StreamParser::new();
        p.process_line(&tool_start(0, "c1", "calculate"));
        p.process_line(&tool_args(0, r#"{"expr":"1+1"}"#));
        // No finish_reason — stream ended abruptly
        let flushed = p.flush();
        assert_eq!(flushed.len(), 1);
        assert!(matches!(&flushed[0], StreamEvent::ToolCallComplete { name, .. } if name == "calculate"));
    }
}
