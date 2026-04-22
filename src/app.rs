use ratatui::widgets::ListState;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use crossterm::event::{self, KeyCode, KeyModifiers};
use std::env;
use regex::Regex;
use once_cell::sync::Lazy;

use crate::context::{ContextManager, ToolCall};
use crate::config::Config;
use crate::icons;
use crate::system_prompt::EXPERT_ENGINEER;
use crate::ui::Theme;
use crate::client::{StreamEvent};
use ratatui::text::Line;

static MARKER_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(r"<\|?/?(?:channel|thought|tool_call|tool_response|turn|bos|eos|think|\||\x22|')[^>]*>?").unwrap());

// Safety limits to prevent UI freezes
const MAX_TOTAL_BLOCKS: usize = 200;

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
    Divider,
    Formulating,
}

#[derive(Clone, Debug)]
pub struct RenderBlock {
    pub block_type: BlockType,
    pub content: String,
    pub success: Option<bool>,
    pub cached_lines: Option<Vec<Line<'static>>>,
}

#[derive(Debug, PartialEq)]
pub enum AppEventOutcome {
    Continue,
    Exit,
    SendPrompt(String),
    ToolApproved(bool, bool),
    Stop,
    ToggleMouse,
}

pub struct App {
    pub input: String,
    pub cursor_pos: usize,
    pub blocks: Vec<RenderBlock>,
    pub output_state: ListState,
    pub is_output_focused: bool,
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
    pub show_hotkeys: bool,
    pub tool_call_pos: Option<usize>,
    pub last_rendered_width: usize,
    pub total_line_count: usize,
    pub mouse_enabled: bool,
    pub current_dir: String,
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
            cursor_pos: 0,
            blocks: vec![RenderBlock { 
                block_type: BlockType::Text, 
                content: "Type a prompt to test tool calling (e.g. 'Run ls'). F12 for debugger.".to_string(),
                success: Some(true),
                cached_lines: None,
            }],
            output_state: ListState::default(),
            is_output_focused: false,
            show_palette: false,
            palette_state,
            palette_items: vec![
                format!("{} Hotkeys", icons::COMMAND),
                format!("{} Themes", icons::THEME),
                format!("{} System Prompt", icons::MODEL),
                format!("{} Clear Context", icons::TRASH),
                format!("{} Toggle Debugger", icons::DEBUG),
                format!("{} Toggle Mouse (Capture)", icons::COMMAND),
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
            show_hotkeys: false,
            tool_call_pos: None,
            last_rendered_width: 0,
            total_line_count: 0,
            mouse_enabled: true,
            current_dir: env::current_dir().map(|p| p.display().to_string()).unwrap_or_else(|_| String::from(".")),
        }
    }

    pub fn add_segment(&mut self, content: String, b_type: BlockType) {
        let mut cleaned_content = MARKER_REGEX.replace_all(&content, "").to_string();
        
        if b_type == BlockType::ToolCall && cleaned_content.contains("write_file") {
             if let Some(Ok((tc, _))) = crate::parser::find_tool_call(&content, true) {
                 if tc.function.name == "write_file" {
                     let path = tc.function.arguments["path"].as_str().unwrap_or("unknown");
                     let file_content = tc.function.arguments["content"].as_str().unwrap_or("");
                     let ext = std::path::Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("txt");
                     cleaned_content = format!("Writing to: {}\n```{}\n{}\n```", path, ext, file_content);
                 }
             }
        }

        if b_type == BlockType::ToolCall && cleaned_content.contains("code_snippet") {
             if let Some(Ok((tc, _))) = crate::parser::find_tool_call(&content, true) {
                 if tc.function.name == "code_snippet" {
                     let name = tc.function.arguments["name"].as_str().unwrap_or("unknown");
                     let file_content = tc.function.arguments["content"].as_str().unwrap_or("");
                     cleaned_content = format!("Storing snippet: {}\n```\n{}\n```", name, file_content);
                 }
             }
        }

        if b_type == BlockType::ToolCall && cleaned_content.contains("search_text") {
             if let Some(Ok((tc, _))) = crate::parser::find_tool_call(&content, true) {
                 if tc.function.name == "search_text" {
                     let pattern = tc.function.arguments["pattern"].as_str().unwrap_or("");
                     let path = tc.function.arguments["path"].as_str().unwrap_or(".");
                     cleaned_content = format!("Searching for: `{}` in `{}`", pattern, path);
                 }
             }
        }

        if b_type == BlockType::Thought && cleaned_content.trim() == "thought" {
            return;
        }

        if b_type == BlockType::User && !self.blocks.is_empty() {
             self.blocks.push(RenderBlock {
                block_type: BlockType::Divider,
                content: String::new(),
                success: None,
                cached_lines: None,
            });
        }

        if let Some(last) = self.blocks.last_mut() {
            // Special handling for Formulating blocks: they get replaced once the tool call arrives
            if last.block_type == BlockType::Formulating && b_type == BlockType::ToolCall {
                last.block_type = BlockType::ToolCall;
                last.content = cleaned_content;
                last.cached_lines = None;
                self.should_redraw = true;
                return;
            }

            if last.block_type == b_type && b_type != BlockType::Divider {
                last.content.push_str(&cleaned_content);
                last.cached_lines = None;
                self.should_redraw = true;
                if self.auto_scroll { self.sync_scroll_to_end(); }
                return;
            }
        }

        let mut success = Some(true);
        if b_type == BlockType::ToolResult && cleaned_content.contains("EXIT_CODE: ") {
            if !cleaned_content.contains("EXIT_CODE: 0") {
                success = Some(false);
            }
        }

        self.blocks.push(RenderBlock {
            block_type: b_type,
            content: cleaned_content,
            success,
            cached_lines: None,
        });
        
        if self.blocks.len() > MAX_TOTAL_BLOCKS {
            self.blocks.remove(0);
        }

        if self.auto_scroll { self.sync_scroll_to_end(); }
        self.should_redraw = true;
    }

    pub fn sync_scroll_to_end(&mut self) {
        if self.total_line_count > 0 {
            self.output_state.select(Some(self.total_line_count.saturating_sub(1)));
        }
    }

    pub fn clear_output(&mut self) {
        self.blocks.clear();
        self.scroll = 0;
        self.auto_scroll = true;
        self.output_state.select(None);
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

    pub fn scroll_output_down(&mut self) {
        if self.total_line_count == 0 { return; }
        let current = self.output_state.selected().unwrap_or(0);
        let next = if current + 3 >= self.total_line_count.saturating_sub(1) { 
            self.total_line_count.saturating_sub(1) 
        } else { 
            current + 3 
        };
        self.output_state.select(Some(next));
        self.auto_scroll = next >= self.total_line_count.saturating_sub(1);
        self.should_redraw = true;
    }

    pub fn scroll_output_up(&mut self) {
        if self.total_line_count == 0 { return; }
        let current = self.output_state.selected().unwrap_or(0);
        let next = current.saturating_sub(3);
        self.output_state.select(Some(next));
        self.auto_scroll = false;
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

pub fn handle_key(app: &mut App, key: event::KeyEvent) -> AppEventOutcome {
    if app.show_cleanup_prompt {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                let _ = std::fs::remove_dir_all(".lethetic");
                app.show_cleanup_prompt = false;
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                app.show_cleanup_prompt = false;
            }
            _ => {}
        }
        return AppEventOutcome::Continue;
    }

    if app.show_hotkeys {
        if key.code == KeyCode::Esc || key.code == KeyCode::Enter {
            app.show_hotkeys = false;
            app.should_redraw = true;
        }
        return AppEventOutcome::Continue;
    }

    if app.show_palette {
        match key.code {
            KeyCode::Char('h') => { app.show_palette = false; app.show_hotkeys = true; }
            KeyCode::Char('t') => { app.show_palette = false; app.show_theme_menu = true; }
            KeyCode::Char('c') => { app.show_palette = false; app.clear_output(); app.context_manager.clear(); }
            KeyCode::Char('d') => { app.show_palette = false; app.show_debug = !app.show_debug; }
            KeyCode::Char('m') => { app.show_palette = false; return AppEventOutcome::ToggleMouse; }
            KeyCode::Char('q') | KeyCode::Esc => { app.show_palette = false; }
            KeyCode::Down | KeyCode::Char('j') => app.next_palette_item(),
            KeyCode::Up | KeyCode::Char('k') => app.previous_palette_item(),
            KeyCode::Enter => {
                let i = app.palette_state.selected().unwrap_or(0);
                match i {
                    0 => { app.show_palette = false; app.show_hotkeys = true; }
                    1 => { app.show_palette = false; app.show_theme_menu = true; }
                    2 => { app.show_palette = false; app.show_prompt_editor = true; }
                    3 => { app.show_palette = false; app.clear_output(); app.context_manager.clear(); }
                    4 => { app.show_palette = false; app.show_debug = !app.show_debug; }
                    5 => { app.show_palette = false; return AppEventOutcome::ToggleMouse; }
                    6 => return AppEventOutcome::Exit,
                    _ => app.show_palette = false,
                }
            }
            _ => {}
        }
        return AppEventOutcome::Continue;
    }

    if app.show_theme_menu {
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                let i = match app.theme_state.selected() {
                    Some(i) => if i >= app.themes.len() - 1 { 0 } else { i + 1 }
                    None => 0,
                };
                app.theme_state.select(Some(i));
                app.theme = app.themes[i].clone();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let i = match app.theme_state.selected() {
                    Some(i) => if i == 0 { app.themes.len() - 1 } else { i - 1 }
                    None => 0,
                };
                app.theme_state.select(Some(i));
                app.theme = app.themes[i].clone();
            }
            KeyCode::Enter | KeyCode::Esc => app.show_theme_menu = false,
            _ => {}
        }
        return AppEventOutcome::Continue;
    }

    if app.show_approval_prompt {
        match key.code {
            KeyCode::Char('a') | KeyCode::Char('A') => return AppEventOutcome::ToolApproved(true, true),
            KeyCode::Char('o') | KeyCode::Char('O') => return AppEventOutcome::ToolApproved(true, false),
            KeyCode::Char('d') | KeyCode::Char('D') | KeyCode::Esc => return AppEventOutcome::ToolApproved(false, false),
            _ => {}
        }
        return AppEventOutcome::Continue;
    }

    // Global Toggles
    match key.code {
        KeyCode::Tab => {
            app.is_output_focused = !app.is_output_focused;
            if app.is_output_focused && app.output_state.selected().is_none() && app.total_line_count > 0 {
                app.output_state.select(Some(app.total_line_count.saturating_sub(1)));
            }
            app.should_redraw = true;
            return AppEventOutcome::Continue;
        }
        KeyCode::F(12) => {
            app.show_debug = !app.show_debug;
            app.should_redraw = true;
            return AppEventOutcome::Continue;
        }
        KeyCode::F(10) => {
            return AppEventOutcome::ToggleMouse;
        }
        _ => {}
    }

    if app.is_output_focused {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => app.scroll_output_up(),
            KeyCode::Down | KeyCode::Char('j') => app.scroll_output_down(),
            KeyCode::PageUp => { for _ in 0..10 { app.scroll_output_up(); } }
            KeyCode::PageDown => { for _ in 0..10 { app.scroll_output_down(); } }
            KeyCode::Esc => app.is_output_focused = false,
            _ => {}
        }
        app.should_redraw = true;
        return AppEventOutcome::Continue;
    }

    match key.code {
        KeyCode::Enter => {
            let p = app.input.drain(..).collect::<String>();
            app.cursor_pos = 0;
            if !p.trim().is_empty() {
                app.should_redraw = true;
                return AppEventOutcome::SendPrompt(p);
            }
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if app.is_processing {
                return AppEventOutcome::Stop;
            } else {
                return AppEventOutcome::Exit;
            }
        }
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.show_palette = true;
            app.should_redraw = true;
        }
        KeyCode::Left => {
            if app.cursor_pos > 0 {
                app.cursor_pos = app.input[..app.cursor_pos].chars().last().map(|c| app.cursor_pos - c.len_utf8()).unwrap_or(0);
                app.should_redraw = true;
            }
        }
        KeyCode::Right => {
            if app.cursor_pos < app.input.len() {
                app.cursor_pos = app.input[app.cursor_pos..].chars().next().map(|c| app.cursor_pos + c.len_utf8()).unwrap_or(app.input.len());
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
        },
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
            if app.is_processing {
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

pub fn handle_tool_call(app: &mut App, calls: Vec<ToolCall>, pos: usize, _tx: mpsc::UnboundedSender<StreamEvent>, cancellation_token: &mut CancellationToken, full_response_content: &str, _is_native: bool) {
    if !app.tool_calls_processed_this_request {
        app.tool_calls_processed_this_request = true;
        cancellation_token.cancel();
        app.tool_call_pos = Some(pos);
        
        app.context_manager.add_message("assistant", full_response_content);

        let tool_call_str = &full_response_content[pos..];
        app.add_segment(tool_call_str.to_string(), BlockType::ToolCall);
        
        let tool_call = calls[0].clone();
        app.pending_tool_call = Some(tool_call.clone()); 
        
        if app.shell_approval_mode == ApprovalMode::Always {
        } else {
            app.show_approval_prompt = true;
            app.is_processing = false;
        }
    }
}
