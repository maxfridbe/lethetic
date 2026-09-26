use lethetic::app::{
    App, AppEventOutcome, BlockType, RenderBlock, handle_tool_call, handle_tool_call_with_provider,
};
use lethetic::client::StreamEvent;
use lethetic::{icons, parser};
use tokio_util::sync::CancellationToken;

use crate::app_events::{
    activate_python_policy, detach_python_policy_runtime, install_python_policy,
};
use crate::context::RuntimeContext;
use crate::formatting::{
    classify_done_reason, looks_like_intention_without_action, truncate_chars_with_ellipsis,
};
use crate::provider::{
    SideEffectKind, dispatch_auto_approved_tool, persist_before_side_effect,
    record_provider_start_failure, settle_lsp_install_result, tool_result_has_provider_call_id,
    trigger_persisted_provider_request,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamControl {
    Continue,
    BreakBatch,
}

fn append_tool_result_block(app: &mut App, content: String, title: String, is_error: bool) {
    let block = RenderBlock::tool_result(content, title, is_error);
    let success = block.success;
    app.add_segment_with_title(
        block.content,
        block.block_type,
        block.title.unwrap_or_else(|| "Action".to_string()),
    );
    if let Some(last) = app.blocks.last_mut() {
        last.success = success;
    }
}

fn settle_provider_cancellation(context: &mut RuntimeContext<'_>) {
    context.app.request_start_time = None;
    context.full_response_content.clear();
    context.app.parser.reset();

    if context.app.is_executing_tool {
        // Provider completion/cancellation and tool containment are independent.
        // Keep the shutdown gate closed until the tool emits its terminal result.
        context.app.is_processing = true;
        context.app.stop_reason =
            "Provider stopped; waiting for tool cancellation containment…".to_string();
    } else {
        context.cancellation_pending = false;
        context.app.is_processing = false;
        context.app.tool_calls_processed_this_request = false;
        context.app.tool_call_dispatched = false;
        context.app.pending_tool_call = None;
        context.app.settle_logical_turn();
        context.app.stop_reason = "Cancelled by user".to_string();
        context
            .app
            .add_segment(format!("\n{} [STOPPED]\n", icons::WARNING), BlockType::Text);
    }
    context.app.save_session();
    context.app.should_redraw = true;
}

fn handle_request_settlement_failure(
    context: &mut RuntimeContext<'_>,
    request_id: &str,
    error: String,
    cancellation_requested: bool,
) {
    let owns_active_request = context.app.active_request_id.as_deref() == Some(request_id);
    let marker_closed = context.app.close_provider_request_marker(request_id);
    let phase = if cancellation_requested {
        "cancellation"
    } else {
        "terminal"
    };

    if !owns_active_request {
        context.app.log_debug(&format!(
            "STALE_PROVIDER_SETTLEMENT_FAILED: {request_id}: {error}"
        ));
        if marker_closed {
            context.app.add_segment(
                format!(
                    "\n{} PROVIDER {phase_upper} CHECKPOINT ERROR FOR AN EARLIER REQUEST: {error}\n",
                    icons::WARNING,
                    phase_upper = phase.to_ascii_uppercase(),
                ),
                BlockType::ProviderError,
            );
            context.app.should_redraw = true;
            context.app.save_session();
        }
        if context.lifecycle.is_shutting_down() {
            context
                .lifecycle
                .record_error(format!("provider request settlement failed: {error}"));
        }
        return;
    }

    if let Err(commit_error) = context.app.commit_partial_assistant_checkpoint() {
        context
            .app
            .log_debug(&format!("PARTIAL_TRANSCRIPT_COMMIT_ERROR: {commit_error}"));
    }
    context.app.request_start_time = None;
    let tool_remains_active = context.app.is_executing_tool;

    if context.cancellation_pending {
        settle_provider_cancellation(context);
    } else if tool_remains_active {
        // Nested summarization, vision/PDF, and sub-agent provider requests run
        // inside the outer tool. Settling their exact request marker must not
        // falsely settle or erase that still-running tool.
        context.app.is_processing = true;
    } else {
        context.app.is_processing = false;
        context.app.pending_tool_call = None;
        context.app.settle_logical_turn();
        context.full_response_content.clear();
        context.app.parser.reset();
    }

    context.app.stop_reason = format!(
        "✗ Provider {phase} checkpoint failed: {}",
        truncate_chars_with_ellipsis(&error, 80, 77)
    );
    context.app.add_segment(
        format!("\n{} PROVIDER CHECKPOINT ERROR: {error}\n", icons::WARNING),
        BlockType::ProviderError,
    );
    if context.lifecycle.is_shutting_down() {
        context
            .lifecycle
            .record_error(format!("provider request settlement failed: {error}"));
    }
    context.app.should_redraw = true;
    context.app.save_session();
}

#[derive(Debug)]
struct AppliedPolicy {
    profile: lethetic::config::ToolProfile,
    source: lethetic::python_policy::PythonPolicySource,
    durability_warning: Option<String>,
}

fn policy_scope(
    persistence: lethetic::python_setup::PolicyPersistence,
) -> Option<lethetic::python_policy::PythonPolicyScope> {
    match persistence {
        lethetic::python_setup::PolicyPersistence::OneTime => None,
        lethetic::python_setup::PolicyPersistence::Project => {
            Some(lethetic::python_policy::PythonPolicyScope::Project)
        }
        lethetic::python_setup::PolicyPersistence::Global => {
            Some(lethetic::python_policy::PythonPolicyScope::Global)
        }
    }
}

fn resolve_effective_policy(
    scope: lethetic::python_policy::PythonPolicyScope,
    workspace: &std::path::Path,
    requested: &lethetic::python_policy::PythonPolicySnapshot,
) -> Result<
    (
        lethetic::python_policy::PythonPolicySnapshot,
        lethetic::python_policy::PythonPolicySource,
    ),
    String,
> {
    lethetic::python_policy::effective_policy_after_write(scope, workspace, requested)
}

fn requested_policy_is_visible(
    scope: lethetic::python_policy::PythonPolicyScope,
    workspace: &std::path::Path,
    requested: &lethetic::python_policy::PythonPolicySnapshot,
) -> Result<bool, String> {
    let path = lethetic::python_policy::policy_path(scope, workspace);
    Ok(lethetic::python_policy::load_policy(&path)?
        .is_some_and(|loaded| loaded.snapshot == *requested))
}

async fn apply_prepared_python_policy_with_persistence<P>(
    context: &mut RuntimeContext<'_>,
    requested: lethetic::python_policy::PythonPolicySnapshot,
    prepared_effective: lethetic::python_policy::PythonPolicySnapshot,
    prepared_source: lethetic::python_policy::PythonPolicySource,
    persistence: lethetic::python_setup::PolicyPersistence,
    expected_revision: Option<lethetic::python_policy::PolicyRevision>,
    persist: P,
) -> Result<AppliedPolicy, String>
where
    P: FnOnce(
        lethetic::python_policy::PythonPolicyScope,
        &std::path::Path,
        &lethetic::python_policy::PythonPolicySnapshot,
        Option<&lethetic::python_policy::PolicyRevision>,
    ) -> Result<lethetic::python_policy::PolicyRevision, String>,
{
    context.app.python_policy.ensure_ui_mutable()?;
    let Some(scope) = policy_scope(persistence) else {
        let profile = prepared_effective.tool_profile;
        activate_python_policy(
            context.app,
            context.config,
            prepared_effective,
            prepared_source,
            false,
        )
        .await?;
        return Ok(AppliedPolicy {
            profile,
            source: prepared_source,
            durability_warning: None,
        });
    };

    let workspace = context.app.tool_runtime.workspace_root().to_path_buf();
    let (effective_before_detach, source_before_detach) =
        resolve_effective_policy(scope, &workspace, &requested)?;
    if effective_before_detach != prepared_effective || source_before_detach != prepared_source {
        return Err(
            "Python policy precedence changed during backend validation; reopen Agent Mode"
                .to_string(),
        );
    }

    // The destination remains untouched until reset/reconciliation succeeds.
    detach_python_policy_runtime(context.app).await?;

    let (effective_after_detach, source_after_detach) =
        resolve_effective_policy(scope, &workspace, &requested)?;
    if effective_after_detach != prepared_effective || source_after_detach != prepared_source {
        return Err(
            "Python policy precedence changed while detaching the runtime; policy was not written"
                .to_string(),
        );
    }

    match persist(scope, &workspace, &requested, expected_revision.as_ref()) {
        Ok(_) => {
            let profile = effective_after_detach.tool_profile;
            install_python_policy(
                context.app,
                context.config,
                effective_after_detach,
                source_after_detach,
                true,
            )?;
            Ok(AppliedPolicy {
                profile,
                source: source_after_detach,
                durability_warning: None,
            })
        }
        Err(error) => {
            // Atomic writers can report a directory-sync error after rename.
            // If the requested destination is visible, converge memory to disk
            // and report uncertain durability instead of claiming no change.
            if requested_policy_is_visible(scope, &workspace, &requested).unwrap_or(false) {
                let (visible_effective, visible_source) =
                    resolve_effective_policy(scope, &workspace, &requested)?;
                let profile = visible_effective.tool_profile;
                install_python_policy(
                    context.app,
                    context.config,
                    visible_effective,
                    visible_source,
                    true,
                )?;
                Ok(AppliedPolicy {
                    profile,
                    source: visible_source,
                    durability_warning: Some(error),
                })
            } else {
                Err(format!(
                    "Python policy was not committed after runtime detach: {error}"
                ))
            }
        }
    }
}

async fn apply_prepared_python_policy(
    context: &mut RuntimeContext<'_>,
    requested: lethetic::python_policy::PythonPolicySnapshot,
    prepared_effective: lethetic::python_policy::PythonPolicySnapshot,
    prepared_source: lethetic::python_policy::PythonPolicySource,
    persistence: lethetic::python_setup::PolicyPersistence,
    expected_revision: Option<lethetic::python_policy::PolicyRevision>,
) -> Result<AppliedPolicy, String> {
    apply_prepared_python_policy_with_persistence(
        context,
        requested,
        prepared_effective,
        prepared_source,
        persistence,
        expected_revision,
        |scope, workspace, snapshot, expected_revision| {
            lethetic::python_policy::persist_policy(scope, workspace, snapshot, expected_revision)
        },
    )
    .await
}

pub(crate) async fn handle_stream_event(
    context: &mut RuntimeContext<'_>,
    stream_event: StreamEvent,
) -> StreamControl {
    match stream_event {
        StreamEvent::CompactionChunk(text) => {
            crate::compaction::append_chunk(context.app, &text);
        }
        StreamEvent::CompactionFinished {
            source_session_id,
            result,
        } => {
            crate::compaction::finish(context.app, &source_session_id, result);
        }
        StreamEvent::ModelCatalogReady {
            connection_id,
            result,
        } => {
            crate::model_catalog::ready(context.app, context.config, connection_id, result);
        }
        StreamEvent::ModelsReady(models) => {
            context.app.available_models = models;
            if !context.app.available_models.is_empty() {
                context.app.model_switcher_state.select(Some(0));
            }
            context.app.should_redraw = true;
        }
        StreamEvent::PythonCapabilities(capabilities) => {
            if let Some(setup) = context.app.python_setup.as_mut()
                && setup.stage == lethetic::python_setup::PythonSetupStage::Probing
            {
                setup.set_capabilities(capabilities);
                context.app.stop_reason = "Python backend probe complete".to_string();
                context.app.should_redraw = true;
            }
        }
        StreamEvent::PythonPolicyPrepared {
            snapshot,
            effective_snapshot,
            effective_source,
            persistence,
            expected_revision,
            validation,
        } => {
            if context.app.python_setup.as_ref().is_some_and(|setup| {
                setup.stage == lethetic::python_setup::PythonSetupStage::Applying
            }) {
                let preparation = validation.and_then(|()| {
                    let setup = context
                        .app
                        .python_setup
                        .as_ref()
                        .ok_or_else(|| "Python setup dialog is no longer active".to_string())?;
                    if setup.snapshot() != snapshot
                        || setup.persistence != persistence
                        || setup.expected_revision() != expected_revision
                    {
                        return Err("Python setup changed during backend validation".to_string());
                    }
                    snapshot.validate()?;
                    effective_snapshot.validate()?;
                    if persistence == lethetic::python_setup::PolicyPersistence::OneTime
                        && (effective_snapshot != snapshot
                            || effective_source
                                != lethetic::python_policy::PythonPolicySource::OneTime)
                    {
                        return Err(
                            "One-time Python policy changed during backend validation".to_string()
                        );
                    }
                    Ok(())
                });

                match preparation {
                    Ok(()) => match apply_prepared_python_policy(
                        context,
                        snapshot,
                        effective_snapshot,
                        effective_source,
                        persistence,
                        expected_revision,
                    )
                    .await
                    {
                        Ok(AppliedPolicy {
                            profile,
                            source,
                            durability_warning,
                        }) => {
                            context.app.python_setup = None;
                            let precedence_note = if persistence
                                == lethetic::python_setup::PolicyPersistence::Global
                                && source == lethetic::python_policy::PythonPolicySource::Project
                            {
                                " Global policy saved; the project policy remains the effective override."
                            } else {
                                ""
                            };
                            if let Some(warning) = durability_warning {
                                context.app.stop_reason = format!(
                                    "⚠ Agent Mode: {profile:?} ({}); policy visibility confirmed but durability is uncertain: {warning}",
                                    source.label()
                                );
                                context.app.add_segment(
                                    format!(
                                        "\n{} Agent Mode updated to {profile:?} ({}).{precedence_note} Policy visibility was confirmed, but durable storage could not be confirmed: {warning}\n",
                                        icons::WARNING,
                                        source.label()
                                    ),
                                    BlockType::Text,
                                );
                            } else {
                                context.app.stop_reason = format!(
                                    "Agent Mode: {profile:?} ({}){precedence_note}",
                                    source.label()
                                );
                                context.app.add_segment(
                                    format!(
                                        "\n{} Agent Mode updated to {profile:?} ({}).{precedence_note}\n",
                                        icons::SUCCESS,
                                        source.label()
                                    ),
                                    BlockType::Text,
                                );
                            }
                        }
                        Err(error) => {
                            if let Some(setup) = context.app.python_setup.as_mut() {
                                setup.stage = lethetic::python_setup::PythonSetupStage::Confirm;
                                setup.error = Some(error.clone());
                            }
                            context.app.stop_reason =
                                format!("⚠ Python policy was not applied: {error}");
                        }
                    },
                    Err(error) => {
                        if let Some(setup) = context.app.python_setup.as_mut() {
                            setup.stage = lethetic::python_setup::PythonSetupStage::Confirm;
                            setup.error = Some(error.clone());
                        }
                        context.app.stop_reason = format!("⚠ {error}");
                    }
                }
                context.app.should_redraw = true;
            }
        }
        StreamEvent::PythonPullFinished(result) => {
            if let Some(setup) = context.app.python_setup.as_mut()
                && setup.stage == lethetic::python_setup::PythonSetupStage::Pulling
            {
                context.app.tool_output_preview.clear();
                match result {
                    Ok(capabilities) => {
                        setup.set_capabilities(capabilities);
                        context.app.stop_reason =
                            "Podman image pulled; backend probe refreshed".to_string();
                    }
                    Err(error) => {
                        setup.stage = setup
                            .previous_stage
                            .take()
                            .unwrap_or(lethetic::python_setup::PythonSetupStage::PodmanImage);
                        setup.error = Some(error.clone());
                        context.app.stop_reason = format!("⚠ {error}");
                    }
                }
                context.app.should_redraw = true;
            }
        }
        #[cfg(target_os = "linux")]
        StreamEvent::RuntimeMaintenanceFinished(result) => match result {
            Ok(report) => {
                for error in &report.errors {
                    context
                        .app
                        .log_debug(&format!("retained runtime maintenance: {error}"));
                }
                if !report.errors.is_empty() {
                    context.app.stop_reason = format!(
                        "⚠ Python runtime maintenance reported {} error(s)",
                        report.errors.len()
                    );
                    context.app.should_redraw = true;
                } else if report.reconciled > 0 || report.removed > 0 {
                    context.app.stop_reason = format!(
                        "Python runtime maintenance: {} reconciled, {} expired removed",
                        report.reconciled, report.removed
                    );
                    context.app.should_redraw = true;
                }
            }
            Err(error) => {
                context
                    .app
                    .log_debug(&format!("retained runtime maintenance failed: {error}"));
                context.app.stop_reason = format!("⚠ Python runtime maintenance failed: {error}");
                context.app.should_redraw = true;
            }
        },
        StreamEvent::DebugLog(msg) => {
            if msg.starts_with("STATS|") {
                let parts: Vec<&str> = msg.split('|').collect();
                if parts.len() == 3 {
                    context.app.memory_usage = parts[1].parse().unwrap_or(0);
                    context.app.git_status = parts[2].to_string();
                    context.app.should_redraw = true;
                }
            } else if let Some(dir) = msg.strip_prefix("DIR_UPDATE|") {
                context.app.current_dir = dir.to_string();
                context.app.should_redraw = true;
            } else {
                context.app.log_debug(&msg);
            }
        }
        StreamEvent::TokenUpdate(count, ms) => {
            if ms > 0.0 {
                context.app.tokens_per_s = (count as f64 / (ms / 1000.0)).max(0.0);
                context.app.should_redraw = true;
            } else if let Some(start) = context.app.request_start_time {
                let elapsed = start.elapsed().as_secs_f64();
                if elapsed > 0.0 {
                    context.app.tokens_per_s = (count as f64 / elapsed).max(0.0);
                    context.app.should_redraw = true;
                }
            }
        }
        StreamEvent::Chunk(chunk) => {
            if context.app.is_processing && !context.cancellation_pending {
                context.full_response_content.push_str(&chunk);
                context.app.should_redraw = true;

                let segments = context.app.parser.parse_chunk(&chunk);
                for (b_type, content) in segments {
                    context.app.add_segment(content, b_type);

                    // Check for loops after adding content
                    if let Some(detection) = context
                        .app
                        .loop_detector
                        .check(&context.app.last_block_content)
                    {
                        context
                            .app
                            .log_debug(&format!("LOOP DETECTED: {}", detection.reason));
                        context.cancellation_token.cancel();
                        context.cancellation_pending = true;
                        let mut loop_msg = format!(
                            "\n{} [LOOP DETECTED] {}\n",
                            icons::WARNING,
                            detection.reason
                        );
                        if let Some(sample) = detection.sample {
                            loop_msg.push_str(&format!(
                                "{} Sample: \"{}\"\n",
                                icons::DEBUG,
                                sample
                            ));
                        }
                        context.app.add_segment(loop_msg, BlockType::Text);
                        context.app.stop_reason = "Containing looped provider request…".to_string();
                        context.app.last_loop_detection_time = Some(std::time::Instant::now());
                        context.app.loop_detection_count =
                            context.app.loop_detection_count.saturating_add(1);
                        break;
                    }
                }
                #[cfg(target_os = "linux")]
                if let Err(error) = context
                    .app
                    .persist_partial_assistant_checkpoint(context.full_response_content.clone())
                {
                    context.cancellation_token.cancel();
                    context.cancellation_pending = true;
                    context.app.stop_reason =
                        format!("✗ Streaming transcript checkpoint failed: {error}");
                    context.app.add_segment(
                        format!(
                            "\n{} SESSION SAVE ERROR: streaming response was contained: {error}\n",
                            icons::WARNING
                        ),
                        BlockType::ProviderError,
                    );
                }
            }
        }
        StreamEvent::ToolCalls {
            calls,
            provider_content,
        } => {
            if context.cancellation_pending {
                for call in &calls {
                    if call.function.name != "python" {
                        continue;
                    }
                    let source = call.function.arguments["code"].as_str().unwrap_or("");
                    let description = call.function.arguments["description"]
                        .as_str()
                        .unwrap_or("Python cell");
                    let session_directory = context
                        .app
                        .current_session_dir
                        .as_deref()
                        .map(std::path::Path::new);
                    let audit_call_id = call.provider_id.as_deref().unwrap_or(&call.id);
                    if context
                        .app
                        .tool_runtime
                        .begin_python_audit_attempt(
                            session_directory,
                            &context.app.config,
                            audit_call_id,
                            source,
                            description,
                            &context.app.current_dir,
                        )
                        .is_ok()
                    {
                        let _ = context.app.tool_runtime.mark_python_audit_status(
                            audit_call_id,
                            lethetic::python::notebook::NotebookAttemptStatus::Cancelled,
                            Some("Provider turn was cancelled before Python dispatch."),
                        );
                        if let Some(path) = context.app.tool_runtime.take_python_notebook_notice() {
                            let _ = context.tx.send(StreamEvent::PythonNotebookNotice(path));
                        }
                    }
                }
            } else if !context.app.tool_calls_processed_this_request
                && let AppEventOutcome::ToolApproved(..) = handle_tool_call_with_provider(
                    context.app,
                    calls,
                    context.full_response_content.len(),
                    context.tx.clone(),
                    &mut context.cancellation_token,
                    &context.full_response_content,
                    true,
                    provider_content,
                )
            {
                dispatch_auto_approved_tool(
                    context.app,
                    &context.tx,
                    &context.cancellation_token,
                    &context.client,
                    context.config,
                );
            }
        }
        StreamEvent::ToolResult {
            id,
            func_name,
            result,
            cwd,
            is_error,
            provenance,
        } => {
            let presentation_id = id.as_deref().unwrap_or("local_operation");
            let presented = lethetic::tools::present_tool_execution(
                presentation_id,
                lethetic::tools::ToolExecution {
                    output: result,
                    cwd,
                    is_error,
                    provenance,
                },
            );
            if !tool_result_has_provider_call_id(&id) {
                let success = !presented.is_error;
                let description = if func_name == "lsp_install" {
                    "Install LSP server"
                } else {
                    "Local operation"
                };
                append_tool_result_block(
                    context.app,
                    format!("\n{}\n", presented.ui),
                    description.to_string(),
                    presented.is_error,
                );
                settle_lsp_install_result(context.app, &func_name, success);
                context.app.save_session();
                context.app.should_redraw = true;
            } else {
                context.app.is_executing_tool = false;
                context.app.tool_output_preview.clear();
                context.app.current_dir = presented.cwd;

                let tool_args = context
                    .app
                    .pending_tool_call
                    .as_ref()
                    .map(|tc| tc.function.arguments.clone())
                    .unwrap_or(serde_json::json!({}));
                let success = !presented.is_error;
                let mut full_result = presented.context;
                let ui_result = presented.ui;

                let description = context
                    .app
                    .pending_tool_call
                    .as_ref()
                    .and_then(|tc| tc.function.arguments["description"].as_str())
                    .unwrap_or("Action")
                    .to_string();

                append_tool_result_block(
                    context.app,
                    format!("\n{}\n", ui_result),
                    description,
                    presented.is_error,
                );

                if let Some(tc_id) = id {
                    context.app.pending_tool_call.take();

                    if success {
                        if func_name == "read_file" {
                            if let Some(path) = tool_args["path"].as_str() {
                                let full_path =
                                    std::path::Path::new(&context.app.current_dir).join(path);
                                if let Ok(content) = std::fs::read_to_string(&full_path) {
                                    context
                                        .app
                                        .context_manager
                                        .update_latest_file(path.to_string(), content);
                                    context.app.add_segment(
                                        format!(
                                            "\n{} File `{}` has been placed in context.\n",
                                            icons::SUCCESS,
                                            path
                                        ),
                                        BlockType::Text,
                                    );
                                    full_result = "[File read successfully. Contents are now available in your Latest Files context.]".to_string();
                                }
                            }
                        } else if func_name == "write_file" {
                            if let Some(path) = tool_args["path"].as_str()
                                && let Some(content) = tool_args["content"].as_str()
                            {
                                context
                                    .app
                                    .context_manager
                                    .update_latest_file(path.to_string(), content.to_string());
                                context.app.add_segment(
                                    format!(
                                        "\n{} File `{}` has been placed in context.\n",
                                        icons::SUCCESS,
                                        path
                                    ),
                                    BlockType::Text,
                                );
                            }
                        } else if func_name == "apply_patch"
                            && full_result.contains("Successfully patched")
                        {
                            if let Some(path) = tool_args["file_path"].as_str() {
                                let full_path =
                                    std::path::Path::new(&context.app.current_dir).join(path);
                                if let Ok(content) = std::fs::read_to_string(&full_path) {
                                    context
                                        .app
                                        .context_manager
                                        .update_latest_file(path.to_string(), content);
                                    context.app.add_segment(
                                        format!(
                                            "\n{} File `{}` has been updated in context.\n",
                                            icons::SUCCESS,
                                            path
                                        ),
                                        BlockType::Text,
                                    );
                                }
                            }
                        } else if func_name == "replace_text"
                            && full_result.contains("Successfully replaced")
                            && let Some(path) = tool_args["path"].as_str()
                        {
                            let full_path =
                                std::path::Path::new(&context.app.current_dir).join(path);
                            if let Ok(content) = std::fs::read_to_string(&full_path) {
                                context
                                    .app
                                    .context_manager
                                    .update_latest_file(path.to_string(), content);
                                context.app.add_segment(
                                    format!(
                                        "\n{} File `{}` has been updated in context.\n",
                                        icons::SUCCESS,
                                        path
                                    ),
                                    BlockType::Text,
                                );
                            }
                        }
                    }

                    // Track successfully applied edits for "already applied" detection
                    if (func_name == "edit" || func_name == "replace_text")
                        && full_result.contains("Successfully")
                    {
                        let old_str = tool_args["old_string"].as_str().unwrap_or("").to_string();
                        if !old_str.is_empty() {
                            context.app.applied_edits.insert(old_str);
                        }
                    }

                    // Detect "already applied" — edit fails with "not found" but we already applied it
                    if (func_name == "edit" || func_name == "replace_text")
                        && full_result.contains("not found")
                    {
                        let old_str = tool_args["old_string"].as_str().unwrap_or("");
                        if !old_str.is_empty() && context.app.applied_edits.contains(old_str) {
                            let msg = "⚠ EDIT ALREADY APPLIED: This exact `old_string` was successfully replaced in a prior call. \
                     The file already contains your updated version. \
                     Do not retry this edit — move on to the next issue.".to_string();
                            context.app.context_manager.add_message("user", &msg);
                            context.app.add_segment("\n⚠ [EDIT ALREADY APPLIED] old_string was replaced earlier this session — move on.\n".to_string(), BlockType::Text);
                            full_result = msg;
                        }
                    }

                    context.app.context_manager.add_tool_message_with_status(
                        tc_id,
                        &func_name,
                        &full_result,
                        presented.is_error,
                    );
                }

                // Duplicate tool call detection: same (tool, key-args) called 2+ times for edit/replace_text, 3+ for others
                {
                    let path = tool_args["path"]
                        .as_str()
                        .or_else(|| tool_args["file_path"].as_str())
                        .unwrap_or("");
                    let fingerprint = match func_name.as_str() {
                        "read_file" => format!("read_file:{}", path),
                        "read_file_lines" => format!(
                            "read_file_lines:{}:{}-{}",
                            path,
                            tool_args["start_line"].as_u64().unwrap_or(0),
                            tool_args["end_line"].as_u64().unwrap_or(0)
                        ),
                        "search_text" => format!(
                            "search_text:{}:{}",
                            tool_args["pattern"].as_str().unwrap_or(""),
                            path
                        ),
                        "run_shell_command" => format!(
                            "run_shell_command:{}",
                            tool_args["command"].as_str().unwrap_or("")
                        ),
                        other => format!(
                            "{}:{}",
                            other,
                            serde_json::to_string(&tool_args).unwrap_or_default()
                        ),
                    };
                    let count = {
                        let c = context
                            .app
                            .tool_call_fingerprints
                            .entry(fingerprint)
                            .or_insert(0);
                        *c += 1;
                        *c
                    };
                    let dup_threshold = match func_name.as_str() {
                        "edit" | "replace_text" => 2,
                        "run_shell_command" => {
                            let cmd = tool_args["command"].as_str().unwrap_or("");
                            if cmd.contains("rm ")
                                || cmd.contains("unlink ")
                                || cmd.contains(" mv ")
                                || cmd.contains("del ")
                            {
                                2
                            } else {
                                3
                            }
                        }
                        _ => 3,
                    };
                    if count >= dup_threshold {
                        let path_hint = tool_args["path"]
                            .as_str()
                            .or_else(|| tool_args["file_path"].as_str())
                            .unwrap_or("this file");
                        let hint = match func_name.as_str() {
                            "read_file" | "read_file_lines" => format!(
                                "You have called `{}` on `{}` {} times and received the same result. \
                     The file may be too large for this approach. Try: \
                     `search_text` with a specific pattern to locate the code you need, \
                     `read_file_lines` with a narrower range (50–100 lines at a time), \
                     or `summarize_content` with the file path for an overview.",
                                func_name, path_hint, count
                            ),
                            "search_text" => format!(
                                "You have run this search {} times and received the same result. \
                     Try a more specific pattern or use `find_symbol` for definition/reference lookup.",
                                count
                            ),
                            other => format!(
                                "You have called `{}` with identical parameters {} times. \
                     Try a different approach or a different tool.",
                                other, count
                            ),
                        };
                        let warn = format!("⚠ DUPLICATE TOOL CALL: {}", hint);
                        context.app.context_manager.add_message("user", &warn);
                        context.app.add_segment(
                            format!("\n⚠ [DUPLICATE TOOL CALL x{}] {}\n", count, hint),
                            BlockType::Text,
                        );
                    }
                }

                if context.cancellation_pending {
                    context.app.tool_calls_processed_this_request = false;
                    context.app.tool_call_dispatched = false;
                    context.app.tool_call_pos = None;
                    context.app.request_start_time = None;
                    context.full_response_content.clear();
                    if context.app.active_request_id.is_some() {
                        context.app.stop_reason =
                            "Tool stopped; waiting for provider cancellation containment…"
                                .to_string();
                    } else {
                        context.cancellation_pending = false;
                        context.app.is_processing = false;
                        context.app.settle_logical_turn();
                        context.app.stop_reason = "Cancelled by user".to_string();
                        context.app.add_segment(
                            format!("\n{} [STOPPED]\n", icons::WARNING),
                            BlockType::Text,
                        );
                    }
                    context.app.save_session();
                    context.app.should_redraw = true;
                } else {
                    context.app.is_processing = true;
                    context.app.tool_calls_processed_this_request = false;
                    context.app.tool_call_dispatched = false;
                    context.app.tool_call_pos = None;
                    context.app.server_prompt_tokens = None;
                    context.app.server_completion_tokens = None;
                    context.app.server_usage = None;
                    context.full_response_content.clear();
                    context.cancellation_token = CancellationToken::new();
                    context.app.request_start_time = Some(tokio::time::Instant::now());
                    context
                        .app
                        .context_manager
                        .set_cwd(context.app.current_dir.clone());
                    context.app.parser.reset();
                    if persist_before_side_effect(
                        context.app,
                        "continue the provider request after the tool result",
                        SideEffectKind::Provider,
                    ) && let Err(error) = trigger_persisted_provider_request(
                        context.app,
                        &context.client,
                        context.config,
                        &context.tx,
                        &context.cancellation_token,
                    ) {
                        record_provider_start_failure(context.app, "Provider continuation", &error);
                    }
                }
            }
        }
        StreamEvent::PreparingToolCall(name) => {
            context.app.stop_reason = format!(
                "Lethetic Intelligence Engine Processing (Preparing tool call: {})…",
                name
            );
            context.app.should_redraw = true;
        }
        StreamEvent::ToolProgress(msg) => {
            context.app.tool_output_preview = msg;
            context.app.should_redraw = true;
        }
        StreamEvent::TodoUpdated(snapshot) => {
            let active = snapshot
                .todos
                .iter()
                .filter(|todo| {
                    matches!(
                        todo.status,
                        lethetic::todo_store::TodoStatus::Pending
                            | lethetic::todo_store::TodoStatus::InProgress
                    )
                })
                .count();
            let message = format!(
                "Todo list refreshed via lethetic_todo: {} task(s), {active} active (revision {})",
                snapshot.todos.len(),
                snapshot.revision
            );
            context.app.tool_output_preview = message.clone();
            context
                .app
                .add_segment(format!("\n{message}\n"), BlockType::Text);
            context.app.should_redraw = true;
        }
        StreamEvent::PythonRuntimeNotice(notice) => {
            let message = notice.render();
            context.app.tool_output_preview = message.clone();
            context
                .app
                .add_segment(format!("\n{message}\n"), BlockType::Text);
            context.app.should_redraw = true;
        }
        StreamEvent::PythonNotebookNotice(path) => {
            let message = format!("Python notebook audit: {}", path.display());
            context.app.tool_output_preview = message.clone();
            context
                .app
                .add_segment(format!("\n{message}\n"), BlockType::Text);
            context.app.should_redraw = true;
        }
        StreamEvent::PersistRequestCheckpoint {
            checkpoint,
            acknowledgement,
        } => {
            let request_id = checkpoint.request.request_id.clone();
            let result = context
                .app
                .persist_provider_checkpoint(&checkpoint)
                .map(|_| ());
            if let Err(error) = &result {
                context
                    .app
                    .log_debug(&format!("REQUEST_CHECKPOINT_ERROR: {error}"));
                context.app.stop_reason =
                    format!("✗ Provider checkpoint could not be persisted: {error}");
                // The actor owns the durable checkpoint and therefore also owns
                // failure settlement. Settle here before acknowledging the hook;
                // a nested provider's private event receiver may disappear as soon
                // as this acknowledgement is delivered.
                handle_request_settlement_failure(
                    context,
                    &request_id,
                    error.clone(),
                    context.cancellation_pending,
                );
            }
            let _ = acknowledgement.send(result);
            context.app.should_redraw = true;
        }
        StreamEvent::RequestStarted(request) => {
            #[cfg(target_os = "linux")]
            context
                .app
                .log_debug(&format!("REQUEST_STARTED_EVENT: {}", request.request_id));
            #[cfg(not(target_os = "linux"))]
            {
                context
                    .app
                    .begin_provider_request(request.request_id.clone());
                if let Err(error) = context.app.record_provider_request(request) {
                    context
                        .app
                        .log_debug(&format!("ACCOUNTING_START_ERROR: {error}"));
                }
                context.app.should_redraw = true;
            }
        }
        StreamEvent::RequestFinished(request) => {
            #[cfg(target_os = "linux")]
            context
                .app
                .log_debug(&format!("REQUEST_FINISHED_EVENT: {}", request.request_id));
            #[cfg(not(target_os = "linux"))]
            {
                if let Err(error) = context.app.record_provider_request(request) {
                    context.app.log_debug(&format!("ACCOUNTING_ERROR: {error}"));
                }
                context.app.should_redraw = true;
            }
        }
        StreamEvent::RequestSettlementFailed {
            request_id,
            error,
            cancellation_requested,
        } => {
            handle_request_settlement_failure(context, &request_id, error, cancellation_requested);
        }
        StreamEvent::RequestCancelled { request_id } => {
            if let Err(error) = context.app.commit_partial_assistant_checkpoint() {
                context
                    .app
                    .log_debug(&format!("PARTIAL_TRANSCRIPT_COMMIT_ERROR: {error}"));
            }
            context.app.close_provider_request_marker(&request_id);
            if context.app.active_request_id.as_deref() == Some(request_id.as_str()) {
                context.app.active_request_id = None;
            }
            context
                .app
                .log_debug(&format!("REQUEST_CANCELLED: {request_id}"));
            if context.cancellation_pending {
                settle_provider_cancellation(context);
            }
        }
        StreamEvent::UsageUpdate { request_id, usage } => {
            if context.app.update_request_usage(&request_id, usage) {
                context.app.should_redraw = true;
            }
        }
        StreamEvent::Done {
            request_id,
            completion_tokens,
            prompt_tokens,
            usage: _,
            tg_per_s,
            pp_per_s,
            stop_reason: provider_stop_reason,
            provider_content,
        } => {
            if context.cancellation_pending {
                context
                    .app
                    .log_debug("REQUEST_COMPLETED_DURING_CANCELLATION");
                if context.app.active_request_id.as_deref() == Some(request_id.as_str()) {
                    context.app.active_request_id = None;
                }
                settle_provider_cancellation(context);
            } else {
                context.app.is_processing = false;
                if !context.config.active_connection_kind().uses_native_tools()
                    && (context.app.parser.state == lethetic::parser::ParserState::Text
                        || context.app.parser.state == lethetic::parser::ParserState::ToolCall)
                    && !context.app.tool_calls_processed_this_request
                {
                    match parser::find_tool_call(&context.full_response_content, true) {
                        Some(Ok((tool_call, position))) => {
                            if let AppEventOutcome::ToolApproved(..) = handle_tool_call(
                                context.app,
                                vec![tool_call],
                                position,
                                context.tx.clone(),
                                &mut context.cancellation_token,
                                &context.full_response_content,
                                false,
                            ) {
                                dispatch_auto_approved_tool(
                                    context.app,
                                    &context.tx,
                                    &context.cancellation_token,
                                    &context.client,
                                    context.config,
                                );
                            }
                        }
                        Some(Err((error, _))) => {
                            context
                                .app
                                .log_debug(&format!("Tool call syntax error: {error}"));
                            context.app.tool_calls_processed_this_request = true;
                            context
                                .app
                                .context_manager
                                .upsert_assistant_message(&context.full_response_content, None);
                            context.app.is_processing = true;
                            let _ = context.tx.send(StreamEvent::ToolResult {
                                id: Some("raw_call".to_string()),
                                func_name: "syntax_error".to_string(),
                                result: format!("Syntax Error in tool call: {error}"),
                                cwd: context.app.current_dir.clone(),
                                is_error: true,
                                provenance: lethetic::tools::ToolOutputProvenance::OrdinaryHost,
                            });
                        }
                        None => {}
                    }
                }

                if !context.app.tool_calls_processed_this_request {
                    context
                        .app
                        .context_manager
                        .upsert_assistant_message(&context.full_response_content, provider_content);

                    // Detect "intention text": model described an action in plain text
                    // but never issued a tool call. Re-prompt once so it acts.
                    if looks_like_intention_without_action(&context.full_response_content) {
                        context.app.log_debug("INTENT_TEXT_DETECTED: model described action without tool call — re-prompting");
                        context.app.stop_reason =
                            "→ Described action without tool call — re-prompting".to_string();
                        context.app.context_manager.add_message("user", "You described an action but did not call a tool. Please call the appropriate tool now.");
                        context.app.is_processing = true;
                        context.app.tool_calls_processed_this_request = false;
                        context.app.tool_call_dispatched = false;
                        context.app.server_prompt_tokens = None;
                        context.app.server_completion_tokens = None;
                        context.app.server_usage = None;
                        context.full_response_content.clear();
                        context.cancellation_token = CancellationToken::new();
                        context.app.parser.reset();
                        if persist_before_side_effect(
                            context.app,
                            "send the automatic provider follow-up",
                            SideEffectKind::Provider,
                        ) && let Err(error) = trigger_persisted_provider_request(
                            context.app,
                            &context.client,
                            context.config,
                            &context.tx,
                            &context.cancellation_token,
                        ) {
                            record_provider_start_failure(
                                context.app,
                                "Automatic provider follow-up",
                                &error,
                            );
                        }
                        return StreamControl::BreakBatch;
                    }

                    // Set heuristic stop reason for normal / degenerate completion
                    context.app.stop_reason = classify_done_reason(
                        completion_tokens,
                        prompt_tokens,
                        &context.full_response_content,
                        false,
                        context.app.max_tokens,
                    );
                } else if let Some(tool_name) = context
                    .app
                    .pending_tool_call
                    .as_ref()
                    .map(|tool_call| tool_call.function.name.as_str())
                {
                    // A single admitted tool is waiting for approval/result.
                    context.app.stop_reason = classify_done_reason(
                        completion_tokens,
                        prompt_tokens,
                        &context.full_response_content,
                        true,
                        context.app.max_tokens,
                    );
                    if context.app.stop_reason.starts_with("Tool dispatched") {
                        context.app.stop_reason =
                            format!("→ Tool dispatched: {tool_name} — awaiting result");
                    }
                }

                if !context.app.tool_calls_processed_this_request {
                    match provider_stop_reason.as_deref() {
                        Some("max_tokens") => {
                            context.app.stop_reason =
                                "⚠ Response reached the output token limit".to_string();
                        }
                        Some("refusal") => {
                            context.app.stop_reason = "⚠ Model refused the request".to_string();
                        }
                        Some("pause_turn") => {
                            context.app.stop_reason =
                                "⚠ Model paused before completing the turn".to_string();
                        }
                        _ => {}
                    }
                }

                // Use server-reported speeds if available, else fall back to wall-clock
                if let Some(tg) = tg_per_s {
                    context.app.tokens_per_s = tg;
                } else if let Some(start) = context.app.request_start_time {
                    let elapsed = start.elapsed().as_secs_f64();
                    if elapsed > 0.0 {
                        let count = completion_tokens.unwrap_or(
                            context.full_response_content.split_whitespace().count() as u32,
                        );
                        context.app.tokens_per_s = (count as f64 / elapsed).max(0.0);
                    }
                }
                if let Some(pp) = pp_per_s {
                    context.app.pp_tokens_per_s = pp;
                }
                if let Some(pt) = prompt_tokens {
                    context.app.server_prompt_tokens = Some(pt);
                }
                if let Some(ct) = completion_tokens {
                    context.app.server_completion_tokens = Some(ct);
                }
                context.app.request_start_time = None;
                if !context.app.tool_calls_processed_this_request && !context.app.is_executing_tool
                {
                    context.app.settle_logical_turn();
                }
                context.app.should_redraw = true;
                context.app.save_session(); // Final save on completion

                if context.app.tool_calls_processed_this_request
                    && context.app.pending_tool_call.is_some()
                    && context.app.approval_matches_current_policy()
                    && !context.app.tool_call_dispatched
                {
                    dispatch_auto_approved_tool(
                        context.app,
                        &context.tx,
                        &context.cancellation_token,
                        &context.client,
                        context.config,
                    );
                }
            }
        }
        StreamEvent::Error(e) => {
            if let Err(error) = context.app.commit_partial_assistant_checkpoint() {
                context
                    .app
                    .log_debug(&format!("PARTIAL_TRANSCRIPT_COMMIT_ERROR: {error}"));
            }
            context.app.is_processing = false;
            context.app.request_start_time = None;
            context.app.add_segment(
                format!("\n{} ERROR: {}\n", icons::WARNING, e),
                BlockType::ProviderError,
            );
            if context.cancellation_pending {
                context.app.active_request_id = None;
                settle_provider_cancellation(context);
            } else {
                if context.app.is_executing_tool {
                    context.app.is_processing = true;
                } else {
                    context.app.settle_logical_turn();
                }
                let short = truncate_chars_with_ellipsis(&e, 80, 77);
                context.app.stop_reason = format!("✗ Server error: {}", short);
            }
            context.app.should_redraw = true;
            context.app.save_session();
        }
        StreamEvent::LoadProgress(percentage, status) => {
            crate::session_load::progress(context.app, percentage, status);
        }
        StreamEvent::SessionLoadFailed(error) => {
            if context.lifecycle.is_shutting_down() {
                crate::session_load::discard_loaded_for_shutdown(context.app);
            } else {
                crate::session_load::failed(context.app, error);
            }
        }
        StreamEvent::SessionLoaded {
            dir,
            state,
            #[cfg(target_os = "linux")]
            lease,
        } => {
            if context.lifecycle.is_shutting_down() {
                crate::session_load::discard_loaded_for_shutdown(context.app);
            } else {
                #[cfg(target_os = "linux")]
                crate::session_load::apply_loaded(
                    context.app,
                    context.config,
                    dir,
                    state,
                    context.shutdown_cancellation.child_token(),
                    lease,
                )
                .await;
                #[cfg(not(target_os = "linux"))]
                crate::session_load::apply_loaded(
                    context.app,
                    context.config,
                    dir,
                    state,
                    context.shutdown_cancellation.child_token(),
                )
                .await;
            }
        }
    }

    StreamControl::Continue
}

#[cfg(test)]
mod tests;
