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
    WipeSessions,
    ToggleHistory,
    FetchModels,
    SwitchModel(String, String), // (connection_id, model_id)
    OpenPythonSetup,
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
            loop_mode: format!("{:?}", self.loop_detector.config.mode),
            agent_mode: format!("{:?}", self.config.tool_profile),
            agent_mode_locked: self.python_policy.is_cli_locked(),
            session_name: self.display_name.clone(),
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
                LoopDetectionMode::Combined => LoopDetectionMode::Off,
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
            return AppEventOutcome::OpenPythonSetup;
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
