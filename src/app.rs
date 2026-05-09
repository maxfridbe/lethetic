use ratatui::widgets::ListState;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use crossterm::event::{self, KeyCode, KeyModifiers};
use std::env;
use regex::Regex;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

use crate::context::{ContextManager, ToolCall};
use crate::config::Config;
use crate::icons;
use crate::ui::Theme;
use crate::client::{StreamEvent};
use crate::parser::StreamParser;
use crate::loop_detector::{LoopDetector, LoopDetectorConfig};
use ratatui::text::Line;

static MARKER_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(r"<\|?/?(?:channel|thought|tool_call|tool_response|turn|bos|eos|think|\||\x22|')[^>]*>?(?:thought|text|model|system)?").unwrap());

// Safety limits to prevent UI freezes
const MAX_TOTAL_BLOCKS: usize = 200;

#[derive(PartialEq, Debug, Clone, Copy)]
pub enum ApprovalMode {
    None,
    Always,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RenderBlock {
    pub block_type: BlockType,
    pub content: String,
    pub title: Option<String>,
    pub success: Option<bool>,
    #[serde(skip)]
    pub cached_lines: Option<Vec<Line<'static>>>,
}

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
    ToggleHistory,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SessionState {
    pub messages: Vec<crate::context::Message>,
    pub blocks: Vec<RenderBlock>,
    pub history: Vec<String>,
    #[serde(default)]
    pub theme_name: String,
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
    pub pp_tokens_per_s: f64,
    pub server_prompt_tokens: Option<u32>,
    pub model_name: String,
    pub server_url: String,
    pub max_tokens: usize,
    pub pending_tool_call: Option<ToolCall>,
    pub shell_approval_mode: ApprovalMode,
    pub show_approval_prompt: bool,
    pub spinner_index: usize,
    pub tool_spinner_index: usize,
    pub is_executing_tool: bool,
    pub tool_output_preview: String,
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
    pub show_prompt_save_dialog: bool,
    pub prompt_save_name: String,
    pub system_prompt_manager: crate::system_prompt::SystemPromptManager,
    pub show_prompt_manager: bool,
    pub prompt_files: Vec<String>,
    pub prompt_list_state: ListState,
    pub show_cleanup_prompt: bool,
    pub show_hotkeys: bool,
    pub tool_call_pos: Option<usize>,
    pub last_rendered_width: usize,
    pub total_line_count: usize,
    pub current_dir: String,
    pub current_session_dir: Option<String>,
    pub session_files: Vec<String>,
    pub session_list_state: ListState,
    pub show_session_manager: bool,
    pub needs_save: bool,
    pub request_start_time: Option<tokio::time::Instant>,
    pub is_asking_user: bool,
    pub prompt_cursor_pos: usize,
    pub prompt_scroll: usize,
    pub parser: StreamParser,
    pub loop_detector: LoopDetector,
    pub last_block_content: String,
    pub loop_detection_count: usize,
    pub last_loop_detection_time: Option<std::time::Instant>,
    pub is_loading_session: bool,
    pub load_progress: f32,
    pub load_status: String,
    pub history: Vec<String>,
    pub history_state: ListState,
    pub backbuffer: String,
    pub show_history: bool,
    pub show_latest_files: bool,
    pub latest_files_state: ListState,
    pub config: Config,
    }

    impl App {
    pub fn new(config: &Config) -> App {
        let mut palette_state = ListState::default();
        palette_state.select(Some(0));

        let mut session_list_state = ListState::default();
        session_list_state.select(Some(0));
        let mut latest_files_state = ListState::default();
        latest_files_state.select(Some(0));
        let history_state = ListState::default();
        
        let system_prompt_manager = crate::system_prompt::SystemPromptManager::new();
        let system_prompt = system_prompt_manager.load_prompt("software_engineer").unwrap_or_else(|| crate::system_prompt::DEFAULT_PROMPT_TEMPLATE.to_string());
        
        let cwd = std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| ".".to_string());
        let resolved_prompt = crate::system_prompt::SystemPromptManager::resolve_prompt(&system_prompt, &cwd, config);
        let context_manager = ContextManager::new(
            config.context_size, 
            Some(resolved_prompt)
        );

        let mut app = App {
            input: String::new(),
            cursor_pos: 0,
            blocks: vec![RenderBlock { 
                block_type: BlockType::Text,
                content: "Type a prompt to test tool calling (e.g. 'Run ls'). F12 for debugger.".to_string(),
                title: None,
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
                format!("{} Input History", icons::COMMAND),
                format!("{} Loop Detection: Combined", icons::PROCESSING),
                format!("{} System Prompt", icons::MODEL),
                format!("{} Clear UI (Keep Context)", icons::TRASH),
                format!("{} Clear All Context", icons::TRASH),
                format!("{} Toggle Debugger", icons::DEBUG),
                format!("{} Sessions", icons::COMMAND),
                format!("{} Latest Files", icons::COMMAND),
                format!("{} Quit", icons::QUIT),
            ],
            theme: {
                let all = Theme::all();
                if let Some(name) = &config.theme {
                    all.iter().find(|t| t.name.eq_ignore_ascii_case(name)).cloned().unwrap_or_else(Theme::default)
                } else {
                    Theme::default()
                }
            },
            themes: Theme::all(),
            show_theme_menu: false,
            theme_state: {
                let all = Theme::all();
                let idx = config.theme.as_ref()
                    .and_then(|n| all.iter().position(|t| t.name.eq_ignore_ascii_case(n)))
                    .unwrap_or(0);
                let mut s = ListState::default();
                s.select(Some(idx));
                s
            },
            is_processing: false,
            context_manager,
            tokens_per_s: 0.0,
            pp_tokens_per_s: 0.0,
            server_prompt_tokens: None,
            model_name: config.model.clone(),
            server_url: config.server_url.clone(),
            max_tokens: config.context_size,
            pending_tool_call: None,
            shell_approval_mode: ApprovalMode::None,
            show_approval_prompt: false,
            spinner_index: 0,
            tool_spinner_index: 0,
            is_executing_tool: false,
            tool_output_preview: String::new(),
            show_debug: true,
            debug_log: Vec::new(),
            should_redraw: true,
            tool_calls_processed_this_request: false,
            cwd: String::from("N/A"),
            git_status: String::from("N/A"),
            scroll: 0,
            auto_scroll: true,
            memory_usage: 0,
            system_prompt: system_prompt.clone(),
            show_prompt_editor: false,
            is_editing_prompt: false,
            show_prompt_save_dialog: false,
            prompt_save_name: String::new(),
            system_prompt_manager,
            show_prompt_manager: false,
            prompt_files: Vec::new(),
            prompt_list_state: ListState::default(),
            show_cleanup_prompt: false,
            show_hotkeys: false,
            tool_call_pos: None,
            last_rendered_width: 0,
            total_line_count: 0,
            current_dir: env::current_dir().map(|p| p.display().to_string()).unwrap_or_else(|_| String::from(".")),
            current_session_dir: None,
            session_files: Vec::new(),
            session_list_state: ListState::default(),
            show_session_manager: false,
            needs_save: false,
            request_start_time: None,
            is_asking_user: false,
            prompt_cursor_pos: system_prompt.len(),
            prompt_scroll: 0,
            parser: StreamParser::new(),
            loop_detector: LoopDetector::new(LoopDetectorConfig::default()),
            last_block_content: String::new(),
            loop_detection_count: 0,
            last_loop_detection_time: None,
            is_loading_session: false,
            load_progress: 0.0,
            load_status: String::new(),
            history: Vec::new(),
            history_state: history_state,
            backbuffer: String::new(),
            show_history: false,
            show_latest_files: false,
            latest_files_state: latest_files_state,
            config: config.clone(),
            };
        app.refresh_session_list();
        if !app.session_files.is_empty() {
            app.show_session_manager = true;
            app.session_list_state.select(Some(0));
        } else {
            app.start_new_session();
        }

        app
    }

    pub fn start_new_session(&mut self) {
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
        let session_dir = format!(".lethetic/sessions/session_{}", timestamp);
        let _ = std::fs::create_dir_all(&session_dir);
        self.current_session_dir = Some(session_dir);
        self.blocks.clear();
        self.blocks.push(RenderBlock { 
            block_type: BlockType::Text, 
            content: "New session started. Type a prompt to begin.".to_string(),
            title: None,
            success: Some(true),
            cached_lines: None,
        });
        self.context_manager.clear();
        self.save_session();
    }

    pub fn add_to_history(&mut self, text: String) {
        let trimmed = text.trim();
        if trimmed.is_empty() { return; }

        // Remove if already exists to move it to the end (most recent)
        if let Some(pos) = self.history.iter().position(|x| x == trimmed) {
            self.history.remove(pos);
        }
        self.history.push(trimmed.to_string());

        // Limit history size to 100
        if self.history.len() > 100 {
            self.history.remove(0);
        }
    }

    pub fn save_session(&mut self) {
        if let Some(ref dir) = self.current_session_dir {
            let _ = std::fs::create_dir_all(dir);

            // Save unified session state with history
            let state = SessionState {
                messages: self.context_manager.get_messages().to_vec(),
                blocks: self.blocks.clone(),
                history: self.history.clone(),
                theme_name: self.theme.name.clone(),
            };
            if let Ok(json) = serde_json::to_string_pretty(&state) {
                let _ = std::fs::write(format!("{}/session_state.json", dir), json);
            }
        }
        self.needs_save = false;
    }

    pub fn refresh_session_list(&mut self) {
        let mut dirs = Vec::new();
        let sessions_root = ".lethetic/sessions";
        let _ = std::fs::create_dir_all(sessions_root);
        if let Ok(entries) = std::fs::read_dir(sessions_root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        if name.starts_with("session_") {
                            dirs.push(path.display().to_string());
                        }
                    }
                }
            }
        }
        dirs.sort_by(|a, b| b.cmp(a)); // Newest first
        self.session_files = dirs;
        if self.session_files.is_empty() {
            self.session_list_state.select(None);
        } else if self.session_list_state.selected().is_none() {
            self.session_list_state.select(Some(0));
        }
    }

    pub fn refresh_prompt_list(&mut self) {
        self.prompt_files = self.system_prompt_manager.list_prompts();
        if self.prompt_list_state.selected().is_none() && !self.prompt_files.is_empty() {
            self.prompt_list_state.select(Some(0));
        }
    }

    pub fn load_session(&mut self, session_dir: &str) {
        // Try loading unified session state first
        if let Ok(content) = std::fs::read_to_string(format!("{}/session_state.json", session_dir)) {
            if let Ok(state) = serde_json::from_str::<SessionState>(&content) {
                self.blocks = state.blocks;
                self.history = state.history;
                self.context_manager.clear();
                for msg in state.messages {
                    self.context_manager.add_message_raw(msg);
                }
                // Restore theme if saved
                if !state.theme_name.is_empty() {
                    if let Some(t) = self.themes.iter().find(|t| t.name == state.theme_name) {
                        let t = t.clone();
                        if let Some(idx) = self.themes.iter().position(|th| th.name == t.name) {
                            self.theme_state.select(Some(idx));
                        }
                        self.theme = t;
                        for block in &mut self.blocks { block.cached_lines = None; }
                    }
                }
                self.current_session_dir = Some(session_dir.to_string());
                self.should_redraw = true;
                self.needs_save = false;
                if self.auto_scroll { self.sync_scroll_to_end(); }
                return;
            }
        }

        // Fallback to legacy individual files
        if let Ok(content) = std::fs::read_to_string(format!("{}/ui_state.json", session_dir)) {
            if let Ok(blocks) = serde_json::from_str::<Vec<RenderBlock>>(&content) {
                self.blocks = blocks;
            }
        }
        
        // Load Context
        if let Ok(content) = std::fs::read_to_string(format!("{}/context.json", session_dir)) {
            if let Ok(messages) = serde_json::from_str::<Vec<crate::context::Message>>(&content) {
                self.context_manager.clear();
                for msg in messages {
                    self.context_manager.add_message_raw(msg);
                }
            }
        }
        
        self.current_session_dir = Some(session_dir.to_string());
        self.should_redraw = true;
        self.needs_save = false;
        if self.auto_scroll { self.sync_scroll_to_end(); }
    }

    pub fn add_segment(&mut self, content: String, b_type: BlockType) {
        // Splitting Logic: If content contains markers, we need to process parts separately
        let mut last_pos = 0;
        let mut parts = Vec::new();
        
        for m in MARKER_REGEX.find_iter(&content) {
            if m.start() > last_pos {
                parts.push((&content[last_pos..m.start()], false));
            }
            parts.push((&content[m.start()..m.end()], true));
            last_pos = m.end();
        }
        if last_pos < content.len() {
            parts.push((&content[last_pos..], false));
        }

        if parts.is_empty() { return; }

        for (part, is_marker) in parts {
            if is_marker && b_type != BlockType::ToolCall && b_type != BlockType::Formulating {
                // Skip markers in UI content for Text/Thought blocks, but KEEP them for tool blocks
                continue;
            }
            self.add_segment_internal(part.to_string(), b_type.clone());
        }
    }

    pub fn add_segment_with_title(&mut self, content: String, b_type: BlockType, title: String) {
        self.add_segment_internal_with_title(content, b_type, Some(title));
    }

    fn add_segment_internal(&mut self, cleaned_content: String, b_type: BlockType) {
        self.add_segment_internal_with_title(cleaned_content, b_type, None);
    }

    fn add_segment_internal_with_title(&mut self, cleaned_content: String, b_type: BlockType, title: Option<String>) {
        if cleaned_content.is_empty() && b_type != BlockType::Divider {
            return;
        }

        if let Some(last) = self.blocks.last_mut() {
            if last.block_type == BlockType::Formulating && b_type == BlockType::ToolCall {
                last.block_type = BlockType::ToolCall;
                last.content = cleaned_content.clone();
                last.title = title;
                last.cached_lines = None;
                self.last_block_content = cleaned_content;
                self.should_redraw = true;
                self.needs_save = true;
                return;
            }

            if last.block_type == b_type && b_type != BlockType::Divider && last.title == title {
                last.content.push_str(&cleaned_content);
                self.last_block_content.push_str(&cleaned_content);
                last.cached_lines = None;
                self.should_redraw = true;
                if self.auto_scroll { self.sync_scroll_to_end(); }
                self.needs_save = true;
                return;
            }
        }

        self.last_block_content = cleaned_content.clone();
        self.add_block(cleaned_content, b_type, title);
    }

    fn add_block(&mut self, content: String, b_type: BlockType, title: Option<String>) {
        if b_type == BlockType::User && !self.blocks.is_empty() {
             self.blocks.push(RenderBlock {
                block_type: BlockType::Divider,
                content: String::new(),
                title: None,
                success: None,
                cached_lines: None,
            });
        }

        let mut success = Some(true);
        if b_type == BlockType::ToolResult && content.contains("EXIT_CODE: ") {
            if !content.contains("EXIT_CODE: 0") {
                success = Some(false);
            }
        }

        self.blocks.push(RenderBlock {
            block_type: b_type.clone(),
            content: content.clone(),
            title: title.clone(),
            success,
            cached_lines: None,
        });

        // Append verbatim block to ui_log.txt for post-run diagnosis
        if let Some(ref session_dir) = self.current_session_dir {
            let header = match &b_type {
                BlockType::User       => "=== USER ===".to_string(),
                BlockType::Thought    => "=== THOUGHT ===".to_string(),
                BlockType::Text | BlockType::Markdown => "=== TEXT ===".to_string(),
                BlockType::ToolCall   => format!("=== TOOL CALL: {} ===", title.as_deref().unwrap_or("")),
                BlockType::ToolResult => "=== TOOL RESULT ===".to_string(),
                BlockType::Formulating => "=== FORMULATING ===".to_string(),
                BlockType::Divider    => String::new(),
            };
            if !header.is_empty() {
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true)
                    .open(format!("{}/ui_log.txt", session_dir))
                {
                    use std::io::Write as _;
                    let _ = writeln!(f, "{}\n{}\n", header, content);
                }
            }
        }

        if self.blocks.len() > MAX_TOTAL_BLOCKS {
            self.blocks.remove(0);
        }

        if self.auto_scroll { self.sync_scroll_to_end(); }
        self.should_redraw = true;
        self.needs_save = true;
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
        self.needs_save = true;
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

    pub fn scroll_output_down(&mut self, amount: usize) {
        if self.total_line_count == 0 { return; }
        let current = self.output_state.selected().unwrap_or(0);
        let next = if current + amount >= self.total_line_count.saturating_sub(1) {
            self.total_line_count.saturating_sub(1)
        } else {
            current + amount
        };
        self.output_state.select(Some(next));
        self.auto_scroll = next >= self.total_line_count.saturating_sub(1);
        self.should_redraw = true;
    }

    pub fn scroll_output_up(&mut self, amount: usize) {
        if self.total_line_count == 0 { return; }
        let current = self.output_state.selected().unwrap_or(0);
        let next = current.saturating_sub(amount);
        self.output_state.select(Some(next));
        self.auto_scroll = false;
        self.should_redraw = true;
    }

    pub fn scroll_to_top(&mut self) {
        self.output_state.select(Some(0));
        self.auto_scroll = false;
        self.should_redraw = true;
    }

    pub fn scroll_to_bottom(&mut self) {
        if self.total_line_count > 0 {
            self.output_state.select(Some(self.total_line_count.saturating_sub(1)));
            self.auto_scroll = true;
            self.should_redraw = true;
        }
    }
    pub fn tick_spinner(&mut self) {
        self.spinner_index = (self.spinner_index + 1) % icons::SPINNER.len();
        self.tool_spinner_index = (self.tool_spinner_index + 1) % icons::TOOL_SPINNER.len();
        self.should_redraw = true;
    }

    pub fn log_debug(&mut self, msg: &str) {
        let now = chrono::Local::now();
        let timestamp = now.format("%H:%M:%S%.3f");
        let log_entry = format!("[{}] {}", timestamp, msg);
        
        self.debug_log.push(log_entry.clone());
        if self.debug_log.len() > 200 { self.debug_log.remove(0); }
        self.should_redraw = true;

        if let Some(session_dir) = &self.current_session_dir {
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(format!("{}/logs.txt", session_dir))
            {
                use std::io::Write;
                let _ = writeln!(file, "{}", log_entry);
            }
        }
    }

    pub fn refresh_system_stats(&mut self) {
        self.cwd = env::current_dir().map(|p| p.display().to_string()).unwrap_or_else(|_| String::from("N/A"));
    }
}

pub fn handle_key(app: &mut App, key: event::KeyEvent) -> AppEventOutcome {
    if app.show_prompt_editor {
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
                        app.prompt_cursor_pos = app.system_prompt[..app.prompt_cursor_pos].chars().last().map(|c| app.prompt_cursor_pos - c.len_utf8()).unwrap_or(0);
                        app.should_redraw = true;
                    }
                }
                KeyCode::Right => {
                    if app.prompt_cursor_pos < app.system_prompt.len() {
                        app.prompt_cursor_pos = app.system_prompt[app.prompt_cursor_pos..].chars().next().map(|c| app.prompt_cursor_pos + c.len_utf8()).unwrap_or(app.system_prompt.len());
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
                        let prev_char = app.system_prompt[..app.prompt_cursor_pos].chars().last().unwrap();
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
                        let _ = app.system_prompt_manager.save_prompt(&name, &app.system_prompt);
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
                    let resolved = crate::system_prompt::SystemPromptManager::resolve_prompt(&app.system_prompt, &app.cwd, &app.config);
                    app.context_manager.update_system_prompt(resolved);
                    // Also auto-save as software_engineer.md
                    let _ = app.system_prompt_manager.save_prompt("software_engineer", &app.system_prompt);
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
        return AppEventOutcome::Continue;
    }

    if app.show_prompt_manager {
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                let max = app.prompt_files.len() + 1; // +1 for "Create New"
                let i = match app.prompt_list_state.selected() {
                    Some(i) => if i >= max.saturating_sub(1) { 0 } else { i + 1 },
                    None => 0,
                };
                if max > 0 { app.prompt_list_state.select(Some(i)); }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let max = app.prompt_files.len() + 1;
                let i = match app.prompt_list_state.selected() {
                    Some(i) => if i == 0 { max.saturating_sub(1) } else { i - 1 },
                    None => 0,
                };
                if max > 0 { app.prompt_list_state.select(Some(i)); }
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
        return AppEventOutcome::Continue;
    }

    if app.show_history {
        match key.code {
            KeyCode::Esc => {
                app.show_history = false;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let i = match app.history_state.selected() {
                    Some(i) => if i == 0 { app.history.len().saturating_sub(1) } else { i - 1 },
                    None => 0,
                };
                app.history_state.select(Some(i));
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let i = match app.history_state.selected() {
                    Some(i) => if i >= app.history.len().saturating_sub(1) { 0 } else { i + 1 },
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
        return AppEventOutcome::Continue;
    }

    if app.show_session_manager {
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                let i = match app.session_list_state.selected() {
                    Some(i) => if i >= app.session_files.len().saturating_sub(1) { 0 } else { i + 1 }
                    None => 0,
                };
                if !app.session_files.is_empty() { app.session_list_state.select(Some(i)); }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let i = match app.session_list_state.selected() {
                    Some(i) => if i == 0 { app.session_files.len().saturating_sub(1) } else { i - 1 }
                    None => 0,
                };
                if !app.session_files.is_empty() { app.session_list_state.select(Some(i)); }
            }
            KeyCode::Enter => {
                if let Some(i) = app.session_list_state.selected() {
                    if i < app.session_files.len() {
                        let filename = app.session_files[i].clone();
                        app.show_session_manager = false;
                        return AppEventOutcome::ResumeSession(filename);
                    }
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                app.show_session_manager = false;
                return AppEventOutcome::NewSession;
            }
            KeyCode::Char('d') | KeyCode::Char('D') => {
                if let Some(i) = app.session_list_state.selected() {
                    if i < app.session_files.len() {
                        let filename = app.session_files[i].clone();
                        return AppEventOutcome::DeleteSession(filename);
                    }
                }
            }
            KeyCode::Char('x') | KeyCode::Char('X') => {
                for f in &app.session_files {
                    let _ = std::fs::remove_dir_all(f);
                }
                app.session_files.clear();
                app.session_list_state.select(None);
                app.show_session_manager = false;
                return AppEventOutcome::NewSession;
            }
            KeyCode::Esc => {
                if app.current_session_dir.is_some() {
                    app.show_session_manager = false;
                }
            }
            _ => {}
        }
        app.should_redraw = true;
        return AppEventOutcome::Continue;
    }

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
            KeyCode::Char('c') => { app.show_palette = false; app.blocks.clear(); app.should_redraw = true; app.needs_save = true; }
            KeyCode::Char('d') => { app.show_palette = false; app.show_debug = !app.show_debug; }
            KeyCode::Char('q') | KeyCode::Esc => { app.show_palette = false; }
            KeyCode::Down | KeyCode::Char('j') => app.next_palette_item(),
            KeyCode::Up | KeyCode::Char('k') => app.previous_palette_item(),
            KeyCode::Enter => {
                let i = app.palette_state.selected().unwrap_or(0);
                match i {
                    0 => { app.show_palette = false; app.show_hotkeys = true; }
                    1 => { app.show_palette = false; app.show_theme_menu = true; }
                    2 => { 
                        if !app.history.is_empty() {
                            app.show_palette = false;
                            app.show_history = true;
                            app.history_state.select(Some(0));
                        }
                    }
                    3 => { 
                        // Cycle loop detection mode
                        use crate::loop_detector::LoopDetectionMode;
                        let next_mode = match app.loop_detector.config.mode {
                            LoopDetectionMode::Off => LoopDetectionMode::BlockLimit,
                            LoopDetectionMode::BlockLimit => LoopDetectionMode::NGram,
                            LoopDetectionMode::NGram => LoopDetectionMode::PhraseFrequency,
                            LoopDetectionMode::PhraseFrequency => LoopDetectionMode::Combined,
                            LoopDetectionMode::Combined => LoopDetectionMode::Off,
                        };
                        app.loop_detector.config.mode = next_mode;
                        app.palette_items[3] = format!("{} Loop Detection: {:?}", icons::PROCESSING, next_mode);
                        app.should_redraw = true;
                    }
                    4 => { app.show_palette = false; app.refresh_prompt_list(); app.show_prompt_manager = true; }
                    5 => { app.show_palette = false; app.blocks.clear(); app.should_redraw = true; app.needs_save = true; }
                    6 => { app.show_palette = false; app.context_manager.clear(); app.start_new_session(); }
                    7 => { app.show_palette = false; app.show_debug = !app.show_debug; }
                    8 => { app.show_palette = false; app.refresh_session_list(); app.show_session_manager = true; }
                    9 => { app.show_palette = false; app.show_latest_files = true; app.latest_files_state.select(Some(0)); }
                    10 => return AppEventOutcome::Exit,
                    _ => app.show_palette = false,
                }
            }
            _ => {}
        }
        return AppEventOutcome::Continue;
    }

    if app.show_latest_files {
        match key.code {
            KeyCode::Esc => { app.show_latest_files = false; }
            KeyCode::Down | KeyCode::Char('j') => {
                let num_files = app.context_manager.latest_files.len();
                if num_files > 0 {
                    let i = match app.latest_files_state.selected() {
                        Some(i) => if i >= num_files - 1 { 0 } else { i + 1 },
                        None => 0,
                    };
                    app.latest_files_state.select(Some(i));
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let num_files = app.context_manager.latest_files.len();
                if num_files > 0 {
                    let i = match app.latest_files_state.selected() {
                        Some(i) => if i == 0 { num_files - 1 } else { i - 1 },
                        None => 0,
                    };
                    app.latest_files_state.select(Some(i));
                }
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                let paths: Vec<String> = app.context_manager.latest_files.keys().cloned().collect();
                if let Some(i) = app.latest_files_state.selected() {
                    if let Some(path) = paths.get(i) {
                        app.context_manager.remove_latest_file(path);
                        // Adjust selection
                        let num_files = app.context_manager.latest_files.len();
                        if num_files == 0 {
                            app.latest_files_state.select(None);
                        } else if i >= num_files {
                            app.latest_files_state.select(Some(num_files - 1));
                        }
                    }
                }
            }
            _ => {}
        }
        app.should_redraw = true;
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
                for block in &mut app.blocks { block.cached_lines = None; }
                app.needs_save = true;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let i = match app.theme_state.selected() {
                    Some(i) => if i == 0 { app.themes.len() - 1 } else { i - 1 }
                    None => 0,
                };
                app.theme_state.select(Some(i));
                app.theme = app.themes[i].clone();
                for block in &mut app.blocks { block.cached_lines = None; }
                app.needs_save = true;
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
        _ => {}
    }

    if app.is_output_focused {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => app.scroll_output_up(1),
            KeyCode::Down | KeyCode::Char('j') => app.scroll_output_down(1),
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => app.scroll_output_up(10),
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => app.scroll_output_down(10),
            KeyCode::PageUp => app.scroll_output_up(20),
            KeyCode::PageDown => app.scroll_output_down(20),
            KeyCode::Home => app.scroll_to_top(),
            KeyCode::End => app.scroll_to_bottom(),
            KeyCode::Esc => app.is_output_focused = false,
            _ => {}
        }
        app.should_redraw = true;
        return AppEventOutcome::Continue;
    }

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
            app.blocks.clear();
            app.blocks.push(RenderBlock {
                block_type: BlockType::Text,
                content: "UI Cleared. (Context preserved)".to_string(),
                title: None,
                success: Some(true),
                cached_lines: None,
            });
            app.output_state.select(Some(0));
            app.should_redraw = true;
            app.needs_save = true;
            return AppEventOutcome::Continue;
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
                let column = app.input[current_line_start + 1..app.cursor_pos].chars().count();
                
                let prev_line_start = if current_line_start == 0 {
                    0
                } else {
                    app.input[..current_line_start].rfind('\n').map(|idx| idx + 1).unwrap_or(0)
                };
                
                let prev_line = &app.input[prev_line_start..current_line_start];
                let prev_line_chars: Vec<char> = prev_line.chars().collect();
                let target_column = column.min(prev_line_chars.len());
                
                let mut new_pos = prev_line_start;
                for i in 0..target_column {
                    new_pos += prev_line_chars[i].len_utf8();
                }
                app.cursor_pos = new_pos;
                app.should_redraw = true;
            }
        }
        KeyCode::Down => {
            if let Some(next_newline) = app.input[app.cursor_pos..].find('\n') {
                let current_line_start = app.input[..app.cursor_pos].rfind('\n').map(|idx| idx + 1).unwrap_or(0);
                let column = app.input[current_line_start..app.cursor_pos].chars().count();
                
                let next_line_start = app.cursor_pos + next_newline + 1;
                let next_line_rest = &app.input[next_line_start..];
                let next_line_end = next_line_rest.find('\n').map(|idx| next_line_start + idx).unwrap_or(app.input.len());
                
                let next_line = &app.input[next_line_start..next_line_end];
                let next_line_chars: Vec<char> = next_line.chars().collect();
                let target_column = column.min(next_line_chars.len());
                
                let mut new_pos = next_line_start;
                for i in 0..target_column {
                    new_pos += next_line_chars[i].len_utf8();
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

pub fn handle_tool_call(app: &mut App, calls: Vec<ToolCall>, pos: usize, _tx: mpsc::UnboundedSender<StreamEvent>, cancellation_token: &mut CancellationToken, full_response_content: &str, _is_native: bool) {
    if !app.tool_calls_processed_this_request {
        app.tool_calls_processed_this_request = true;
        cancellation_token.cancel();
        *cancellation_token = CancellationToken::new();
        
        app.tool_call_pos = Some(pos);

        let tool_call = calls[0].clone();
        app.pending_tool_call = Some(tool_call.clone());
        app.context_manager.add_assistant_tool_call(full_response_content, vec![tool_call.clone()]);

        let description = tool_call.function.arguments["description"].as_str().unwrap_or("Action").to_string();
        app.log_debug(&format!("[TOOL CALL] {}: {}", tool_call.function.name, description));

        // Reconstruct a clean version of the call for the UI
        let clean_args = serde_json::to_string(&tool_call.function.arguments).unwrap_or_default();
        let clean_call = format!("call:{}{}", tool_call.function.name, clean_args);

        app.add_segment_with_title(clean_call, BlockType::ToolCall, description);

        if tool_call.function.name == "ask_the_user" {
            app.is_asking_user = true;
            app.is_processing = false;
        } else if app.shell_approval_mode == ApprovalMode::Always {
        } else {
            app.show_approval_prompt = true;
            app.is_processing = false;
        }
    }
}
