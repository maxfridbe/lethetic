use ratatui::widgets::ListState;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use crossterm::event::{self, KeyCode, KeyModifiers};
use std::env;

use crate::context::{ContextManager, ToolCall};
use crate::config::Config;
use crate::icons;
use crate::system_prompt::EXPERT_ENGINEER;
use crate::ui::Theme;
use crate::client::{StreamEvent};
use crate::tool_executor::{execute_shell, execute_read_file_lines, execute_apply_patch};

#[derive(PartialEq, Debug, Clone, Copy)]
pub enum ApprovalMode {
    None,
    Always,
}

#[derive(Clone, PartialEq, Debug)]
pub enum BlockType {
    Text,
    User,
    Thought,
    Markdown,
    ToolCall,
    ToolResult,
}

#[derive(Clone, Debug)]
pub struct RenderBlock {
    pub block_type: BlockType,
    pub content: String,
}

#[derive(Debug, PartialEq)]
pub enum AppEventOutcome {
    Continue,
    Exit,
    SendPrompt(String),
    ToolApproved(bool, bool),
    Stop,
}

pub struct App {
    pub input: String,
    pub blocks: Vec<RenderBlock>,
    pub show_palette: bool,
    pub palette_state: ListState,
    pub palette_items: Vec<String>,
    pub theme: Theme,
    pub themes: Vec<Theme>,
    pub show_theme_menu: bool,
    pub theme_state: ListState,
    pub is_processing: bool,
    pub context_manager: ContextManager,
    pub tokens_per_s: f64,
    pub model_name: String,
    pub server_url: String,
    pub max_tokens: usize,
    pub pending_tool_call: Option<ToolCall>,
    pub shell_approval_mode: ApprovalMode,
    pub show_approval_prompt: bool,
    pub spinner_index: usize,
    pub show_debug: bool,
    pub debug_log: Vec<String>,
    pub should_redraw: bool,
    pub tool_calls_processed_this_request: bool,
    pub cwd: String,
    pub git_status: String,
    pub scroll: u16,
    pub auto_scroll: bool,
    pub memory_usage: u64,
    pub system_prompt: String,
    pub show_prompt_editor: bool,
    pub is_editing_prompt: bool,
    pub show_cleanup_prompt: bool,
    pub tool_call_pos: Option<usize>,
}

impl App {
    pub fn new(config: &Config) -> App {
        let mut palette_state = ListState::default();
        palette_state.select(Some(0));
        let mut theme_state = ListState::default();
        theme_state.select(Some(0));
        
        let system_prompt = EXPERT_ENGINEER.to_string();
        let context_manager = ContextManager::new(
            config.context_size, 
            Some(system_prompt.clone())
        );

        let show_cleanup_prompt = std::path::Path::new(".lethetic").exists();

        App {
            input: String::new(),
            blocks: vec![RenderBlock { 
                block_type: BlockType::Text, 
                content: "Type a prompt to test tool calling (e.g. 'Run ls'). F12 for debugger.".to_string() 
            }],
            show_palette: false,
            palette_state,
            palette_items: vec![
                format!("{} Themes", icons::THEME),
                format!("{} System Prompt", icons::MODEL),
                format!("{} Clear Context", icons::TRASH),
                format!("{} Toggle Debugger", icons::DEBUG),
                format!("{} Quit", icons::QUIT),
            ],
            theme: Theme::default(),
            themes: Theme::all(),
            show_theme_menu: false,
            theme_state,
            is_processing: false,
            context_manager,
            tokens_per_s: 0.0,
            model_name: config.model.clone(),
            server_url: config.server_url.clone(),
            max_tokens: config.context_size,
            pending_tool_call: None,
            shell_approval_mode: ApprovalMode::None,
            show_approval_prompt: false,
            spinner_index: 0,
            show_debug: true,
            debug_log: Vec::new(),
            should_redraw: true,
            tool_calls_processed_this_request: false,
            cwd: String::from("N/A"),
            git_status: String::from("N/A"),
            scroll: 0,
            auto_scroll: true,
            memory_usage: 0,
            system_prompt,
            show_prompt_editor: false,
            is_editing_prompt: false,
            show_cleanup_prompt,
            tool_call_pos: None,
        }
    }

    pub fn add_segment(&mut self, content: String, b_type: BlockType) {
        if let Some(last) = self.blocks.last_mut() {
            if last.block_type == b_type {
                last.content.push_str(&content);
                self.should_redraw = true;
                return;
            }
        }
        self.blocks.push(RenderBlock {
            block_type: b_type,
            content,
        });
        self.should_redraw = true;
    }

    pub fn clear_output(&mut self) {
        self.blocks.clear();
        self.scroll = 0;
        self.auto_scroll = true;
        self.should_redraw = true;
    }

    pub fn next_palette_item(&mut self) {
        let i = match self.palette_state.selected() {
            Some(i) => if i >= self.palette_items.len() - 1 { 0 } else { i + 1 }
            None => 0,
        };
        self.palette_state.select(Some(i));
        self.should_redraw = true;
    }

    pub fn previous_palette_item(&mut self) {
        let i = match self.palette_state.selected() {
            Some(i) => if i == 0 { self.palette_items.len() - 1 } else { i - 1 }
            None => 0,
        };
        self.palette_state.select(Some(i));
        self.should_redraw = true;
    }

    pub fn tick_spinner(&mut self) {
        self.spinner_index = (self.spinner_index + 1) % icons::SPINNER.len();
        self.should_redraw = true;
    }

    pub fn log_debug(&mut self, msg: &str) {
        let timestamp = chrono::Local::now().format("%H:%M:%S%.3f");
        self.debug_log.push(format!("[{}] {}", timestamp, msg));
        if self.debug_log.len() > 200 { self.debug_log.remove(0); }
        self.should_redraw = true;
    }

    pub fn refresh_system_stats(&mut self) {
        self.cwd = env::current_dir().map(|p| p.display().to_string()).unwrap_or_else(|_| String::from("N/A"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use crate::context::FunctionCall;

    #[test]
    fn test_ctrl_c_quits() {
        let mut app = App::new(&Config { server_url: "url".to_string(), model: "model".to_string(), context_size: 4000, tool_wrapper: None });
        assert_eq!(handle_key(&mut app, event::KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)), AppEventOutcome::Exit);
    }

    #[test]
    fn test_approval_outcome() {
        let mut app = App::new(&Config { server_url: "url".to_string(), model: "model".to_string(), context_size: 4000, tool_wrapper: None });
        app.show_approval_prompt = true;
        app.pending_tool_call = Some(ToolCall { 
            id: "1".into(), 
            tool_type: None, 
            function: FunctionCall { name: "test".into(), arguments: json!({}) }, 
            index: None 
        });
        assert_eq!(handle_key(&mut app, event::KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)), AppEventOutcome::ToolApproved(true, true));
    }
}

pub fn handle_key(app: &mut App, key: event::KeyEvent) -> AppEventOutcome {
    app.should_redraw = true;
    
    if key.code == KeyCode::F(12) {
        app.show_debug = !app.show_debug;
        return AppEventOutcome::Continue;
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return AppEventOutcome::Exit;
    }

    if app.show_cleanup_prompt {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                if let Err(e) = std::fs::remove_dir_all(".lethetic") {
                    app.log_debug(&format!("Failed to clear .lethetic: {}", e));
                } else {
                    app.log_debug("Cleared .lethetic directory.");
                }
                app.show_cleanup_prompt = false;
                return AppEventOutcome::Continue;
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                app.show_cleanup_prompt = false;
                return AppEventOutcome::Continue;
            }
            _ => return AppEventOutcome::Continue,
        }
    }

    if app.show_prompt_editor {
        if app.is_editing_prompt {
            match key.code {
                KeyCode::Esc => { app.is_editing_prompt = false; AppEventOutcome::Continue }
                KeyCode::Char(c) => { app.system_prompt.push(c); AppEventOutcome::Continue }
                KeyCode::Backspace => { app.system_prompt.pop(); AppEventOutcome::Continue }
                KeyCode::Enter => { app.system_prompt.push('\n'); AppEventOutcome::Continue }
                _ => AppEventOutcome::Continue,
            }
        } else {
            match key.code {
                KeyCode::Esc => { app.show_prompt_editor = false; AppEventOutcome::Continue }
                KeyCode::Char('m') | KeyCode::Char('M') => { app.is_editing_prompt = true; AppEventOutcome::Continue }
                KeyCode::Char('s') | KeyCode::Char('S') => {
                    app.context_manager.update_system_prompt(app.system_prompt.clone());
                    app.log_debug("System prompt updated in context.");
                    app.show_prompt_editor = false;
                    AppEventOutcome::Continue
                }
                _ => AppEventOutcome::Continue,
            }
        }
    } else if app.show_approval_prompt {
        match key.code {
            KeyCode::Char('a') | KeyCode::Char('A') => return AppEventOutcome::ToolApproved(true, true),
            KeyCode::Char('o') | KeyCode::Char('O') => return AppEventOutcome::ToolApproved(true, false),
            KeyCode::Char('d') | KeyCode::Char('D') => return AppEventOutcome::ToolApproved(false, false),
            KeyCode::Esc => { app.show_approval_prompt = false; return AppEventOutcome::Continue; }
            _ => return AppEventOutcome::Continue,
        }
    } else if key.modifiers.contains(KeyModifiers::CONTROL) && (key.code == KeyCode::Char('p') || key.code == KeyCode::Char('P')) {
        app.show_palette = !app.show_palette;
        app.show_theme_menu = false;
        return AppEventOutcome::Continue;
    } else if app.show_theme_menu {
        match key.code {
            KeyCode::Esc => { app.show_theme_menu = false; AppEventOutcome::Continue }
            KeyCode::Up => { 
                let i = match app.theme_state.selected() {
                    Some(i) => if i == 0 { app.themes.len() - 1 } else { i - 1 }
                    None => 0,
                };
                app.theme_state.select(Some(i));
                AppEventOutcome::Continue 
            }
            KeyCode::Down => { 
                let i = match app.theme_state.selected() {
                    Some(i) => if i >= app.themes.len() - 1 { 0 } else { i + 1 }
                    None => 0,
                };
                app.theme_state.select(Some(i));
                AppEventOutcome::Continue 
            }
            KeyCode::Enter => {
                if let Some(i) = app.theme_state.selected() { app.theme = app.themes[i].clone(); }
                app.show_theme_menu = false;
                AppEventOutcome::Continue
            }
            _ => AppEventOutcome::Continue,
        }
    } else if app.show_palette {
        match key.code {
            KeyCode::Esc => { app.show_palette = false; AppEventOutcome::Continue }
            KeyCode::Up => { app.previous_palette_item(); AppEventOutcome::Continue }
            KeyCode::Down => { app.next_palette_item(); AppEventOutcome::Continue }
            KeyCode::Enter => {
                if let Some(i) = app.palette_state.selected() {
                    let item = &app.palette_items[i];
                    if item.contains("Quit") {
                        return AppEventOutcome::Exit;
                    } else if item.contains("Themes") {
                        app.show_theme_menu = true;
                        app.show_palette = false;
                    } else if item.contains("System Prompt") {
                        app.show_prompt_editor = true;
                        app.show_palette = false;
                    } else if item.contains("Clear Context") {
                        app.context_manager.clear();
                        app.clear_output();
                        app.add_segment(format!("{} Context cleared.", icons::SUCCESS), BlockType::Text);
                    } else if item.contains("Toggle Debugger") {
                        app.show_debug = !app.show_debug;
                    }
                }
                app.show_palette = false;
                AppEventOutcome::Continue
            }
            _ => AppEventOutcome::Continue,
        }
    } else {
        match key.code {
            KeyCode::Esc => if app.is_processing { AppEventOutcome::Stop } else { AppEventOutcome::Continue }
            KeyCode::Up => { app.scroll = app.scroll.saturating_sub(1); app.auto_scroll = false; AppEventOutcome::Continue }
            KeyCode::Down => { app.scroll = app.scroll.saturating_add(1); AppEventOutcome::Continue }
            KeyCode::PageUp => { app.scroll = app.scroll.saturating_sub(10); app.auto_scroll = false; AppEventOutcome::Continue }
            KeyCode::PageDown => { app.scroll = app.scroll.saturating_add(10); AppEventOutcome::Continue }
            KeyCode::Char(c) => { app.input.push(c); AppEventOutcome::Continue }
            KeyCode::Backspace => { app.input.pop(); AppEventOutcome::Continue }
            KeyCode::Enter => {
                if app.is_processing { AppEventOutcome::Continue } else {
                    let prompt = app.input.clone();
                    app.input.clear();
                    AppEventOutcome::SendPrompt(prompt)
                }
            }
            _ => AppEventOutcome::Continue,
        }
    }
}

pub fn handle_tool_call(app: &mut App, calls: Vec<ToolCall>, pos: usize, _tx: mpsc::UnboundedSender<StreamEvent>, cancellation_token: &mut CancellationToken, _full_response_content: &str, _is_native: bool) {
    if !app.tool_calls_processed_this_request {
        app.tool_calls_processed_this_request = true;
        cancellation_token.cancel();
        app.tool_call_pos = Some(pos);
        app.add_segment(format!("\n{} [PROCESSING HALTED] Reason: Tool Call Intercepted\n", icons::WARNING), BlockType::Text);
        
        let tool_call = calls[0].clone();
        app.pending_tool_call = Some(tool_call.clone()); // Store it for later context update and approval
        
        let tc_header = format!("{} [TOOL CALL: {}]", icons::COMMAND, tool_call.function.name);
        app.log_debug(&format!("TOOL REQUESTED: {} with params: {}", tool_call.function.name, tool_call.function.arguments));
        app.add_segment(format!("\n\n{} \nArguments: {}\n", tc_header, tool_call.function.arguments), BlockType::ToolCall);

        if app.shell_approval_mode == ApprovalMode::Always {
            // We'll trigger this in main.rs after Done to ensure context is updated first
        } else {
            app.show_approval_prompt = true;
            app.is_processing = false;
        }
    }
}
