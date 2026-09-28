use super::*;
use crate::lifecycle::RuntimeMode;
use lethetic::{app::App, config::Config};
use tokio::sync::mpsc;

fn snapshot_with_executable(executable: &str) -> lethetic::python_policy::PythonPolicySnapshot {
    let mut config = Config::default();
    config.python_runtime.python_executable = executable.to_string();
    lethetic::python_policy::PythonPolicySnapshot::from_config(&config)
}

#[tokio::test]
async fn failed_and_successful_local_results_use_distinct_typed_blocks() {
    let mut config = Config::default();
    let mut app = App::new(&config);
    app.blocks.clear();
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut context = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Interactive);

    for (result, is_error) in [
        ("private local failure req-local-7", true),
        ("local operation completed", false),
    ] {
        assert_eq!(
            handle_stream_event(
                &mut context,
                StreamEvent::ToolResult {
                    id: None,
                    func_name: "local_operation".to_string(),
                    result: result.to_string(),
                    cwd: "unchanged".to_string(),
                    is_error,
                    provenance: lethetic::tools::ToolOutputProvenance::OrdinaryHost,
                },
            )
            .await,
            StreamControl::Continue
        );
    }

    assert_eq!(context.app.blocks.len(), 2);
    assert_eq!(
        context.app.blocks[0].block_type,
        lethetic::app::BlockType::ToolError
    );
    assert_eq!(context.app.blocks[0].success, Some(false));
    assert_eq!(
        context.app.blocks[1].block_type,
        lethetic::app::BlockType::ToolResult
    );
    assert_eq!(context.app.blocks[1].success, Some(true));
}

#[tokio::test]
async fn python_provenance_uses_only_worker_local_recovery_in_stream() {
    let mut config = Config::default();
    let mut app = App::new(&config);
    app.blocks.clear();
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut context = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Interactive);
    let metadata = lethetic::python::PythonOutputMetadata {
        cell: 23,
        retained: true,
        artifact_id: "01234567-89ab-4def-8123-456789abcdef".to_string(),
        sections: vec![lethetic::python::PythonOutputSectionMetadata {
            section: lethetic::python::PythonOutputSection::Stdout,
            captured_bytes: 30_000,
            original_bytes: 30_000,
            excerpt_bytes: 30_000,
            truncated: false,
        }],
    };

    handle_stream_event(
        &mut context,
        StreamEvent::ToolResult {
            id: None,
            func_name: "python".to_string(),
            result: format!("HEAD{}TAIL", "x".repeat(30_000)),
            cwd: "worker-cwd".to_string(),
            is_error: false,
            provenance: lethetic::tools::ToolOutputProvenance::PythonCell(metadata),
        },
    )
    .await;

    let content = &context.app.blocks.last().unwrap().content;
    assert!(content.contains("HEAD"));
    assert!(content.contains("TAIL"));
    assert!(content.contains("lethetic_output.info(\"01234567-89ab-4def-8123-456789abcdef\")"));
    assert!(!content.contains(".lethetic"));
    assert!(!content.contains("read_file_lines"));
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn streaming_checkpoint_failure_uses_provider_error_provenance() {
    let directory = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    let mut app = App::new(&config);
    app.blocks.clear();
    app.current_session_dir = Some(directory.path().to_string_lossy().into_owned());
    app.session_directory_binding = None;
    app.is_processing = true;
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut context = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Interactive);

    handle_stream_event(&mut context, StreamEvent::Chunk("partial".to_string())).await;

    let error = context
        .app
        .blocks
        .iter()
        .find(|block| block.block_type == lethetic::app::BlockType::ProviderError)
        .expect("checkpoint failure must retain typed local detail");
    assert_eq!(error.success, Some(false));
    assert!(error.content.contains("no directory identity binding"));
    assert!(context.cancellation_pending);
}

#[tokio::test]
async fn stale_prepared_policy_cannot_weaken_cli_lock() {
    let baseline = Config::default();
    let mut config = baseline.clone();
    crate::cli::apply_literal_python_mode(&mut config, crate::cli::LiteralPythonMode::Permissive)
        .unwrap();
    let policy = lethetic::python_policy::PythonPolicyState::from_config(
        &baseline,
        lethetic::python_policy::PythonPolicySource::Config,
    )
    .with_process_literal(&config)
    .unwrap();
    let mut app = App::new_with_python_policy_state(&config, policy);
    let requested = lethetic::python_policy::PythonPolicySnapshot::from_config(&baseline);
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut context = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Interactive);
    let persist_called = std::cell::Cell::new(false);

    let error = apply_prepared_python_policy_with_persistence(
        &mut context,
        requested.clone(),
        requested,
        lethetic::python_policy::PythonPolicySource::OneTime,
        lethetic::python_setup::PolicyPersistence::OneTime,
        None,
        |_, _, _, _| {
            persist_called.set(true);
            unreachable!("CLI lock must reject before persistence")
        },
    )
    .await
    .unwrap_err();

    assert_eq!(
        error,
        lethetic::python_policy::CLI_PYTHON_POLICY_LOCKED_ERROR
    );
    assert!(!persist_called.get());
    assert_eq!(
        context.config.tool_profile,
        lethetic::config::ToolProfile::PythonOnly
    );
    assert_eq!(
        context.config.python_runtime.sandbox.network,
        Some(lethetic::config::NetworkAccess::Full)
    );
    assert_eq!(
        context.app.python_policy.effective_source(),
        lethetic::python_policy::PythonPolicySource::CliLocked
    );
}

#[tokio::test]
async fn stale_policy_revision_leaves_disk_and_memory_unchanged() {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().canonicalize().unwrap();
    let old = snapshot_with_executable("python-old");
    let requested = snapshot_with_executable("python-requested");
    lethetic::python_policy::persist_policy(
        lethetic::python_policy::PythonPolicyScope::Project,
        &workspace,
        &old,
        Some(&lethetic::python_policy::PolicyRevision::Missing),
    )
    .unwrap();
    let (effective, source) = lethetic::python_policy::effective_policy_after_write(
        lethetic::python_policy::PythonPolicyScope::Project,
        &workspace,
        &requested,
    )
    .unwrap();

    let mut config = Config::default();
    old.apply_to(&mut config);
    let mut app = App::new(&config);
    app.current_dir = workspace.to_string_lossy().into_owned();
    app.tool_runtime = lethetic::tool_runtime::ToolRuntime::interactive(workspace.clone());
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut context = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Interactive);

    let error = apply_prepared_python_policy_with_persistence(
        &mut context,
        requested,
        effective,
        source,
        lethetic::python_setup::PolicyPersistence::Project,
        Some(lethetic::python_policy::PolicyRevision::Missing),
        |scope, workspace, snapshot, expected_revision| {
            lethetic::python_policy::persist_policy(scope, workspace, snapshot, expected_revision)
        },
    )
    .await
    .unwrap_err();

    assert!(
        error.contains("changed while it was being edited"),
        "{error}"
    );
    let loaded = lethetic::python_policy::load_policy(
        &lethetic::python_policy::project_policy_path(&workspace),
    )
    .unwrap()
    .unwrap();
    assert_eq!(loaded.snapshot, old);
    assert_eq!(
        lethetic::python_policy::PythonPolicySnapshot::from_config(context.config),
        old
    );
    assert_eq!(
        lethetic::python_policy::PythonPolicySnapshot::from_config(&context.app.config),
        old
    );
}

#[tokio::test]
async fn persistence_failure_after_detach_keeps_the_old_policy_coherent() {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().canonicalize().unwrap();
    let old = snapshot_with_executable("python-old");
    let requested = snapshot_with_executable("python-requested");
    let (effective, source) = lethetic::python_policy::effective_policy_after_write(
        lethetic::python_policy::PythonPolicyScope::Project,
        &workspace,
        &requested,
    )
    .unwrap();
    let mut config = Config::default();
    old.apply_to(&mut config);
    let mut app = App::new(&config);
    app.current_dir = workspace.to_string_lossy().into_owned();
    app.tool_runtime = lethetic::tool_runtime::ToolRuntime::interactive(workspace.clone());
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut context = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Interactive);
    let persist_called = std::cell::Cell::new(false);

    let error = apply_prepared_python_policy_with_persistence(
        &mut context,
        requested,
        effective,
        source,
        lethetic::python_setup::PolicyPersistence::Project,
        Some(lethetic::python_policy::PolicyRevision::Missing),
        |_, _, _, _| {
            persist_called.set(true);
            Err("injected policy write failure".to_string())
        },
    )
    .await
    .unwrap_err();

    assert!(persist_called.get());
    assert!(error.contains("injected policy write failure"));
    assert!(!lethetic::python_policy::project_policy_path(&workspace).exists());
    assert_eq!(
        lethetic::python_policy::PythonPolicySnapshot::from_config(context.config),
        old
    );
    assert_eq!(
        lethetic::python_policy::PythonPolicySnapshot::from_config(&context.app.config),
        old
    );
}

#[tokio::test]
async fn visible_post_rename_failure_converges_memory_to_disk() {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().canonicalize().unwrap();
    let old = snapshot_with_executable("python-old");
    let requested = snapshot_with_executable("python-requested");
    let (effective, source) = lethetic::python_policy::effective_policy_after_write(
        lethetic::python_policy::PythonPolicyScope::Project,
        &workspace,
        &requested,
    )
    .unwrap();
    let mut config = Config::default();
    old.apply_to(&mut config);
    let mut app = App::new(&config);
    app.current_dir = workspace.to_string_lossy().into_owned();
    app.tool_runtime = lethetic::tool_runtime::ToolRuntime::interactive(workspace.clone());
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut context = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Interactive);

    let applied = apply_prepared_python_policy_with_persistence(
        &mut context,
        requested.clone(),
        effective,
        source,
        lethetic::python_setup::PolicyPersistence::Project,
        Some(lethetic::python_policy::PolicyRevision::Missing),
        |scope, workspace, snapshot, expected_revision| {
            lethetic::python_policy::persist_policy(scope, workspace, snapshot, expected_revision)?;
            Err("injected directory sync failure after rename".to_string())
        },
    )
    .await
    .unwrap();

    assert!(
        applied
            .durability_warning
            .as_deref()
            .is_some_and(|warning| warning.contains("directory sync failure"))
    );
    let loaded = lethetic::python_policy::load_policy(
        &lethetic::python_policy::project_policy_path(&workspace),
    )
    .unwrap()
    .unwrap();
    assert_eq!(loaded.snapshot, requested);
    assert_eq!(
        lethetic::python_policy::PythonPolicySnapshot::from_config(context.config),
        requested
    );
    assert_eq!(
        lethetic::python_policy::PythonPolicySnapshot::from_config(&context.app.config),
        requested
    );
    assert_eq!(context.app.python_policy.persisted_snapshot(), &requested);
    assert_eq!(
        context.app.python_policy.persisted_source(),
        lethetic::python_policy::PythonPolicySource::Project
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn detach_failure_never_attempts_policy_persistence() {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().canonicalize().unwrap();
    let old = snapshot_with_executable("python-old");
    let requested = snapshot_with_executable("python-requested");
    let (effective, source) = lethetic::python_policy::effective_policy_after_write(
        lethetic::python_policy::PythonPolicyScope::Project,
        &workspace,
        &requested,
    )
    .unwrap();
    let mut config = Config::default();
    old.apply_to(&mut config);
    let mut app = App::new(&config);
    app.current_dir = workspace.to_string_lossy().into_owned();
    app.tool_runtime = lethetic::tool_runtime::ToolRuntime::interactive(workspace.clone());
    let identity = lethetic::python::runtime_store::WorkspaceIdentity::capture(&workspace).unwrap();
    app.tool_runtime
        .bind_session(lethetic::tool_runtime::SessionBinding {
            session_id: "11111111-2222-4333-8444-555555555555".to_string(),
            runtime_id: Some("01234567-89ab-4def-8123-456789abcdef".to_string()),
            managed_workspace: identity.canonical_path,
            workspace_device: identity.device,
            workspace_inode: identity.inode,
            workspace_binding_hash: identity.binding_hash,
            shared_workspace: None,
            surface: lethetic::tool_runtime::ToolSurface::Interactive,
        })
        .await
        .unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut context = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Interactive);
    let persist_called = std::cell::Cell::new(false);

    let error = apply_prepared_python_policy_with_persistence(
        &mut context,
        requested,
        effective,
        source,
        lethetic::python_setup::PolicyPersistence::Project,
        Some(lethetic::python_policy::PolicyRevision::Missing),
        |_, _, _, _| {
            persist_called.set(true);
            unreachable!("persistence must follow successful runtime detach")
        },
    )
    .await
    .unwrap_err();

    assert!(error.contains("detach reconciliation failed"), "{error}");
    assert!(!persist_called.get());
    assert!(!lethetic::python_policy::project_policy_path(&workspace).exists());
    assert_eq!(
        lethetic::python_policy::PythonPolicySnapshot::from_config(context.config),
        old
    );
}

#[tokio::test]
async fn rejected_parallel_batch_stays_terminal_when_done_arrives() {
    let mut config = Config {
        context_size: 100_000,
        ..Default::default()
    };
    let mut app = App::new(&config);
    app.shell_approval_mode = lethetic::app::ApprovalMode::Always;
    app.approval_policy_fingerprint = Some(app.config.python_policy_fingerprint());
    let calls = ["first", "second"]
        .into_iter()
        .map(|id| lethetic::context::ToolCall {
            id: id.to_string(),
            provider_id: Some(format!("provider-{id}")),
            function: lethetic::context::FunctionCall {
                name: "todowrite".to_string(),
                arguments: serde_json::json!({
                    "todos": [],
                    "description": id,
                }),
            },
        })
        .collect::<Vec<_>>();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut context = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Interactive);
    context.full_response_content = "Provider preamble".to_string();

    handle_stream_event(
        &mut context,
        StreamEvent::ToolCalls {
            calls,
            provider_content: None,
        },
    )
    .await;
    let rejection_reason = context.app.stop_reason.clone();
    assert!(rejection_reason.contains("Rejected 2 parallel tool calls"));
    assert!(context.app.pending_tool_call.is_none());

    handle_stream_event(
        &mut context,
        StreamEvent::Done {
            request_id: "parallel-batch".to_string(),
            completion_tokens: Some(1),
            prompt_tokens: Some(1),
            usage: None,
            tg_per_s: None,
            pp_per_s: None,
            stop_reason: Some("tool_calls".to_string()),
            provider_content: None,
        },
    )
    .await;

    assert_eq!(context.app.stop_reason, rejection_reason);
    assert!(!context.app.is_processing);
    assert!(!context.app.is_executing_tool);
    assert!(context.app.pending_tool_call.is_none());
    assert!(
        rx.try_recv().is_err(),
        "rejected batch continued into a tool result"
    );
}

#[tokio::test]
async fn late_old_settlement_preserves_successor_request_state() {
    let mut config = Config {
        context_size: 100_000,
        ..Default::default()
    };
    let mut app = App::new(&config);
    app.add_logical_turn_user_segment("request".to_string());
    app.begin_provider_request("old-request".to_string());
    app.begin_provider_request("successor-request".to_string());
    app.is_processing = true;
    app.request_start_time = Some(tokio::time::Instant::now());
    app.stop_reason = "successor streaming".to_string();
    app.pending_tool_call = Some(lethetic::context::ToolCall {
        id: "successor-tool".to_string(),
        provider_id: Some("provider-successor-tool".to_string()),
        function: lethetic::context::FunctionCall {
            name: "todowrite".to_string(),
            arguments: serde_json::json!({"todos": []}),
        },
    });
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut context = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Service);
    context.full_response_content = "successor partial response".to_string();

    handle_stream_event(
        &mut context,
        StreamEvent::RequestSettlementFailed {
            request_id: "old-request".to_string(),
            error: "late old checkpoint failure".to_string(),
            cancellation_requested: true,
        },
    )
    .await;

    assert_eq!(
        context.app.active_request_id.as_deref(),
        Some("successor-request")
    );
    assert!(context.app.is_processing);
    assert!(context.app.request_start_time.is_some());
    assert_eq!(context.app.stop_reason, "successor streaming");
    assert_eq!(
        context
            .app
            .pending_tool_call
            .as_ref()
            .map(|call| call.id.as_str()),
        Some("successor-tool")
    );
    assert_eq!(context.full_response_content, "successor partial response");
    assert!(
        !context.app.close_provider_request_marker("old-request"),
        "late old marker was not closed"
    );
    assert!(
        context
            .app
            .close_provider_request_marker("successor-request"),
        "successor marker was cleared by an old settlement"
    );
}

#[tokio::test]
async fn nested_settlement_failure_preserves_outer_tool_until_bounded_containment() {
    let mut config = Config {
        context_size: 100_000,
        ..Default::default()
    };
    let mut app = App::new(&config);
    app.add_logical_turn_user_segment("request".to_string());
    let outer_call = lethetic::context::ToolCall {
        id: "outer-tool".to_string(),
        provider_id: Some("provider-outer-tool".to_string()),
        function: lethetic::context::FunctionCall {
            name: "task".to_string(),
            arguments: serde_json::json!({
                "prompt": "inspect",
                "description": "inspect",
            }),
        },
    };
    app.context_manager
        .upsert_assistant_tool_call_with_provider("", vec![outer_call.clone()], None);
    app.pending_tool_call = Some(outer_call);
    app.is_processing = true;
    app.is_executing_tool = true;
    app.begin_provider_request("nested-request".to_string());
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut context = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Service);
    context.begin_shutdown(crate::lifecycle::ShutdownReason::UserExit);

    handle_stream_event(
        &mut context,
        StreamEvent::RequestSettlementFailed {
            request_id: "nested-request".to_string(),
            error: "simulated nested durability failure".to_string(),
            cancellation_requested: true,
        },
    )
    .await;

    assert!(context.app.active_request_id.is_none());
    assert!(context.app.blocks.iter().any(|block| {
        block.block_type == lethetic::app::BlockType::ProviderError
            && block
                .content
                .contains("simulated nested durability failure")
    }));
    assert!(context.app.is_executing_tool);
    assert!(context.app.pending_tool_call.is_some());
    assert!(context.cancellation_pending);
    assert!(!context.shutdown_contained());

    let cwd = context.app.current_dir.clone();
    handle_stream_event(
        &mut context,
        StreamEvent::ToolResult {
            id: Some("outer-tool".to_string()),
            func_name: "task".to_string(),
            result: "Sub-agent failed after checkpoint failure".to_string(),
            cwd,
            is_error: true,
            provenance: lethetic::tools::ToolOutputProvenance::OrdinaryHost,
        },
    )
    .await;

    assert!(!context.app.is_executing_tool);
    assert!(context.app.pending_tool_call.is_none());
    assert!(!context.cancellation_pending);
    assert!(context.shutdown_contained());
    let error = context.finish_shutdown().unwrap_err();
    assert!(error.contains("simulated nested durability failure"));
}

#[tokio::test]
async fn checkpoint_actor_settles_before_acknowledging_failure() {
    let mut config = Config {
        context_size: 100_000,
        ..Default::default()
    };
    let mut app = App::new(&config);
    app.add_logical_turn_user_segment("request".to_string());
    app.begin_provider_request("nested-request".to_string());
    app.is_processing = true;
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut context = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Service);
    context.begin_shutdown(crate::lifecycle::ShutdownReason::UserExit);
    let (acknowledgement, result) = std::sync::mpsc::sync_channel(1);

    handle_stream_event(
        &mut context,
        StreamEvent::PersistRequestCheckpoint {
            checkpoint: lethetic::client::ProviderRequestCheckpoint {
                request: lethetic::accounting::ProviderRequestAccounting {
                    request_id: "nested-request".to_string(),
                    connection_id: "test".to_string(),
                    model: "test-model".to_string(),
                    usage: Default::default(),
                    usage_reported: false,
                    estimated_cost: None,
                    completed: false,
                    in_flight: false,
                },
                transcript: None,
            },
            acknowledgement,
        },
    )
    .await;

    let error = result.recv().unwrap().unwrap_err();
    assert!(error.contains("durable session directory"), "{error}");
    assert!(
        !context.app.close_provider_request_marker("nested-request"),
        "checkpoint acknowledgement preceded actor-owned settlement"
    );
    assert!(!context.cancellation_pending);
    assert!(context.shutdown_contained());
}

#[tokio::test]
async fn failed_shutdown_checkpoint_closes_request_marker_and_finishes_bounded() {
    let mut config = Config {
        context_size: 100_000,
        ..Default::default()
    };
    let mut app = App::new(&config);
    app.add_logical_turn_user_segment("request".to_string());
    app.begin_provider_request("request-one".to_string());
    app.is_processing = true;
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut context = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Service);
    context.begin_shutdown(crate::lifecycle::ShutdownReason::UserExit);
    assert!(!context.shutdown_contained());

    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        handle_stream_event(
            &mut context,
            StreamEvent::RequestSettlementFailed {
                request_id: "request-one".to_string(),
                error: "simulated checkpoint disk failure".to_string(),
                cancellation_requested: true,
            },
        ),
    )
    .await
    .expect("terminal settlement hung");

    assert!(context.shutdown_contained());
    let error = context.finish_shutdown().unwrap_err();
    assert!(error.contains("simulated checkpoint disk failure"));
}

#[tokio::test]
async fn provider_error_during_cancellation_retains_typed_local_detail() {
    let mut config = Config::default();
    let mut app = App::new(&config);
    app.blocks.clear();
    app.is_processing = true;
    app.begin_provider_request("cancelled-request".to_string());
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut context = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Service);
    context.cancellation_pending = true;
    let raw_error = "transport failed for tenant violet req-cancel-7";

    handle_stream_event(&mut context, StreamEvent::Error(raw_error.to_string())).await;

    assert!(!context.cancellation_pending);
    assert!(context.app.active_request_id.is_none());
    assert!(context.app.blocks.iter().any(|block| {
        block.block_type == lethetic::app::BlockType::ProviderError
            && block.content.contains(raw_error)
    }));
}

#[test]
fn provider_cancellation_waits_for_active_tool_containment() {
    let mut config = Config::default();
    let mut app = App::new(&config);
    app.is_processing = true;
    app.is_executing_tool = true;
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut context = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Service);
    context.cancellation_pending = true;

    settle_provider_cancellation(&mut context);
    assert!(context.cancellation_pending);
    assert!(context.app.is_processing);
    assert!(context.app.is_executing_tool);

    context.app.is_executing_tool = false;
    settle_provider_cancellation(&mut context);
    assert!(!context.cancellation_pending);
    assert!(!context.app.is_processing);
}

fn calculate_calls(ids: &[&str]) -> Vec<lethetic::context::ToolCall> {
    ids.iter()
        .map(|id| lethetic::context::ToolCall {
            id: id.to_string(),
            provider_id: Some(format!("provider-{id}")),
            function: lethetic::context::FunctionCall {
                name: "calculate".to_string(),
                arguments: serde_json::json!({"expression": "1 + 1", "description": id}),
            },
        })
        .collect()
}

/// The call id a stored tool result answers.
fn result_id(message: &lethetic::context::Message) -> String {
    let marker = "tool_call_id:<|'|>";
    let text = message.content.as_str();
    let start = text.rfind(marker).map(|index| index + marker.len()).unwrap_or(0);
    text[start..].split("<|'|>").next().unwrap_or("").to_string()
}

fn calculate_result(id: &str) -> StreamEvent {
    StreamEvent::ToolResult {
        id: Some(id.to_string()),
        func_name: "calculate".to_string(),
        result: "2".to_string(),
        cwd: ".".to_string(),
        is_error: false,
        provenance: lethetic::tools::ToolOutputProvenance::OrdinaryHost,
    }
}

#[tokio::test]
async fn sequential_batch_runs_every_call_before_asking_the_model_again() {
    let mut config = Config {
        context_size: 100_000,
        tool_calls: lethetic::tool_call_mode::ToolCallMode::Sequential,
        ..Default::default()
    };
    let mut app = App::new(&config);
    app.shell_approval_mode = lethetic::app::ApprovalMode::Always;
    app.approval_policy_fingerprint = Some(app.config.python_policy_fingerprint());
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut context = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Interactive);
    context.full_response_content = "Checking two things.".to_string();

    handle_stream_event(
        &mut context,
        StreamEvent::ToolCalls {
            calls: calculate_calls(&["first", "second"]),
            provider_content: None,
        },
    )
    .await;
    assert_eq!(context.app.pending_tool_call.as_ref().unwrap().id, "first");
    assert_eq!(context.app.queued_tool_calls.len(), 1);

    handle_stream_event(&mut context, calculate_result("first")).await;
    assert_eq!(
        context.app.pending_tool_call.as_ref().map(|call| call.id.as_str()),
        Some("second"),
        "the second call starts instead of a new model request"
    );
    assert!(context.app.queued_tool_calls.is_empty());
    assert!(context.app.active_request_id.is_none());

    handle_stream_event(&mut context, calculate_result("second")).await;
    let messages = context.app.context_manager.get_messages();
    let assistant = messages
        .iter()
        .rposition(|message| message.role == "assistant")
        .unwrap();
    let ids: Vec<&str> = messages[assistant]
        .tool_calls
        .as_ref()
        .unwrap()
        .iter()
        .map(|call| call.id.as_str())
        .collect();
    assert_eq!(ids, ["first", "second"]);
    let results: Vec<String> = messages[assistant + 1..]
        .iter()
        .filter(|message| message.role == "tool")
        .map(result_id)
        .collect();
    assert_eq!(results, ["first", "second"]);
}

#[tokio::test]
async fn stopping_mid_batch_gives_unrun_calls_an_error_result() {
    let mut config = Config {
        context_size: 100_000,
        tool_calls: lethetic::tool_call_mode::ToolCallMode::Sequential,
        ..Default::default()
    };
    let mut app = App::new(&config);
    app.shell_approval_mode = lethetic::app::ApprovalMode::Always;
    app.approval_policy_fingerprint = Some(app.config.python_policy_fingerprint());
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut context = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Interactive);

    handle_stream_event(
        &mut context,
        StreamEvent::ToolCalls {
            calls: calculate_calls(&["one", "two", "three"]),
            provider_content: None,
        },
    )
    .await;
    context.cancellation_pending = true;
    handle_stream_event(&mut context, calculate_result("one")).await;

    assert!(context.app.pending_tool_call.is_none());
    assert!(context.app.queued_tool_calls.is_empty());
    let unrun: Vec<(String, bool)> = context
        .app
        .context_manager
        .get_messages()
        .iter()
        .filter(|message| message.role == "tool")
        .map(|message| {
            (result_id(message), message.tool_result_is_error)
        })
        .collect();
    assert_eq!(
        unrun,
        [
            ("one".to_string(), false),
            ("two".to_string(), true),
            ("three".to_string(), true)
        ]
    );
}

const DROPPED: &str =
    "The model server closed the reply before finishing it (it may have crashed or restarted)";

fn processing_app(config: &Config) -> App {
    let mut app = App::new(config);
    app.add_logical_turn_user_segment("question".to_string());
    app.is_processing = true;
    app
}

#[tokio::test]
async fn a_dropped_reply_is_retried_until_the_limit_then_reported() {
    let mut config = Config {
        context_size: 100_000,
        provider_retries: Some(2),
        ..Default::default()
    };
    let mut app = processing_app(&config);
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut context = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Interactive);
    context.full_response_content = "partial answer".to_string();

    for attempt in 1..=2 {
        handle_stream_event(&mut context, StreamEvent::Error(DROPPED.to_string())).await;
        assert_eq!(context.app.provider_retry_attempts, attempt);
        assert!(context.app.provider_retry_at.is_some(), "retry {attempt} scheduled");
        assert!(context.app.is_processing, "the turn stays open while retrying");
        assert!(context.full_response_content.is_empty(), "partial reply discarded");
        assert!(context.app.stop_reason.starts_with("↻ Retrying"));
        context.app.provider_retry_at = None;
    }

    handle_stream_event(&mut context, StreamEvent::Error(DROPPED.to_string())).await;
    assert!(context.app.provider_retry_at.is_none());
    assert!(!context.app.is_processing);
    assert!(context.app.stop_reason.starts_with("✗ Server error"));
    assert_eq!(context.app.provider_retry_attempts, 0);
}

#[tokio::test]
async fn rejections_and_disabled_retries_are_reported_at_once() {
    for (retries, error) in [
        (Some(3), "Server 401 Unauthorized: bad key"),
        (Some(0), DROPPED),
    ] {
        let mut config = Config {
            context_size: 100_000,
            provider_retries: retries,
            ..Default::default()
        };
        let mut app = processing_app(&config);
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut context =
            RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Interactive);
        handle_stream_event(&mut context, StreamEvent::Error(error.to_string())).await;
        assert!(context.app.provider_retry_at.is_none(), "{error}");
        assert!(!context.app.is_processing, "{error}");
    }
}

#[tokio::test]
async fn stopping_during_the_retry_wait_cancels_it() {
    let mut config = Config {
        context_size: 100_000,
        ..Default::default()
    };
    let mut app = processing_app(&config);
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut context = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Interactive);
    handle_stream_event(&mut context, StreamEvent::Error(DROPPED.to_string())).await;
    assert!(context.app.provider_retry_at.is_some());

    let _ = context.dispatch_app_event(AppEventOutcome::Stop).await;
    assert!(context.app.provider_retry_at.is_none());
    assert!(!context.app.is_processing);
    assert!(!context.cancellation_pending, "nothing in flight to contain");
    assert_eq!(context.app.stop_reason, "Cancelled by user");
}
