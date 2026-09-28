use reqwest::Client;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use lethetic::app::{App, BlockType, RenderBlock};
#[cfg(any(target_os = "linux", test))]
use lethetic::client::RequestAccountingHook;
use lethetic::client::StreamEvent;
#[cfg(not(target_os = "linux"))]
use lethetic::client::trigger_llm_request_with_surface;
#[cfg(target_os = "linux")]
use lethetic::client::{prepare_llm_request, trigger_prepared_llm_request_with_hook};
use lethetic::config::Config;
use lethetic::context::ToolCall;
use lethetic::icons;

pub(crate) fn request_lsp_install_cancellation(
    app: &mut App,
    cancellation_token: &CancellationToken,
    reason: &str,
) -> bool {
    if !app.lsp_install_in_progress {
        return false;
    }
    if !app.lsp_install_cancel_pending {
        if !cancellation_token.is_cancelled() {
            cancellation_token.cancel();
        }
        app.lsp_install_cancel_pending = true;
        app.add_segment(
            format!("\n{} [STOPPING LSP INSTALL]\n", icons::WARNING),
            BlockType::Text,
        );
    }
    app.stop_reason = reason.to_string();
    app.should_redraw = true;
    true
}

pub(crate) fn tool_result_has_provider_call_id(id: &Option<String>) -> bool {
    id.is_some()
}

fn local_result_owns_execution_state(func_name: &str, lsp_install_in_progress: bool) -> bool {
    func_name == "lsp_install" && lsp_install_in_progress
}

pub(crate) fn settle_lsp_install_result(app: &mut App, func_name: &str, success: bool) -> bool {
    if !local_result_owns_execution_state(func_name, app.lsp_install_in_progress) {
        return false;
    }
    let was_cancelled = std::mem::take(&mut app.lsp_install_cancel_pending);
    app.lsp_install_in_progress = false;
    app.is_executing_tool = false;
    app.settle_standalone_cancellation();
    app.tool_output_preview.clear();
    app.stop_reason = if was_cancelled {
        "LSP server installation cancelled".to_string()
    } else if success {
        "✓ LSP server installation completed".to_string()
    } else {
        "✗ LSP server installation failed".to_string()
    };
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SideEffectKind {
    Provider,
    Tool,
}

impl SideEffectKind {
    fn error_block_type(self) -> BlockType {
        match self {
            Self::Provider => BlockType::ProviderError,
            Self::Tool => BlockType::ToolError,
        }
    }
}

pub(crate) fn persist_before_side_effect(
    app: &mut App,
    action: &str,
    kind: SideEffectKind,
) -> bool {
    match app.save_session_checked() {
        Ok(()) => true,
        Err(error) => {
            app.is_processing = false;
            app.request_start_time = None;
            app.settle_logical_turn();
            app.stop_reason = format!("✗ Cannot {action}: session save failed: {error}");
            app.add_segment(
                format!(
                    "\n{} SESSION SAVE ERROR: refusing to {action}: {error}\n",
                    icons::WARNING
                ),
                kind.error_block_type(),
            );
            app.should_redraw = true;
            false
        }
    }
}

pub(crate) fn record_provider_start_failure(app: &mut App, action: &str, error: &str) {
    app.is_processing = false;
    app.request_start_time = None;
    app.settle_logical_turn();
    app.stop_reason = format!("✗ {action} was not started: {error}");
    app.add_segment(
        format!(
            "\n{} PROVIDER CHECKPOINT ERROR: {action} was not started: {error}\n",
            icons::WARNING
        ),
        BlockType::ProviderError,
    );
    app.should_redraw = true;
}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn interactive_request_checkpoint_hook(
    tx: &mpsc::UnboundedSender<StreamEvent>,
    include_transcript: bool,
) -> RequestAccountingHook {
    let tx = tx.clone();
    std::sync::Arc::new(move |checkpoint| {
        let mut checkpoint = checkpoint.clone();
        if !include_transcript {
            checkpoint.transcript = None;
        }
        let (acknowledgement, result) = std::sync::mpsc::sync_channel(1);
        tx.send(StreamEvent::PersistRequestCheckpoint {
            checkpoint,
            acknowledgement,
        })
        .map_err(|_| "interactive request checkpoint receiver closed".to_string())?;
        result
            .recv()
            .map_err(|_| "interactive request checkpoint acknowledgement was dropped".to_string())?
    })
}

#[cfg(target_os = "linux")]
pub(crate) fn trigger_persisted_provider_request(
    app: &mut App,
    client: &Client,
    config: &Config,
    tx: &mpsc::UnboundedSender<StreamEvent>,
    cancellation_token: &CancellationToken,
) -> Result<String, String> {
    let prepared = prepare_llm_request(
        config,
        &app.context_manager,
        lethetic::tools::ToolSurface::Interactive,
    )?;
    app.persist_provider_request_start(prepared.accounting_start().clone())?;
    trigger_prepared_llm_request_with_hook(
        client.clone(),
        prepared,
        tx.clone(),
        cancellation_token.clone(),
        app.show_debug,
        app.current_session_dir.clone(),
        Some(interactive_request_checkpoint_hook(tx, true)),
    )
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn trigger_persisted_provider_request(
    app: &mut App,
    client: &Client,
    config: &Config,
    tx: &mpsc::UnboundedSender<StreamEvent>,
    cancellation_token: &CancellationToken,
) -> Result<String, String> {
    trigger_llm_request_with_surface(
        client.clone(),
        config.clone(),
        &app.context_manager,
        tx.clone(),
        cancellation_token.clone(),
        app.show_debug,
        None,
        lethetic::tools::ToolSurface::Interactive,
    )
}

pub(crate) fn record_terminal_tool_error(app: &mut App, tool_call: ToolCall, result: &str) {
    let description = tool_call.function.arguments["description"]
        .as_str()
        .unwrap_or("Action")
        .to_string();
    app.context_manager.add_tool_message_with_status(
        tool_call.id,
        &tool_call.function.name,
        result,
        true,
    );
    let block = RenderBlock::tool_result(format!("\n{result}\n"), description, true);
    app.add_segment_with_title(
        block.content,
        block.block_type,
        block.title.unwrap_or_else(|| "Action".to_string()),
    );
}

pub(crate) const PENDING_INTERACTION_CANCELLED_RESULT: &str =
    "ERROR: The pending tool interaction was cancelled by the user.";
const SHUTDOWN_INTERACTION_CANCELLED_RESULT: &str =
    "ERROR: The pending tool interaction was cancelled because Lethetic is shutting down.";

fn settle_pending_interaction_with_result_checked(
    app: &mut App,
    result: &'static str,
    audit_reason: &'static str,
    restore_on_failure: bool,
) -> Result<bool, String> {
    let interaction_is_pending = app.show_approval_prompt || app.is_asking_user;
    if !interaction_is_pending {
        return Ok(false);
    }
    let tool_call = app
        .pending_tool_call
        .as_ref()
        .ok_or_else(|| "pending interaction has no bound tool call".to_string())?
        .clone();
    let audit_error = if tool_call.function.name == "python" {
        app.tool_runtime
            .mark_python_audit_status(
                tool_call.provider_id.as_deref().unwrap_or(&tool_call.id),
                lethetic::python::notebook::NotebookAttemptStatus::Cancelled,
                Some(audit_reason),
            )
            .err()
    } else {
        None
    };
    if restore_on_failure && let Some(error) = audit_error.as_ref() {
        return Err(format!(
            "pending interaction audit cancellation could not be persisted: {error}"
        ));
    }

    let messages_before = app.context_manager.get_messages().to_vec();
    let blocks_before = app.blocks.clone();
    let pending_before = app.pending_tool_call.clone();
    let queued_before = app.queued_tool_calls.clone();
    let notes_before = app.deferred_batch_notes.clone();
    let flags_before = (
        app.show_approval_prompt,
        app.is_asking_user,
        app.python_approval_show_original,
        app.python_approval_scroll,
        app.is_processing,
        app.is_executing_tool,
        app.tool_calls_processed_this_request,
        app.tool_call_dispatched,
        app.tool_call_pos,
        app.request_start_time,
        app.needs_save,
        app.should_redraw,
        app.stop_reason.clone(),
        app.tool_output_preview.clone(),
        app.output_state.clone(),
        app.scroll,
        app.total_line_count,
    );

    app.pending_tool_call = None;
    app.show_approval_prompt = false;
    app.is_asking_user = false;
    app.python_approval_show_original = false;
    app.python_approval_scroll = 0;
    app.is_processing = false;
    app.is_executing_tool = false;
    app.tool_calls_processed_this_request = false;
    app.tool_call_dispatched = false;
    app.tool_call_pos = None;
    app.request_start_time = None;
    app.tool_output_preview.clear();
    let pending_call_is_in_context =
        app.context_manager
            .get_messages()
            .iter()
            .rev()
            .find(|message| message.role == "assistant")
            .is_some_and(|message| {
                message
                        .tool_calls
                        .as_ref()
                        .is_some_and(|calls| calls.iter().any(|call| call.id == tool_call.id))
            });
    if !pending_call_is_in_context {
        let mut compact_call = tool_call.clone();
        compact_call.function.arguments = serde_json::json!({
            "description": "Cancelled pending interaction"
        });
        app.context_manager
            .upsert_assistant_tool_call_with_provider("", vec![compact_call], None);
    }
    record_terminal_tool_error(app, tool_call, result);
    app.abandon_queued_tool_calls("the user cancelled the batch");
    app.stop_reason = "Cancelled by user".to_string();
    app.should_redraw = true;

    let save_error = app.save_session_checked().err();
    if restore_on_failure && let Some(error) = save_error.as_ref() {
        app.context_manager.set_messages(messages_before);
        app.blocks = blocks_before;
        app.pending_tool_call = pending_before;
        app.queued_tool_calls = queued_before;
        app.deferred_batch_notes = notes_before;
        (
            app.show_approval_prompt,
            app.is_asking_user,
            app.python_approval_show_original,
            app.python_approval_scroll,
            app.is_processing,
            app.is_executing_tool,
            app.tool_calls_processed_this_request,
            app.tool_call_dispatched,
            app.tool_call_pos,
            app.request_start_time,
            app.needs_save,
            app.should_redraw,
            app.stop_reason,
            app.tool_output_preview,
            app.output_state,
            app.scroll,
            app.total_line_count,
        ) = flags_before;
        return Err(format!(
            "pending interaction cancellation could not be persisted: {error}"
        ));
    }
    app.settle_logical_turn();
    match (audit_error, save_error) {
        (Some(audit_error), Some(save_error)) => Err(format!(
            "pending interaction cancellation had multiple persistence failures: audit checkpoint: {audit_error}; session save: {save_error}"
        )),
        (Some(error), None) => Err(format!(
            "pending interaction audit cancellation could not be persisted: {error}"
        )),
        (None, Some(error)) => Err(format!(
            "pending interaction cancellation could not be persisted: {error}"
        )),
        (None, None) => Ok(true),
    }
}

pub(crate) fn settle_pending_interaction_checked(app: &mut App) -> Result<bool, String> {
    settle_pending_interaction_with_result_checked(
        app,
        PENDING_INTERACTION_CANCELLED_RESULT,
        "Tool execution was cancelled by the user while awaiting interaction.",
        true,
    )
}

pub(crate) fn cancel_pending_interaction_for_shutdown(app: &mut App) -> Result<(), String> {
    settle_pending_interaction_with_result_checked(
        app,
        SHUTDOWN_INTERACTION_CANCELLED_RESULT,
        "Tool execution was cancelled because Lethetic is shutting down.",
        false,
    )
    .map(|_| ())
}

/// Dispatch `pending_tool_call` immediately (used when ApprovalMode::Always auto-approved it).
pub(crate) fn dispatch_auto_approved_tool(
    app: &mut App,
    tx: &mpsc::UnboundedSender<StreamEvent>,
    cancellation_token: &CancellationToken,
    client: &Client,
    config: &Config,
) {
    let Some(pending_tool_call) = app.pending_tool_call.as_ref().cloned() else {
        return;
    };
    if let Some(error) = lethetic::tools::tool_admission_error(
        config,
        lethetic::tools::ToolSurface::Interactive,
        &pending_tool_call.function.name,
    ) {
        let mut result = format!("ERROR: {error}");
        if pending_tool_call.function.name == "python"
            && let Err(audit_error) = app.tool_runtime.mark_python_audit_status(
                pending_tool_call
                    .provider_id
                    .as_deref()
                    .unwrap_or(&pending_tool_call.id),
                lethetic::python::notebook::NotebookAttemptStatus::Denied,
                Some("Python dispatch was rejected by the active tool policy."),
            )
        {
            result.push_str(&format!(
                " The notebook rejection checkpoint also failed: {audit_error}"
            ));
        }
        app.is_processing = true;
        app.tool_call_dispatched = true;
        app.show_approval_prompt = false;
        let _ = tx.send(StreamEvent::ToolResult {
            id: Some(pending_tool_call.id),
            func_name: pending_tool_call.function.name,
            result,
            cwd: app.current_dir.clone(),
            is_error: true,
            provenance: lethetic::tools::ToolOutputProvenance::OrdinaryHost,
        });
        return;
    }
    if let Some(tool_call) = app
        .pending_tool_call
        .as_ref()
        .filter(|call| call.function.name == "python")
        .cloned()
        && let Err(error) = app.tool_runtime.mark_python_audit_status(
            tool_call.provider_id.as_deref().unwrap_or(&tool_call.id),
            lethetic::python::notebook::NotebookAttemptStatus::Approved,
            None,
        )
    {
        app.is_processing = true;
        app.tool_call_dispatched = true;
        app.show_approval_prompt = false;
        let _ = tx.send(StreamEvent::ToolResult {
            id: Some(tool_call.id),
            func_name: tool_call.function.name,
            result: format!(
                "ERROR: Python execution was not started because its approval audit checkpoint failed: {error}"
            ),
            cwd: app.current_dir.clone(),
            is_error: true,
            provenance: lethetic::tools::ToolOutputProvenance::OrdinaryHost,
        });
        return;
    }
    if !persist_before_side_effect(app, "start the tool", SideEffectKind::Tool) {
        if let Some(tool_call) = app.pending_tool_call.take() {
            let mut result =
                "ERROR: Tool execution was not started because session state could not be saved."
                    .to_string();
            if tool_call.function.name == "python"
                && let Err(error) = app.tool_runtime.mark_python_audit_status(
                    tool_call.provider_id.as_deref().unwrap_or(&tool_call.id),
                    lethetic::python::notebook::NotebookAttemptStatus::Interrupted,
                    Some(
                        "Tool execution was not started because session state could not be saved.",
                    ),
                )
            {
                result.push_str(&format!(
                    " The notebook interruption checkpoint also failed: {error}"
                ));
            }
            record_terminal_tool_error(app, tool_call, &result);
        }
        app.tool_call_dispatched = true;
        app.show_approval_prompt = false;
        return;
    }

    let Some(tool_call) = app.pending_tool_call.as_ref() else {
        return;
    };
    let tc_id = tool_call.id.clone();
    let audit_tc_id = tool_call
        .provider_id
        .clone()
        .unwrap_or_else(|| tc_id.clone());
    let func_name = tool_call.function.name.clone();
    let mut args = tool_call.function.arguments.clone();
    if func_name == "python"
        && let Some(arguments) = args.as_object_mut()
    {
        arguments.insert(
            lethetic::tools::INTERNAL_TOOL_CALL_ID_KEY.to_string(),
            serde_json::Value::String(audit_tc_id),
        );
    }
    let current_dir = app.current_dir.clone();
    let ctx_tx = tx.clone();
    let tool_cancel = cancellation_token.clone();
    let client = client.clone();
    let config = config.clone();
    let tool_runtime = app.tool_runtime.clone();
    #[cfg(target_os = "linux")]
    let request_hook = Some(interactive_request_checkpoint_hook(tx, false));
    #[cfg(not(target_os = "linux"))]
    let request_hook = None;
    app.is_executing_tool = true;
    app.tool_call_dispatched = true;
    tokio::spawn(async move {
        let execution = lethetic::tools::execute_with_runtime_and_request_hook(
            &tool_runtime,
            func_name.as_str(),
            &args,
            &current_dir,
            tool_cancel,
            ctx_tx.clone(),
            &client,
            &config,
            request_hook,
        )
        .await;
        let _ = ctx_tx.send(StreamEvent::ToolResult {
            id: Some(tc_id),
            func_name,
            result: execution.output,
            cwd: execution.cwd.clone(),
            is_error: execution.is_error,
            provenance: execution.provenance,
        });
        let _ = ctx_tx.send(StreamEvent::DebugLog(format!(
            "DIR_UPDATE|{}",
            execution.cwd
        )));
    });
    app.is_processing = true;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistence_gate_records_typed_local_errors() {
        for (kind, expected) in [
            (SideEffectKind::Provider, BlockType::ProviderError),
            (SideEffectKind::Tool, BlockType::ToolError),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let mut app = App::new(&Config::default());
            app.blocks.clear();
            app.current_session_dir = Some(directory.path().to_string_lossy().into_owned());
            app.session_directory_binding = None;

            assert!(!persist_before_side_effect(&mut app, "test boundary", kind));

            assert_eq!(app.blocks.len(), 1);
            assert_eq!(app.blocks[0].block_type, expected);
            assert_eq!(app.blocks[0].success, Some(false));
            assert!(
                app.blocks[0]
                    .content
                    .contains("no directory identity binding")
            );
        }
    }

    #[test]
    fn provider_start_failure_records_typed_local_detail() {
        let mut app = App::new(&Config::default());
        app.blocks.clear();

        record_provider_start_failure(&mut app, "Provider request", "checkpoint-local-violet");

        assert_eq!(app.blocks.len(), 1);
        assert_eq!(app.blocks[0].block_type, BlockType::ProviderError);
        assert_eq!(app.blocks[0].success, Some(false));
        assert!(app.blocks[0].content.contains("checkpoint-local-violet"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn request_validation_fails_before_durable_start_checkpoint() {
        let config = Config {
            context_size: 100_000,
            extra_body: Some(serde_json::json!({"tools": []})),
            ..Default::default()
        };
        let mut app = App::new(&config);
        app.context_manager.add_message("user", "must not start");
        let (tx, _rx) = mpsc::unbounded_channel();

        let error = trigger_persisted_provider_request(
            &mut app,
            &Client::new(),
            &config,
            &tx,
            &CancellationToken::new(),
        )
        .unwrap_err();

        assert!(error.contains("tools"), "{error}");
        assert!(!error.contains("durable session"), "{error}");
        assert!(app.active_request_id.is_none());
    }

    #[test]
    fn pending_python_approval_is_checkpointed_and_cleared_for_shutdown() {
        let workspace = tempfile::tempdir().unwrap();
        let workspace = workspace.path().canonicalize().unwrap();
        let mut config = Config::default();
        config.tool_profile = lethetic::config::ToolProfile::PythonOnly;
        config.python_runtime.target = Some(lethetic::config::PythonExecutionTarget::Host);
        let mut app = App::new(&config);
        app.current_dir = workspace.to_string_lossy().into_owned();
        app.tool_runtime = lethetic::tool_runtime::ToolRuntime::interactive(workspace);
        app.tool_runtime
            .begin_python_audit_attempt(
                None,
                &config,
                "toolu-exit",
                "1 + 1",
                "pending approval",
                &app.current_dir,
            )
            .unwrap();
        app.pending_tool_call = Some(lethetic::context::ToolCall {
            id: "effective-exit".to_string(),
            provider_id: Some("toolu-exit".to_string()),
            function: lethetic::context::FunctionCall {
                name: "python".to_string(),
                arguments: serde_json::json!({
                    "code": "1 + 1",
                    "description": "pending approval"
                }),
            },
        });
        app.show_approval_prompt = true;

        cancel_pending_interaction_for_shutdown(&mut app).unwrap();

        assert!(app.pending_tool_call.is_none());
        assert!(!app.show_approval_prompt);
        let notebook: serde_json::Value = serde_json::from_slice(
            &std::fs::read(app.tool_runtime.python_notebook_path().unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            notebook["cells"][0]["metadata"]["lethetic"]["status"],
            "cancelled"
        );
    }

    #[test]
    fn pending_ask_user_is_cancelled_without_provider_continuation() {
        let config = Config {
            context_size: 100_000,
            ..Default::default()
        };
        let mut app = App::new(&config);
        app.pending_tool_call = Some(lethetic::context::ToolCall {
            id: "question-one".to_string(),
            provider_id: None,
            function: lethetic::context::FunctionCall {
                name: "ask_the_user".to_string(),
                arguments: serde_json::json!({"question": "secret question"}),
            },
        });
        app.is_asking_user = true;

        cancel_pending_interaction_for_shutdown(&mut app).unwrap();

        assert!(!app.is_asking_user);
        assert!(app.pending_tool_call.is_none());
        let last = app.context_manager.get_messages().last().unwrap();
        assert_eq!(last.role, "tool");
        assert!(last.tool_result_is_error);
        let terminal = app.blocks.last().unwrap();
        assert_eq!(terminal.block_type, BlockType::ToolError);
        assert_eq!(terminal.success, Some(false));
        assert!(terminal.content.contains("shutting down"));
    }

    #[test]
    fn failed_shutdown_interaction_save_keeps_terminal_state_and_reports_the_failure() {
        let mut app = App::new(&Config::default());
        app.add_logical_turn_user_segment("question".to_string());
        app.context_manager.add_message("user", "question");
        let tool_call = lethetic::context::ToolCall {
            id: "question-shutdown-save-failure".to_string(),
            provider_id: None,
            function: lethetic::context::FunctionCall {
                name: "ask_the_user".to_string(),
                arguments: serde_json::json!({"question": "Continue?"}),
            },
        };
        app.context_manager
            .upsert_assistant_tool_call_with_provider("", vec![tool_call.clone()], None);
        app.pending_tool_call = Some(tool_call);
        app.is_asking_user = true;
        let directory = tempfile::tempdir().unwrap();
        app.current_session_dir = Some(directory.path().to_string_lossy().into_owned());
        app.session_directory_binding = None;

        let error = cancel_pending_interaction_for_shutdown(&mut app).unwrap_err();

        assert!(error.contains("could not be persisted"), "{error}");
        assert!(!app.is_asking_user);
        assert!(app.pending_tool_call.is_none());
        assert!(app.active_cancellation_id().is_none());
        assert_eq!(
            app.context_manager
                .get_messages()
                .iter()
                .filter(|message| {
                    message.role == "tool"
                        && message
                            .content
                            .contains(SHUTDOWN_INTERACTION_CANCELLED_RESULT)
                })
                .count(),
            1
        );
        assert!(cancel_pending_interaction_for_shutdown(&mut app).is_ok());
        assert_eq!(
            app.context_manager
                .get_messages()
                .iter()
                .filter(|message| {
                    message.role == "tool"
                        && message
                            .content
                            .contains(SHUTDOWN_INTERACTION_CANCELLED_RESULT)
                })
                .count(),
            1
        );
    }

    #[test]
    fn checked_pending_interaction_settlement_is_persisted_and_idempotent() {
        let config = Config {
            context_size: 100_000,
            ..Default::default()
        };
        let mut app = App::new(&config);
        app.blocks.clear();
        app.add_logical_turn_user_segment("question".to_string());
        let tool_call = lethetic::context::ToolCall {
            id: "question-checked".to_string(),
            provider_id: None,
            function: lethetic::context::FunctionCall {
                name: "ask_the_user".to_string(),
                arguments: serde_json::json!({"question": "unavailable remotely"}),
            },
        };
        app.context_manager
            .upsert_assistant_tool_call_with_provider("", vec![tool_call.clone()], None);
        app.pending_tool_call = Some(tool_call);
        app.is_asking_user = true;
        app.show_approval_prompt = true;
        app.is_processing = true;
        app.is_executing_tool = true;
        app.tool_call_dispatched = true;
        app.tool_call_pos = Some(4);
        app.tool_output_preview = "private preview".to_string();

        assert_eq!(settle_pending_interaction_checked(&mut app), Ok(true));
        assert!(app.pending_tool_call.is_none());
        assert!(!app.is_asking_user);
        assert!(!app.show_approval_prompt);
        assert!(!app.is_processing);
        assert!(!app.is_executing_tool);
        assert!(!app.tool_call_dispatched);
        assert!(app.tool_call_pos.is_none());
        assert!(app.tool_output_preview.is_empty());
        assert!(app.active_cancellation_id().is_none());
        let results = app
            .context_manager
            .get_messages()
            .iter()
            .filter(|message| {
                message.role == "tool"
                    && message
                        .content
                        .contains(PENDING_INTERACTION_CANCELLED_RESULT)
            })
            .count();
        assert_eq!(results, 1);
        assert_eq!(
            app.blocks
                .iter()
                .filter(|block| block.block_type == BlockType::ToolError)
                .count(),
            1
        );
        assert_eq!(settle_pending_interaction_checked(&mut app), Ok(false));
        assert_eq!(
            app.context_manager
                .get_messages()
                .iter()
                .filter(|message| {
                    message.role == "tool"
                        && message
                            .content
                            .contains(PENDING_INTERACTION_CANCELLED_RESULT)
                })
                .count(),
            1
        );
    }

    #[test]
    fn failed_pending_interaction_save_restores_the_cancellable_state() {
        let mut app = App::new(&Config::default());
        app.add_logical_turn_user_segment("question".to_string());
        let tool_call = lethetic::context::ToolCall {
            id: "question-save-failure".to_string(),
            provider_id: None,
            function: lethetic::context::FunctionCall {
                name: "ask_the_user".to_string(),
                arguments: serde_json::json!({"question": "Continue?"}),
            },
        };
        app.context_manager
            .upsert_assistant_tool_call_with_provider("", vec![tool_call.clone()], None);
        app.pending_tool_call = Some(tool_call);
        app.is_asking_user = true;
        let cancel_id = app.active_cancellation_id().unwrap().to_string();
        let directory = tempfile::tempdir().unwrap();
        app.current_session_dir = Some(directory.path().to_string_lossy().into_owned());
        app.session_directory_binding = None;
        let messages_before = app.context_manager.get_messages().to_vec();
        let blocks_before = app.blocks.clone();

        assert!(settle_pending_interaction_checked(&mut app).is_err());
        assert!(app.is_asking_user);
        assert!(app.pending_tool_call.is_some());
        assert_eq!(app.active_cancellation_id(), Some(cancel_id.as_str()));
        assert_eq!(app.context_manager.get_messages(), messages_before);
        assert_eq!(
            serde_json::to_value(&app.blocks).unwrap(),
            serde_json::to_value(&blocks_before).unwrap()
        );
    }

    #[test]
    fn local_operation_result_never_continues_provider_tool_loop() {
        assert!(!tool_result_has_provider_call_id(&None));
        assert!(tool_result_has_provider_call_id(&Some(
            "provider-tool-call".to_string()
        )));
        assert!(!local_result_owns_execution_state("lsp_install", false));
        assert!(!local_result_owns_execution_state("other_local", true));
        assert!(local_result_owns_execution_state("lsp_install", true));
    }

    #[test]
    fn lsp_cancellation_does_not_mutate_provider_cancellation_state() {
        let mut app = App::new(&Config::default());
        app.lsp_install_in_progress = true;
        app.is_executing_tool = true;
        app.is_processing = true;
        let token = CancellationToken::new();

        assert!(request_lsp_install_cancellation(
            &mut app,
            &token,
            "Cancelling LSP install",
        ));
        assert!(token.is_cancelled());
        assert!(app.lsp_install_cancel_pending);
        let block_count = app.blocks.len();
        assert!(request_lsp_install_cancellation(
            &mut app,
            &token,
            "Still cancelling LSP install",
        ));
        assert_eq!(app.blocks.len(), block_count);

        app.active_request_id = Some("provider-request".to_string());
        app.current_dir = "provider-cwd".to_string();
        app.tool_output_preview = "installer output".to_string();
        assert!(settle_lsp_install_result(&mut app, "lsp_install", false));
        assert!(app.is_processing);
        assert_eq!(app.active_request_id.as_deref(), Some("provider-request"));
        assert_eq!(app.current_dir, "provider-cwd");
        assert!(!app.lsp_install_in_progress);
        assert!(!app.lsp_install_cancel_pending);
        assert!(!app.is_executing_tool);
        assert!(app.tool_output_preview.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn checkpoint_hook_waits_for_durable_acknowledgement() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let hook = interactive_request_checkpoint_hook(&tx, true);
        let checkpoint = lethetic::client::ProviderRequestCheckpoint {
            request: lethetic::accounting::ProviderRequestAccounting {
                request_id: "request-one".to_string(),
                connection_id: "proxy".to_string(),
                model: "test-model".to_string(),
                usage: Default::default(),
                usage_reported: false,
                estimated_cost: None,
                completed: false,
                in_flight: true,
            },
            transcript: None,
        };
        let task = tokio::task::spawn_blocking(move || hook(&checkpoint));

        let StreamEvent::PersistRequestCheckpoint {
            checkpoint,
            acknowledgement,
        } = rx.recv().await.unwrap()
        else {
            panic!("checkpoint hook emitted the wrong event");
        };
        assert_eq!(checkpoint.request.request_id, "request-one");
        assert!(!task.is_finished());
        acknowledgement.send(Ok(())).unwrap();
        assert!(task.await.unwrap().is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn nested_checkpoint_hook_strips_subagent_transcript() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let hook = interactive_request_checkpoint_hook(&tx, false);
        let checkpoint = lethetic::client::ProviderRequestCheckpoint {
            request: lethetic::accounting::ProviderRequestAccounting {
                request_id: "nested-request".to_string(),
                connection_id: "proxy".to_string(),
                model: "test-model".to_string(),
                usage: Default::default(),
                usage_reported: false,
                estimated_cost: None,
                completed: true,
                in_flight: false,
            },
            transcript: Some(vec![lethetic::context::Message {
                role: "assistant".to_string(),
                content: "subagent-only".to_string(),
                tool_calls: None,
                provider_content: None,
                tool_result_is_error: false,
            }]),
        };
        let task = tokio::task::spawn_blocking(move || hook(&checkpoint));

        let StreamEvent::PersistRequestCheckpoint {
            checkpoint,
            acknowledgement,
        } = rx.recv().await.unwrap()
        else {
            panic!("checkpoint hook emitted the wrong event");
        };
        assert!(checkpoint.transcript.is_none());
        acknowledgement.send(Ok(())).unwrap();
        assert!(task.await.unwrap().is_ok());
    }
}
