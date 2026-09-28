use super::*;
use crate::client::StreamEvent;
use crate::icons;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(PartialEq, Debug, Clone, Copy)]
pub enum ApprovalMode {
    None,
    Always,
}

impl App {
    pub fn approval_matches_current_policy(&self) -> bool {
        let fingerprint = self.config.python_policy_fingerprint();
        self.shell_approval_mode == ApprovalMode::Always
            && self.approval_policy_fingerprint.as_deref() == Some(fingerprint.as_str())
    }

    pub fn clear_tool_approval(&mut self) {
        self.shell_approval_mode = ApprovalMode::None;
        self.approval_policy_fingerprint = None;
    }
}

pub fn handle_tool_call(
    app: &mut App,
    calls: Vec<ToolCall>,
    pos: usize,
    tx: mpsc::UnboundedSender<StreamEvent>,
    cancellation_token: &mut CancellationToken,
    full_response_content: &str,
    is_native: bool,
) -> AppEventOutcome {
    handle_tool_call_with_provider(
        app,
        calls,
        pos,
        tx,
        cancellation_token,
        full_response_content,
        is_native,
        None,
    )
}

pub fn handle_tool_call_with_provider(
    app: &mut App,
    calls: Vec<ToolCall>,
    pos: usize,
    tx: mpsc::UnboundedSender<StreamEvent>,
    cancellation_token: &mut CancellationToken,
    full_response_content: &str,
    is_native: bool,
    provider_content: Option<Vec<serde_json::Value>>,
) -> AppEventOutcome {
    if app.tool_calls_processed_this_request {
        return AppEventOutcome::Continue;
    }
    app.tool_calls_processed_this_request = true;
    reset_non_native_stream(cancellation_token, is_native);
    app.tool_call_pos = Some(pos);

    if calls.len() > 1 {
        reject_multiple_tool_calls(app, &calls, full_response_content, &tx);
        return AppEventOutcome::Continue;
    }

    let Some(tool_call) = calls.into_iter().next() else {
        app.is_processing = false;
        app.settle_logical_turn();
        app.stop_reason = "⚠ Model returned an empty tool-call event".to_string();
        return AppEventOutcome::Continue;
    };
    register_pending_tool_call(app, &tool_call, full_response_content, provider_content);
    let description = tool_call_description(&tool_call);

    if tool_call.function.name == "python"
        && prepare_python_tool_call(app, &tool_call, &description, &tx)
    {
        return AppEventOutcome::Continue;
    }

    add_tool_call_to_transcript(app, &tool_call, &description);
    if let Err(error) = app.save_session_checked() {
        handle_tool_call_save_failure(app, &tool_call, error);
        return AppEventOutcome::Continue;
    }
    if let Some(error) = crate::tools::tool_admission_error(
        &app.config,
        crate::tools::ToolSurface::Interactive,
        &tool_call.function.name,
    ) {
        reject_disallowed_tool(app, &tool_call, &error, &tx);
        return AppEventOutcome::Continue;
    }
    dispatch_tool_call_outcome(app, &tool_call, &description)
}

fn reset_non_native_stream(cancellation_token: &mut CancellationToken, is_native: bool) {
    if is_native {
        return;
    }
    cancellation_token.cancel();
    *cancellation_token = CancellationToken::new();
}

fn reject_multiple_tool_calls(
    app: &mut App,
    calls: &[ToolCall],
    full_response_content: &str,
    tx: &mpsc::UnboundedSender<StreamEvent>,
) {
    let reason = format!(
        "Provider returned {} tool calls in one turn; Lethetic requires exactly one. The entire batch was rejected and no tool was executed.",
        calls.len()
    );
    let session_directory = app.current_session_dir.as_deref().map(std::path::Path::new);
    let mut audit_errors = Vec::new();
    for call in calls.iter().filter(|call| call.function.name == "python") {
        let validated = crate::tools::python::validate_arguments(&call.function.arguments);
        let source = validated.as_ref().map(|(source, _)| *source).unwrap_or("");
        let description = tool_call_description(call);
        let audit_id = call.provider_id.as_deref().unwrap_or(&call.id);
        match app.tool_runtime.begin_python_audit_attempt(
            session_directory,
            &app.config,
            audit_id,
            source,
            &description,
            &app.current_dir,
        ) {
            Ok(_) => {
                if let Err(error) = app.tool_runtime.mark_python_audit_status(
                    audit_id,
                    crate::python::notebook::NotebookAttemptStatus::Denied,
                    Some("Provider returned multiple tool calls; the entire batch was rejected."),
                ) {
                    audit_errors.push(format!("{audit_id}: {error}"));
                }
            }
            Err(error) => audit_errors.push(format!("{audit_id}: {error}")),
        }
        if let Some(path) = app.tool_runtime.take_python_notebook_notice() {
            let _ = tx.send(StreamEvent::PythonNotebookNotice(path));
        }
    }

    let existing_ids = calls
        .iter()
        .map(|call| call.id.as_str())
        .collect::<Vec<_>>();
    let mut messages = app.context_manager.get_messages().to_vec();
    if messages.last().is_some_and(|message| {
        message.role == "assistant"
            && message.tool_calls.as_ref().is_some_and(|checkpointed| {
                checkpointed
                    .iter()
                    .map(|call| call.id.as_str())
                    .eq(existing_ids.iter().copied())
            })
    }) {
        messages.pop();
        app.context_manager.set_messages(messages);
    }
    let repaired_content = if full_response_content.trim().is_empty() {
        reason.clone()
    } else {
        format!("{}\n\n{reason}", full_response_content.trim_end())
    };
    app.context_manager
        .upsert_assistant_message(&repaired_content, None);

    app.pending_tool_call = None;
    app.tool_call_pos = None;
    app.show_approval_prompt = false;
    app.is_asking_user = false;
    app.is_processing = false;
    app.is_executing_tool = false;
    app.tool_call_dispatched = false;
    app.settle_logical_turn();
    app.stop_reason = if audit_errors.is_empty() {
        format!("⚠ Rejected {} parallel tool calls", calls.len())
    } else {
        format!(
            "✗ Rejected {} parallel tool calls; {} Python audit checkpoint(s) failed",
            calls.len(),
            audit_errors.len()
        )
    };
    app.add_segment(format!("\n{} {reason}\n", icons::WARNING), BlockType::Text);
    if !audit_errors.is_empty() {
        app.add_segment(
            format!(
                "\n{} PYTHON AUDIT ERROR: {}\n",
                icons::WARNING,
                audit_errors.join(" | ")
            ),
            BlockType::ToolError,
        );
    }
    if let Err(error) = app.save_session_checked() {
        app.stop_reason = format!("✗ Multi-tool rejection session save failed: {error}");
    }
    app.should_redraw = true;
}

fn truncate_chars_with_ellipsis(value: &str, maximum_chars: usize, prefix_chars: usize) -> String {
    if value.chars().count() > maximum_chars {
        format!("{}…", value.chars().take(prefix_chars).collect::<String>())
    } else {
        value.to_string()
    }
}

fn register_pending_tool_call(
    app: &mut App,
    tool_call: &ToolCall,
    full_response_content: &str,
    provider_content: Option<Vec<serde_json::Value>>,
) {
    app.pending_tool_call = Some(tool_call.clone());
    app.python_approval_show_original = false;
    app.python_approval_scroll = 0;
    app.context_manager
        .upsert_assistant_tool_call_with_provider(
            full_response_content,
            vec![tool_call.clone()],
            provider_content,
        );
}

fn tool_call_description(tool_call: &ToolCall) -> String {
    tool_call.function.arguments["description"]
        .as_str()
        .unwrap_or("Action")
        .to_string()
}

/// Returns true when Python admission produced a terminal tool result.
fn prepare_python_tool_call(
    app: &mut App,
    tool_call: &ToolCall,
    description: &str,
    tx: &mpsc::UnboundedSender<StreamEvent>,
) -> bool {
    let validated = crate::tools::python::validate_arguments(&tool_call.function.arguments);
    let source = validated.as_ref().map(|(source, _)| *source).unwrap_or("");
    let display = crate::python::display::format_python_for_display(source);
    if let Some(reason) = display.fallback_reason {
        app.log_debug(&format!("[PYTHON DISPLAY] {reason}"));
    }
    let session_directory = app.current_session_dir.as_deref().map(std::path::Path::new);
    let audit_call_id = tool_call.provider_id.as_deref().unwrap_or(&tool_call.id);
    if let Err(error) = app.tool_runtime.begin_python_audit_attempt(
        session_directory,
        &app.config,
        audit_call_id,
        source,
        description,
        &app.current_dir,
    ) {
        app.stop_reason = format!("✗ Python notebook audit checkpoint failed: {error}");
        app.is_processing = true;
        app.tool_call_dispatched = true;
        send_tool_result(
            app,
            tool_call,
            format!(
                "ERROR: Python execution was not started because its notebook audit checkpoint failed: {error}"
            ),
            tx,
        );
        return true;
    }
    if let Some(path) = app.tool_runtime.take_python_notebook_notice() {
        let _ = tx.send(StreamEvent::PythonNotebookNotice(path));
    }
    let Err(argument_error) = validated else {
        return false;
    };

    let message = format!("Malformed Python tool call: {argument_error}.");
    let audit_error = app
        .tool_runtime
        .mark_python_audit_status(
            audit_call_id,
            crate::python::notebook::NotebookAttemptStatus::Denied,
            Some(&message),
        )
        .err();
    app.stop_reason = "⚠ Blocked malformed Python tool call".to_string();
    app.is_processing = true;
    app.tool_call_dispatched = true;
    let result = match audit_error {
        Some(error) => {
            format!("ERROR: {message} The notebook rejection checkpoint also failed: {error}")
        }
        None => format!("ERROR: {message}"),
    };
    send_tool_result(app, tool_call, result, tx);
    true
}

fn add_tool_call_to_transcript(app: &mut App, tool_call: &ToolCall, description: &str) {
    app.record_tool_use(&tool_call.function.name);
    app.tool_call_started_at = Some(std::time::Instant::now());
    app.log_debug(&format!(
        "[TOOL CALL] {}: {}",
        tool_call.function.name, description
    ));
    let block = RenderBlock::tool_call(tool_call);
    app.add_segment_with_title(
        block.content,
        block.block_type,
        block.title.unwrap_or_else(|| "Action".to_string()),
    );
}

fn handle_tool_call_save_failure(app: &mut App, tool_call: &ToolCall, error: String) {
    let audit_error = (tool_call.function.name == "python")
        .then(|| {
            app.tool_runtime.mark_python_audit_status(
                tool_call.provider_id.as_deref().unwrap_or(&tool_call.id),
                crate::python::notebook::NotebookAttemptStatus::Interrupted,
                Some("Tool handling stopped because session state could not be saved."),
            )
        })
        .transpose()
        .err();
    let failure = match audit_error {
        Some(audit_error) => format!(
            "ERROR: Tool handling was stopped because session state could not be saved. The notebook interruption checkpoint also failed: {audit_error}"
        ),
        None => {
            "ERROR: Tool handling was stopped because session state could not be saved.".to_string()
        }
    };
    if let Some(pending) = app.pending_tool_call.take() {
        app.context_manager.add_tool_message_with_status(
            pending.id,
            &pending.function.name,
            &failure,
            true,
        );
    }
    app.tool_call_dispatched = true;
    app.is_processing = false;
    app.show_approval_prompt = false;
    app.settle_logical_turn();
    app.stop_reason = format!("✗ Cannot handle tool call: session save failed: {error}");
    app.add_segment(
        format!("\n{} SESSION SAVE ERROR: {error}\n", icons::WARNING),
        BlockType::ToolError,
    );
    app.should_redraw = true;
}

fn reject_disallowed_tool(
    app: &mut App,
    tool_call: &ToolCall,
    admission_error: &str,
    tx: &mpsc::UnboundedSender<StreamEvent>,
) {
    let mut result = format!("ERROR: {admission_error}");
    if tool_call.function.name == "python"
        && let Err(error) = app.tool_runtime.mark_python_audit_status(
            tool_call.provider_id.as_deref().unwrap_or(&tool_call.id),
            crate::python::notebook::NotebookAttemptStatus::Denied,
            Some("Python dispatch was rejected by the active tool profile."),
        )
    {
        result.push_str(&format!(
            " The notebook rejection checkpoint also failed: {error}"
        ));
    }
    app.stop_reason = format!("⚠ Blocked disallowed tool: {}", tool_call.function.name);
    app.is_processing = true;
    app.tool_call_dispatched = true;
    send_tool_result(app, tool_call, result, tx);
}

fn send_tool_result(
    app: &App,
    tool_call: &ToolCall,
    result: String,
    tx: &mpsc::UnboundedSender<StreamEvent>,
) {
    let _ = tx.send(StreamEvent::ToolResult {
        id: Some(tool_call.id.clone()),
        func_name: tool_call.function.name.clone(),
        result,
        cwd: app.current_dir.clone(),
        is_error: true,
        provenance: crate::tools::ToolOutputProvenance::OrdinaryHost,
    });
}

fn dispatch_tool_call_outcome(
    app: &mut App,
    tool_call: &ToolCall,
    description: &str,
) -> AppEventOutcome {
    if tool_call.function.name == "ask_the_user" {
        app.is_asking_user = true;
        app.is_processing = false;
        let question = tool_call.function.arguments["question"]
            .as_str()
            .unwrap_or("…");
        let q_short = truncate_chars_with_ellipsis(question, 60, 57);
        app.stop_reason = format!("⏸ Waiting for your answer: {}", q_short);
        return AppEventOutcome::Continue;
    }
    if app.approval_matches_current_policy() {
        return AppEventOutcome::ToolApproved(true, true);
    }
    app.show_approval_prompt = true;
    app.is_processing = false;
    app.stop_reason = format!(
        "⏸ Awaiting approval: {} — {}",
        tool_call.function.name, description
    );
    AppEventOutcome::Continue
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::context::FunctionCall;

    #[test]
    fn save_failure_uses_tool_error_provenance() {
        let config = Config {
            context_size: 100_000,
            ..Default::default()
        };
        let mut app = App::new(&config);
        app.blocks.clear();
        let tool_call = ToolCall {
            id: "tool-save-failure".to_string(),
            provider_id: None,
            function: FunctionCall {
                name: "calculate".to_string(),
                arguments: serde_json::json!({"expression": "2 + 2"}),
            },
        };
        register_pending_tool_call(&mut app, &tool_call, "", None);

        handle_tool_call_save_failure(&mut app, &tool_call, "save-local-violet".to_string());

        assert_eq!(app.blocks.len(), 1);
        assert_eq!(app.blocks[0].block_type, BlockType::ToolError);
        assert_eq!(app.blocks[0].success, Some(false));
        assert!(app.blocks[0].content.contains("save-local-violet"));
        assert!(
            app.context_manager
                .get_messages()
                .last()
                .is_some_and(|message| message.tool_result_is_error)
        );
    }

    #[test]
    fn question_preview_preserves_utf8_boundaries_and_threshold() {
        let rendered = truncate_chars_with_ellipsis(&"界".repeat(61), 60, 57);
        assert_eq!(rendered, format!("{}…", "界".repeat(57)));
        assert_eq!(
            truncate_chars_with_ellipsis(&"界".repeat(60), 60, 57),
            "界".repeat(60)
        );
    }
}
