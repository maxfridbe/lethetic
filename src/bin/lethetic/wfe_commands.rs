use lethetic::app::AppEventOutcome;
use lethetic::wfe::contracts::{CommandErrorCode, CommandOutcome, LspAction, PanelId, WebCommand};
use lethetic::wfe::runtime::{AdmittedCommand, RuntimeFailure, WfeRuntime};

use crate::app_events::activate_python_policy;
use crate::context::RuntimeContext;
use crate::provider::settle_pending_interaction_checked;
use crate::session_transition::start_committed_session_transition;

pub(crate) struct PendingWfeSessionLoad {
    pub(crate) command: AdmittedCommand,
    pub(crate) session_id: String,
}

pub(crate) enum WfeCommandDisposition {
    Complete {
        command: AdmittedCommand,
        result: Result<CommandOutcome, RuntimeFailure>,
    },
    AwaitSessionLoad(PendingWfeSessionLoad),
}

pub(crate) fn wfe_failure(
    code: CommandErrorCode,
    message: &'static str,
    retryable: bool,
) -> RuntimeFailure {
    RuntimeFailure::new(code, message, retryable)
}

fn require_confirmation(
    wfe: &mut WfeRuntime,
    confirmation: &WebCommand,
    title: &'static str,
    message: &'static str,
) -> Result<CommandOutcome, RuntimeFailure> {
    match wfe.begin_confirmation_for_command(confirmation, title, message) {
        Ok(_) => Err(wfe_failure(
            CommandErrorCode::ConfirmationRequired,
            "Explicit confirmation is required",
            false,
        )),
        Err(_) => Err(wfe_failure(
            CommandErrorCode::Internal,
            "Confirmation could not be created safely",
            false,
        )),
    }
}

fn wfe_panel_for_command(command: lethetic::commands::CommandId) -> Option<PanelId> {
    use lethetic::commands::CommandId;
    match command {
        CommandId::Hotkeys => Some(PanelId::Hotkeys),
        CommandId::Themes => Some(PanelId::Themes),
        CommandId::InputHistory => Some(PanelId::InputHistory),
        CommandId::SystemPrompt => Some(PanelId::SystemPrompt),
        CommandId::Sessions => Some(PanelId::Sessions),
        CommandId::NameSession => Some(PanelId::NameSession),
        CommandId::LatestFiles => Some(PanelId::LatestFiles),
        CommandId::Models => Some(PanelId::Models),
        CommandId::LspServers => Some(PanelId::LspServers),
        CommandId::AgentMode
        | CommandId::AgentGeneral
        | CommandId::PythonIsolated
        | CommandId::PythonNonlocal
        | CommandId::PythonPermissive => Some(PanelId::AgentMode),
        CommandId::LoopDetection
        | CommandId::ClearUi
        | CommandId::ClearContext
        | CommandId::ToggleDebugger
        | CommandId::DeletePythonRuntime
        | CommandId::Quit => None,
    }
}

/// This command match deliberately remains heap-pinned. Its many typed command
/// variants produce a large debug-build future that can overflow the standard
/// test/runtime thread stack when returned inline. RuntimeContext still removes
/// the former argument staircase and dispatch macros without changing await
/// ordering or borrow lifetimes.
pub(crate) fn execute_wfe_command<'a>(
    command: AdmittedCommand,
    wfe: &'a mut WfeRuntime,
    context: &'a mut RuntimeContext<'_>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = WfeCommandDisposition> + 'a>> {
    Box::pin(async move {
        let web_command = command.request.command.clone();
        let confirmation_command = web_command.clone();
        let result = match web_command {
            WebCommand::InvokeCommand { command_id } => {
                let view = context.app.command_view(command_id);
                if !view.enabled {
                    Err(wfe_failure(
                        CommandErrorCode::Busy,
                        "Command is not available in the current state",
                        true,
                    ))
                } else if command_id == lethetic::commands::CommandId::LoopDetection {
                    wfe.open_loop_modes(context.app);
                    Ok(CommandOutcome::PanelOpened {
                        panel: PanelId::LoopDetection,
                    })
                } else if command_id.opens_agent_mode() {
                    wfe.open_agent_modes(context.app);
                    Ok(CommandOutcome::PanelOpened {
                        panel: PanelId::AgentMode,
                    })
                } else {
                    let outcome = lethetic::app::dispatch_command(context.app, command_id);
                    let _ = context.dispatch_app_event(outcome).await;
                    if command_id == lethetic::commands::CommandId::NameSession
                        && !context.app.show_session_name_dialog
                    {
                        Err(wfe_failure(
                            CommandErrorCode::SaveFailed,
                            "Session naming could not be opened safely",
                            true,
                        ))
                    } else if let Some(panel) = wfe_panel_for_command(command_id) {
                        Ok(CommandOutcome::PanelOpened { panel })
                    } else {
                        Ok(CommandOutcome::Applied)
                    }
                }
            }
            WebCommand::SendPrompt { prompt, .. } => {
                let _ = context
                    .dispatch_app_event(AppEventOutcome::SendPrompt(prompt))
                    .await;
                if context.app.is_processing {
                    Ok(CommandOutcome::PromptAccepted {
                        session_id: context.app.session_id.clone(),
                    })
                } else {
                    Err(wfe_failure(
                        CommandErrorCode::BackendUnavailable,
                        "Prompt could not be started safely",
                        true,
                    ))
                }
            }
            WebCommand::Stop {
                session_id,
                cancel_id,
            } => {
                if context.app.session_id != session_id
                    || context.app.live_cancellation_id() != Some(cancel_id.as_str())
                {
                    Err(wfe_failure(
                        CommandErrorCode::NotFound,
                        "Cancellation target no longer matches active work",
                        false,
                    ))
                } else if (context.app.show_approval_prompt || context.app.is_asking_user)
                    && !context.app.is_executing_tool
                {
                    match settle_pending_interaction_checked(context.app) {
                        Ok(true) => {
                            context.cancellation_pending = false;
                            Ok(CommandOutcome::Applied)
                        }
                        Ok(false) => Err(wfe_failure(
                            CommandErrorCode::NotFound,
                            "Pending interaction is no longer active",
                            false,
                        )),
                        Err(_) => Err(wfe_failure(
                            CommandErrorCode::SaveFailed,
                            "Pending interaction cancellation could not be saved",
                            true,
                        )),
                    }
                } else {
                    let _ = context.dispatch_app_event(AppEventOutcome::Stop).await;
                    Ok(CommandOutcome::Applied)
                }
            }
            WebCommand::ApproveToolOnce { tool_call_id, .. } => {
                let _ = context
                    .dispatch_app_event(AppEventOutcome::ToolApproved(true, false))
                    .await;
                Ok(CommandOutcome::ToolDecisionRecorded { tool_call_id })
            }
            WebCommand::ApproveToolAlways { tool_call_id, .. } => {
                let _ = context
                    .dispatch_app_event(AppEventOutcome::ToolApproved(true, true))
                    .await;
                Ok(CommandOutcome::ToolDecisionRecorded { tool_call_id })
            }
            WebCommand::DenyTool { tool_call_id, .. } => {
                let _ = context
                    .dispatch_app_event(AppEventOutcome::ToolApproved(false, false))
                    .await;
                Ok(CommandOutcome::ToolDecisionRecorded { tool_call_id })
            }
            WebCommand::RenameSession { name, .. } => {
                let value = name.unwrap_or_default();
                match context.app.set_session_display_name(&value) {
                    Ok(()) => {
                        context.app.show_session_name_dialog = false;
                        wfe.clear_requested_panel();
                        Ok(CommandOutcome::SessionRenamed {
                            session_id: context.app.session_id.clone(),
                            display_name: context.app.display_name.clone(),
                        })
                    }
                    Err(_) => Err(wfe_failure(
                        CommandErrorCode::SaveFailed,
                        "Session name could not be saved",
                        true,
                    )),
                }
            }
            WebCommand::AnswerUser {
                tool_call_id,
                answers,
                ..
            } => {
                let answer = answers
                    .into_iter()
                    .next()
                    .and_then(|answer| answer.other_text)
                    .unwrap_or_default();
                let _ = context
                    .dispatch_app_event(AppEventOutcome::SendPrompt(answer))
                    .await;
                if !context.app.is_asking_user
                    && context
                        .app
                        .pending_tool_call
                        .as_ref()
                        .is_some_and(|pending| pending.id == tool_call_id)
                {
                    Ok(CommandOutcome::ToolDecisionRecorded { tool_call_id })
                } else {
                    Err(wfe_failure(
                        CommandErrorCode::SaveFailed,
                        "The answer could not be continued safely",
                        true,
                    ))
                }
            }
            WebCommand::SelectTheme { theme_id } => {
                let Some(index) =
                    lethetic::wfe::runtime::theme_choice_index(context.app, &theme_id)
                else {
                    return WfeCommandDisposition::Complete {
                        command,
                        result: Err(wfe_failure(
                            CommandErrorCode::NotFound,
                            "Theme choice is no longer available",
                            true,
                        )),
                    };
                };
                let previous_theme = context.app.theme.clone();
                let previous_selection = context.app.theme_state.selected();
                let previous_needs_save = context.app.needs_save;
                context.app.theme_state.select(Some(index));
                context.app.theme = context.app.themes[index].clone();
                for block in &mut context.app.blocks {
                    block.invalidate();
                }
                context.app.needs_save = true;
                match context.app.save_session_checked() {
                    Ok(()) => {
                        context.app.show_theme_menu = false;
                        wfe.clear_requested_panel();
                        Ok(CommandOutcome::Applied)
                    }
                    Err(_) => {
                        context.app.theme = previous_theme;
                        context.app.theme_state.select(previous_selection);
                        context.app.needs_save = previous_needs_save;
                        Err(wfe_failure(
                            CommandErrorCode::SaveFailed,
                            "Theme choice could not be saved",
                            true,
                        ))
                    }
                }
            }
            WebCommand::SelectModel { model_id } => {
                if !context.app.is_fully_idle() {
                    Err(wfe_failure(
                        CommandErrorCode::Busy,
                        "Wait for active work before switching models",
                        true,
                    ))
                } else if let Some((connection_id, selected_model)) =
                    lethetic::wfe::runtime::model_choice_map(context.app).remove(&model_id)
                {
                    let available = context.app.available_models.iter().any(|choice| {
                        choice.connection_id == connection_id
                            && choice.model_id == selected_model
                            && choice.available
                    });
                    if !available {
                        Err(wfe_failure(
                            CommandErrorCode::NotFound,
                            "Model choice is unavailable",
                            true,
                        ))
                    } else {
                        let _ = context
                            .dispatch_app_event(AppEventOutcome::SwitchModel(
                                connection_id,
                                selected_model.clone(),
                            ))
                            .await;
                        if context.app.model_name == selected_model {
                            context.app.show_model_switcher = false;
                            if context.app.save_session_checked().is_err() {
                                Err(wfe_failure(
                                    CommandErrorCode::SaveFailed,
                                    "Model choice could not be saved",
                                    true,
                                ))
                            } else {
                                wfe.clear_requested_panel();
                                Ok(CommandOutcome::Applied)
                            }
                        } else {
                            Err(wfe_failure(
                                CommandErrorCode::BackendUnavailable,
                                "Model choice could not be activated",
                                true,
                            ))
                        }
                    }
                } else {
                    Err(wfe_failure(
                        CommandErrorCode::NotFound,
                        "Model choice is no longer available",
                        true,
                    ))
                }
            }
            WebCommand::NewSession => {
                match start_committed_session_transition(
                    context.app,
                    context.config,
                    context.shutdown_cancellation.child_token(),
                )
                .await
                {
                    Ok(committed) => {
                        wfe.clear_requested_panel();
                        Ok(CommandOutcome::SessionCreated {
                            session_id: committed.session_id,
                        })
                    }
                    Err(_) => Err(wfe_failure(
                        CommandErrorCode::SaveFailed,
                        "A new session could not be created safely",
                        false,
                    )),
                }
            }
            WebCommand::ResumeSession { session_id } => {
                if context.app.current_session_dir.is_some() && context.app.session_id == session_id
                {
                    wfe.clear_requested_panel();
                    Ok(CommandOutcome::SessionLoaded { session_id })
                } else {
                    let _ = context
                        .dispatch_app_event(AppEventOutcome::ResumeSession(session_id.clone()))
                        .await;
                    if context.app.is_loading_session {
                        return WfeCommandDisposition::AwaitSessionLoad(PendingWfeSessionLoad {
                            command,
                            session_id,
                        });
                    }
                    Err(wfe_failure(
                        CommandErrorCode::BackendUnavailable,
                        "Session loading could not be started safely",
                        true,
                    ))
                }
            }
            WebCommand::DeleteSession {
                session_id,
                confirmed,
                ..
            } => {
                if !confirmed {
                    require_confirmation(
                        wfe,
                        &confirmation_command,
                        "Delete session?",
                        "This permanently deletes the selected session and its bound runtime.",
                    )
                } else {
                    wfe.clear_confirmation();
                    let target = session_id.clone();
                    let _ = context
                        .dispatch_app_event(AppEventOutcome::DeleteSession(session_id))
                        .await;
                    context.app.refresh_session_list();
                    if context
                        .app
                        .session_ids_for_cleanup()
                        .iter()
                        .any(|id| id == &target)
                    {
                        Err(wfe_failure(
                            CommandErrorCode::BackendUnavailable,
                            "Session deletion did not complete",
                            true,
                        ))
                    } else {
                        Ok(CommandOutcome::SessionDeleted { session_id: target })
                    }
                }
            }
            WebCommand::WipeSessions { confirmed, .. } => {
                if !confirmed {
                    require_confirmation(
                        wfe,
                        &confirmation_command,
                        "Delete all sessions?",
                        "This permanently deletes all sessions and their bound runtimes.",
                    )
                } else {
                    wfe.clear_confirmation();
                    let targets = context.app.session_ids_for_cleanup();
                    let _ = context
                        .dispatch_app_event(AppEventOutcome::WipeSessions)
                        .await;
                    context.app.refresh_session_list();
                    let remaining = context.app.session_ids_for_cleanup();
                    if targets
                        .iter()
                        .all(|target| !remaining.iter().any(|session| session == target))
                    {
                        Ok(CommandOutcome::SessionsWiped)
                    } else {
                        Err(wfe_failure(
                            CommandErrorCode::BackendUnavailable,
                            "Some sessions could not be deleted",
                            true,
                        ))
                    }
                }
            }
            WebCommand::SelectHistoryEntry {
                session_id,
                entry_id,
            } => {
                if context.app.session_id != session_id {
                    Err(wfe_failure(
                        CommandErrorCode::SessionMismatch,
                        "History choice no longer targets the active session",
                        true,
                    ))
                } else {
                    match wfe.lossless_history_entry(context.app, &entry_id) {
                        Ok(editor_content) => {
                            context.app.show_history = false;
                            wfe.clear_requested_panel();
                            Ok(CommandOutcome::HistoryEntrySelected {
                                session_id,
                                entry_id,
                                editor_content,
                            })
                        }
                        Err(error) => Err(error),
                    }
                }
            }
            WebCommand::SelectLatestFile { file_id, .. } => {
                if let Some(path) =
                    lethetic::wfe::runtime::file_choice_map(context.app).remove(&file_id)
                {
                    context.app.context_manager.remove_latest_file(&path);
                    Ok(CommandOutcome::Applied)
                } else {
                    Err(wfe_failure(
                        CommandErrorCode::NotFound,
                        "File choice is no longer available",
                        true,
                    ))
                }
            }
            WebCommand::SelectSystemPrompt { prompt_id, .. } => {
                if let Some(name) =
                    lethetic::wfe::runtime::prompt_choice_map(context.app).remove(&prompt_id)
                {
                    match context.app.system_prompt_manager.load_prompt_checked(&name) {
                        Ok(Some(content)) => {
                            context.app.system_prompt = content;
                            context.app.prompt_save_name = name;
                            context.app.show_prompt_manager = false;
                            context.app.show_prompt_editor = true;
                            context.app.prompt_cursor_pos = context.app.system_prompt.len();
                            Ok(CommandOutcome::Applied)
                        }
                        Ok(None) | Err(_) => Err(wfe_failure(
                            CommandErrorCode::NotFound,
                            "System prompt could not be loaded safely",
                            true,
                        )),
                    }
                } else {
                    Err(wfe_failure(
                        CommandErrorCode::NotFound,
                        "System prompt choice is no longer available",
                        true,
                    ))
                }
            }
            WebCommand::SaveSystemPrompt {
                name,
                content,
                confirmed_overwrite,
                ..
            } => match lethetic::system_prompt::normalize_system_prompt_name(&name) {
                Err(_) => Err(wfe_failure(
                    CommandErrorCode::BadRequest,
                    "Prompt name contains unsupported characters",
                    false,
                )),
                Ok(name) => match context
                    .app
                    .system_prompt_manager
                    .prompt_exists_checked(&name)
                {
                    Err(_) => Err(wfe_failure(
                        CommandErrorCode::BackendUnavailable,
                        "System prompt could not be inspected safely",
                        true,
                    )),
                    Ok(true) if !confirmed_overwrite => require_confirmation(
                        wfe,
                        &confirmation_command,
                        "Overwrite system prompt?",
                        "This replaces the selected saved system prompt.",
                    ),
                    Ok(_) => {
                        if confirmed_overwrite {
                            wfe.clear_confirmation();
                        }
                        match context.app.system_prompt_manager.save_prompt_checked(
                            &name,
                            &content,
                            confirmed_overwrite,
                        ) {
                            Ok(()) => {
                                context.app.system_prompt = content;
                                context.app.prompt_save_name = name;
                                context.app.refresh_prompt_list();
                                let resolved =
                                    lethetic::system_prompt::SystemPromptManager::resolve_prompt(
                                        &context.app.system_prompt,
                                        &context.app.current_dir,
                                        &context.app.config,
                                    );
                                context.app.context_manager.update_system_prompt(resolved);
                                Ok(CommandOutcome::Applied)
                            }
                            Err(_) => Err(wfe_failure(
                                CommandErrorCode::SaveFailed,
                                "System prompt could not be saved safely",
                                true,
                            )),
                        }
                    }
                },
            },
            WebCommand::SetLoopDetection { mode_id, .. } => {
                if let Some(mode) = lethetic::wfe::runtime::loop_mode_map().remove(&mode_id) {
                    let previous = context.app.loop_detector.config.mode;
                    let previous_needs_save = context.app.needs_save;
                    context.app.loop_detector.config.mode = mode;
                    context.app.needs_save = true;
                    match context.app.save_session_checked() {
                        Ok(()) => {
                            wfe.clear_requested_panel();
                            Ok(CommandOutcome::Applied)
                        }
                        Err(_) => {
                            context.app.loop_detector.config.mode = previous;
                            context.app.needs_save = previous_needs_save;
                            Err(wfe_failure(
                                CommandErrorCode::SaveFailed,
                                "Loop detection mode could not be saved",
                                true,
                            ))
                        }
                    }
                } else {
                    Err(wfe_failure(
                        CommandErrorCode::NotFound,
                        "Loop detection mode is no longer available",
                        true,
                    ))
                }
            }
            WebCommand::SetAgentMode { mode_id, .. } => {
                if context.app.python_policy.is_cli_locked() {
                    Err(wfe_failure(
                        CommandErrorCode::BadRequest,
                        lethetic::python_policy::CLI_PYTHON_POLICY_LOCKED_ERROR,
                        false,
                    ))
                } else if !context.app.is_fully_idle() {
                    Err(wfe_failure(
                        CommandErrorCode::Busy,
                        "Wait for active work before changing Agent Mode",
                        true,
                    ))
                } else if let Some(profile) =
                    lethetic::wfe::runtime::agent_mode_map().remove(&mode_id)
                {
                    let mut snapshot =
                        lethetic::python_policy::PythonPolicySnapshot::from_config(context.config);
                    snapshot.tool_profile = profile;
                    if snapshot.validate().is_err() {
                        Err(wfe_failure(
                            CommandErrorCode::BadRequest,
                            "The selected Agent Mode has an incomplete Python policy",
                            false,
                        ))
                    } else {
                        match activate_python_policy(
                            context.app,
                            context.config,
                            snapshot,
                            lethetic::python_policy::PythonPolicySource::OneTime,
                            false,
                        )
                        .await
                        {
                            Ok(()) => {
                                context.app.python_setup = None;
                                wfe.clear_requested_panel();
                                Ok(CommandOutcome::Applied)
                            }
                            Err(_) => Err(wfe_failure(
                                CommandErrorCode::BackendUnavailable,
                                "Agent Mode could not be changed safely",
                                true,
                            )),
                        }
                    }
                } else {
                    Err(wfe_failure(
                        CommandErrorCode::NotFound,
                        "Agent Mode choice is no longer available",
                        true,
                    ))
                }
            }
            WebCommand::RunLspAction {
                server_id, action, ..
            } => {
                let Some(server) = lethetic::wfe::runtime::lsp_choice_map().remove(&server_id)
                else {
                    return WfeCommandDisposition::Complete {
                        command,
                        result: Err(wfe_failure(
                            CommandErrorCode::NotFound,
                            "LSP server choice is no longer available",
                            true,
                        )),
                    };
                };
                match action {
                    LspAction::Install => {
                        if !context.app.is_fully_idle() {
                            Err(wfe_failure(
                                CommandErrorCode::Busy,
                                "Wait for active work before installing an LSP server",
                                true,
                            ))
                        } else if let Some(install_cmd) = server.install_cmd {
                            if lethetic::lsp::registry::check_installed(server) {
                                Ok(CommandOutcome::Applied)
                            } else {
                                context.app.show_lsp_manager = false;
                                context.app.lsp_install_cmd = Some(install_cmd.to_string());
                                let _ = context.dispatch_app_event(AppEventOutcome::Continue).await;
                                if context.app.lsp_install_in_progress {
                                    Ok(CommandOutcome::Applied)
                                } else {
                                    Err(wfe_failure(
                                        CommandErrorCode::SaveFailed,
                                        "LSP installation could not be started safely",
                                        true,
                                    ))
                                }
                            }
                        } else {
                            Err(wfe_failure(
                                CommandErrorCode::BackendUnavailable,
                                "No automatic installer is available for this LSP server",
                                false,
                            ))
                        }
                    }
                    LspAction::Enable | LspAction::Disable => Err(wfe_failure(
                        CommandErrorCode::BackendUnavailable,
                        "This LSP state change is not available",
                        false,
                    )),
                    LspAction::CancelInstall => {
                        if context.app.lsp_install_in_progress {
                            let _ = context.dispatch_app_event(AppEventOutcome::Stop).await;
                            Ok(CommandOutcome::Applied)
                        } else {
                            Err(wfe_failure(
                                CommandErrorCode::NotFound,
                                "No LSP installation is active",
                                false,
                            ))
                        }
                    }
                }
            }
            WebCommand::ClearContext { confirmed, .. } => {
                if !confirmed {
                    require_confirmation(
                        wfe,
                        &confirmation_command,
                        "Clear all context?",
                        "This starts a new session and removes the active conversation context.",
                    )
                } else {
                    wfe.clear_confirmation();
                    match start_committed_session_transition(
                        context.app,
                        context.config,
                        context.shutdown_cancellation.child_token(),
                    )
                    .await
                    {
                        Ok(_) => Ok(CommandOutcome::Applied),
                        Err(_) => Err(wfe_failure(
                            CommandErrorCode::SaveFailed,
                            "Context could not be cleared safely",
                            false,
                        )),
                    }
                }
            }
            WebCommand::DeletePythonRuntime { confirmed, .. } => {
                if !confirmed {
                    require_confirmation(
                        wfe,
                        &confirmation_command,
                        "Delete Python package layer?",
                        "This removes the retained Python package layer for the active session.",
                    )
                } else {
                    wfe.clear_confirmation();
                    let had_runtime = context.app.python_runtime_id.is_some();
                    let _ = context
                        .dispatch_app_event(AppEventOutcome::DeletePythonRuntime)
                        .await;
                    if !had_runtime || context.app.python_runtime_id.is_none() {
                        Ok(CommandOutcome::Applied)
                    } else {
                        Err(wfe_failure(
                            CommandErrorCode::BackendUnavailable,
                            "Python package layer could not be deleted",
                            true,
                        ))
                    }
                }
            }
            WebCommand::DismissOverlay => {
                if context.app.python_setup_is_busy() {
                    context.cancel_python_setup_operation(true);
                }
                wfe.dismiss_overlay(context.app);
                Ok(CommandOutcome::Applied)
            }
            WebCommand::RequestSnapshot => Ok(CommandOutcome::SnapshotQueued),
            WebCommand::Quit { confirmed, .. } => {
                if !confirmed {
                    require_confirmation(
                        wfe,
                        &confirmation_command,
                        "Exit Lethetic?",
                        "This stops active work safely before closing the session.",
                    )
                } else {
                    wfe.clear_confirmation();
                    let _ = context.dispatch_app_event(AppEventOutcome::Exit).await;
                    if context.lifecycle.is_shutting_down() {
                        Ok(CommandOutcome::ShuttingDown)
                    } else {
                        Err(wfe_failure(
                            CommandErrorCode::Busy,
                            "The application cannot exit while session work is pending",
                            true,
                        ))
                    }
                }
            }
        };

        WfeCommandDisposition::Complete { command, result }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lifecycle::RuntimeMode;
    use lethetic::config::Config;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn actor_dispatches_and_publishes_before_acknowledgement() {
        let mut config = Config::default();
        let mut app = lethetic::app::App::new(&config);
        app.show_session_manager = false;
        let (frontend, mut runtime) = WfeRuntime::new(&app, Vec::new()).unwrap();
        let response = frontend
            .commands
            .try_submit(
                lethetic::wfe::actor::ConnectionId::new("browser-one".to_string()).unwrap(),
                lethetic::wfe::contracts::ICommandRequest {
                    id: "open-hotkeys".to_string(),
                    expected_revision: 0,
                    command: WebCommand::InvokeCommand {
                        command_id: lethetic::commands::CommandId::Hotkeys,
                    },
                },
            )
            .unwrap();
        let envelope = runtime.recv().await.unwrap();
        let admitted = runtime.admit(&app, envelope).unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut context = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Interactive);

        let disposition = execute_wfe_command(admitted, &mut runtime, &mut context).await;
        runtime.publish(context.app).unwrap();
        let WfeCommandDisposition::Complete { command, result } = disposition else {
            panic!("opening hotkeys must complete synchronously");
        };
        runtime.complete(command, result).unwrap();

        let response = response.await.unwrap();
        assert!(matches!(
            response.result,
            lethetic::wfe::contracts::CommandResult::Ok {
                revision: 1,
                outcome: CommandOutcome::PanelOpened {
                    panel: PanelId::Hotkeys
                }
            }
        ));
        assert_eq!(
            frontend.mirror.latest_snapshot().state.overlay.active_panel,
            Some(PanelId::Hotkeys)
        );
    }

    #[tokio::test]
    async fn direct_agent_mode_mutation_is_rejected_while_cli_locked() {
        let baseline = Config::default();
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
        let mut app = lethetic::app::App::new_with_python_policy_state(&config, policy);
        app.show_session_manager = false;
        let session_id = app.session_id.clone();
        let (frontend, mut runtime) = WfeRuntime::new(&app, Vec::new()).unwrap();
        let response = frontend
            .commands
            .try_submit(
                lethetic::wfe::actor::ConnectionId::new("browser-one".to_string()).unwrap(),
                lethetic::wfe::contracts::ICommandRequest {
                    id: "weaken-agent-mode".to_string(),
                    expected_revision: 0,
                    command: WebCommand::SetAgentMode {
                        session_id,
                        mode_id: "general".to_string(),
                    },
                },
            )
            .unwrap();
        let envelope = runtime.recv().await.unwrap();
        let admitted = runtime.admit(&app, envelope).unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut context = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Interactive);

        let disposition = execute_wfe_command(admitted, &mut runtime, &mut context).await;
        let WfeCommandDisposition::Complete { command, result } = disposition else {
            panic!("CLI-locked Agent Mode mutation must complete with an error");
        };
        runtime.complete(command, result).unwrap();

        let response = response.await.unwrap();
        let lethetic::wfe::contracts::CommandResult::Error { error } = response.result else {
            panic!("CLI-locked Agent Mode mutation was accepted");
        };
        assert_eq!(error.code, CommandErrorCode::BadRequest);
        assert_eq!(
            error.message,
            lethetic::python_policy::CLI_PYTHON_POLICY_LOCKED_ERROR
        );
        assert!(!error.retryable);
        assert_eq!(
            context.config.tool_profile,
            lethetic::config::ToolProfile::PythonOnly
        );
        assert_eq!(
            context.app.python_policy.effective_source(),
            lethetic::python_policy::PythonPolicySource::CliLocked
        );
    }

    #[tokio::test]
    async fn history_selection_returns_the_full_original_without_mutating_server_drafts() {
        let mut config = Config::default();
        let mut app = lethetic::app::App::new(&config);
        app.show_session_manager = false;
        let original = "界".repeat(2_000);
        app.history = vec![original.clone()];
        app.input = "server draft".to_string();
        app.backbuffer = "server backbuffer".to_string();
        app.show_history = true;
        let session_id = app.session_id.clone();
        let (frontend, mut runtime) = WfeRuntime::new(&app, Vec::new()).unwrap();
        let lethetic::wfe::contracts::PanelDataView::InputHistory { entries, .. } = frontend
            .mirror
            .latest_snapshot()
            .state
            .overlay
            .data
            .clone()
            .unwrap()
        else {
            panic!("history panel was not projected");
        };
        let entry_id = entries[0].entry_id.clone();
        assert_ne!(
            entries[0].label, original,
            "long label must stay display-only"
        );
        let response = frontend
            .commands
            .try_submit(
                lethetic::wfe::actor::ConnectionId::new("browser-history".to_string()).unwrap(),
                lethetic::wfe::contracts::ICommandRequest {
                    id: "select-full-history".to_string(),
                    expected_revision: 0,
                    command: WebCommand::SelectHistoryEntry {
                        session_id: session_id.clone(),
                        entry_id: entry_id.clone(),
                    },
                },
            )
            .unwrap();
        let envelope = runtime.recv().await.unwrap();
        let admitted = runtime.admit(&app, envelope).unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut context = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Service);

        let disposition = execute_wfe_command(admitted, &mut runtime, &mut context).await;
        runtime.publish(context.app).unwrap();
        let WfeCommandDisposition::Complete { command, result } = disposition else {
            panic!("history selection must complete synchronously");
        };
        assert!(matches!(
            &result,
            Ok(CommandOutcome::HistoryEntrySelected {
                session_id: outcome_session,
                entry_id: outcome_entry,
                editor_content,
            }) if outcome_session == &session_id
                && outcome_entry == &entry_id
                && editor_content == &original
        ));
        runtime.complete(command, result).unwrap();
        assert!(matches!(
            response.await.unwrap().result,
            lethetic::wfe::contracts::CommandResult::Ok {
                outcome: CommandOutcome::HistoryEntrySelected { editor_content, .. },
                ..
            } if editor_content == original
        ));
        assert_eq!(context.app.input, "server draft");
        assert_eq!(context.app.backbuffer, "server backbuffer");
    }

    #[tokio::test]
    async fn stop_rechecks_the_session_and_cancellation_target_at_execution() {
        let mut config = Config::default();
        let mut app = lethetic::app::App::new(&config);
        app.show_session_manager = false;
        app.add_logical_turn_user_segment("first turn".to_string());
        app.is_processing = true;
        let first_cancel_id = app.live_cancellation_id().unwrap().to_string();
        let session_id = app.session_id.clone();
        let (frontend, mut runtime) = WfeRuntime::new(&app, Vec::new()).unwrap();
        let response = frontend
            .commands
            .try_submit(
                lethetic::wfe::actor::ConnectionId::new("browser-stop-race".to_string()).unwrap(),
                lethetic::wfe::contracts::ICommandRequest {
                    id: "stop-raced-target".to_string(),
                    expected_revision: 0,
                    command: WebCommand::Stop {
                        session_id,
                        cancel_id: first_cancel_id,
                    },
                },
            )
            .unwrap();
        let envelope = runtime.recv().await.unwrap();
        let admitted = runtime.admit(&app, envelope).unwrap();

        app.is_processing = false;
        app.settle_logical_turn();
        app.add_logical_turn_user_segment("successor turn".to_string());
        app.is_processing = true;
        let successor_cancel_id = app.live_cancellation_id().unwrap().to_string();
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut context = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Service);
        let disposition = execute_wfe_command(admitted, &mut runtime, &mut context).await;
        let WfeCommandDisposition::Complete { command, result } = disposition else {
            panic!("stale stop must complete with an error");
        };
        assert!(matches!(
            &result,
            Err(RuntimeFailure {
                code: CommandErrorCode::NotFound,
                ..
            })
        ));
        runtime.publish(context.app).unwrap();
        runtime.complete(command, result).unwrap();

        assert_eq!(
            context.app.live_cancellation_id(),
            Some(successor_cancel_id.as_str())
        );
        assert!(context.app.is_processing);
        assert!(matches!(
            response.await.unwrap().result,
            lethetic::wfe::contracts::CommandResult::Error {
                error: lethetic::wfe::contracts::CommandError {
                    code: CommandErrorCode::NotFound,
                    ..
                }
            }
        ));
    }

    #[tokio::test]
    async fn browser_stop_settles_unavailable_question_before_ack_without_continuation() {
        let mut config = Config::default();
        let mut app = lethetic::app::App::new(&config);
        app.show_session_manager = false;
        app.blocks.clear();
        app.add_logical_turn_user_segment("question turn".to_string());
        app.context_manager.add_message("user", "question turn");
        let tool_call = lethetic::context::ToolCall {
            id: "question-browser-stop".to_string(),
            provider_id: None,
            function: lethetic::context::FunctionCall {
                name: "ask_the_user".to_string(),
                arguments: serde_json::json!({
                    "question": format!("read /etc/private/question? {}", "x".repeat(8_000))
                }),
            },
        };
        app.context_manager
            .upsert_assistant_tool_call_with_provider("", vec![tool_call.clone()], None);
        app.pending_tool_call = Some(tool_call);
        app.is_asking_user = true;
        let session_id = app.session_id.clone();
        let cancel_id = app.active_cancellation_id().unwrap().to_string();
        let (frontend, mut runtime) = WfeRuntime::new(&app, Vec::new()).unwrap();
        let snapshot = frontend.mirror.latest_snapshot();
        assert!(snapshot.state.activity.cancellable);
        assert_eq!(
            snapshot.state.activity.cancel_id.as_deref(),
            Some(cancel_id.as_str())
        );
        assert_eq!(
            snapshot.state.pending_question.as_ref().unwrap().questions[0].prompt,
            lethetic::wfe::presentation::QUESTION_PREVIEW_UNAVAILABLE
        );
        let response = frontend
            .commands
            .try_submit(
                lethetic::wfe::actor::ConnectionId::new("browser-question-stop".to_string())
                    .unwrap(),
                lethetic::wfe::contracts::ICommandRequest {
                    id: "stop-unavailable-question".to_string(),
                    expected_revision: 0,
                    command: WebCommand::Stop {
                        session_id,
                        cancel_id,
                    },
                },
            )
            .unwrap();
        let envelope = runtime.recv().await.unwrap();
        let admitted = runtime.admit(&app, envelope).unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut context = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Service);

        let disposition = execute_wfe_command(admitted, &mut runtime, &mut context).await;
        runtime.publish(context.app).unwrap();
        let WfeCommandDisposition::Complete { command, result } = disposition else {
            panic!("question cancellation must complete synchronously");
        };
        assert_eq!(result, Ok(CommandOutcome::Applied));
        runtime.complete(command, result).unwrap();

        assert!(matches!(
            response.await.unwrap().result,
            lethetic::wfe::contracts::CommandResult::Ok {
                outcome: CommandOutcome::Applied,
                ..
            }
        ));
        assert!(!context.app.is_asking_user);
        assert!(context.app.pending_tool_call.is_none());
        assert!(context.app.active_cancellation_id().is_none());
        let messages = context.app.context_manager.get_messages();
        let result_index = messages
            .iter()
            .position(|message| {
                message.role == "tool"
                    && message
                        .content
                        .contains(crate::provider::PENDING_INTERACTION_CANCELLED_RESULT)
            })
            .expect("fixed terminal tool result must remain in context");
        assert!(result_index > 0);
        assert!(
            messages[result_index - 1]
                .tool_calls
                .as_ref()
                .is_some_and(|calls| {
                    calls.iter().any(|call| call.id == "question-browser-stop")
                })
        );
        assert_eq!(
            messages
                .iter()
                .filter(|message| {
                    message.role == "tool"
                        && message
                            .content
                            .contains(crate::provider::PENDING_INTERACTION_CANCELLED_RESULT)
                })
                .count(),
            1
        );
        assert!(matches!(
            rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn destructive_request_only_opens_confirmation_before_consent() {
        let mut config = Config::default();
        let mut app = lethetic::app::App::new(&config);
        app.show_session_manager = false;
        let original_session = app.session_id.clone();
        let (frontend, mut runtime) = WfeRuntime::new(&app, Vec::new()).unwrap();
        let response = frontend
            .commands
            .try_submit(
                lethetic::wfe::actor::ConnectionId::new("browser-one".to_string()).unwrap(),
                lethetic::wfe::contracts::ICommandRequest {
                    id: "clear-context".to_string(),
                    expected_revision: 0,
                    command: WebCommand::ClearContext {
                        session_id: original_session.clone(),
                        confirmed: false,
                        confirmation_id: None,
                    },
                },
            )
            .unwrap();
        let envelope = runtime.recv().await.unwrap();
        let admitted = runtime.admit(&app, envelope).unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut context = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Interactive);

        let disposition = execute_wfe_command(admitted, &mut runtime, &mut context).await;
        runtime.publish(context.app).unwrap();
        let WfeCommandDisposition::Complete { command, result } = disposition else {
            panic!("confirmation request must complete synchronously");
        };
        runtime.complete(command, result).unwrap();

        assert_eq!(context.app.session_id, original_session);
        assert!(matches!(
            response.await.unwrap().result,
            lethetic::wfe::contracts::CommandResult::Error {
                error: lethetic::wfe::contracts::CommandError {
                    code: CommandErrorCode::ConfirmationRequired,
                    current_revision: Some(1),
                    ..
                }
            }
        ));
        assert_eq!(
            frontend.mirror.latest_snapshot().state.overlay.active_panel,
            Some(PanelId::Confirmation)
        );
    }
}
