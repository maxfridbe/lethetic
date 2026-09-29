use std::time::Duration;

use lethetic::app::{App, AppEventOutcome, ApprovalMode, BlockType};
use lethetic::client::{ModelChoice, StreamEvent};
use lethetic::config::Config;
use lethetic::icons;

use crate::context::{PythonSetupCompletion, PythonSetupOperation, RuntimeContext};
use crate::lifecycle::ShutdownReason;
use crate::provider::{
    SideEffectKind, dispatch_auto_approved_tool, persist_before_side_effect,
    record_provider_start_failure, record_terminal_tool_error, request_lsp_install_cancellation,
    settle_pending_interaction_checked, trigger_persisted_provider_request,
};

pub(crate) async fn detach_python_policy_runtime(app: &mut App) -> Result<(), String> {
    app.tool_runtime.unbind_session_checked().await
}

pub(crate) fn reassert_effective_python_policy(app: &mut App, config: &mut Config) {
    app.python_policy.apply_effective_to(config);
    app.config = config.clone();
    let refreshed = lethetic::system_prompt::SystemPromptManager::resolve_prompt(
        &app.system_prompt,
        &app.current_dir,
        config,
    );
    app.context_manager.update_system_prompt(refreshed);
}

pub(crate) fn install_python_policy(
    app: &mut App,
    config: &mut Config,
    snapshot: lethetic::python_policy::PythonPolicySnapshot,
    source: lethetic::python_policy::PythonPolicySource,
    persist_as_baseline: bool,
) -> Result<(), String> {
    app.python_policy.ensure_ui_mutable()?;
    if persist_as_baseline {
        app.python_policy.replace_persisted(snapshot, source)?;
    } else if source == lethetic::python_policy::PythonPolicySource::OneTime {
        app.python_policy.set_chat_one_time(snapshot)?;
    } else {
        app.python_policy.replace_persisted(snapshot, source)?;
    }
    app.clear_tool_approval();
    reassert_effective_python_policy(app, config);
    Ok(())
}

pub(crate) async fn activate_python_policy(
    app: &mut App,
    config: &mut Config,
    snapshot: lethetic::python_policy::PythonPolicySnapshot,
    source: lethetic::python_policy::PythonPolicySource,
    persist_as_baseline: bool,
) -> Result<(), String> {
    app.python_policy.ensure_ui_mutable()?;
    detach_python_policy_runtime(app).await?;
    install_python_policy(app, config, snapshot, source, persist_as_baseline)
}

pub(crate) async fn prepare_python_policy_for_session_transition(
    app: &mut App,
    config: &mut Config,
) -> Result<(), String> {
    detach_python_policy_runtime(app).await?;
    app.clear_tool_approval();
    app.python_policy.expire_chat_one_time();
    reassert_effective_python_policy(app, config);
    Ok(())
}

async fn reap_finished_auxiliary_tasks(
    app: &mut App,
    tasks: &mut Vec<tokio::task::JoinHandle<()>>,
) {
    let mut pending = Vec::with_capacity(tasks.len());
    for task in std::mem::take(tasks) {
        if task.is_finished() {
            if task.await.is_err() {
                app.log_runtime_debug("AUXILIARY_TASK_FAILED");
            }
        } else {
            pending.push(task);
        }
    }
    *tasks = pending;
}

fn transition_theme_by_name(app: &mut App, name: &str) -> bool {
    let Some(index) = app
        .themes
        .iter()
        .position(|theme| theme.name.eq_ignore_ascii_case(name))
    else {
        return false;
    };
    let changed = app.theme.name != app.themes[index].name;
    app.theme_state.select(Some(index));
    if changed {
        app.theme = app.themes[index].clone();
        for block in &mut app.blocks {
            block.invalidate();
        }
        app.needs_save = true;
    }
    true
}

/// Point `config`/`app` at a different connection/model, pulling in that
/// server's credentials, limits, theme, and parser dialect. Shared by the
/// interactive model switcher and by session resume (which restores the model
/// last used). Returns the parser dialect on success.
pub(crate) fn apply_model_switch(
    app: &mut App,
    config: &mut Config,
    connection_id: &str,
    new_model: &str,
) -> Result<String, String> {
    let mut candidate = config.clone();
    if connection_id == "__active" {
        candidate.activate_current_model(new_model)?;
    } else {
        candidate.activate_model(connection_id, new_model)?;
    }
    app.python_policy.apply_effective_to(&mut candidate);
    candidate.validate()?;
    let parser = candidate.active_parser().to_string();
    let input_token_budget = candidate.input_token_budget();
    let refreshed = lethetic::system_prompt::SystemPromptManager::resolve_prompt(
        &app.system_prompt,
        &app.current_dir,
        &candidate,
    );
    app.context_manager
        .set_input_token_budget(input_token_budget)
        .map_err(|error| format!("Model switch rejected: {error}"))?;
    app.max_tokens = input_token_budget;
    app.server_url = candidate.server_url.clone();
    app.model_name = new_model.to_string();
    app.context_manager.mode = candidate
        .context_mode
        .unwrap_or(lethetic::context::ContextMode::Lethetic);
    app.context_manager.update_system_prompt(refreshed);
    if let Some(name) = &candidate.theme {
        transition_theme_by_name(app, name);
    }
    app.parser
        .set_mode(lethetic::parser::ParserMode::from(parser.as_str()));
    app.parser.reset();
    app.config = candidate.clone();
    *config = candidate;
    if let Some(connection_id) = config.active_connection_id() {
        let last = lethetic::config::LastModel {
            connection_id: connection_id.to_string(),
            model: new_model.to_string(),
        };
        if let Err(error) = last.save(std::path::Path::new(&app.current_dir)) {
            app.log_debug(&format!("Could not remember the selected model: {error}"));
        }
    }
    Ok(parser)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AppEventControl {
    Handled,
    ContinueRunLoop,
}

pub(crate) async fn handle_app_event_outcome(
    outcome: AppEventOutcome,
    runtime: &mut RuntimeContext<'_>,
) -> AppEventControl {
    let outcome = match outcome {
        AppEventOutcome::Exit => {
            runtime.begin_shutdown(ShutdownReason::UserExit);
            return AppEventControl::ContinueRunLoop;
        }
        outcome => outcome,
    };

    let RuntimeContext {
        app,
        config,
        client,
        tx,
        cancellation_token,
        shutdown_cancellation,
        background_cancellation,
        python_setup_operation,
        auxiliary_tasks,
        full_response_content,
        cancellation_pending,
        remote_control_request,
        ..
    } = runtime;
    let app = &mut **app;
    let config = &mut **config;

    match outcome {
        AppEventOutcome::NewSession => {
            match crate::session_transition::start_committed_session_transition(
                app,
                config,
                shutdown_cancellation.child_token(),
            )
            .await
            {
                Ok(_) => {}
                Err(_) if shutdown_cancellation.is_cancelled() => {
                    return AppEventControl::ContinueRunLoop;
                }
                Err(error) => {
                    app.stop_reason = format!("✗ Session creation failed: {error}");
                }
            }
            app.should_redraw = true;
        }
        AppEventOutcome::ResumeSession(session_id) => {
            if !crate::session_load::begin(app, session_id, tx, shutdown_cancellation.child_token())
            {
                return AppEventControl::ContinueRunLoop;
            }
        }
        AppEventOutcome::ToggleHistory => {
            app.show_history = !app.show_history;
            if app.show_history {
                app.history_state.select(Some(0));
            }
            app.should_redraw = true;
        }
        AppEventOutcome::DeleteSession(session_id) => {
            #[cfg(target_os = "linux")]
            {
                let deletion = app
                    .delete_session_transaction_with_cancellation(
                        &session_id,
                        shutdown_cancellation.child_token(),
                    )
                    .await;
                if shutdown_cancellation.is_cancelled() {
                    return AppEventControl::ContinueRunLoop;
                }
                match deletion {
                    Ok(deleted_active) => {
                        if deleted_active {
                            if let Err(error) =
                                prepare_python_policy_for_session_transition(app, config).await
                            {
                                app.stop_reason = format!(
                                    "Session deleted, but Python policy detach failed: {error}"
                                );
                            } else if shutdown_cancellation.is_cancelled() {
                                return AppEventControl::ContinueRunLoop;
                            } else if let Err(error) = app.start_new_session_checked() {
                                app.stop_reason = format!(
                                    "Session deleted, but replacement creation failed: {error}"
                                );
                            }
                        } else {
                            app.stop_reason = "Session deleted".to_string();
                        }
                    }
                    Err(error) => {
                        app.stop_reason = format!("✗ Session deletion failed: {error}");
                    }
                }
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = session_id;
                app.stop_reason = "✗ Durable session deletion is disabled because secure session locking is available only on Linux".to_string();
            }
            app.refresh_session_list();
            app.should_redraw = true;
        }
        AppEventOutcome::WipeSessions => {
            #[cfg(not(target_os = "linux"))]
            {
                app.stop_reason = "✗ Durable session wipe is disabled because secure session locking is available only on Linux".to_string();
                app.should_redraw = true;
            }
            #[cfg(target_os = "linux")]
            {
                if !app.is_fully_idle() {
                    app.stop_reason = "✗ Wait for active work before wiping sessions".to_string();
                    app.should_redraw = true;
                    return AppEventControl::ContinueRunLoop;
                }
                app.refresh_session_list();
                let targets = app.session_ids_for_cleanup();
                let mut errors = Vec::new();
                for session_id in targets {
                    if shutdown_cancellation.is_cancelled() {
                        break;
                    }
                    if let Err(error) = app
                        .delete_session_transaction_with_cancellation(
                            &session_id,
                            shutdown_cancellation.child_token(),
                        )
                        .await
                    {
                        errors.push(format!("{session_id}: {error}"));
                    }
                }
                if shutdown_cancellation.is_cancelled() {
                    return AppEventControl::ContinueRunLoop;
                }
                app.refresh_session_list();
                if app.current_session_dir.is_none() {
                    if let Err(error) =
                        prepare_python_policy_for_session_transition(app, config).await
                    {
                        errors.push(format!("Python policy detach: {error}"));
                    } else if shutdown_cancellation.is_cancelled() {
                        return AppEventControl::ContinueRunLoop;
                    } else if let Err(error) = app.start_new_session_checked() {
                        errors.push(format!("replacement session: {error}"));
                    }
                }
                if errors.is_empty() {
                    app.stop_reason =
                        "All chat sessions and bound Python runtimes were deleted".to_string();
                } else {
                    app.stop_reason = format!(
                        "✗ Session wipe left {} error(s): {}",
                        errors.len(),
                        errors.join(" | ")
                    );
                }
                app.should_redraw = true;
            }
        }
        AppEventOutcome::DeletePythonRuntime => {
            #[cfg(target_os = "linux")]
            {
                let deletion = app
                    .delete_python_runtime_transaction_with_cancellation(
                        shutdown_cancellation.child_token(),
                    )
                    .await;
                if shutdown_cancellation.is_cancelled() {
                    return AppEventControl::ContinueRunLoop;
                }
                match deletion {
                    Ok(true) => {
                        app.stop_reason =
                            "Python runtime/package layer deleted; managed source workspace retained"
                                .to_string();
                    }
                    Ok(false) => {
                        app.stop_reason =
                            "This chat has no retained Python package layer".to_string();
                    }
                    Err(error) => {
                        app.stop_reason = format!("✗ Python runtime deletion failed: {error}");
                    }
                }
            }
            #[cfg(not(target_os = "linux"))]
            {
                app.stop_reason =
                    "Retained Python package deletion is supported only on Linux".to_string();
            }
            app.should_redraw = true;
        }
        AppEventOutcome::FetchModels => {
            reap_finished_auxiliary_tasks(app, auxiliary_tasks).await;
            if !auxiliary_tasks.is_empty() {
                app.stop_reason = "Model discovery is already in progress".to_string();
                app.should_redraw = true;
                return AppEventControl::ContinueRunLoop;
            }
            app.show_palette = false;
            app.show_model_switcher = true;
            app.available_models.clear();
            app.model_switcher_state.select(Some(0));
            app.should_redraw = true;
            let mut servers = config
                .model_servers
                .iter()
                .map(|server| {
                    (
                        server.connection_id().to_string(),
                        server.name.clone(),
                        server.kind,
                        server.url.clone(),
                        server.model.clone(),
                        server.api_key.clone(),
                        server.discover_models,
                        server.models.clone(),
                    )
                })
                .collect::<Vec<_>>();
            if servers.is_empty() {
                servers.push((
                    "__active".to_string(),
                    config.model.clone(),
                    config.active_connection_kind(),
                    config.server_url.clone(),
                    config.model.clone(),
                    config.api_key.clone(),
                    true,
                    Vec::new(),
                ));
            }
            let client_clone = (*client).clone();
            let tx_clone = (*tx).clone();
            let model_cancellation = background_cancellation.child_token();
            let model_task = tokio::spawn(async move {
                const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
                let probes = servers.iter().map(
                    |(id, name, kind, url, default_model, api_key, discover, allowlist)| {
                        let client = client_clone.clone();
                        let cancellation = model_cancellation.child_token();
                        async move {
                            if !*discover {
                                // Configured-only entry: no probe, no discovery.
                                return (
                                    id.clone(),
                                    name.clone(),
                                    *kind,
                                    url.clone(),
                                    default_model.clone(),
                                    allowlist.clone(),
                                    Ok(Vec::new()),
                                );
                            }
                            let probe = tokio::time::timeout(
                                PROBE_TIMEOUT,
                                lethetic::client::get_available_models(
                                    &client,
                                    *kind,
                                    url,
                                    api_key.as_deref(),
                                ),
                            );
                            let live = tokio::select! {
                                biased;
                                _ = cancellation.cancelled() => {
                                    Err("Connection probe cancelled".to_string())
                                }
                                result = probe => match result {
                                    Ok(result) => result,
                                    Err(_) => Err(format!(
                                        "Connection probe timed out after {}s",
                                        PROBE_TIMEOUT.as_secs()
                                    )),
                                }
                            };
                            (
                                id.clone(),
                                name.clone(),
                                *kind,
                                url.clone(),
                                default_model.clone(),
                                allowlist.clone(),
                                live,
                            )
                        }
                    },
                );
                let mut models = Vec::new();
                for (id, name, kind, url, default_model, allowlist, live) in
                    futures_util::future::join_all(probes).await
                {
                    let live = live.map(|discovered| {
                        if allowlist.is_empty() {
                            discovered
                        } else {
                            discovered
                                .into_iter()
                                .filter(|model| allowlist.contains(&model.id))
                                .collect()
                        }
                    });
                    match live {
                        Ok(discovered) if !discovered.is_empty() => {
                            for model in discovered {
                                models.push(ModelChoice {
                                    display: format!("{} — {}", name, model.display_name),
                                    connection_id: id.clone(),
                                    kind,
                                    url: url.clone(),
                                    model_id: model.id,
                                    available: true,
                                });
                            }
                        }
                        Ok(_) => {
                            let configured = if allowlist.is_empty() {
                                vec![default_model]
                            } else {
                                allowlist
                            };
                            for model_id in configured {
                                models.push(ModelChoice {
                                    display: format!("{} — {} (configured)", name, model_id),
                                    connection_id: id.clone(),
                                    kind,
                                    url: url.clone(),
                                    model_id,
                                    available: true,
                                });
                            }
                        }
                        Err(error) => models.push(ModelChoice {
                            display: format!("{} (offline: {})", name, error),
                            connection_id: id,
                            kind,
                            url,
                            model_id: default_model,
                            available: false,
                        }),
                    }
                }
                if model_cancellation.is_cancelled() {
                    return;
                }
                let _ = tx_clone.send(StreamEvent::ModelsReady(models));
            });
            auxiliary_tasks.push(model_task);
        }
        AppEventOutcome::SwitchModel(connection_id, new_model) => {
            match apply_model_switch(app, config, &connection_id, &new_model) {
                Err(error) => {
                    app.stop_reason = format!("⚠ {error}");
                    app.add_segment(format!("\n{} {error}\n", icons::WARNING), BlockType::Text);
                }
                Ok(parser) => {
                    app.add_segment(
                        format!(
                            "\n{} Switched to model: {} ({}) — parser: {}\n",
                            icons::SUCCESS,
                            new_model,
                            config.server_url,
                            parser
                        ),
                        BlockType::Text,
                    );
                    app.stop_reason = format!("Model: {new_model}");
                }
            }
            app.should_redraw = true;
        }
        AppEventOutcome::StartRemoteControl {
            target,
            open,
            files,
        } => {
            *remote_control_request = Some(crate::context::RemoteControlRequest::Start {
                target,
                open,
                files,
            });
        }
        AppEventOutcome::InstallSkill { name } => {
            let client = client.clone();
            let tx = tx.clone();
            let cancel = background_cancellation.child_token();
            auxiliary_tasks.push(tokio::spawn(async move {
                let root = lethetic::skills::user_skill_root();
                let install = lethetic::skills::catalog::install(&client, &name, &root);
                let result = tokio::select! {
                    result = install => result.map(|path| path.display().to_string()),
                    () = cancel.cancelled() => return,
                };
                let _ = tx.send(StreamEvent::SkillInstallFinished { name, result });
            }));
            app.should_redraw = true;
        }
        AppEventOutcome::CycleToolCallMode => {
            if config.tool_profile == lethetic::config::ToolProfile::PythonOnly {
                app.stop_reason = "Tool calls: Python-only mode always uses one per turn".to_string();
            } else {
                config.tool_calls = config.tool_calls.next();
                app.config.tool_calls = config.tool_calls;
                let refreshed = lethetic::system_prompt::SystemPromptManager::resolve_prompt(
                    &app.system_prompt,
                    &app.current_dir,
                    config,
                );
                app.context_manager.update_system_prompt(refreshed);
                app.stop_reason = format!("Tool calls: {}", config.tool_calls.label());
            }
            app.should_redraw = true;
        }
        AppEventOutcome::StopRemoteControl => {
            *remote_control_request = Some(crate::context::RemoteControlRequest::Stop);
        }
        AppEventOutcome::ScanModels { connection_id } => {
            match crate::model_catalog::scan(
                config,
                client,
                tx,
                background_cancellation.child_token(),
                connection_id,
            ) {
                Some(task) => auxiliary_tasks.push(task),
                None => {
                    app.model_catalog = None;
                    app.stop_reason = "⚠ That connection is not in the config".to_string();
                }
            }
            app.should_redraw = true;
        }
        AppEventOutcome::SaveModel {
            connection_id,
            model_id,
        } => {
            crate::model_catalog::save(app, config, &connection_id, &model_id);
            return Box::pin(handle_app_event_outcome(
                AppEventOutcome::FetchModels,
                runtime,
            ))
            .await;
        }
        AppEventOutcome::CompactSession {
            session_id,
            connection_id,
            model_id,
        } => {
            crate::compaction::begin(
                app,
                config,
                client,
                tx,
                background_cancellation,
                session_id,
                connection_id,
                model_id,
            );
        }
        AppEventOutcome::OpenPythonSetup { preset } => {
            if let Err(error) = app.python_policy.ensure_ui_mutable() {
                app.stop_reason = format!("⚠ {error}");
            } else if python_setup_operation.is_some() || !app.is_fully_idle() {
                app.stop_reason =
                    "⚠ Wait for the active turn or Python setup operation before configuring Agent Mode"
                        .to_string();
            } else {
                let workspace = app.tool_runtime.workspace_root().to_path_buf();
                let mut dialog =
                    lethetic::python_setup::PythonSetupDialog::new(config, workspace.clone());
                if let Some(preset) = preset {
                    dialog.apply_preset(preset, config);
                }
                app.python_setup = Some(dialog);
                let draft = config.clone();
                if let Err(error) = PythonSetupOperation::start_child(
                    python_setup_operation,
                    shutdown_cancellation,
                    move |cancellation| async move {
                        PythonSetupCompletion::Capabilities(
                            lethetic::python::backend::probe_all_backends_with_cancellation(
                                &draft,
                                &workspace,
                                cancellation,
                            )
                            .await,
                        )
                    },
                ) {
                    app.python_setup = None;
                    app.stop_reason = format!("⚠ {error}");
                }
            }
            app.should_redraw = true;
        }
        AppEventOutcome::CancelPythonSetup {
            dismiss_when_settled,
        } => {
            if PythonSetupOperation::cancel(python_setup_operation, dismiss_when_settled) {
                app.stop_reason = "Cancelling Python setup operation…".to_string();
            } else {
                if dismiss_when_settled {
                    app.python_setup = None;
                }
                app.stop_reason = "Python setup operation is no longer active".to_string();
            }
            app.should_redraw = true;
        }
        AppEventOutcome::ApplyPythonPolicy {
            snapshot,
            persistence,
            expected_revision,
        } => {
            let preparation = (|| -> Result<
                (
                    Config,
                    lethetic::python_policy::PythonPolicySnapshot,
                    lethetic::python_policy::PythonPolicySource,
                ),
                String,
            > {
                app.python_policy.ensure_ui_mutable()?;
                if !app.is_idle_except_python_setup() {
                    return Err("Wait for the active turn before changing Agent Mode".to_string());
                }
                let setup = app
                    .python_setup
                    .as_ref()
                    .ok_or_else(|| "Python setup dialog is no longer active".to_string())?;
                if setup.snapshot() != snapshot
                    || setup.persistence != persistence
                    || setup.expected_revision() != expected_revision
                {
                    return Err("Python setup changed before it could be applied".to_string());
                }
                snapshot.validate()?;
                let workspace = app.tool_runtime.workspace_root();
                let (effective_snapshot, effective_source) = match persistence {
                    lethetic::python_setup::PolicyPersistence::OneTime => (
                        snapshot.clone(),
                        lethetic::python_policy::PythonPolicySource::OneTime,
                    ),
                    lethetic::python_setup::PolicyPersistence::Project => {
                        lethetic::python_policy::effective_policy_after_write(
                            lethetic::python_policy::PythonPolicyScope::Project,
                            workspace,
                            &snapshot,
                        )?
                    }
                    lethetic::python_setup::PolicyPersistence::Global => {
                        lethetic::python_policy::effective_policy_after_write(
                            lethetic::python_policy::PythonPolicyScope::Global,
                            workspace,
                            &snapshot,
                        )?
                    }
                };
                effective_snapshot.validate()?;
                let mut draft = config.clone();
                effective_snapshot.apply_to(&mut draft);
                Ok((draft, effective_snapshot, effective_source))
            })();

            match preparation {
                Ok((draft, effective_snapshot, effective_source)) => {
                    let workspace = app.tool_runtime.workspace_root().to_path_buf();
                    let choice = lethetic::tool_runtime::backend_choice(&draft);
                    let started = PythonSetupOperation::start_child(
                        python_setup_operation,
                        shutdown_cancellation,
                        move |cancellation| async move {
                            let validation = match choice {
                                Some(choice) => {
                                    match lethetic::python::backend::probe_backend_with_cancellation(
                                        &draft,
                                        &workspace,
                                        choice,
                                        cancellation,
                                    )
                                    .await
                                    {
                                        Ok(capability) if capability.available => Ok(()),
                                        Ok(capability) => Err(capability.reason),
                                        Err(error) => Err(error),
                                    }
                                }
                                None if cancellation.is_cancelled() => {
                                    Err("Python policy validation was cancelled".to_string())
                                }
                                None => Ok(()),
                            };
                            PythonSetupCompletion::PolicyPrepared {
                                snapshot,
                                effective_snapshot: Box::new(effective_snapshot),
                                effective_source,
                                persistence,
                                expected_revision,
                                validation,
                            }
                        },
                    );
                    match started {
                        Ok(()) => {
                            app.stop_reason =
                                "Validating Python backend before applying policy…".to_string();
                        }
                        Err(error) => {
                            if let Some(setup) = app.python_setup.as_mut() {
                                setup.stage = lethetic::python_setup::PythonSetupStage::Confirm;
                                setup.error = Some(error.clone());
                            }
                            app.stop_reason = format!("⚠ {error}");
                        }
                    }
                }
                Err(error) => {
                    if let Some(setup) = app.python_setup.as_mut() {
                        setup.stage = lethetic::python_setup::PythonSetupStage::Confirm;
                        setup.error = Some(error.clone());
                    }
                    app.stop_reason = format!("⚠ {error}");
                }
            }
            app.should_redraw = true;
        }
        AppEventOutcome::PullPodmanImage(image) => {
            let Some(setup) = app.python_setup.as_ref() else {
                app.stop_reason = "⚠ Python setup dialog is no longer active".to_string();
                return AppEventControl::ContinueRunLoop;
            };
            let mut draft = config.clone();
            setup.snapshot().apply_to(&mut draft);
            let workspace = app.tool_runtime.workspace_root().to_path_buf();
            let progress_tx = (*tx).clone();
            let image_label = image.clone();
            let started = PythonSetupOperation::start_child(
                python_setup_operation,
                shutdown_cancellation,
                move |cancellation| async move {
                    let result = lethetic::python::backend::pull_podman_image(
                        &image,
                        cancellation.clone(),
                        Some(progress_tx),
                    )
                    .await;
                    let result = match result {
                        Ok(()) => {
                            lethetic::python::backend::probe_all_backends_with_cancellation(
                                &draft,
                                &workspace,
                                cancellation,
                            )
                            .await
                        }
                        Err(error) => Err(error),
                    };
                    PythonSetupCompletion::PullFinished(result)
                },
            );
            match started {
                Ok(()) => app.stop_reason = format!("Pulling Podman image {image_label}…"),
                Err(error) => {
                    if let Some(setup) = app.python_setup.as_mut() {
                        setup.stage = setup
                            .previous_stage
                            .take()
                            .unwrap_or(lethetic::python_setup::PythonSetupStage::PodmanImage);
                        setup.error = Some(error.clone());
                    }
                    app.stop_reason = format!("⚠ {error}");
                }
            }
            app.should_redraw = true;
        }
        AppEventOutcome::SendPrompt(prompt) => {
            if !app.is_asking_user && !app.is_fully_idle() {
                app.stop_reason = "✗ Wait for active work or session loading to finish".to_string();
                app.should_redraw = true;
                return AppEventControl::ContinueRunLoop;
            }
            if !prompt.starts_with(lethetic::background::NOTICE_PREFIX) {
                app.add_to_history(prompt.clone());
            }
            if app.is_asking_user {
                app.is_asking_user = false;
                app.add_segment(prompt.clone(), BlockType::User);

                if !persist_before_side_effect(
                    app,
                    "continue the provider tool call",
                    SideEffectKind::Tool,
                ) {
                    if let Some(tool_call) = app.pending_tool_call.take() {
                        record_terminal_tool_error(
                            app,
                            tool_call,
                            "ERROR: The user answer could not be continued because session state could not be saved.",
                        );
                    }
                    return AppEventControl::ContinueRunLoop;
                }

                if let Some(tool_call) = app.pending_tool_call.as_ref() {
                    let tc_id = tool_call.id.clone();
                    let func_name = tool_call.function.name.clone();
                    let _ = tx.send(StreamEvent::ToolResult {
                        id: Some(tc_id),
                        func_name,
                        result: prompt,
                        cwd: app.current_dir.clone(),
                        is_error: false,
                        provenance: lethetic::tools::ToolOutputProvenance::OrdinaryHost,
                    });
                }
            } else {
                if let Some(error) = config.python_mode_validation_error() {
                    app.stop_reason = format!("✗ Invalid Python-only policy: {error}");
                    app.add_segment(
                        format!(
                            "\n{} PYTHON POLICY ERROR: provider request was not started: {error}\n",
                            icons::WARNING
                        ),
                        BlockType::ProviderError,
                    );
                    if let Err(save_error) = app.save_session_checked() {
                        app.stop_reason = format!(
                            "✗ Invalid Python-only policy; local error save failed: {save_error}"
                        );
                    }
                    app.should_redraw = true;
                    return AppEventControl::ContinueRunLoop;
                }
                #[cfg(target_os = "linux")]
                if let Err(error) = app
                    .ensure_nonlocal_python_session_with_cancellation(
                        shutdown_cancellation.child_token(),
                    )
                    .await
                {
                    app.stop_reason = format!("✗ Nonlocal Python preflight failed: {error}");
                    app.add_segment(
                        format!(
                            "\n{} NONLOCAL PYTHON PREFLIGHT ERROR: {error}\n",
                            icons::WARNING
                        ),
                        BlockType::ToolError,
                    );
                    app.should_redraw = true;
                    return AppEventControl::ContinueRunLoop;
                }
                #[cfg(not(target_os = "linux"))]
                if lethetic::config::is_exact_retained_nonlocal_python_policy(
                    config.tool_profile,
                    &config.python_runtime,
                ) {
                    app.stop_reason =
                        "✗ Nonlocal retained Python is supported only on Linux".to_string();
                    app.should_redraw = true;
                    return AppEventControl::ContinueRunLoop;
                }
                if shutdown_cancellation.is_cancelled() {
                    return AppEventControl::ContinueRunLoop;
                }
                app.provider_retry_attempts = 0;
                app.add_logical_turn_user_segment(prompt.clone());
                app.context_manager.set_cwd(app.current_dir.clone());
                app.context_manager.add_message("user", &prompt);
                app.stop_reason = "Processing…".to_string();
                app.tool_call_fingerprints.clear();
                app.applied_edits.clear();
                app.is_processing = true;
                app.tool_calls_processed_this_request = false;
                app.tool_call_dispatched = false;
                app.tool_call_pos = None;
                full_response_content.clear();
                *cancellation_token = shutdown_cancellation.child_token();
                app.request_start_time = Some(tokio::time::Instant::now());
                app.parser.reset();
                if !persist_before_side_effect(
                    app,
                    "send the provider request",
                    SideEffectKind::Provider,
                ) {
                    return AppEventControl::ContinueRunLoop;
                }
                if let Err(error) =
                    trigger_persisted_provider_request(app, client, config, tx, cancellation_token)
                {
                    record_provider_start_failure(app, "Provider request", &error);
                    return AppEventControl::ContinueRunLoop;
                }
            }
        }
        AppEventOutcome::ToolApproved(approved, always) => {
            if app.pending_tool_call.is_some() {
                if approved {
                    if always {
                        app.shell_approval_mode = ApprovalMode::Always;
                        app.approval_policy_fingerprint =
                            Some(app.config.python_policy_fingerprint());
                    }
                    dispatch_auto_approved_tool(app, tx, cancellation_token, client, config);
                } else if let Some(tool_call) = app.pending_tool_call.clone() {
                    let denial = if tool_call.function.name == "python" {
                        app.tool_runtime.mark_python_audit_status(
                            tool_call.provider_id.as_deref().unwrap_or(&tool_call.id),
                            lethetic::python::notebook::NotebookAttemptStatus::Denied,
                            Some("Tool execution denied by user."),
                        )
                    } else {
                        Ok(())
                    };
                    app.add_segment(
                        format!("\n{} Tool execution denied by user.\n", icons::WARNING),
                        BlockType::Text,
                    );
                    app.is_processing = true;
                    let result = match denial {
                        Ok(()) => "ERROR: Tool execution denied by user.".to_string(),
                        Err(error) => format!(
                            "ERROR: Tool execution was denied, but the notebook denial checkpoint failed: {error}"
                        ),
                    };
                    let _ = tx.send(StreamEvent::ToolResult {
                        id: Some(tool_call.id.clone()),
                        func_name: tool_call.function.name.clone(),
                        result,
                        cwd: app.current_dir.clone(),
                        is_error: true,
                        provenance: lethetic::tools::ToolOutputProvenance::OrdinaryHost,
                    });
                }
            }
            app.show_approval_prompt = false;
            app.should_redraw = true;
        }
        AppEventOutcome::Stop => {
            if request_lsp_install_cancellation(
                app,
                cancellation_token,
                "Cancelling LSP server installation…",
            ) {
                // LSP cancellation is tracked separately from provider work.
            } else if (app.show_approval_prompt || app.is_asking_user) && !app.is_executing_tool {
                match settle_pending_interaction_checked(app) {
                    Ok(true) => {
                        *cancellation_pending = false;
                        app.stop_reason = "Cancelled by user".to_string();
                    }
                    Ok(false) => {
                        app.stop_reason = "No pending interaction to cancel".to_string();
                    }
                    Err(error) => {
                        app.stop_reason =
                            format!("✗ Pending interaction could not be cancelled safely: {error}");
                    }
                }
            } else if !app.is_executing_tool
                && crate::stream_events::abandon_provider_retry(app, "Cancelled by user")
            {
                // A scheduled retry had no request in flight; nothing to contain.
            } else if app.is_processing || app.is_executing_tool {
                cancellation_token.cancel();
                *cancellation_pending = true;
                app.is_processing = true;
                app.stop_reason = "Cancelling and containing active work…".to_string();
                app.add_segment(
                    format!("\n{} [STOPPING]\n", icons::WARNING),
                    BlockType::Text,
                );
            } else {
                app.stop_reason = "No active work to cancel".to_string();
            }
            app.tool_output_preview.clear();
            app.should_redraw = true;
        }
        AppEventOutcome::Continue => {
            app.should_redraw = true;
        }
        AppEventOutcome::Exit => unreachable!("exit is handled before borrowing runtime state"),
    }

    // LSP install requested from the server panel
    if let Some(cmd) = app.lsp_install_cmd.take() {
        if !app.is_fully_idle() {
            app.stop_reason =
                "⚠ Wait for the active turn before installing an LSP server".to_string();
            app.should_redraw = true;
        } else {
            app.add_segment(
                format!("\n⟳ Installing LSP server…\n$ {}\n", cmd),
                BlockType::Text,
            );
            if persist_before_side_effect(app, "start the LSP installer", SideEffectKind::Tool) {
                if let Err(error) = app.begin_standalone_cancellation() {
                    app.stop_reason = format!("⚠ LSP installer could not start: {error}");
                    app.should_redraw = true;
                    return AppEventControl::Handled;
                }
                app.stop_reason = "⟳ Installing LSP server…".to_string();
                app.lsp_install_in_progress = true;
                app.lsp_install_cancel_pending = false;
                app.is_executing_tool = true;
                *cancellation_token = shutdown_cancellation.child_token();
                let ctx_tx = (*tx).clone();
                let tool_cancel = (*cancellation_token).clone();
                let cwd = app.current_dir.clone();
                tokio::spawn(async move {
                    let execution = lethetic::tools::execute(
                        "run_shell_command",
                        &serde_json::json!({"command": cmd, "description": "Install LSP server", "tool_call_id": "lsp_install"}),
                        &cwd,
                        tool_cancel,
                        ctx_tx.clone(),
                        &reqwest::Client::new(),
                        &lethetic::config::Config::default(),
                    )
                    .await;
                    let _ = ctx_tx.send(StreamEvent::ToolResult {
                        id: None,
                        func_name: "lsp_install".to_string(),
                        result: execution.output,
                        cwd: execution.cwd,
                        is_error: execution.is_error,
                        provenance: execution.provenance,
                    });
                });
            }
        }
    }

    AppEventControl::Handled
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lifecycle::RuntimeMode;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn every_literal_python_policy_survives_session_transition_reset() {
        for (mode, network, packages) in [
            (
                crate::cli::LiteralPythonMode::FullyIsolated,
                lethetic::config::NetworkAccess::None,
                lethetic::config::PackageAccess::Disabled,
            ),
            (
                crate::cli::LiteralPythonMode::Nonlocal,
                lethetic::config::NetworkAccess::Nonlocal,
                lethetic::config::PackageAccess::Session,
            ),
            (
                crate::cli::LiteralPythonMode::Permissive,
                lethetic::config::NetworkAccess::Full,
                lethetic::config::PackageAccess::Disabled,
            ),
        ] {
            let baseline = Config::default();
            let mut config = baseline.clone();
            crate::cli::apply_literal_python_mode(&mut config, mode).unwrap();
            let policy = lethetic::python_policy::PythonPolicyState::from_config(
                &baseline,
                lethetic::python_policy::PythonPolicySource::Config,
            )
            .with_process_literal(&config)
            .unwrap();
            let mut app = App::new_with_python_policy_state(&config, policy);

            prepare_python_policy_for_session_transition(&mut app, &mut config)
                .await
                .unwrap();

            assert_eq!(
                config.tool_profile,
                lethetic::config::ToolProfile::PythonOnly
            );
            assert_eq!(config.python_runtime.sandbox.network, Some(network));
            assert_eq!(config.python_runtime.sandbox.package_access, packages);
            assert_eq!(
                config.python_invocation.workspace_exposure,
                lethetic::config::PythonWorkspaceExposure::SharedLaunchCwd
            );
            assert_eq!(app.config.tool_profile, config.tool_profile);
            assert_eq!(app.config.python_runtime, config.python_runtime);
            assert_eq!(app.config.python_invocation, config.python_invocation);
            assert_eq!(
                app.python_policy.effective_source(),
                lethetic::python_policy::PythonPolicySource::CliLocked
            );
            let tools =
                lethetic::tools::get_api_tools(&config, lethetic::tools::ToolSurface::Interactive);
            assert_eq!(tools.len(), 1);
            assert_eq!(tools[0].name, "python");
        }
    }

    #[tokio::test]
    async fn invalid_python_policy_rejects_prompt_before_provider_or_user_turn() {
        let mut config = Config {
            context_size: 100_000,
            tool_profile: lethetic::config::ToolProfile::PythonOnly,
            ..Default::default()
        };
        let mut app = App::new(&config);
        app.blocks.clear();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut runtime = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Interactive);

        runtime
            .dispatch_app_event(AppEventOutcome::SendPrompt("must not send".to_string()))
            .await;

        assert!(!runtime.app.is_processing);
        assert!(
            runtime
                .app
                .context_manager
                .get_messages()
                .iter()
                .all(|message| message.role != "user")
        );
        assert!(
            runtime
                .app
                .blocks
                .iter()
                .any(|block| block.block_type == BlockType::ProviderError)
        );
        assert!(matches!(
            rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn global_policy_preparation_uses_the_effective_project_override() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().canonicalize().unwrap();
        let mut project_config = Config::default();
        project_config.python_runtime.python_executable = "project-executable".to_string();
        project_config.python_runtime.sandbox.podman_image = "project-image".to_string();
        let project_snapshot =
            lethetic::python_policy::PythonPolicySnapshot::from_config(&project_config);
        lethetic::python_policy::persist_policy(
            lethetic::python_policy::PythonPolicyScope::Project,
            &workspace,
            &project_snapshot,
            Some(&lethetic::python_policy::PolicyRevision::Missing),
        )
        .unwrap();

        let mut requested_config = Config::default();
        requested_config.python_runtime.python_executable = "trusted-global-executable".to_string();
        requested_config.python_runtime.sandbox.podman_image = "global-image".to_string();
        let requested =
            lethetic::python_policy::PythonPolicySnapshot::from_config(&requested_config);
        let expected_revision = Some(lethetic::python_policy::PolicyRevision::Missing);
        let (expected_effective, expected_source) =
            lethetic::python_policy::effective_policy_after_write(
                lethetic::python_policy::PythonPolicyScope::Global,
                &workspace,
                &requested,
            )
            .unwrap();

        let mut config = Config {
            context_size: 100_000,
            ..Default::default()
        };
        let mut app = App::new(&config);
        app.current_dir = workspace.to_string_lossy().into_owned();
        app.tool_runtime = lethetic::tool_runtime::ToolRuntime::interactive(workspace.clone());
        let mut setup = lethetic::python_setup::PythonSetupDialog::new(&config, workspace.clone());
        setup.profile = requested.tool_profile;
        setup.runtime = requested.python_runtime.clone();
        setup.persistence = lethetic::python_setup::PolicyPersistence::Global;
        setup.global_revision = lethetic::python_policy::PolicyRevision::Missing;
        setup.stage = lethetic::python_setup::PythonSetupStage::Applying;
        app.python_setup = Some(setup);
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut runtime = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Interactive);

        runtime
            .dispatch_app_event(AppEventOutcome::ApplyPythonPolicy {
                snapshot: requested.clone(),
                persistence: lethetic::python_setup::PolicyPersistence::Global,
                expected_revision: expected_revision.clone(),
            })
            .await;

        let settlement = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            runtime.settle_python_setup_operation(),
        )
        .await
        .expect("policy preparation timed out")
        .expect("policy preparation operation disappeared");
        assert!(!settlement.dismiss_when_settled);
        let Ok(PythonSetupCompletion::PolicyPrepared {
            snapshot,
            effective_snapshot,
            effective_source,
            persistence,
            expected_revision: prepared_revision,
            validation,
        }) = settlement.completion
        else {
            panic!("unexpected policy preparation completion");
        };
        assert_eq!(snapshot, requested);
        assert_eq!(effective_snapshot.as_ref(), &expected_effective);
        assert_eq!(effective_source, expected_source);
        assert_eq!(
            effective_source,
            lethetic::python_policy::PythonPolicySource::Project
        );
        assert_eq!(
            effective_snapshot.python_runtime.python_executable,
            "trusted-global-executable"
        );
        assert_eq!(
            persistence,
            lethetic::python_setup::PolicyPersistence::Global
        );
        assert_eq!(prepared_revision, expected_revision);
        assert_eq!(validation, Ok(()));

        install_python_policy(
            runtime.app,
            runtime.config,
            effective_snapshot.as_ref().clone(),
            effective_source,
            true,
        )
        .unwrap();
        assert_eq!(
            lethetic::python_policy::PythonPolicySnapshot::from_config(runtime.config),
            effective_snapshot.as_ref().clone()
        );
        assert_eq!(
            runtime.app.python_policy.persisted_source(),
            effective_source
        );
    }

    #[tokio::test]
    async fn ask_user_answer_stays_ui_only_until_native_tool_result() {
        let mut config = Config {
            context_size: 100_000,
            ..Default::default()
        };
        let mut app = App::new(&config);
        let tool_call = lethetic::context::ToolCall {
            id: "question-one".to_string(),
            provider_id: Some("provider-question-one".to_string()),
            function: lethetic::context::FunctionCall {
                name: "ask_the_user".to_string(),
                arguments: serde_json::json!({"question": "Which option?"}),
            },
        };
        app.context_manager
            .upsert_assistant_tool_call_with_provider("", vec![tool_call.clone()], None);
        app.pending_tool_call = Some(tool_call);
        app.is_asking_user = true;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut runtime = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Interactive);

        runtime
            .dispatch_app_event(AppEventOutcome::SendPrompt("the answer".to_string()))
            .await;

        let StreamEvent::ToolResult {
            id,
            func_name,
            result,
            is_error,
            ..
        } = rx.recv().await.unwrap()
        else {
            panic!("ask-user answer did not become a tool result");
        };
        assert_eq!(id.as_deref(), Some("question-one"));
        assert_eq!(func_name, "ask_the_user");
        assert_eq!(result, "the answer");
        assert!(!is_error);
        assert!(
            runtime
                .app
                .context_manager
                .get_messages()
                .iter()
                .all(|message| message.role != "user")
        );

        runtime.app.context_manager.add_tool_message_with_status(
            "question-one".to_string(),
            "ask_the_user",
            &result,
            false,
        );
        let messages = runtime.app.context_manager.get_messages();
        let roles = messages
            .iter()
            .map(|message| message.role.as_str())
            .collect::<Vec<_>>();
        assert_eq!(roles, ["assistant", "tool"]);
        assert_eq!(
            messages
                .iter()
                .filter(|message| message.content.contains("the answer"))
                .count(),
            1
        );
        assert!(
            runtime.app.blocks.iter().any(|block| {
                block.block_type == BlockType::User && block.content == "the answer"
            })
        );
    }

    #[tokio::test]
    async fn stopping_a_pending_tool_records_a_typed_terminal_failure() {
        let mut config = Config {
            context_size: 100_000,
            ..Default::default()
        };
        let mut app = App::new(&config);
        app.blocks.clear();
        let tool_call = lethetic::context::ToolCall {
            id: "pending-stop".to_string(),
            provider_id: None,
            function: lethetic::context::FunctionCall {
                name: "calculate".to_string(),
                arguments: serde_json::json!({
                    "expression": "2 + 2",
                    "description": "Pending calculation"
                }),
            },
        };
        app.context_manager
            .upsert_assistant_tool_call_with_provider("", vec![tool_call.clone()], None);
        app.pending_tool_call = Some(tool_call);
        app.show_approval_prompt = true;
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut runtime = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Interactive);

        runtime.dispatch_app_event(AppEventOutcome::Stop).await;

        assert!(runtime.app.pending_tool_call.is_none());
        let result = runtime.app.context_manager.get_messages().last().unwrap();
        assert_eq!(result.role, "tool");
        assert!(result.tool_result_is_error);
        let failure = runtime
            .app
            .blocks
            .iter()
            .find(|block| block.block_type == BlockType::ToolError)
            .unwrap();
        assert_eq!(failure.success, Some(false));
        assert_eq!(
            failure.content.trim(),
            crate::provider::PENDING_INTERACTION_CANCELLED_RESULT
        );
    }

    #[tokio::test]
    async fn model_switch_updates_theme_selection_and_invalidates_cached_blocks() {
        let mut config = Config {
            context_size: 100_000,
            model: "before".to_string(),
            ..Default::default()
        };
        let mut app = App::new(&config);
        let target_index = app
            .themes
            .iter()
            .position(|theme| theme.name != app.theme.name)
            .expect("test requires two themes");
        let target_name = app.themes[target_index].name.clone();
        config.theme = Some(target_name.clone());
        app.add_segment("cached".to_string(), BlockType::Text);
        let block = app.blocks.last_mut().unwrap();
        block.cached_lines = Some(vec!["cached".to_string().into()]);
        block.cached_line_count = Some(1);
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut runtime = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Interactive);

        runtime
            .dispatch_app_event(AppEventOutcome::SwitchModel(
                "__active".to_string(),
                "after".to_string(),
            ))
            .await;

        assert_eq!(runtime.app.theme.name, target_name);
        assert_eq!(runtime.app.theme_state.selected(), Some(target_index));
        assert!(
            runtime
                .app
                .blocks
                .iter()
                .all(|block| { block.cached_lines.is_none() && block.cached_line_count.is_none() })
        );
    }

    #[tokio::test]
    async fn model_switch_reasserts_cli_locked_python_policy_across_connections() {
        let target: lethetic::config::ModelServer = serde_yaml::from_str(
            "id: proxy\nname: Proxy\nkind: claude_code_proxy\nurl: http://proxy\nmodel: target\nparser: default\n",
        )
        .unwrap();
        let baseline = Config {
            server_url: "http://before".to_string(),
            model: "before".to_string(),
            context_size: 100_000,
            model_servers: vec![target],
            ..Default::default()
        };
        let mut config = baseline.clone();
        crate::cli::apply_literal_python_mode(
            &mut config,
            crate::cli::LiteralPythonMode::Permissive,
        )
        .unwrap();
        let policy = lethetic::python_policy::PythonPolicyState::from_config(
            &baseline,
            lethetic::python_policy::PythonPolicySource::Config,
        )
        .with_process_literal(&config)
        .unwrap();
        let mut app = App::new_with_python_policy_state(&config, policy);
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut runtime = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Interactive);

        runtime
            .dispatch_app_event(AppEventOutcome::SwitchModel(
                "proxy".to_string(),
                "target".to_string(),
            ))
            .await;

        assert_eq!(runtime.config.model, "target");
        assert_eq!(
            runtime.config.connection_kind,
            lethetic::config::ConnectionKind::ClaudeCodeProxy
        );
        assert_eq!(
            runtime.config.tool_profile,
            lethetic::config::ToolProfile::PythonOnly
        );
        assert_eq!(
            runtime.config.python_runtime.sandbox.network,
            Some(lethetic::config::NetworkAccess::Full)
        );
        assert_eq!(
            runtime.config.python_invocation.workspace_exposure,
            lethetic::config::PythonWorkspaceExposure::SharedLaunchCwd
        );
        assert_eq!(runtime.app.config.tool_profile, runtime.config.tool_profile);
        assert_eq!(
            runtime.app.python_policy.effective_source(),
            lethetic::python_policy::PythonPolicySource::CliLocked
        );
        let tools = lethetic::tools::get_api_tools(
            runtime.config,
            lethetic::tools::ToolSurface::Interactive,
        );
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "python");
    }

    #[tokio::test]
    async fn model_switch_rejects_reserved_extra_body_without_committing_candidate() {
        let target: lethetic::config::ModelServer = serde_yaml::from_str(
            "id: guarded\nname: Guarded\nurl: http://guarded\nmodel: target\nextra_body:\n  tools: []\n",
        )
        .unwrap();
        let mut config = Config {
            server_url: "http://before".to_string(),
            model: "before".to_string(),
            context_size: 100_000,
            model_servers: vec![target],
            ..Default::default()
        };
        let mut app = App::new(&config);
        let original_app_config = app.config.clone();
        let original_runtime_config = config.clone();
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut runtime = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Interactive);

        runtime
            .dispatch_app_event(AppEventOutcome::SwitchModel(
                "guarded".to_string(),
                "target".to_string(),
            ))
            .await;

        assert_eq!(runtime.config.model, original_runtime_config.model);
        assert_eq!(
            runtime.config.server_url,
            original_runtime_config.server_url
        );
        assert_eq!(runtime.app.config.model, original_app_config.model);
        assert_eq!(
            runtime.app.config.server_url,
            original_app_config.server_url
        );
        assert!(runtime.app.stop_reason.contains("model server 'guarded'"));
        assert!(runtime.app.stop_reason.contains("tools"));
    }

    #[tokio::test]
    async fn control_enum_preserves_continue_boundaries_and_exit_gating() {
        let mut config = Config::default();
        let mut app = App::new(&config);
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut runtime = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Interactive);

        let control = runtime.dispatch_app_event(AppEventOutcome::Continue).await;
        assert_eq!(control, AppEventControl::Handled);
        assert!(runtime.app.should_redraw);

        runtime.app.lsp_install_cmd = Some("installer must remain pending".to_string());
        let control = runtime.dispatch_app_event(AppEventOutcome::Exit).await;
        assert_eq!(control, AppEventControl::ContinueRunLoop);
        assert!(runtime.lifecycle.is_shutting_down());
        assert_eq!(
            runtime.app.lsp_install_cmd.as_deref(),
            Some("installer must remain pending")
        );
        assert!(!runtime.app.lsp_install_in_progress);
    }
}
