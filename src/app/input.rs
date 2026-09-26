use super::session_state::is_forbidden_session_name_character;
use super::setup_input::handle_python_setup_key;
use super::*;
use crate::commands::CommandId;
use crate::python_setup::PythonSetupStage;
use crossterm::event::{self, KeyCode, KeyModifiers};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputLayer {
    PythonSetup,
    PromptEditor,
    PromptManager,
    History,
    SessionName,
    SessionManager,
    Cleanup,
    Hotkeys,
    Palette,
    LatestFiles,
    ModelSwitcher,
    LspManager,
    ThemeMenu,
    Approval,
}

// Later-rendered overlays receive input first so visual and keyboard z-order agree.
const INPUT_LAYER_PRECEDENCE: [InputLayer; 14] = [
    InputLayer::SessionName,
    InputLayer::Hotkeys,
    InputLayer::PromptEditor,
    InputLayer::Approval,
    InputLayer::PythonSetup,
    InputLayer::Cleanup,
    InputLayer::LatestFiles,
    InputLayer::SessionManager,
    InputLayer::PromptManager,
    InputLayer::History,
    InputLayer::ThemeMenu,
    InputLayer::LspManager,
    InputLayer::ModelSwitcher,
    InputLayer::Palette,
];

impl InputLayer {
    fn is_active(self, app: &App) -> bool {
        match self {
            Self::PythonSetup => app.python_setup.is_some(),
            Self::PromptEditor => app.show_prompt_editor,
            Self::PromptManager => app.show_prompt_manager,
            Self::History => app.show_history,
            Self::SessionName => app.show_session_name_dialog,
            Self::SessionManager => app.show_session_manager,
            Self::Cleanup => app.show_cleanup_prompt,
            Self::Hotkeys => app.show_hotkeys,
            Self::Palette => app.show_palette,
            Self::LatestFiles => app.show_latest_files,
            Self::ModelSwitcher => app.show_model_switcher,
            Self::LspManager => app.show_lsp_manager,
            Self::ThemeMenu => app.show_theme_menu,
            Self::Approval => app.show_approval_prompt,
        }
    }
}

fn first_active_input_layer(mut is_active: impl FnMut(InputLayer) -> bool) -> Option<InputLayer> {
    INPUT_LAYER_PRECEDENCE
        .into_iter()
        .find(|layer| is_active(*layer))
}

fn active_input_layer(app: &App) -> Option<InputLayer> {
    first_active_input_layer(|layer| layer.is_active(app))
}

fn dispatch_input_layer(app: &mut App, key: event::KeyEvent, layer: InputLayer) -> AppEventOutcome {
    match layer {
        InputLayer::PythonSetup => handle_python_setup_key(app, key),
        InputLayer::PromptEditor => handle_prompt_editor_key(app, key),
        InputLayer::PromptManager => handle_prompt_manager_key(app, key),
        InputLayer::History => handle_history_key(app, key),
        InputLayer::SessionName => handle_session_name_key(app, key),
        InputLayer::SessionManager => handle_session_manager_key(app, key),
        InputLayer::Cleanup => handle_cleanup_key(app, key),
        InputLayer::Hotkeys => handle_hotkeys_key(app, key),
        InputLayer::Palette => handle_palette_key(app, key),
        InputLayer::LatestFiles => handle_latest_files_key(app, key),
        InputLayer::ModelSwitcher => handle_model_switcher_key(app, key),
        InputLayer::LspManager => handle_lsp_manager_key(app, key),
        InputLayer::ThemeMenu => handle_theme_menu_key(app, key),
        InputLayer::Approval => handle_approval_key(app, key),
    }
}

pub fn handle_key(app: &mut App, key: event::KeyEvent) -> AppEventOutcome {
    if let Some(layer) = active_input_layer(app) {
        return dispatch_input_layer(app, key, layer);
    }
    if handle_global_key(app, key) {
        return AppEventOutcome::Continue;
    }
    if app.is_output_focused {
        return handle_output_key(app, key);
    }
    handle_main_input_key(app, key)
}

fn handle_prompt_editor_key(app: &mut App, key: event::KeyEvent) -> AppEventOutcome {
    if app.is_editing_prompt {
        match key.code {
            KeyCode::Esc => {
                app.is_editing_prompt = false;
                app.should_redraw = true;
            }
            KeyCode::Up => {
                // Simple line-up approximation (move back ~80 chars)
                app.prompt_cursor_pos = app.prompt_cursor_pos.saturating_sub(80);
                app.should_redraw = true;
            }
            KeyCode::Down => {
                // Simple line-down approximation
                app.prompt_cursor_pos = (app.prompt_cursor_pos + 80).min(app.system_prompt.len());
                app.should_redraw = true;
            }
            KeyCode::Left => {
                if app.prompt_cursor_pos > 0 {
                    app.prompt_cursor_pos = app.system_prompt[..app.prompt_cursor_pos]
                        .chars()
                        .last()
                        .map(|c| app.prompt_cursor_pos - c.len_utf8())
                        .unwrap_or(0);
                    app.should_redraw = true;
                }
            }
            KeyCode::Right => {
                if app.prompt_cursor_pos < app.system_prompt.len() {
                    app.prompt_cursor_pos = app.system_prompt[app.prompt_cursor_pos..]
                        .chars()
                        .next()
                        .map(|c| app.prompt_cursor_pos + c.len_utf8())
                        .unwrap_or(app.system_prompt.len());
                    app.should_redraw = true;
                }
            }
            KeyCode::PageUp => {
                app.prompt_scroll = app.prompt_scroll.saturating_sub(10);
                app.should_redraw = true;
            }
            KeyCode::PageDown => {
                app.prompt_scroll += 10;
                app.should_redraw = true;
            }
            KeyCode::Char(c) => {
                app.system_prompt.insert(app.prompt_cursor_pos, c);
                app.prompt_cursor_pos += c.len_utf8();
                app.should_redraw = true;
            }
            KeyCode::Backspace => {
                if app.prompt_cursor_pos > 0 {
                    let prev_char = app.system_prompt[..app.prompt_cursor_pos]
                        .chars()
                        .last()
                        .unwrap();
                    app.prompt_cursor_pos -= prev_char.len_utf8();
                    app.system_prompt.remove(app.prompt_cursor_pos);
                    app.should_redraw = true;
                }
            }
            KeyCode::Delete => {
                if app.prompt_cursor_pos < app.system_prompt.len() {
                    app.system_prompt.remove(app.prompt_cursor_pos);
                    app.should_redraw = true;
                }
            }
            KeyCode::Enter => {
                app.system_prompt.insert(app.prompt_cursor_pos, '\n');
                app.prompt_cursor_pos += 1;
                app.should_redraw = true;
            }
            _ => {}
        }
    } else if app.show_prompt_save_dialog {
        match key.code {
            KeyCode::Esc => {
                app.show_prompt_save_dialog = false;
                app.should_redraw = true;
            }
            KeyCode::Enter => {
                let name = app.prompt_save_name.trim().to_string();
                if !name.is_empty() {
                    let _ = app
                        .system_prompt_manager
                        .save_prompt(&name, &app.system_prompt);
                    app.log_debug(&format!("System prompt saved as {}.md", name));
                }
                app.show_prompt_save_dialog = false;
                app.should_redraw = true;
            }
            KeyCode::Char(c) => {
                app.prompt_save_name.push(c);
                app.should_redraw = true;
            }
            KeyCode::Backspace => {
                app.prompt_save_name.pop();
                app.should_redraw = true;
            }
            _ => {}
        }
    } else {
        match key.code {
            KeyCode::Char('m') | KeyCode::Char('M') => {
                app.is_editing_prompt = true;
                app.prompt_cursor_pos = app.system_prompt.len();
                app.should_redraw = true;
            }
            KeyCode::Char('s') | KeyCode::Char('S') => {
                let resolved = crate::system_prompt::SystemPromptManager::resolve_prompt(
                    &app.system_prompt,
                    &app.cwd,
                    &app.config,
                );
                app.context_manager.update_system_prompt(resolved);
                // Also auto-save as software_engineer.md
                let _ = app
                    .system_prompt_manager
                    .save_prompt("software_engineer", &app.system_prompt);
                app.show_prompt_editor = false;
                app.should_redraw = true;
                app.log_debug("System prompt updated and saved as software_engineer.md.");
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                app.show_prompt_save_dialog = true;
                app.prompt_save_name.clear();
                app.should_redraw = true;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                app.prompt_scroll = app.prompt_scroll.saturating_sub(1);
                app.should_redraw = true;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                app.prompt_scroll += 1;
                app.should_redraw = true;
            }
            KeyCode::Esc => {
                app.show_prompt_editor = false;
                app.should_redraw = true;
            }
            _ => {}
        }
    }
    AppEventOutcome::Continue
}

fn handle_prompt_manager_key(app: &mut App, key: event::KeyEvent) -> AppEventOutcome {
    match key.code {
        KeyCode::Down | KeyCode::Char('j') => {
            let max = app.prompt_files.len() + 1; // +1 for "Create New"
            let i = match app.prompt_list_state.selected() {
                Some(i) => {
                    if i >= max.saturating_sub(1) {
                        0
                    } else {
                        i + 1
                    }
                }
                None => 0,
            };
            if max > 0 {
                app.prompt_list_state.select(Some(i));
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            let max = app.prompt_files.len() + 1;
            let i = match app.prompt_list_state.selected() {
                Some(i) => {
                    if i == 0 {
                        max.saturating_sub(1)
                    } else {
                        i - 1
                    }
                }
                None => 0,
            };
            if max > 0 {
                app.prompt_list_state.select(Some(i));
            }
        }
        KeyCode::Enter => {
            if let Some(i) = app.prompt_list_state.selected() {
                if i == 0 {
                    app.system_prompt = crate::system_prompt::DEFAULT_PROMPT_TEMPLATE.to_string();
                    app.prompt_save_name.clear();
                } else if i - 1 < app.prompt_files.len() {
                    let name = &app.prompt_files[i - 1];
                    if let Some(content) = app.system_prompt_manager.load_prompt(name) {
                        app.system_prompt = content;
                        app.prompt_save_name = name.clone();
                    }
                }
                app.show_prompt_manager = false;
                app.show_prompt_editor = true;
                app.prompt_cursor_pos = app.system_prompt.len();
                app.should_redraw = true;
            }
        }
        KeyCode::Esc => {
            app.show_prompt_manager = false;
            app.should_redraw = true;
        }
        _ => {}
    }
    app.should_redraw = true;
    AppEventOutcome::Continue
}

fn handle_history_key(app: &mut App, key: event::KeyEvent) -> AppEventOutcome {
    match key.code {
        KeyCode::Esc => {
            app.show_history = false;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            let i = match app.history_state.selected() {
                Some(i) => {
                    if i == 0 {
                        app.history.len().saturating_sub(1)
                    } else {
                        i - 1
                    }
                }
                None => 0,
            };
            app.history_state.select(Some(i));
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let i = match app.history_state.selected() {
                Some(i) => {
                    if i >= app.history.len().saturating_sub(1) {
                        0
                    } else {
                        i + 1
                    }
                }
                None => 0,
            };
            app.history_state.select(Some(i));
        }
        KeyCode::Enter => {
            if let Some(i) = app.history_state.selected() {
                // History is shown reversed (newest first)
                let idx = app.history.len().saturating_sub(1).saturating_sub(i);
                if let Some(selected) = app.history.get(idx).cloned() {
                    if selected == app.input {
                        // If same, restore backbuffer
                        if !app.backbuffer.is_empty() {
                            app.input = app.backbuffer.clone();
                            app.backbuffer.clear();
                        }
                    } else {
                        app.backbuffer = app.input.clone();
                        app.input = selected;
                    }
                    app.cursor_pos = app.input.len();
                    app.show_history = false;
                }
            }
        }
        _ => {}
    }
    app.should_redraw = true;
    AppEventOutcome::Continue
}

fn handle_session_name_key(app: &mut App, key: event::KeyEvent) -> AppEventOutcome {
    match key.code {
        KeyCode::Esc => {
            app.show_session_name_dialog = false;
            app.session_name_error = None;
        }
        KeyCode::Enter => {
            let value = app.session_name_input.clone();
            match app.set_session_display_name(&value) {
                Ok(()) => {
                    app.show_session_name_dialog = false;
                    app.session_name_error = None;
                    app.stop_reason = match &app.display_name {
                        Some(name) => format!("Session named {name}"),
                        None => "Session name cleared".to_string(),
                    };
                }
                Err(error) => app.session_name_error = Some(error),
            }
        }
        KeyCode::Backspace => {
            app.session_name_input.pop();
            app.session_name_error = None;
        }
        KeyCode::Char(character)
            if (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT)
                && !is_forbidden_session_name_character(character) =>
        {
            app.session_name_input.push(character);
            app.session_name_error = None;
        }
        _ => {}
    }
    app.should_redraw = true;
    AppEventOutcome::Continue
}

fn handle_session_manager_key(app: &mut App, key: event::KeyEvent) -> AppEventOutcome {
    match key.code {
        KeyCode::Down | KeyCode::Char('j') => {
            let i = match app.session_list_state.selected() {
                Some(i) => {
                    if i >= app.session_summaries.len().saturating_sub(1) {
                        0
                    } else {
                        i + 1
                    }
                }
                None => 0,
            };
            if !app.session_summaries.is_empty() {
                app.session_list_state.select(Some(i));
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            let i = match app.session_list_state.selected() {
                Some(i) => {
                    if i == 0 {
                        app.session_summaries.len().saturating_sub(1)
                    } else {
                        i - 1
                    }
                }
                None => 0,
            };
            if !app.session_summaries.is_empty() {
                app.session_list_state.select(Some(i));
            }
        }
        KeyCode::Enter => {
            if let Some(i) = app.session_list_state.selected()
                && i < app.session_summaries.len()
            {
                let session_id = app.session_summaries[i].session_id.clone();
                app.show_session_manager = false;
                if app.current_session_dir.is_some() && session_id == app.session_id {
                    app.stop_reason = format!("Session {session_id} is already active");
                    app.should_redraw = true;
                    return AppEventOutcome::Continue;
                }
                return AppEventOutcome::ResumeSession(session_id);
            }
        }
        KeyCode::Char('n') | KeyCode::Char('N') => {
            app.show_session_manager = false;
            return AppEventOutcome::NewSession;
        }
        KeyCode::Char('d') | KeyCode::Char('D') => {
            if let Some(i) = app.session_list_state.selected()
                && i < app.session_summaries.len()
            {
                let session_id = app.session_summaries[i].session_id.clone();
                return AppEventOutcome::DeleteSession(session_id);
            }
        }
        KeyCode::Char('c') | KeyCode::Char('C') => {
            if let Some(i) = app.session_list_state.selected()
                && i < app.session_summaries.len()
            {
                if app.compaction_popup.is_some() {
                    app.stop_reason = "⚠ A compaction is already running".to_string();
                } else {
                    let session_id = app.session_summaries[i].session_id.clone();
                    app.compact_model_picker_src = Some(session_id);
                    app.show_session_manager = false;
                    return AppEventOutcome::FetchModels;
                }
            }
        }
        KeyCode::Char('x') | KeyCode::Char('X') => {
            app.show_session_manager = false;
            return AppEventOutcome::WipeSessions;
        }
        KeyCode::Esc if app.current_session_dir.is_some() => {
            app.show_session_manager = false;
        }
        _ => {}
    }
    app.should_redraw = true;
    AppEventOutcome::Continue
}

fn handle_cleanup_key(app: &mut App, key: event::KeyEvent) -> AppEventOutcome {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            app.show_cleanup_prompt = false;
            return AppEventOutcome::WipeSessions;
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
            app.show_cleanup_prompt = false;
        }
        _ => {}
    }
    AppEventOutcome::Continue
}

fn handle_hotkeys_key(app: &mut App, key: event::KeyEvent) -> AppEventOutcome {
    if key.code == KeyCode::Esc || key.code == KeyCode::Enter {
        app.show_hotkeys = false;
        app.should_redraw = true;
    }
    AppEventOutcome::Continue
}

fn handle_palette_key(app: &mut App, key: event::KeyEvent) -> AppEventOutcome {
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => {
            app.show_palette = false;
            app.should_redraw = true;
        }
        KeyCode::Down | KeyCode::Char('j') => app.next_palette_item(),
        KeyCode::Up | KeyCode::Char('k') => app.previous_palette_item(),
        KeyCode::Enter => {
            let index = app.palette_state.selected().unwrap_or(0);
            let command = app
                .palette_items
                .get(index)
                .copied()
                .unwrap_or(CommandId::Hotkeys);
            return dispatch_command(app, command);
        }
        KeyCode::Char(accelerator) => {
            if let Some(command) = CommandId::from_accelerator(accelerator) {
                return dispatch_command(app, command);
            }
        }
        _ => {}
    }
    AppEventOutcome::Continue
}

fn handle_latest_files_key(app: &mut App, key: event::KeyEvent) -> AppEventOutcome {
    match key.code {
        KeyCode::Esc => {
            app.show_latest_files = false;
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let num_files =
                app.context_manager.active_files.len() + app.context_manager.latest_files.len();
            if num_files > 0 {
                let i = match app.latest_files_state.selected() {
                    Some(i) => {
                        if i >= num_files - 1 {
                            0
                        } else {
                            i + 1
                        }
                    }
                    None => 0,
                };
                app.latest_files_state.select(Some(i));
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            let num_files =
                app.context_manager.active_files.len() + app.context_manager.latest_files.len();
            if num_files > 0 {
                let i = match app.latest_files_state.selected() {
                    Some(i) => {
                        if i == 0 {
                            num_files - 1
                        } else {
                            i - 1
                        }
                    }
                    None => 0,
                };
                app.latest_files_state.select(Some(i));
            }
        }
        KeyCode::Char('r') | KeyCode::Char('R') => {
            let paths: Vec<String> = app
                .context_manager
                .all_cached_files()
                .into_iter()
                .map(|(p, _, _)| p)
                .collect();
            if let Some(i) = app.latest_files_state.selected()
                && let Some(path) = paths.get(i)
            {
                app.context_manager.remove_latest_file(path);
                let num_files =
                    app.context_manager.active_files.len() + app.context_manager.latest_files.len();
                if num_files == 0 {
                    app.latest_files_state.select(None);
                } else if i >= num_files {
                    app.latest_files_state.select(Some(num_files - 1));
                }
            }
        }
        _ => {}
    }
    app.should_redraw = true;
    AppEventOutcome::Continue
}

fn handle_model_switcher_key(app: &mut App, key: event::KeyEvent) -> AppEventOutcome {
    let num_models = app.available_models.len();
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.show_model_switcher = false;
            if app.compact_model_picker_src.take().is_some() {
                app.show_session_manager = true;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if num_models > 0 {
                let i = app.model_switcher_state.selected().unwrap_or(0);
                app.model_switcher_state
                    .select(Some(if i + 1 >= num_models { 0 } else { i + 1 }));
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if num_models > 0 {
                let i = app.model_switcher_state.selected().unwrap_or(0);
                app.model_switcher_state
                    .select(Some(if i == 0 { num_models - 1 } else { i - 1 }));
            }
        }
        KeyCode::Enter => {
            if let Some(index) = app.model_switcher_state.selected()
                && let Some(choice) = app.available_models.get(index)
            {
                if app.is_processing
                    || app.is_executing_tool
                    || app.show_approval_prompt
                    || app.is_asking_user
                {
                    app.stop_reason =
                        "⚠ Wait for the active turn before switching models".to_string();
                } else if choice.available {
                    let connection_id = choice.connection_id.clone();
                    let model_id = choice.model_id.clone();
                    if let Some(session_id) = app.compact_model_picker_src.take() {
                        app.show_model_switcher = false;
                        app.should_redraw = true;
                        return AppEventOutcome::CompactSession {
                            session_id,
                            connection_id,
                            model_id,
                        };
                    }
                    let outcome = AppEventOutcome::SwitchModel(connection_id, model_id);
                    app.show_model_switcher = false;
                    app.should_redraw = true;
                    return outcome;
                } else {
                    app.stop_reason = format!("⚠ {} is unavailable", choice.display);
                }
            }
        }
        _ => {}
    }
    app.should_redraw = true;
    AppEventOutcome::Continue
}

fn handle_lsp_manager_key(app: &mut App, key: event::KeyEvent) -> AppEventOutcome {
    let num_servers = crate::lsp::registry::SERVERS.len();
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.show_lsp_manager = false;
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let i = app.lsp_server_list_state.selected().unwrap_or(0);
            app.lsp_server_list_state
                .select(Some(if i + 1 >= num_servers { 0 } else { i + 1 }));
        }
        KeyCode::Up | KeyCode::Char('k') => {
            let i = app.lsp_server_list_state.selected().unwrap_or(0);
            app.lsp_server_list_state
                .select(Some(if i == 0 { num_servers - 1 } else { i - 1 }));
        }
        KeyCode::Enter | KeyCode::Char('i') => {
            if !app.is_fully_idle() {
                app.stop_reason =
                    "⚠ Wait for the active turn before installing an LSP server".to_string();
            } else if let Some(i) = app.lsp_server_list_state.selected()
                && let Some(def) = crate::lsp::registry::SERVERS.get(i)
                && !crate::lsp::registry::check_installed(def)
            {
                if let Some(install_cmd) = def.install_cmd {
                    app.show_lsp_manager = false;
                    app.lsp_install_cmd = Some(install_cmd.to_string());
                } else {
                    app.stop_reason = format!(
                        "⚠ No safe automatic installer is configured for `{}`. {}",
                        def.binary, def.install_note
                    );
                }
            }
        }
        _ => {}
    }
    app.should_redraw = true;
    AppEventOutcome::Continue
}

fn handle_theme_menu_key(app: &mut App, key: event::KeyEvent) -> AppEventOutcome {
    match key.code {
        KeyCode::Down | KeyCode::Char('j') => {
            let i = match app.theme_state.selected() {
                Some(i) => {
                    if i >= app.themes.len() - 1 {
                        0
                    } else {
                        i + 1
                    }
                }
                None => 0,
            };
            app.theme_state.select(Some(i));
            app.theme = app.themes[i].clone();
            for block in &mut app.blocks {
                block.invalidate();
            }
            app.needs_save = true;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            let i = match app.theme_state.selected() {
                Some(i) => {
                    if i == 0 {
                        app.themes.len() - 1
                    } else {
                        i - 1
                    }
                }
                None => 0,
            };
            app.theme_state.select(Some(i));
            app.theme = app.themes[i].clone();
            for block in &mut app.blocks {
                block.invalidate();
            }
            app.needs_save = true;
        }
        KeyCode::Enter | KeyCode::Esc => app.show_theme_menu = false,
        _ => {}
    }
    AppEventOutcome::Continue
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ApprovalDecision {
    Always,
    Once,
    Deny,
}

fn allows_affirmative_approval(modifiers: KeyModifiers) -> bool {
    modifiers.is_empty() || modifiers == KeyModifiers::SHIFT
}

fn approval_decision(key: event::KeyEvent) -> Option<ApprovalDecision> {
    match key.code {
        KeyCode::Char('a') | KeyCode::Char('A') if allows_affirmative_approval(key.modifiers) => {
            Some(ApprovalDecision::Always)
        }
        KeyCode::Char('o') | KeyCode::Char('O') if allows_affirmative_approval(key.modifiers) => {
            Some(ApprovalDecision::Once)
        }
        KeyCode::Char('d') | KeyCode::Char('D') | KeyCode::Esc => Some(ApprovalDecision::Deny),
        _ => None,
    }
}

fn handle_approval_key(app: &mut App, key: event::KeyEvent) -> AppEventOutcome {
    if let Some(decision) = approval_decision(key) {
        app.python_approval_show_original = false;
        app.python_approval_scroll = 0;
        return match decision {
            ApprovalDecision::Always => AppEventOutcome::ToolApproved(true, true),
            ApprovalDecision::Once => AppEventOutcome::ToolApproved(true, false),
            ApprovalDecision::Deny => AppEventOutcome::ToolApproved(false, false),
        };
    }

    let python_approval = app
        .pending_tool_call
        .as_ref()
        .is_some_and(|call| call.function.name == "python");
    match key.code {
        KeyCode::Up | KeyCode::Char('k') if python_approval => {
            app.python_approval_scroll = app.python_approval_scroll.saturating_sub(1);
            app.should_redraw = true;
        }
        KeyCode::Down | KeyCode::Char('j') if python_approval => {
            app.python_approval_scroll = app.python_approval_scroll.saturating_add(1);
            app.should_redraw = true;
        }
        KeyCode::PageUp if python_approval => {
            app.python_approval_scroll = app.python_approval_scroll.saturating_sub(10);
            app.should_redraw = true;
        }
        KeyCode::PageDown if python_approval => {
            app.python_approval_scroll = app.python_approval_scroll.saturating_add(10);
            app.should_redraw = true;
        }
        KeyCode::Home if python_approval => {
            app.python_approval_scroll = 0;
            app.should_redraw = true;
        }
        KeyCode::Char('v') | KeyCode::Char('V') if python_approval => {
            app.python_approval_show_original = !app.python_approval_show_original;
            app.python_approval_scroll = 0;
            app.should_redraw = true;
        }
        _ => {}
    }
    AppEventOutcome::Continue
}

fn handle_global_key(app: &mut App, key: event::KeyEvent) -> bool {
    match key.code {
        KeyCode::Tab => {
            app.is_output_focused = !app.is_output_focused;
            if app.is_output_focused
                && app.output_state.selected().is_none()
                && app.total_line_count > 0
            {
                app.output_state
                    .select(Some(app.total_line_count.saturating_sub(1)));
            }
            app.should_redraw = true;
            return true;
        }
        KeyCode::F(12) => {
            app.show_debug = !app.show_debug;
            app.should_redraw = true;
            return true;
        }
        _ => {}
    }
    false
}

fn handle_output_key(app: &mut App, key: event::KeyEvent) -> AppEventOutcome {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => app.scroll_output_up(1),
        KeyCode::Down | KeyCode::Char('j') => app.scroll_output_down(1),
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.scroll_output_up(10)
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.scroll_output_down(10)
        }
        KeyCode::PageUp => app.scroll_output_up(20),
        KeyCode::PageDown => app.scroll_output_down(20),
        KeyCode::Home => app.scroll_to_top(),
        KeyCode::End => app.scroll_to_bottom(),
        KeyCode::Esc => app.is_output_focused = false,
        _ => {}
    }
    app.should_redraw = true;
    AppEventOutcome::Continue
}

fn handle_main_input_key(app: &mut App, key: event::KeyEvent) -> AppEventOutcome {
    match key.code {
        KeyCode::Up if key.modifiers.contains(KeyModifiers::ALT) => {
            app.scroll_output_up(1);
            app.should_redraw = true;
            return AppEventOutcome::Continue;
        }
        KeyCode::Down if key.modifiers.contains(KeyModifiers::ALT) => {
            app.scroll_output_down(1);
            app.should_redraw = true;
            return AppEventOutcome::Continue;
        }
        KeyCode::PageUp => {
            app.scroll_output_up(20);
            app.should_redraw = true;
            return AppEventOutcome::Continue;
        }
        KeyCode::PageDown => {
            app.scroll_output_down(20);
            app.should_redraw = true;
            return AppEventOutcome::Continue;
        }
        KeyCode::Home if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.scroll_to_top();
            app.should_redraw = true;
            return AppEventOutcome::Continue;
        }
        KeyCode::End if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.scroll_to_bottom();
            app.should_redraw = true;
            return AppEventOutcome::Continue;
        }
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => {
            app.input.insert(app.cursor_pos, '\n');
            app.cursor_pos += 1;
            app.should_redraw = true;
        }
        KeyCode::Enter => {
            let p = app.input.drain(..).collect::<String>();
            app.cursor_pos = 0;
            if !p.trim().is_empty() {
                app.should_redraw = true;
                return AppEventOutcome::SendPrompt(p);
            }
        }
        KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.clear_ui_preserving_context();
            return AppEventOutcome::Continue;
        }
        KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.toggle_hide_thinking();
            return AppEventOutcome::Continue;
        }
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.show_palette = true;
            app.should_redraw = true;
        }
        KeyCode::Left => {
            if app.cursor_pos > 0 {
                app.cursor_pos = app.input[..app.cursor_pos]
                    .chars()
                    .last()
                    .map(|c| app.cursor_pos - c.len_utf8())
                    .unwrap_or(0);
                app.should_redraw = true;
            }
        }
        KeyCode::Right => {
            if app.cursor_pos < app.input.len() {
                app.cursor_pos = app.input[app.cursor_pos..]
                    .chars()
                    .next()
                    .map(|c| app.cursor_pos + c.len_utf8())
                    .unwrap_or(app.input.len());
                app.should_redraw = true;
            }
        }
        KeyCode::Home => {
            app.cursor_pos = 0;
            app.should_redraw = true;
        }
        KeyCode::End => {
            app.cursor_pos = app.input.len();
            app.should_redraw = true;
        }
        KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.cursor_pos = 0;
            app.should_redraw = true;
        }
        _ if key.code == KeyCode::Home && key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.cursor_pos = 0;
            app.should_redraw = true;
        }
        _ if key.code == KeyCode::End && key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.cursor_pos = app.input.len();
            app.should_redraw = true;
        }
        KeyCode::Char(c) => {
            app.input.insert(app.cursor_pos, c);
            app.cursor_pos += c.len_utf8();
            app.should_redraw = true;
        }
        KeyCode::Up => {
            let first_newline = app.input.find('\n');
            let is_on_first_line = match first_newline {
                None => true,
                Some(idx) => app.cursor_pos <= idx,
            };

            if is_on_first_line {
                if app.cursor_pos > 0 {
                    // If we're at the beginning of the first line, show history.
                    // But if we're not at the beginning, maybe we want to scroll?
                    // Actually, usually Up always goes to history if on first line.
                    // The user says Up is going to input instead of scrolling.
                    // Let's make it so if we are on the first line, we scroll up instead of history,
                    // unless we are specifically wanting history.
                    app.scroll_output_up(1);
                    app.should_redraw = true;
                } else if !app.history.is_empty() {
                    app.show_history = true;
                    app.history_state.select(Some(0));
                    app.should_redraw = true;
                }
            } else {
                // Move cursor up one line
                let current_line_start = app.input[..app.cursor_pos].rfind('\n').unwrap();
                let column = app.input[current_line_start + 1..app.cursor_pos]
                    .chars()
                    .count();

                let prev_line_start = if current_line_start == 0 {
                    0
                } else {
                    app.input[..current_line_start]
                        .rfind('\n')
                        .map(|idx| idx + 1)
                        .unwrap_or(0)
                };

                let prev_line = &app.input[prev_line_start..current_line_start];
                let prev_line_chars: Vec<char> = prev_line.chars().collect();
                let target_column = column.min(prev_line_chars.len());

                let mut new_pos = prev_line_start;
                for c in prev_line_chars.iter().take(target_column) {
                    new_pos += c.len_utf8();
                }
                app.cursor_pos = new_pos;
                app.should_redraw = true;
            }
        }
        KeyCode::Down => {
            if let Some(next_newline) = app.input[app.cursor_pos..].find('\n') {
                let current_line_start = app.input[..app.cursor_pos]
                    .rfind('\n')
                    .map(|idx| idx + 1)
                    .unwrap_or(0);
                let column = app.input[current_line_start..app.cursor_pos]
                    .chars()
                    .count();

                let next_line_start = app.cursor_pos + next_newline + 1;
                let next_line_rest = &app.input[next_line_start..];
                let next_line_end = next_line_rest
                    .find('\n')
                    .map(|idx| next_line_start + idx)
                    .unwrap_or(app.input.len());

                let next_line = &app.input[next_line_start..next_line_end];
                let next_line_chars: Vec<char> = next_line.chars().collect();
                let target_column = column.min(next_line_chars.len());

                let mut new_pos = next_line_start;
                for c in next_line_chars.iter().take(target_column) {
                    new_pos += c.len_utf8();
                }
                app.cursor_pos = new_pos;
                app.should_redraw = true;
            } else {
                // On last line, scroll output down
                app.scroll_output_down(1);
                app.should_redraw = true;
            }
        }
        KeyCode::Backspace => {
            if app.cursor_pos > 0 {
                let prev_char = app.input[..app.cursor_pos].chars().last().unwrap();
                app.cursor_pos -= prev_char.len_utf8();
                app.input.remove(app.cursor_pos);
                app.should_redraw = true;
            }
        }
        KeyCode::Delete => {
            if app.cursor_pos < app.input.len() {
                app.input.remove(app.cursor_pos);
                app.should_redraw = true;
            }
        }
        KeyCode::Esc => {
            if app.is_processing || app.is_executing_tool {
                return AppEventOutcome::Stop;
            } else {
                app.show_palette = true;
                app.should_redraw = true;
            }
        }
        _ => {}
    }

    AppEventOutcome::Continue
}

impl App {
    pub fn handle_paste(&mut self, text: &str) {
        if let Some(setup) = self.python_setup.as_mut() {
            match setup.stage {
                PythonSetupStage::PodmanImage => setup.replace_pasted_image(text),
                PythonSetupStage::Grants => setup.path_picker.replace_pasted_path(text),
                _ => {}
            }
        } else if self.show_session_name_dialog {
            self.session_name_input.push_str(text);
            self.session_name_error = None;
        } else if self.show_prompt_editor && self.is_editing_prompt {
            self.system_prompt.insert_str(self.prompt_cursor_pos, text);
            self.prompt_cursor_pos += text.len();
        } else if self.show_prompt_editor && self.show_prompt_save_dialog {
            self.prompt_save_name.push_str(text);
        } else if !self.is_processing {
            self.input.insert_str(self.cursor_pos, text);
            self.cursor_pos += text.len();
        }
        self.should_redraw = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> event::KeyEvent {
        event::KeyEvent::new(code, modifiers)
    }

    #[test]
    fn affirmative_approval_requires_plain_or_shift_only_key() {
        assert_eq!(
            approval_decision(key(KeyCode::Char('a'), KeyModifiers::empty())),
            Some(ApprovalDecision::Always)
        );
        assert_eq!(
            approval_decision(key(KeyCode::Char('A'), KeyModifiers::SHIFT)),
            Some(ApprovalDecision::Always)
        );
        assert_eq!(
            approval_decision(key(KeyCode::Char('o'), KeyModifiers::empty())),
            Some(ApprovalDecision::Once)
        );
        assert_eq!(
            approval_decision(key(KeyCode::Char('O'), KeyModifiers::SHIFT)),
            Some(ApprovalDecision::Once)
        );

        for modifiers in [
            KeyModifiers::CONTROL,
            KeyModifiers::ALT,
            KeyModifiers::SUPER,
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            KeyModifiers::ALT | KeyModifiers::SHIFT,
            KeyModifiers::SUPER | KeyModifiers::SHIFT,
        ] {
            assert_eq!(approval_decision(key(KeyCode::Char('a'), modifiers)), None);
            assert_eq!(approval_decision(key(KeyCode::Char('o'), modifiers)), None);
            assert_eq!(
                approval_decision(key(KeyCode::Char('d'), modifiers)),
                Some(ApprovalDecision::Deny)
            );
        }
    }

    #[test]
    fn approval_layer_preempts_primary_overlays() {
        assert_eq!(
            first_active_input_layer(|layer| {
                matches!(
                    layer,
                    InputLayer::Approval
                        | InputLayer::Palette
                        | InputLayer::History
                        | InputLayer::SessionManager
                        | InputLayer::ModelSwitcher
                )
            }),
            Some(InputLayer::Approval)
        );

        let mut app = App::new(&Config::default());
        app.show_session_manager = false;
        app.show_palette = true;
        app.show_approval_prompt = true;
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Char('a'), KeyModifiers::empty())),
            AppEventOutcome::ToolApproved(true, true)
        );
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Char('o'), KeyModifiers::empty())),
            AppEventOutcome::ToolApproved(true, false)
        );
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Char('d'), KeyModifiers::empty())),
            AppEventOutcome::ToolApproved(false, false)
        );
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Char('a'), KeyModifiers::CONTROL)),
            AppEventOutcome::Continue
        );
        assert!(
            app.show_palette,
            "approval input must not dispatch the palette"
        );
    }
}
