use super::*;
use crate::commands::{CommandContext, CommandId, CommandView};
use crate::python_policy::{PolicyRevision, PythonPolicySnapshot};
use crate::python_setup::PolicyPersistence;

#[derive(Debug, PartialEq)]
pub enum AppEventOutcome {
    Continue,
    Exit,
    SendPrompt(String),
    ToolApproved(bool, bool),
    Stop,
    NewSession,
    ResumeSession(String),
    DeleteSession(String),
    CompactSession {
        session_id: String,
        connection_id: String,
        model_id: String,
    },
    WipeSessions,
    ToggleHistory,
    FetchModels,
    SwitchModel(String, String), // (connection_id, model_id)
    /// Start the HTTPS remote-control listener inside this session.
    StartRemoteControl {
        target: String,
        open: bool,
        files: bool,
    },
    StopRemoteControl,
    /// Fetch a connection's full catalog for the "scan for more" picker.
    ScanModels {
        connection_id: String,
    },
    /// Add a model from the catalog to the connection's saved list.
    SaveModel {
        connection_id: String,
        model_id: String,
    },
    OpenPythonSetup {
        preset: Option<crate::python_setup::PythonSetupPreset>,
    },
    CancelPythonSetup {
        dismiss_when_settled: bool,
    },
    ApplyPythonPolicy {
        snapshot: PythonPolicySnapshot,
        persistence: PolicyPersistence,
        expected_revision: Option<PolicyRevision>,
    },
    PullPodmanImage(String),
    DeletePythonRuntime,
}

impl App {
    pub fn refresh_prompt_list(&mut self) {
        self.prompt_files = self.system_prompt_manager.list_prompts();
        if self.prompt_list_state.selected().is_none() && !self.prompt_files.is_empty() {
            self.prompt_list_state.select(Some(0));
        }
    }
}

impl App {
    pub fn command_context(&self) -> CommandContext {
        CommandContext {
            loop_mode: self.loop_detector.config.mode.label().to_string(),
            agent_mode: format!("{:?}", self.config.tool_profile),
            agent_mode_locked: self.python_policy.is_cli_locked(),
            session_name: self.display_name.clone(),
            remote_control_target: self.remote_control_target.clone(),
            remote_control_locked: self.remote_control_locked,
            has_history: !self.history.is_empty(),
            fully_idle: self.is_fully_idle(),
        }
    }

    pub fn command_view(&self, command: CommandId) -> CommandView {
        command.view(&self.command_context())
    }
}

pub fn dispatch_command(app: &mut App, command: CommandId) -> AppEventOutcome {
    let view = app.command_view(command);
    if !view.enabled {
        app.stop_reason = format!(
            "⚠ {}",
            view.disabled_reason
                .unwrap_or_else(|| "Command is unavailable".to_string())
        );
        app.should_redraw = true;
        return AppEventOutcome::Continue;
    }

    match command {
        CommandId::Hotkeys => {
            app.show_palette = false;
            app.show_hotkeys = true;
        }
        CommandId::Themes => {
            app.show_palette = false;
            app.show_theme_menu = true;
        }
        CommandId::InputHistory => {
            app.show_palette = false;
            app.show_history = true;
            app.history_state.select(Some(0));
        }
        CommandId::LoopDetection => {
            use crate::loop_detector::LoopDetectionMode;
            app.loop_detector.config.mode = match app.loop_detector.config.mode {
                LoopDetectionMode::Off => LoopDetectionMode::BlockLimit,
                LoopDetectionMode::BlockLimit => LoopDetectionMode::NGram,
                LoopDetectionMode::NGram => LoopDetectionMode::PhraseFrequency,
                LoopDetectionMode::PhraseFrequency => LoopDetectionMode::Combined,
                LoopDetectionMode::Combined => LoopDetectionMode::CombinedWithBlockLimit,
                LoopDetectionMode::CombinedWithBlockLimit => LoopDetectionMode::Off,
            };
        }
        CommandId::SystemPrompt => {
            app.show_palette = false;
            app.refresh_prompt_list();
            app.show_prompt_manager = true;
        }
        CommandId::ClearUi => {
            app.show_palette = false;
            app.clear_ui_preserving_context();
        }
        CommandId::ClearContext => {
            app.show_palette = false;
            return AppEventOutcome::NewSession;
        }
        CommandId::ToggleDebugger => {
            app.show_palette = false;
            app.show_debug = !app.show_debug;
        }
        CommandId::ToggleTodos => {
            app.show_palette = false;
            app.toggle_todos();
        }
        CommandId::Sessions => {
            app.show_palette = false;
            app.refresh_session_list();
            app.show_session_manager = true;
        }
        CommandId::NameSession => {
            if let Err(error) = app.open_session_name_dialog_checked() {
                app.stop_reason = format!("⚠ {error}");
            }
        }
        CommandId::LatestFiles => {
            app.show_palette = false;
            app.show_latest_files = true;
            app.latest_files_state.select(Some(0));
        }
        CommandId::Models => {
            app.show_palette = false;
            return AppEventOutcome::FetchModels;
        }
        CommandId::LspServers => {
            app.show_palette = false;
            app.show_lsp_manager = true;
            app.lsp_server_list_state.select(Some(0));
        }
        CommandId::AgentMode => {
            app.show_palette = false;
            return AppEventOutcome::OpenPythonSetup { preset: None };
        }
        CommandId::AgentGeneral
        | CommandId::PythonIsolated
        | CommandId::PythonNonlocal
        | CommandId::PythonPermissive => {
            use crate::config::PythonPreset;
            use crate::python_setup::PythonSetupPreset;
            app.show_palette = false;
            let preset = match command {
                CommandId::AgentGeneral => PythonSetupPreset::General,
                CommandId::PythonIsolated => PythonSetupPreset::Python(PythonPreset::Isolated),
                CommandId::PythonNonlocal => PythonSetupPreset::Python(PythonPreset::Nonlocal),
                _ => PythonSetupPreset::Python(PythonPreset::Permissive),
            };
            return AppEventOutcome::OpenPythonSetup {
                preset: Some(preset),
            };
        }
        CommandId::RemoteControl => {
            app.show_palette = false;
            if app.remote_control_target.is_some() {
                return AppEventOutcome::StopRemoteControl;
            }
            app.rc_setup = Some(RcSetupState::new());
        }
        CommandId::DeletePythonRuntime => {
            app.show_palette = false;
            return AppEventOutcome::DeletePythonRuntime;
        }
        CommandId::Quit => return AppEventOutcome::Exit,
    }
    app.should_redraw = true;
    AppEventOutcome::Continue
}
