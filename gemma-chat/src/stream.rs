use crate::sse::parse_sse_line;
use crate::types::{AssistantToolCall, Chunk, Delta, FunctionCall, StreamEvent, Usage};
use std::collections::HashMap;

#[derive(Default)]
struct PendingDone {
    usage: Option<Usage>,
    tg_per_s: Option<f64>,
    pp_per_s: Option<f64>,
    stop_reason: Option<String>,
    terminal_error: Option<String>,
}

impl PendingDone {
    fn into_event(self) -> StreamEvent {
        if let Some(error) = self.terminal_error {
            return StreamEvent::Error(error);
        }
        let completion_tokens = self
            .usage
            .as_ref()
            .and_then(|usage| usage.completion_tokens)
            .and_then(|tokens| u32::try_from(tokens).ok());
        let prompt_tokens = self
            .usage
            .as_ref()
            .and_then(|usage| usage.prompt_tokens)
            .and_then(|tokens| u32::try_from(tokens).ok());
        StreamEvent::Done {
            completion_tokens,
            prompt_tokens,
            usage: self.usage,
            tg_per_s: self.tg_per_s,
            pp_per_s: self.pp_per_s,
            stop_reason: self.stop_reason,
        }
    }
}

/// Stateful parser that converts raw SSE lines into `StreamEvent`s.
/// Mirrors opencode's `openai-compatible-chat-language-model.ts` stream transform.
pub struct StreamParser {
    /// Accumulated tool call state: index → (id, name, args_so_far)
    tool_calls: HashMap<usize, (String, String, String)>,
    is_reasoning: bool,
    is_text: bool,
    latest_usage: Option<Usage>,
    pending_done: Option<PendingDone>,
}

impl Default for StreamParser {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamParser {
    pub fn new() -> Self {
        Self {
            tool_calls: HashMap::new(),
            is_reasoning: false,
            is_text: false,
            latest_usage: None,
            pending_done: None,
        }
    }

    /// Process one raw SSE line. Returns zero or more events.
    pub fn process_line(&mut self, line: &str) -> Vec<StreamEvent> {
        let trimmed = line.trim();
        if trimmed
            .strip_prefix("data:")
            .map(|data| data.strip_prefix(' ').unwrap_or(data))
            == Some("[DONE]")
        {
            return self.finalize_pending(false);
        }
        let json_str = match parse_sse_line(line) {
            Some(s) => s,
            None => return vec![],
        };

        let chunk: Chunk = match serde_json::from_str(json_str) {
            Ok(c) => c,
            Err(e) => return vec![StreamEvent::Error(format!("JSON parse error: {e}"))],
        };

        let mut events = Vec::new();
        let choices_are_empty = chunk
            .choices
            .as_ref()
            .map(|choices| choices.is_empty())
            .unwrap_or(true);
        if let Some(usage) = chunk.usage.clone() {
            let usage = self.observe_usage(usage, chunk.timings.as_ref());
            events.push(StreamEvent::UsageUpdate(usage));
        }
        if choices_are_empty {
            if chunk.usage.is_some() && self.pending_done.is_none() {
                events.push(StreamEvent::Error(
                    "Provider sent terminal usage before a finish reason".to_string(),
                ));
            }
            return events;
        }

        if let Some(pending) = self.pending_done.as_mut() {
            // OpenRouter-style providers echo the finished choice (empty delta,
            // same finish reason) on the trailing usage chunk. That carries no
            // data, so it is not an error; anything with real content is.
            let choices = chunk.choices.as_ref().expect("nonempty choices");
            let harmless_echo = choices.len() == 1
                && choices[0].delta.as_ref().is_none_or(Delta::is_empty)
                && choices[0]
                    .finish_reason
                    .as_deref()
                    .filter(|reason| *reason != "null")
                    .is_none_or(|reason| Some(reason) == pending.stop_reason.as_deref());
            if !harmless_echo {
                if pending.terminal_error.is_none() {
                    pending.terminal_error =
                        Some("Provider sent choice data after its finish reason".to_string());
                }
                self.tool_calls.clear();
            }
            return events;
        }

        let choices = chunk.choices.as_ref().expect("nonempty choices");
        if choices.len() != 1 {
            return vec![StreamEvent::Error(
                "Provider returned an ambiguous number of streaming choices".to_string(),
            )];
        }
        let choice = &choices[0];

        if let Some(delta) = &choice.delta {
            self.process_delta(delta, &mut events);
        }

        let Some(reason) = choice
            .finish_reason
            .as_deref()
            .filter(|reason| *reason != "null")
        else {
            return events;
        };
        let terminal_error = match reason {
            "stop" if self.tool_calls.is_empty() => None,
            "stop" => Some("Provider returned tool calls with finish reason 'stop'".to_string()),
            "tool_calls" if self.tool_calls.is_empty() => {
                Some("Provider ended for tool calls without returning any".to_string())
            }
            // Every call must be complete; how many a turn may carry is the
            // host's decision, not the parser's.
            "tool_calls" => self.tool_calls.values().find_map(|(id, name, arguments)| {
                if id.is_empty() || name.is_empty() {
                    Some("Provider returned an incomplete tool-call identity".to_string())
                } else if !serde_json::from_str::<serde_json::Value>(arguments)
                    .is_ok_and(|arguments| arguments.is_object())
                {
                    Some("Provider returned invalid tool-call arguments".to_string())
                } else {
                    None
                }
            }),
            unsupported => Some(format!(
                "Provider ended with non-complete finish reason '{unsupported}'"
            )),
        };
        if terminal_error.is_some() {
            self.tool_calls.clear();
        }

        self.pending_done = Some(PendingDone {
            usage: self.latest_usage.clone(),
            tg_per_s: chunk
                .timings
                .as_ref()
                .and_then(|timings| timings.predicted_per_second),
            pp_per_s: chunk
                .timings
                .as_ref()
                .and_then(|timings| timings.prompt_per_second),
            stop_reason: Some(reason.to_string()),
            terminal_error,
        });
        events
    }

    fn process_delta(&mut self, delta: &crate::types::Delta, events: &mut Vec<StreamEvent>) {
        if let Some(reasoning) = delta.reasoning()
            && !reasoning.is_empty()
        {
            self.is_reasoning = true;
            events.push(StreamEvent::ReasoningDelta(reasoning.to_string()));
        }
        if let Some(text) = &delta.content
            && !text.is_empty()
        {
            self.is_reasoning = false;
            self.is_text = true;
            events.push(StreamEvent::TextDelta(text.clone()));
        }
        if let Some(tool_deltas) = &delta.tool_calls {
            self.is_reasoning = false;
            for delta in tool_deltas {
                let entry = self
                    .tool_calls
                    .entry(delta.index)
                    .or_insert_with(|| (String::new(), String::new(), String::new()));
                if let Some(id) = &delta.id
                    && !id.is_empty()
                {
                    entry.0 = id.clone();
                }
                if let Some(function) = &delta.function {
                    if let Some(name) = &function.name
                        && !name.is_empty()
                    {
                        entry.1 = name.clone();
                        events.push(StreamEvent::ToolCallStart {
                            id: entry.0.clone(),
                            index: delta.index,
                            name: name.clone(),
                        });
                    }
                    if let Some(arguments) = &function.arguments {
                        entry.2.push_str(arguments);
                        events.push(StreamEvent::ToolCallDelta {
                            index: delta.index,
                            args_fragment: arguments.clone(),
                        });
                    }
                }
            }
        }
    }

    fn observe_usage(&mut self, usage: Usage, timings: Option<&crate::types::Timings>) -> Usage {
        let usage = match self.latest_usage.take() {
            Some(previous) => previous.merged_with(usage),
            None => usage,
        };
        self.latest_usage = Some(usage.clone());
        if let Some(pending) = self.pending_done.as_mut() {
            pending.usage = Some(usage.clone());
            if let Some(timings) = timings {
                pending.tg_per_s = timings.predicted_per_second.or(pending.tg_per_s);
                pending.pp_per_s = timings.prompt_per_second.or(pending.pp_per_s);
            }
        }
        usage
    }

    fn finalize_pending(&mut self, require_usage: bool) -> Vec<StreamEvent> {
        let Some(pending) = self.pending_done.take() else {
            return vec![StreamEvent::Error(
                "The model server closed the reply before finishing it (it may have crashed or restarted)"
                    .to_string(),
            )];
        };
        if pending.terminal_error.is_some() {
            self.tool_calls.clear();
            return vec![pending.into_event()];
        }
        if require_usage && pending.usage.is_none() {
            self.tool_calls.clear();
            return vec![StreamEvent::Error(
                "Provider stream ended before terminal usage".to_string(),
            )];
        }
        let mut events = Vec::new();
        if pending.stop_reason.as_deref() == Some("tool_calls") {
            let mut calls: Vec<_> = self.tool_calls.drain().collect();
            if calls.is_empty() {
                return vec![StreamEvent::Error(
                    "Provider terminal tool call disappeared".to_string(),
                )];
            }
            calls.sort_by_key(|(index, _)| *index);
            for (index, (id, name, arguments)) in calls {
                let arguments = match serde_json::from_str::<serde_json::Value>(&arguments) {
                    Ok(arguments) if arguments.is_object() => arguments,
                    _ => {
                        return vec![StreamEvent::Error(
                            "Provider returned invalid tool-call arguments".to_string(),
                        )];
                    }
                };
                events.push(StreamEvent::ToolCallComplete {
                    index,
                    id,
                    name,
                    arguments,
                });
            }
        } else {
            self.tool_calls.clear();
        }
        events.push(pending.into_event());
        events
    }

    pub fn awaiting_terminal(&self) -> bool {
        self.pending_done.is_some()
    }

    /// Finish only after a clean HTTP EOF. Usage is required because requests
    /// explicitly ask compatible providers for a terminal usage snapshot.
    pub fn finish_clean(&mut self) -> Vec<StreamEvent> {
        if self
            .pending_done
            .as_ref()
            .is_some_and(|pending| pending.stop_reason.as_deref() == Some("tool_calls"))
        {
            self.abort();
            return vec![StreamEvent::Error(
                "Provider tool-call stream ended before an explicit terminal marker".to_string(),
            )];
        }
        self.finalize_pending(true)
    }

    pub fn abort(&mut self) {
        self.pending_done = None;
        self.tool_calls.clear();
    }

    /// Compatibility alias for clean EOF handling.
    pub fn flush(&mut self) -> Vec<StreamEvent> {
        self.finish_clean()
    }

    /// Collect all accumulated tool calls as `AssistantToolCall` for history
    pub fn take_tool_calls(&mut self) -> Vec<AssistantToolCall> {
        self.tool_calls
            .drain()
            .map(|(_, (id, name, args))| AssistantToolCall {
                id,
                kind: "function".into(),
                function: FunctionCall {
                    name,
                    arguments: args,
                },
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
        format!(
            r#"data: {{"choices":[{{"delta":{{"tool_calls":[{{"index":{index},"id":"{id}","function":{{"name":"{name}","arguments":""}}}}]}}}}]}}"#
        )
    }

    fn tool_args(index: usize, args: &str) -> String {
        // escape the args for JSON embedding
        let escaped = args.replace('"', "\\\"");
        format!(
            r#"data: {{"choices":[{{"delta":{{"tool_calls":[{{"index":{index},"function":{{"arguments":"{escaped}"}}}}]}}}}]}}"#
        )
    }

    fn finish(reason: &str) -> String {
        format!(
            r#"data: {{"choices":[{{"delta":{{}},"finish_reason":"{reason}"}}],"usage":{{"completion_tokens":10,"prompt_tokens":5}}}}"#
        )
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

        let finish_events = p.process_line(&finish("tool_calls"));
        assert!(
            finish_events
                .iter()
                .any(|event| matches!(event, StreamEvent::UsageUpdate(_)))
        );
        assert!(
            !finish_events
                .iter()
                .any(|event| matches!(event, StreamEvent::ToolCallComplete { .. }))
        );
        let done_events = p.process_line("data: [DONE]");
        let complete = done_events
            .iter()
            .find(|event| matches!(event, StreamEvent::ToolCallComplete { .. }));
        assert!(complete.is_some(), "Expected ToolCallComplete");
        if let Some(StreamEvent::ToolCallComplete {
            name, arguments, ..
        }) = complete
        {
            assert_eq!(name, "read_file");
            assert_eq!(arguments["path"].as_str().unwrap(), "src/main.rs");
        }
    }

    #[test]
    fn finish_reason_emits_done_at_terminal_sentinel() {
        let mut p = StreamParser::new();
        let before_terminal = p.process_line(&finish("stop"));
        assert!(
            before_terminal
                .iter()
                .any(|event| matches!(event, StreamEvent::UsageUpdate(_)))
        );
        assert!(
            !before_terminal
                .iter()
                .any(|event| matches!(event, StreamEvent::Done { .. }))
        );
        let events = p.process_line("data: [DONE]");
        assert!(events.iter().any(|event| matches!(
            event,
            StreamEvent::Done {
                completion_tokens: Some(10),
                ..
            }
        )));
    }

    #[test]
    fn separate_usage_chunk_produces_one_cache_aware_done() {
        let mut parser = StreamParser::new();
        let finish = r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#;
        assert!(parser.process_line(finish).is_empty());
        let usage = r#"data: {"choices":[],"usage":{"prompt_tokens":100,"completion_tokens":7,"total_tokens":107,"prompt_tokens_details":{"cached_tokens":80}}}"#;
        let usage_events = parser.process_line(usage);
        assert!(matches!(
            usage_events.as_slice(),
            [StreamEvent::UsageUpdate(_)]
        ));
        let events = parser.process_line("data: [DONE]");
        assert_eq!(events.len(), 1);
        match &events[0] {
            StreamEvent::Done {
                prompt_tokens,
                completion_tokens,
                usage,
                ..
            } => {
                assert_eq!(*prompt_tokens, Some(100));
                assert_eq!(*completion_tokens, Some(7));
                let usage = usage.as_ref().unwrap();
                assert_eq!(usage.prompt_tokens, Some(100));
                assert_eq!(
                    usage
                        .prompt_tokens_details
                        .as_ref()
                        .and_then(|details| details.cached_tokens),
                    Some(80)
                );
            }
            event => panic!("expected Done, got {event:?}"),
        }
    }

    #[test]
    fn rejected_finish_collects_delayed_usage_before_terminal_error() {
        let mut parser = StreamParser::new();
        let finish = r#"data:{"choices":[{"delta":{},"finish_reason":"length"}]}"#;
        assert!(parser.process_line(finish).is_empty());

        let usage = r#"data:{"choices":[],"usage":{"prompt_tokens":100,"completion_tokens":7,"total_tokens":107}}"#;
        assert!(matches!(
            parser.process_line(usage).as_slice(),
            [StreamEvent::UsageUpdate(usage)]
                if usage.prompt_tokens == Some(100) && usage.completion_tokens == Some(7)
        ));

        let events = parser.process_line("data:[DONE]");
        assert!(matches!(
            events.as_slice(),
            [StreamEvent::Error(error)] if error.contains("non-complete finish reason 'length'")
        ));
    }

    #[test]
    fn raw_u64_usage_is_preserved_when_legacy_counter_overflows() {
        let mut parser = StreamParser::new();
        let usage =
            r#"data: {"choices":[],"usage":{"prompt_tokens":4294967296,"completion_tokens":1}}"#;
        let events = parser.process_line(usage);
        assert!(matches!(
            &events[0],
            StreamEvent::UsageUpdate(usage)
                if usage.prompt_tokens == Some(4_294_967_296)
        ));
        assert!(matches!(&events[1], StreamEvent::Error(_)));
    }

    #[test]
    fn empty_and_comment_lines_produce_no_events() {
        let mut p = StreamParser::new();
        assert!(p.process_line("").is_empty());
        assert!(p.process_line(": keepalive").is_empty());
        assert!(
            p.process_line("event: response.output_text.delta")
                .is_empty()
        );
        assert!(matches!(
            p.process_line("data: [DONE]").as_slice(),
            [StreamEvent::Error(_)]
        ));
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
        let finish_events = p.process_line(&finish("tool_calls"));
        assert!(
            finish_events
                .iter()
                .any(|event| matches!(event, StreamEvent::UsageUpdate(_)))
        );
        assert!(
            !finish_events
                .iter()
                .any(|event| matches!(event, StreamEvent::Error(_)))
        );
        let events = p.process_line("data: [DONE]");
        let names: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                StreamEvent::ToolCallComplete { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(names, ["read_file", "run_shell_command"], "index order");
        assert!(!events.iter().any(|event| matches!(event, StreamEvent::Error(_))));
    }

    #[test]
    fn terminal_chunk_processes_its_final_delta_before_finishing() {
        let mut parser = StreamParser::new();
        parser.process_line(&tool_start(0, "c1", "calculate"));
        parser.process_line(&tool_args(0, r#"{"expr":"1+1""#));
        let terminal = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":5,"completion_tokens":2}}"#;
        let events = parser.process_line(terminal);
        assert!(events
            .iter()
            .any(|event| matches!(event, StreamEvent::ToolCallDelta { args_fragment, .. } if args_fragment == "}")));
        let events = parser.process_line("data: [DONE]");
        assert!(events.iter().any(|event| matches!(
            event,
            StreamEvent::ToolCallComplete { arguments, .. }
                if arguments["expr"] == "1+1"
        )));
    }

    #[test]
    fn incomplete_finish_reasons_and_invalid_arguments_never_complete_tools() {
        for reason in ["length", "content_filter"] {
            let mut parser = StreamParser::new();
            parser.process_line(&tool_start(0, "c1", "calculate"));
            parser.process_line(&tool_args(0, r#"{"expr":"1+1"}"#));
            let finish_events = parser.process_line(&finish(reason));
            assert!(
                finish_events
                    .iter()
                    .any(|event| matches!(event, StreamEvent::UsageUpdate(_)))
            );
            assert!(
                !finish_events
                    .iter()
                    .any(|event| matches!(event, StreamEvent::Error(_)))
            );
            let events = parser.process_line("data: [DONE]");
            assert!(
                events
                    .iter()
                    .any(|event| matches!(event, StreamEvent::Error(_)))
            );
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, StreamEvent::ToolCallComplete { .. }))
            );
        }

        let mut parser = StreamParser::new();
        parser.process_line(&tool_start(0, "c1", "calculate"));
        parser.process_line(&tool_args(0, "{"));
        let finish_events = parser.process_line(&finish("tool_calls"));
        assert!(
            finish_events
                .iter()
                .any(|event| matches!(event, StreamEvent::UsageUpdate(_)))
        );
        assert!(
            !finish_events
                .iter()
                .any(|event| matches!(event, StreamEvent::Error(_)))
        );
        let events = parser.process_line("data: [DONE]");
        assert!(
            events.iter().any(
                |event| matches!(event, StreamEvent::Error(error) if error.contains("invalid"))
            )
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, StreamEvent::ToolCallComplete { .. }))
        );
    }

    #[test]
    fn post_finish_tool_delta_cannot_mutate_the_validated_call() {
        let mut parser = StreamParser::new();
        parser.process_line(&tool_start(0, "c1", "calculate"));
        parser.process_line(&tool_args(0, r#"{"expression":"2+2"}"#));
        let finish = r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#;
        assert!(parser.process_line(finish).is_empty());

        let late = parser.process_line(&tool_start(0, "c1", "run_shell_command"));
        assert!(
            !late
                .iter()
                .any(|event| matches!(event, StreamEvent::ToolCallStart { .. }))
        );
        let usage = r#"data: {"choices":[],"usage":{"prompt_tokens":5,"completion_tokens":2}}"#;
        assert!(matches!(
            parser.process_line(usage).as_slice(),
            [StreamEvent::UsageUpdate(_)]
        ));
        let events = parser.process_line("data: [DONE]");
        assert!(matches!(
            events.as_slice(),
            [StreamEvent::Error(error)] if error.contains("after its finish reason")
        ));
        assert!(!events.iter().any(|event| matches!(
            event,
            StreamEvent::ToolCallComplete { .. } | StreamEvent::Done { .. }
        )));
    }

    #[test]
    fn openrouter_usage_chunk_echoing_the_finished_choice_is_accepted() {
        let mut parser = StreamParser::new();
        parser.process_line(r#"data: {"choices":[{"index":0,"delta":{"content":"pong","role":"assistant"},"finish_reason":null}]}"#);
        parser.process_line(r#"data: {"choices":[{"index":0,"delta":{"content":"","role":"assistant","reasoning":null},"finish_reason":"stop"}]}"#);
        let usage = parser.process_line(r#"data: {"choices":[{"index":0,"delta":{"content":"","role":"assistant"},"finish_reason":"stop"}],"usage":{"prompt_tokens":9,"completion_tokens":371,"total_tokens":380}}"#);
        assert!(matches!(usage.as_slice(), [StreamEvent::UsageUpdate(_)]));
        let events = parser.process_line("data: [DONE]");
        assert!(
            events
                .iter()
                .any(|event| matches!(event, StreamEvent::Done { .. })),
            "{events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, StreamEvent::Error(_)))
        );

        // Real content after the finish reason is still rejected.
        let mut parser = StreamParser::new();
        parser.process_line(
            r#"data: {"choices":[{"delta":{"content":"a"},"finish_reason":"stop"}]}"#,
        );
        parser.process_line(
            r#"data: {"choices":[{"delta":{"content":"late"},"finish_reason":null}]}"#,
        );
        let events = parser.process_line("data: [DONE]");
        assert!(matches!(
            events.as_slice(),
            [StreamEvent::Error(error)] if error.contains("after its finish reason")
        ));
    }

    #[test]
    fn clean_eof_after_tool_finish_does_not_authorize_execution() {
        let mut parser = StreamParser::new();
        parser.process_line(&tool_start(0, "c1", "calculate"));
        parser.process_line(&tool_args(0, r#"{"expr":"1+1"}"#));
        parser.process_line(&finish("tool_calls"));
        let events = parser.finish_clean();
        assert!(matches!(events.as_slice(), [StreamEvent::Error(_)]));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, StreamEvent::ToolCallComplete { .. }))
        );
    }

    #[test]
    fn clean_eof_rejects_incomplete_tool_calls() {
        let mut parser = StreamParser::new();
        parser.process_line(&tool_start(0, "c1", "calculate"));
        parser.process_line(&tool_args(0, r#"{"expr":"1+1"}"#));
        let events = parser.finish_clean();
        assert!(matches!(events.as_slice(), [StreamEvent::Error(_)]));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, StreamEvent::ToolCallComplete { .. }))
        );
    }
}
