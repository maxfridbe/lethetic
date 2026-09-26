use super::*;
use tokio_util::sync::CancellationToken;

/// Live state of the session-compaction popup.
pub struct CompactionPopupState {
    pub content: String,
    /// `usize::MAX` means "follow the bottom"; the renderer clamps it.
    pub scroll: usize,
    pub done: bool,
    pub new_session_id: Option<String>,
    pub cancel: CancellationToken,
}

impl CompactionPopupState {
    pub fn new(model_id: &str, cancel: CancellationToken) -> Self {
        Self {
            content: format!("Model: {}\n", model_id),
            scroll: usize::MAX,
            done: false,
            new_session_id: None,
            cancel,
        }
    }

    fn line_count(&self) -> usize {
        self.content.lines().count()
    }

    pub fn scroll_up(&mut self, lines: usize) {
        if self.scroll == usize::MAX {
            self.scroll = self.line_count().saturating_sub(1);
        }
        self.scroll = self.scroll.saturating_sub(lines);
    }

    pub fn scroll_down(&mut self, lines: usize) {
        if self.scroll != usize::MAX {
            let max = self.line_count().saturating_sub(1);
            self.scroll = (self.scroll + lines).min(max);
        }
    }
}

const GLOBAL_HISTORY_PATH: &str = ".lethetic/history.json";
const GLOBAL_HISTORY_CAP: usize = 500;

impl App {
    /// Input history shared across sessions of this project.
    pub fn load_global_history() -> Vec<String> {
        std::fs::read_to_string(GLOBAL_HISTORY_PATH)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub(super) fn persist_global_history(history: &[String]) {
        if let Ok(json) = serde_json::to_string(history) {
            let _ = std::fs::create_dir_all(".lethetic");
            let _ = std::fs::write(GLOBAL_HISTORY_PATH, json);
        }
    }

    pub(super) fn global_history_cap() -> usize {
        GLOBAL_HISTORY_CAP
    }

    /// Replace the in-memory history with the global one, keeping session-only
    /// entries when no global history exists yet.
    pub fn adopt_global_history(&mut self, session_history: Vec<String>) {
        let global = Self::load_global_history();
        self.history = if global.is_empty() {
            session_history
        } else {
            global
        };
    }

    pub fn toggle_hide_thinking(&mut self) {
        self.hide_thinking = !self.hide_thinking;
        for block in &mut self.blocks {
            if block.block_type == BlockType::Thought || block.block_type == BlockType::Formulating
            {
                block.invalidate();
            }
        }
        self.stop_reason = if self.hide_thinking {
            "Thinking blocks hidden (Ctrl+O to show)".to_string()
        } else {
            "Thinking blocks shown (Ctrl+O to hide)".to_string()
        };
        self.needs_save = true;
        self.should_redraw = true;
    }

    /// Read the diagnostic log of a session for compaction. Falls back to the
    /// durable messages when the session predates `ui_log.txt`.
    pub fn compaction_source_text(session_dir: &str) -> Result<String, String> {
        let path = std::path::Path::new(session_dir).join("ui_log.txt");
        if let Ok(text) = std::fs::read_to_string(&path)
            && !text.trim().is_empty()
        {
            return Ok(text);
        }
        let state = SessionState::load_checked(session_dir)?;
        if state.messages.is_empty() {
            return Err("session has no log or messages to compact".to_string());
        }
        let mut out = String::new();
        for message in &state.messages {
            let header = match message.role.as_str() {
                "user" => "=== USER ===",
                "assistant" => "=== TEXT ===",
                "tool" => "=== TOOL RESULT ===",
                _ => "=== TEXT ===",
            };
            out.push_str(header);
            out.push('\n');
            out.push_str(&message.content);
            out.push_str("\n\n");
        }
        Ok(out)
    }

    #[cfg(not(target_os = "linux"))]
    pub fn create_compacted_session(
        &mut self,
        _source_session_id: &str,
        _summary: String,
    ) -> Result<String, String> {
        Err(
            "compacted session creation is disabled because secure session locking is available only on Linux"
                .to_string(),
        )
    }

    /// Write a new durable session whose context is only `summary`, inheriting
    /// the source session's model, prompt, theme, history, and accounting.
    /// The new session is registered but not activated; it appears in the
    /// session manager ready to resume. Returns the new session ID.
    #[cfg(target_os = "linux")]
    pub fn create_compacted_session(
        &mut self,
        source_session_id: &str,
        summary: String,
    ) -> Result<String, String> {
        let source_path = self.session_path_for_id(source_session_id)?;
        let source_label = std::path::Path::new(&source_path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(source_session_id)
            .to_string();
        let original = SessionState::load_checked(&source_path)?;

        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
        let session_id = lifecycle::new_session_id();
        let directory_name = format!("session_{}_{}_compacted", timestamp, &session_id[..8]);
        let store = self.session_store_ref()?;
        let (path, lease) = store.create_locked_session(&directory_name, &session_id)?;
        let path_string = path
            .to_str()
            .ok_or_else(|| "session directory must be UTF-8".to_string())?
            .to_string();

        let context_user_content = format!(
            "Context from compacted session ({}):\n\n{}",
            source_label, summary
        );
        let messages = vec![
            crate::context::Message {
                role: "user".to_string(),
                content: context_user_content,
                tool_calls: None,
                provider_content: None,
                tool_result_is_error: false,
            },
            crate::context::Message {
                role: "assistant".to_string(),
                content: "Understood. I have the context from the previous session and am ready to continue."
                    .to_string(),
                tool_calls: None,
                provider_content: None,
                tool_result_is_error: false,
            },
        ];
        let blocks = vec![RenderBlock::text(format!(
            "**Compacted from `{}`**\n\n{}",
            source_label, summary
        ))];
        let state = SessionState {
            session_id: Some(session_id.clone()),
            display_name: original.display_name.as_deref().and_then(|name| {
                normalize_session_display_name(&format!("{name} (compacted)"))
                    .ok()
                    .flatten()
            }),
            session_directory_binding: Some(lease.binding().clone()),
            messages,
            blocks,
            history: original.history,
            theme_name: original.theme_name,
            accounting: original.accounting,
            connection_id: original.connection_id,
            model_name: original.model_name,
            system_prompt: original.system_prompt,
            hide_thinking: original.hide_thinking,
            ..Default::default()
        };
        let written = state
            .save_to_directory_checked(&path_string)
            .and_then(|()| store.commit_locked_session_creation(&lease));
        if let Err(error) = written {
            let _ = store.remove_locked_session(&lease);
            return Err(error);
        }
        let _ = std::fs::copy(
            std::path::Path::new(&source_path).join("ui_log.txt"),
            path.join("ui_log_original.txt"),
        );
        drop(lease);
        self.refresh_session_list();
        Ok(session_id)
    }
}
