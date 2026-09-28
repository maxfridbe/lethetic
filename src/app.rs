mod accounting;
mod commands;
mod compaction;
mod input;
mod lifecycle;
mod model_catalog;
mod navigation;
mod python_session;
mod remote_control;
mod session_state;
mod setup_input;
mod tool_calls;
mod transcript;

pub use commands::{AppEventOutcome, dispatch_command};
pub use compaction::CompactionPopupState;
pub use input::handle_key;
pub use lifecycle::SessionSummary;
pub use model_catalog::ModelCatalogState;
pub use remote_control::{RcInfoState, RcResumeOffer, RcSetupStage, RcSetupState};
pub use session_state::{
    SessionDirectoryBinding, SessionSettings, SessionState, SessionWorkspaceBinding,
    describe_python_policy, normalize_session_display_name,
};
pub use tool_calls::{
    ApprovalMode, handle_tool_call, handle_tool_call_with_provider, start_next_queued_tool_call,
};
pub use transcript::{BlockType, RenderBlock};
pub(crate) use transcript::{
    LegacyTextErrorKind, legacy_text_error_marker, migrate_legacy_error_blocks,
    reconcile_interrupted_tool_error_blocks,
};

pub(crate) use accounting::apply_accounting_totals_to_user_block;
pub(crate) use session_state::{append_session_file, write_session_file};

use crate::commands::CommandId;
use crate::config::Config;
use crate::context::{ContextManager, ToolCall};
use crate::loop_detector::{LoopDetector, LoopDetectorConfig};
use crate::parser::StreamParser;
use crate::python_policy::{PythonPolicySource, PythonPolicyState};
use crate::python_setup::PythonSetupDialog;
use crate::theme::Theme;
use lifecycle::{SessionCleanupTarget, new_session_id};
use ratatui::layout::Rect;
use ratatui::widgets::ListState;
use std::env;

pub struct App {
    pub input: String,
    pub cursor_pos: usize,
    pub blocks: Vec<RenderBlock>,
    pub output_state: ListState,
    pub is_output_focused: bool,
    pub show_palette: bool,
    pub palette_state: ListState,
    pub palette_items: Vec<CommandId>,
    /// Type-to-filter text in the command palette.
    pub palette_query: String,
    /// First Esc while work runs arms a stop; a second one soon after stops.
    pub stop_esc_armed_at: Option<std::time::Instant>,
    pub theme: Theme,
    pub themes: Vec<Theme>,
    pub show_theme_menu: bool,
    pub theme_state: ListState,
    pub is_processing: bool,
    pub context_manager: ContextManager,
    pub tokens_per_s: f64,
    pub pp_tokens_per_s: f64,
    pub server_prompt_tokens: Option<u32>,
    pub server_completion_tokens: Option<u32>,
    pub server_usage: Option<crate::accounting::Usage>,
    pub logical_turn_usage: Option<crate::accounting::Usage>,
    pub active_logical_turn_id: Option<String>,
    pub active_request_id: Option<String>,
    active_cancellation_id: Option<String>,
    provider_request_turns: std::collections::HashMap<String, String>,
    partial_assistant_checkpoint: Option<crate::context::Message>,
    pub accounting: crate::accounting::SessionAccounting,
    pub model_name: String,
    pub server_url: String,
    pub max_tokens: usize,
    pub pending_tool_call: Option<ToolCall>,
    /// The rest of a multi-call batch, run one at a time after the pending call.
    pub queued_tool_calls: std::collections::VecDeque<ToolCall>,
    /// Harness notes raised mid-batch, sent after the batch's last result so
    /// tool results stay contiguous.
    pub deferred_batch_notes: Vec<String>,
    /// When a failed model request is due to be sent again.
    pub provider_retry_at: Option<std::time::Instant>,
    /// Consecutive failed attempts of the current model request.
    pub provider_retry_attempts: u32,
    pub shell_approval_mode: ApprovalMode,
    pub show_approval_prompt: bool,
    pub python_approval_show_original: bool,
    pub python_approval_scroll: u16,
    pub spinner_index: usize,
    pub tool_spinner_index: usize,
    pub is_executing_tool: bool,
    pub tool_output_preview: String,
    pub show_debug: bool,
    pub debug_log: Vec<String>,
    pub should_redraw: bool,
    pub tool_calls_processed_this_request: bool,
    pub tool_call_dispatched: bool,
    pub cwd: String,
    pub git_status: String,
    pub scroll: u16,
    pub auto_scroll: bool,
    pub memory_usage: u64,
    /// Size of the working directory's `.lethetic` state, refreshed every ~20 s.
    pub lethetic_dir_bytes: Option<u64>,
    pub system_prompt: String,
    pub show_prompt_editor: bool,
    pub is_editing_prompt: bool,
    pub show_prompt_save_dialog: bool,
    pub prompt_save_name: String,
    pub show_session_name_dialog: bool,
    pub session_name_input: String,
    pub session_name_error: Option<String>,
    pub system_prompt_manager: crate::system_prompt::SystemPromptManager,
    pub show_prompt_manager: bool,
    pub prompt_files: Vec<String>,
    pub prompt_list_state: ListState,
    pub show_cleanup_prompt: bool,
    pub show_hotkeys: bool,
    pub tool_call_pos: Option<usize>,
    pub last_rendered_width: usize,
    pub last_rendered_cost_visibility: Option<bool>,
    pub total_line_count: usize,
    pub current_dir: String,
    pub current_session_dir: Option<String>,
    pub session_id: String,
    pub display_name: Option<String>,
    pub session_directory_binding: Option<SessionDirectoryBinding>,
    pub python_runtime_id: Option<String>,
    pub managed_python_workspace: Option<SessionWorkspaceBinding>,
    pub shared_python_workspace: Option<SessionWorkspaceBinding>,
    #[cfg(target_os = "linux")]
    session_store: Option<crate::session_store::SessionStore>,
    #[cfg(target_os = "linux")]
    session_lease: Option<std::sync::Arc<crate::session_store::SessionLease>>,
    #[cfg(target_os = "linux")]
    session_creation_committed: bool,
    pub session_summaries: Vec<SessionSummary>,
    session_cleanup_targets: Vec<SessionCleanupTarget>,
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
    pub stop_reason: String,
    pub history: Vec<String>,
    pub history_state: ListState,
    pub backbuffer: String,
    pub show_history: bool,
    pub show_latest_files: bool,
    pub latest_files_state: ListState,
    pub config: Config,
    pub tool_runtime: crate::tool_runtime::ToolRuntime,
    pub python_setup: Option<PythonSetupDialog>,
    pub python_policy: PythonPolicyState,
    pub approval_policy_fingerprint: Option<String>,
    pub tool_call_fingerprints: std::collections::HashMap<String, usize>,
    pub applied_edits: std::collections::HashSet<String>,
    pub show_model_switcher: bool,
    pub model_switcher_state: ListState,
    pub available_models: Vec<crate::client::ModelChoice>,
    pub show_lsp_manager: bool,
    pub lsp_server_list_state: ListState,
    pub lsp_install_cmd: Option<String>,
    pub lsp_install_in_progress: bool,
    pub lsp_install_cancel_pending: bool,
    /// When Some, the model picker is choosing a compaction model for this session ID.
    pub compact_model_picker_src: Option<String>,
    pub compaction_popup: Option<CompactionPopupState>,
    /// "Scan for more" catalog opened from the model picker.
    pub model_catalog: Option<ModelCatalogState>,
    /// Palette remote-control setup dialog and the post-start URL popup.
    pub rc_setup: Option<RcSetupState>,
    /// Offered after resuming a session whose remote control is not running.
    pub rc_resume_offer: Option<RcResumeOffer>,
    pub rc_info: Option<RcInfoState>,
    /// Controller target while a listener runs (either origin).
    pub remote_control_target: Option<String>,
    /// Remote control was forced by a launch flag and cannot be changed here.
    pub remote_control_locked: bool,
    /// Remote-control details for the status line under the input.
    pub remote_control_open: bool,
    pub remote_control_files: bool,
    pub remote_control_clients: usize,
    /// Most recent browser peer, if any.
    pub remote_control_last_peer: Option<String>,
    /// Right-hand todo pane (F9) and its cached contents.
    pub show_todos: bool,
    /// Right-hand background task pane (F8).
    pub show_background_tasks: bool,
    /// Settings of a session resumed by the library path (`--session-id`),
    /// waiting for the run loop to apply them.
    pub pending_session_settings: Option<SessionSettings>,
    /// Tool calls made in this session, by tool name.
    pub tool_use_counts: std::collections::BTreeMap<String, u64>,
    /// When the newest transcript block started (Thought timing).
    pub(crate) block_started_at: Option<std::time::Instant>,
    /// When the pending tool call was dispatched (tool result timing).
    pub(crate) tool_call_started_at: Option<std::time::Instant>,
    /// When the streamed reply checkpoint was last written to disk.
    last_partial_checkpoint_save: Option<std::time::Instant>,
    pub todos: crate::todo_store::TodoSnapshot,
    pub hide_thinking: bool,
    /// Layout of the output panel from the last draw — used for mouse hit-testing.
    pub last_output_rect: Rect,
    /// Per-block line counts from the last draw (indices match `blocks`).
    pub last_block_line_counts: Vec<usize>,
    /// First visible absolute line index from the last draw.
    pub last_start_line: usize,
}

impl App {
    pub fn new(config: &Config) -> App {
        Self::new_with_policy_source(config, PythonPolicySource::Config)
    }

    pub fn new_with_policy_source(config: &Config, policy_source: PythonPolicySource) -> App {
        Self::new_with_python_policy_state(
            config,
            PythonPolicyState::from_config(config, policy_source),
        )
    }

    pub fn new_with_python_policy_state(config: &Config, python_policy: PythonPolicyState) -> App {
        let mut palette_state = ListState::default();
        palette_state.select(Some(0));

        let mut session_list_state = ListState::default();
        session_list_state.select(Some(0));
        let mut latest_files_state = ListState::default();
        latest_files_state.select(Some(0));
        let history_state = ListState::default();

        let system_prompt_manager = crate::system_prompt::SystemPromptManager::new();
        let system_prompt = system_prompt_manager
            .load_prompt("software_engineer")
            .unwrap_or_else(|| crate::system_prompt::DEFAULT_PROMPT_TEMPLATE.to_string());

        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| ".".to_string());
        #[cfg(target_os = "linux")]
        let (session_store, session_store_error) =
            match crate::session_store::SessionStore::open(std::path::Path::new(&cwd)) {
                Ok(store) => (Some(store), None),
                Err(error) => (None, Some(error)),
            };
        let resolved_prompt =
            crate::system_prompt::SystemPromptManager::resolve_prompt(&system_prompt, &cwd, config);
        let input_token_budget = config.input_token_budget();
        let mut context_manager = ContextManager::new(input_token_budget, Some(resolved_prompt));
        if let Some(mode) = config.context_mode {
            context_manager.mode = mode;
        }

        let themes = Theme::all();
        let theme_idx = config
            .theme
            .as_ref()
            .and_then(|n| themes.iter().position(|t| t.name.eq_ignore_ascii_case(n)))
            .unwrap_or(0);

        let mut app = App {
            input: String::new(),
            cursor_pos: 0,
            blocks: vec![RenderBlock {
                duration_ms: None,
                block_type: BlockType::Text,
                content: "Type a prompt to begin. Ctrl+P (or Esc) opens the command palette; F12 shows the debugger."
                    .to_string(),
                title: None,
                success: Some(true),
                prompt_tokens: None,
                completion_tokens: None,
                usage: None,
                estimated_cost: None,
                logical_turn_id: None,
                cached_lines: None,
                cached_line_count: None,
            }],
            output_state: ListState::default(),
            is_output_focused: false,
            show_palette: false,
            palette_state,
            palette_items: CommandId::ALL.to_vec(),
            palette_query: String::new(),
            stop_esc_armed_at: None,
            theme: {
                config
                    .theme
                    .as_ref()
                    .and_then(|name| {
                        themes
                            .iter()
                            .find(|t| t.name.eq_ignore_ascii_case(name))
                            .cloned()
                    })
                    .unwrap_or_default()
            },
            themes,
            show_theme_menu: false,
            theme_state: {
                let mut s = ListState::default();
                s.select(Some(theme_idx));
                s
            },
            is_processing: false,
            context_manager,
            tokens_per_s: 0.0,
            pp_tokens_per_s: 0.0,
            server_prompt_tokens: None,
            server_completion_tokens: None,
            server_usage: None,
            logical_turn_usage: None,
            active_logical_turn_id: None,
            active_request_id: None,
            active_cancellation_id: None,
            provider_request_turns: std::collections::HashMap::new(),
            partial_assistant_checkpoint: None,
            accounting: crate::accounting::SessionAccounting::default(),
            model_name: config.model.clone(),
            server_url: config.server_url.clone(),
            max_tokens: input_token_budget,
            pending_tool_call: None,
            queued_tool_calls: std::collections::VecDeque::new(),
            deferred_batch_notes: Vec::new(),
            provider_retry_at: None,
            provider_retry_attempts: 0,
            shell_approval_mode: ApprovalMode::None,
            show_approval_prompt: false,
            python_approval_show_original: false,
            python_approval_scroll: 0,
            spinner_index: 0,
            tool_spinner_index: 0,
            is_executing_tool: false,
            tool_output_preview: String::new(),
            show_debug: false,
            debug_log: Vec::new(),
            should_redraw: true,
            tool_calls_processed_this_request: false,
            tool_call_dispatched: false,
            cwd: String::from("N/A"),
            git_status: String::from("N/A"),
            scroll: 0,
            auto_scroll: true,
            memory_usage: 0,
            lethetic_dir_bytes: None,
            system_prompt: system_prompt.clone(),
            show_prompt_editor: false,
            is_editing_prompt: false,
            show_prompt_save_dialog: false,
            prompt_save_name: String::new(),
            show_session_name_dialog: false,
            session_name_input: String::new(),
            session_name_error: None,
            system_prompt_manager,
            show_prompt_manager: false,
            prompt_files: Vec::new(),
            prompt_list_state: ListState::default(),
            show_cleanup_prompt: false,
            show_hotkeys: false,
            tool_call_pos: None,
            last_rendered_width: 0,
            last_rendered_cost_visibility: None,
            total_line_count: 0,
            current_dir: env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| String::from(".")),
            current_session_dir: None,
            session_id: new_session_id(),
            display_name: None,
            session_directory_binding: None,
            python_runtime_id: None,
            managed_python_workspace: None,
            shared_python_workspace: None,
            #[cfg(target_os = "linux")]
            session_store,
            #[cfg(target_os = "linux")]
            session_lease: None,
            #[cfg(target_os = "linux")]
            session_creation_committed: false,
            session_summaries: Vec::new(),
            session_cleanup_targets: Vec::new(),
            session_list_state: ListState::default(),
            show_session_manager: false,
            needs_save: false,
            request_start_time: None,
            is_asking_user: false,
            prompt_cursor_pos: system_prompt.len(),
            prompt_scroll: 0,
            parser: StreamParser::with_mode(crate::parser::ParserMode::from(
                config.active_parser(),
            )),
            loop_detector: LoopDetector::new(LoopDetectorConfig::default()),
            last_block_content: String::new(),
            loop_detection_count: 0,
            last_loop_detection_time: None,
            is_loading_session: false,
            load_progress: 0.0,
            load_status: String::new(),
            stop_reason: "Ready".to_string(),
            history: Self::load_global_history(),
            history_state,
            backbuffer: String::new(),
            show_history: false,
            show_latest_files: false,
            latest_files_state,
            config: config.clone(),
            tool_runtime: crate::tool_runtime::ToolRuntime::interactive(cwd.clone()),
            python_setup: None,
            python_policy,
            approval_policy_fingerprint: None,
            tool_call_fingerprints: std::collections::HashMap::new(),
            applied_edits: std::collections::HashSet::new(),
            show_model_switcher: false,
            model_switcher_state: ListState::default(),
            available_models: Vec::new(),
            show_lsp_manager: false,
            lsp_server_list_state: ListState::default(),
            lsp_install_cmd: None,
            lsp_install_in_progress: false,
            lsp_install_cancel_pending: false,
            compact_model_picker_src: None,
            compaction_popup: None,
            model_catalog: None,
            rc_setup: None,
            rc_resume_offer: None,
            rc_info: None,
            remote_control_target: None,
            remote_control_locked: false,
            remote_control_open: false,
            remote_control_files: false,
            remote_control_clients: 0,
            remote_control_last_peer: None,
            show_todos: false,
            show_background_tasks: false,
            pending_session_settings: None,
            last_partial_checkpoint_save: None,
            tool_use_counts: Default::default(),
            block_started_at: None,
            tool_call_started_at: None,
            todos: Default::default(),
            hide_thinking: false,
            last_output_rect: Rect::default(),
            last_block_line_counts: Vec::new(),
            last_start_line: 0,
        };
        #[cfg(target_os = "linux")]
        if let Some(error) = session_store_error {
            app.stop_reason = format!("Session storage unavailable: {error}");
        }
        app.refresh_todos();
        app.refresh_session_list();
        if !app.session_summaries.is_empty() {
            app.show_session_manager = true;
            app.session_list_state.select(Some(0));
        } else {
            app.start_new_session();
        }

        app
    }
}
