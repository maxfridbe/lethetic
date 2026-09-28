use super::*;
use ratatui::text::Line;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

static MARKER_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<\|?/?(?:channel|thought|tool_call|tool_response|turn|bos|eos|think|\||\x22|')[^>]*>?(?:thought|text|model|system)?").unwrap()
});

pub(super) const MAX_TOTAL_BLOCKS: usize = 200;
const LEGACY_EMPTY_SESSION_NOTICE: &str = "Loaded session has no conversation content";

fn is_legacy_empty_session_provider_error(block: &RenderBlock) -> bool {
    block.block_type == BlockType::ProviderError
        && (block.content == LEGACY_EMPTY_SESSION_NOTICE
            || block.content
                == format!(
                    "\n{} ERROR: {LEGACY_EMPTY_SESSION_NOTICE}\n",
                    crate::icons::WARNING
                ))
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum BlockType {
    Text,
    ProviderError,
    User,
    Thought,
    Markdown,
    ToolCall,
    ToolResult,
    ToolError,
    Divider,
    Formulating,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RenderBlock {
    pub block_type: BlockType,
    pub content: String,
    pub title: Option<String>,
    pub success: Option<bool>,
    #[serde(default)]
    pub prompt_tokens: Option<u32>,
    #[serde(default)]
    pub completion_tokens: Option<u32>,
    #[serde(default)]
    pub usage: Option<crate::accounting::Usage>,
    #[serde(default)]
    pub estimated_cost: Option<crate::accounting::EstimatedCost>,
    #[serde(default)]
    pub logical_turn_id: Option<String>,
    /// How long this step took: thinking time for Thought blocks, execution
    /// time for tool results. Shown as a line under the block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip)]
    pub cached_lines: Option<Vec<Line<'static>>>,
    /// Line count of the last render at the current width. Survives cache
    /// eviction so the virtualization counting pass doesn't re-render
    /// off-screen blocks just to count their lines.
    #[serde(skip)]
    pub cached_line_count: Option<usize>,
}

impl RenderBlock {
    /// Drop both the rendered lines and the line count (content, width, or
    /// theme changed — everything must be recomputed).
    pub fn invalidate(&mut self) {
        self.cached_lines = None;
        self.cached_line_count = None;
    }

    /// Build the same canonical call block used by the interactive transcript.
    pub fn tool_call(call: &crate::context::ToolCall) -> Self {
        let title = call.function.arguments["description"]
            .as_str()
            .unwrap_or("Action")
            .to_string();
        let arguments = serde_json::to_string(&call.function.arguments).unwrap_or_default();
        Self::new(
            BlockType::ToolCall,
            format!("call:{}{}", call.function.name, arguments),
            Some(title),
            true,
        )
    }

    /// Build a typed terminal block without inferring status from its text.
    pub fn tool_result(
        content: impl Into<String>,
        title: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self::new(
            if is_error {
                BlockType::ToolError
            } else {
                BlockType::ToolResult
            },
            content.into(),
            Some(title.into()),
            !is_error,
        )
    }

    pub fn text(content: impl Into<String>) -> Self {
        Self::new(BlockType::Text, content.into(), None, true)
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::new(BlockType::User, content.into(), None, true)
    }

    /// Strip provider/parser markers exactly as `App::add_segment` does for text.
    pub fn assistant_text_content(content: &str) -> String {
        MARKER_REGEX.replace_all(content, "").into_owned()
    }

    /// Append one block while retaining the shared transcript cap.
    /// Returns the number of blocks removed from the front.
    pub fn push_capped(blocks: &mut Vec<Self>, block: Self) -> usize {
        blocks.push(block);
        let removed = blocks.len().saturating_sub(MAX_TOTAL_BLOCKS);
        if removed != 0 {
            blocks.drain(..removed);
        }
        removed
    }

    fn new(block_type: BlockType, content: String, title: Option<String>, success: bool) -> Self {
        Self {
            duration_ms: None,
            block_type,
            content,
            title,
            success: Some(success),
            prompt_tokens: None,
            completion_tokens: None,
            usage: None,
            estimated_cost: None,
            logical_turn_id: None,
            cached_lines: None,
            cached_line_count: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LegacyTextErrorKind {
    Provider,
    Tool,
}

impl LegacyTextErrorKind {
    fn block_type(self) -> BlockType {
        match self {
            Self::Provider => BlockType::ProviderError,
            Self::Tool => BlockType::ToolError,
        }
    }
}

fn classify_legacy_text_error_marker(rest: &str) -> Option<LegacyTextErrorKind> {
    let rest = rest.trim_start_matches(['\u{fe0f}', ' ']);
    if rest.starts_with("ERROR:")
        || rest.starts_with("PROVIDER CHECKPOINT ERROR:")
        || rest.starts_with("PROVIDER TERMINAL CHECKPOINT ERROR FOR AN EARLIER REQUEST:")
        || rest.starts_with("PROVIDER CANCELLATION CHECKPOINT ERROR FOR AN EARLIER REQUEST:")
    {
        return Some(LegacyTextErrorKind::Provider);
    }
    if rest.starts_with("PYTHON AUDIT ERROR:")
        || rest.starts_with("NONLOCAL PYTHON PREFLIGHT ERROR:")
    {
        return Some(LegacyTextErrorKind::Tool);
    }
    if rest.starts_with("SESSION SAVE ERROR:") {
        let lower = rest.to_ascii_lowercase();
        if lower.contains("continue the provider tool call") {
            return Some(LegacyTextErrorKind::Tool);
        }
        return Some(
            if lower.contains("provider") || lower.contains("streaming response") {
                LegacyTextErrorKind::Provider
            } else {
                LegacyTextErrorKind::Tool
            },
        );
    }
    None
}

pub(crate) fn legacy_text_error_marker(content: &str) -> Option<(usize, LegacyTextErrorKind)> {
    let mut offset = 0usize;
    for line in content.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let leading_bytes = line.len().saturating_sub(trimmed.len());
        let marker = trimmed
            .strip_prefix(crate::icons::WARNING)
            .and_then(classify_legacy_text_error_marker)
            .or_else(|| {
                trimmed
                    .strip_prefix('⚠')
                    .and_then(classify_legacy_text_error_marker)
            });
        if let Some(kind) = marker {
            return Some((offset.saturating_add(leading_bytes), kind));
        }
        offset = offset.saturating_add(line.len());
    }
    None
}

pub(crate) fn migrate_legacy_error_blocks(blocks: &mut Vec<RenderBlock>) -> bool {
    let mut migrated = Vec::with_capacity(blocks.len());
    let mut changed = false;
    for mut block in std::mem::take(blocks) {
        if is_legacy_empty_session_provider_error(&block) {
            changed = true;
            continue;
        }
        if block.block_type == BlockType::Text
            && let Some((marker_offset, error_kind)) = legacy_text_error_marker(&block.content)
        {
            changed = true;
            if block.content[..marker_offset].trim().is_empty() {
                block.block_type = error_kind.block_type();
                block.success = Some(false);
                block.invalidate();
                migrated.push(block);
            } else {
                let error_content = block.content.split_off(marker_offset);
                block.invalidate();
                migrated.push(block);
                migrated.push(RenderBlock {
                    duration_ms: None,
                    block_type: error_kind.block_type(),
                    content: error_content,
                    title: None,
                    success: Some(false),
                    prompt_tokens: None,
                    completion_tokens: None,
                    usage: None,
                    estimated_cost: None,
                    logical_turn_id: None,
                    cached_lines: None,
                    cached_line_count: None,
                });
            }
            continue;
        }
        if block.block_type == BlockType::ToolResult {
            if block.success == Some(false) {
                changed = true;
                block.block_type = BlockType::ToolError;
                block.success = Some(false);
                block.invalidate();
                migrated.push(block);
                continue;
            }
            if let Some(marker_offset) =
                crate::tools::legacy_persisted_tool_error_marker_offset(&block.content)
            {
                changed = true;
                if block.content[..marker_offset].trim().is_empty() {
                    block.block_type = BlockType::ToolError;
                    block.success = Some(false);
                    block.invalidate();
                    migrated.push(block);
                } else {
                    let error_content = block.content.split_off(marker_offset);
                    let error_title = block.title.clone();
                    block.success = Some(true);
                    block.invalidate();
                    migrated.push(block);
                    migrated.push(RenderBlock {
                        duration_ms: None,
                        block_type: BlockType::ToolError,
                        content: error_content,
                        title: error_title,
                        success: Some(false),
                        prompt_tokens: None,
                        completion_tokens: None,
                        usage: None,
                        estimated_cost: None,
                        logical_turn_id: None,
                        cached_lines: None,
                        cached_line_count: None,
                    });
                }
                continue;
            }
        }
        migrated.push(block);
    }
    if migrated.len() > MAX_TOTAL_BLOCKS {
        migrated.drain(..migrated.len() - MAX_TOTAL_BLOCKS);
        changed = true;
    }
    *blocks = migrated;
    changed
}

fn terminal_block_after_call(
    blocks: &[RenderBlock],
    call_index: usize,
    expected_title: &str,
) -> Option<bool> {
    for block in &blocks[call_index.saturating_add(1)..] {
        if block.block_type == BlockType::ToolCall {
            break;
        }
        if matches!(
            block.block_type,
            BlockType::ToolResult | BlockType::ToolError
        ) && (block.title.as_deref() == Some(expected_title) || block.title.is_none())
        {
            return Some(
                block.block_type == BlockType::ToolError
                    && block.content.trim() == crate::context::INTERRUPTED_TOOL_RESULT,
            );
        }
    }
    None
}

pub(crate) fn reconcile_interrupted_tool_error_blocks(
    blocks: &mut Vec<RenderBlock>,
    repairs: &[crate::context::InterruptedToolCallRepair],
) -> bool {
    if repairs.is_empty() {
        return false;
    }

    let mut existing_interruption_blocks = blocks
        .iter()
        .filter(|block| {
            block.block_type == BlockType::ToolError
                && block.content.trim() == crate::context::INTERRUPTED_TOOL_RESULT
        })
        .count();
    let mut search_from = 0usize;
    let mut additions = Vec::new();

    for repair in repairs {
        let arguments = serde_json::to_string(&repair.arguments).unwrap_or_default();
        let canonical_call = format!("call:{}{}", repair.function_name, arguments);
        let description = repair.arguments["description"]
            .as_str()
            .unwrap_or("Action")
            .to_string();
        let call_index = blocks[search_from..]
            .iter()
            .position(|block| {
                block.block_type == BlockType::ToolCall && block.content.contains(&canonical_call)
            })
            .map(|relative| search_from.saturating_add(relative));

        if let Some(call_index) = call_index {
            search_from = call_index.saturating_add(1);
            if let Some(is_existing_interruption) =
                terminal_block_after_call(blocks, call_index, &description)
            {
                if is_existing_interruption {
                    existing_interruption_blocks = existing_interruption_blocks.saturating_sub(1);
                }
                continue;
            }
        }
        if existing_interruption_blocks > 0 {
            existing_interruption_blocks -= 1;
            continue;
        }

        additions.push(RenderBlock {
            duration_ms: None,
            block_type: BlockType::ToolError,
            content: crate::context::INTERRUPTED_TOOL_RESULT.to_string(),
            title: Some(description),
            success: Some(false),
            prompt_tokens: None,
            completion_tokens: None,
            usage: None,
            estimated_cost: None,
            logical_turn_id: None,
            cached_lines: None,
            cached_line_count: None,
        });
    }

    if additions.is_empty() {
        return false;
    }
    blocks.extend(additions);
    if blocks.len() > MAX_TOTAL_BLOCKS {
        blocks.drain(..blocks.len() - MAX_TOTAL_BLOCKS);
    }
    true
}

impl App {
    pub fn add_segment(&mut self, content: String, b_type: BlockType) {
        if b_type == BlockType::User {
            self.add_segment_internal(content, b_type);
            return;
        }
        // Split provider text into display blocks while removing parser markers.
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

        if parts.is_empty() {
            return;
        }

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

    fn add_segment_internal_with_title(
        &mut self,
        cleaned_content: String,
        b_type: BlockType,
        title: Option<String>,
    ) {
        if cleaned_content.is_empty() && b_type != BlockType::Divider {
            return;
        }

        // Only update last_block_content for model text/thought — not tool results/calls,
        // so the loop detector doesn't fire on repeated phrases in compiler output or stack traces.
        let feeds_loop_detector = matches!(b_type, BlockType::Text | BlockType::Thought);

        if let Some(last) = self.blocks.last_mut() {
            if last.block_type == BlockType::Formulating && b_type == BlockType::ToolCall {
                last.block_type = BlockType::ToolCall;
                last.content = cleaned_content.clone();
                last.title = title;
                last.invalidate();
                if feeds_loop_detector {
                    self.last_block_content = cleaned_content;
                }
                self.should_redraw = true;
                self.needs_save = true;
                return;
            }

            if last.block_type == b_type
                && !matches!(b_type, BlockType::Divider | BlockType::User)
                && last.title == title
            {
                last.content.push_str(&cleaned_content);
                if feeds_loop_detector {
                    self.last_block_content.push_str(&cleaned_content);
                }
                last.invalidate();
                self.should_redraw = true;
                if self.auto_scroll {
                    self.sync_scroll_to_end();
                }
                self.needs_save = true;
                return;
            }
        }

        if feeds_loop_detector {
            self.last_block_content = cleaned_content.clone();
        }
        self.add_block(cleaned_content, b_type, title);
    }

    /// Close the timing of a Thought block that is still open (the model
    /// moved on or the reply ended).
    pub fn finish_thought_timing(&mut self) {
        let Some(started) = self.block_started_at else {
            return;
        };
        if let Some(last) = self.blocks.last_mut()
            && last.block_type == BlockType::Thought
            && last.duration_ms.is_none()
        {
            last.duration_ms = Some(started.elapsed().as_millis() as u64);
            last.invalidate();
            self.should_redraw = true;
        }
    }

    pub(super) fn add_block(&mut self, content: String, b_type: BlockType, title: Option<String>) {
        self.finish_thought_timing();
        self.block_started_at = Some(std::time::Instant::now());
        let tool_duration_ms = matches!(b_type, BlockType::ToolResult | BlockType::ToolError)
            .then(|| self.tool_call_started_at.take())
            .flatten()
            .map(|started| started.elapsed().as_millis() as u64);
        if b_type == BlockType::User && !self.blocks.is_empty() {
            self.blocks.push(RenderBlock {
                duration_ms: None,
                block_type: BlockType::Divider,
                content: String::new(),
                title: None,
                success: None,
                prompt_tokens: None,
                completion_tokens: None,
                usage: None,
                estimated_cost: None,
                logical_turn_id: None,
                cached_lines: None,
                cached_line_count: None,
            });
        }

        let success = match b_type {
            BlockType::ToolResult => Some(!crate::tools::legacy_output_is_error(&content)),
            BlockType::ProviderError | BlockType::ToolError => Some(false),
            _ => Some(true),
        };

        self.blocks.push(RenderBlock {
            duration_ms: tool_duration_ms,
            block_type: b_type.clone(),
            content: content.clone(),
            title: title.clone(),
            success,
            prompt_tokens: None,
            completion_tokens: None,
            usage: None,
            estimated_cost: None,
            logical_turn_id: None,
            cached_lines: None,
            cached_line_count: None,
        });

        // Append verbatim block to ui_log.txt for post-run diagnosis
        if let Some(ref session_dir) = self.current_session_dir {
            let header = match &b_type {
                BlockType::User => "=== USER ===".to_string(),
                BlockType::Thought => "=== THOUGHT ===".to_string(),
                BlockType::Text | BlockType::Markdown => "=== TEXT ===".to_string(),
                BlockType::ProviderError => "=== PROVIDER ERROR ===".to_string(),
                BlockType::ToolCall => {
                    format!("=== TOOL CALL: {} ===", title.as_deref().unwrap_or(""))
                }
                BlockType::ToolResult | BlockType::ToolError => "=== TOOL RESULT ===".to_string(),
                BlockType::Formulating => "=== FORMULATING ===".to_string(),
                BlockType::Divider => String::new(),
            };
            if !header.is_empty() {
                let entry = format!("{}\n{}\n\n", header, content);
                if let Err(error) = append_session_file(session_dir, "ui_log.txt", entry.as_bytes())
                {
                    self.stop_reason = format!("Session UI log append failed: {error}");
                }
            }
        }

        while self.blocks.len() > MAX_TOTAL_BLOCKS {
            self.blocks.remove(0);
        }

        if self.auto_scroll {
            self.sync_scroll_to_end();
        }
        self.should_redraw = true;
        self.needs_save = true;
    }
}

fn cached_transcript_line_count(blocks: &[RenderBlock]) -> Option<usize> {
    blocks.iter().try_fold(0usize, |total, block| {
        let line_count = block
            .cached_lines
            .as_ref()
            .map(Vec::len)
            .or(block.cached_line_count)?;
        total.checked_add(line_count)
    })
}

impl App {
    pub(super) fn reset_transcript_view_to_tail(&mut self) {
        self.scroll = 0;
        self.auto_scroll = true;
        match cached_transcript_line_count(&self.blocks) {
            Some(total_line_count) => {
                self.total_line_count = total_line_count;
                self.output_state.select(total_line_count.checked_sub(1));
            }
            None => {
                self.total_line_count = 0;
                self.output_state.select(None);
            }
        }
    }

    pub fn clear_output(&mut self) {
        self.blocks.clear();
        self.reset_transcript_view_to_tail();
        self.should_redraw = true;
        self.needs_save = true;
    }
    pub fn log_runtime_debug(&mut self, msg: &str) {
        self.push_debug_entry(msg);
    }

    pub fn log_debug(&mut self, msg: &str) {
        let log_entry = self.push_debug_entry(msg);

        if let Some(session_dir) = &self.current_session_dir {
            let entry = format!("{log_entry}\n");
            if let Err(error) = append_session_file(session_dir, "logs.txt", entry.as_bytes()) {
                self.stop_reason = format!("Session debug log append failed: {error}");
            }
        }
    }

    fn push_debug_entry(&mut self, msg: &str) -> String {
        let now = chrono::Local::now();
        let timestamp = now.format("%H:%M:%S%.3f");
        let log_entry = format!("[{}] {}", timestamp, msg);

        self.debug_log.push(log_entry.clone());
        if self.debug_log.len() > 200 {
            self.debug_log.remove(0);
        }
        self.should_redraw = true;
        log_entry
    }
    pub fn clear_ui_preserving_context(&mut self) {
        self.blocks.clear();
        self.blocks.push(RenderBlock {
            duration_ms: None,
            block_type: BlockType::Text,
            content: "UI Cleared. (Context preserved)".to_string(),
            title: None,
            success: Some(true),
            prompt_tokens: None,
            completion_tokens: None,
            usage: None,
            estimated_cost: None,
            logical_turn_id: None,
            cached_lines: None,
            cached_line_count: None,
        });
        self.reset_transcript_view_to_tail();
        self.should_redraw = true;
        self.needs_save = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn legacy_tool_block(content: &str) -> RenderBlock {
        RenderBlock {
            duration_ms: None,
            block_type: BlockType::ToolResult,
            content: content.to_string(),
            title: Some("Action".to_string()),
            success: Some(true),
            prompt_tokens: None,
            completion_tokens: None,
            usage: None,
            estimated_cost: None,
            logical_turn_id: None,
            cached_lines: Some(Vec::new()),
            cached_line_count: Some(0),
        }
    }

    #[test]
    fn legacy_text_errors_migrate_with_source_provenance() {
        for (marker, expected_type) in [
            ("ERROR: provider failed", BlockType::ProviderError),
            (
                "SESSION SAVE ERROR: streaming response was contained: private",
                BlockType::ProviderError,
            ),
            (
                "SESSION SAVE ERROR: refusing to continue the provider tool call: private",
                BlockType::ToolError,
            ),
            ("PYTHON AUDIT ERROR: private", BlockType::ToolError),
            (
                "NONLOCAL PYTHON PREFLIGHT ERROR: private",
                BlockType::ToolError,
            ),
        ] {
            let content = format!("{} {marker}", crate::icons::WARNING);
            let mut block = legacy_tool_block(&content);
            block.block_type = BlockType::Text;
            block.title = None;
            let mut blocks = vec![block];

            assert!(migrate_legacy_error_blocks(&mut blocks), "{marker}");
            assert_eq!(blocks.len(), 1);
            assert_eq!(blocks[0].block_type, expected_type, "{marker}");
            assert_eq!(blocks[0].success, Some(false));
            assert_eq!(blocks[0].content, content);
        }
    }

    #[test]
    fn legacy_merged_tool_failure_splits_without_losing_local_content() {
        let mut blocks = vec![legacy_tool_block(
            "ok\nERROR: quota denied tenant violet request-local-42",
        )];

        assert!(migrate_legacy_error_blocks(&mut blocks));

        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].block_type, BlockType::ToolResult);
        assert_eq!(blocks[0].content, "ok\n");
        assert_eq!(blocks[0].success, Some(true));
        assert_eq!(blocks[1].block_type, BlockType::ToolError);
        assert_eq!(
            blocks[1].content,
            "ERROR: quota denied tenant violet request-local-42"
        );
        assert_eq!(blocks[1].success, Some(false));
        assert_eq!(blocks[1].title.as_deref(), Some("Action"));
        assert!(blocks.iter().all(|block| block.cached_lines.is_none()));
    }

    #[test]
    fn legacy_shell_success_remains_success_despite_error_like_stdout() {
        let content = "EXIT_CODE: 0\nSTDOUT:\nprogram printed ERROR: ordinary text\nSTDERR:\n";
        let mut blocks = vec![legacy_tool_block(content)];

        assert!(!migrate_legacy_error_blocks(&mut blocks));

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].block_type, BlockType::ToolResult);
        assert_eq!(blocks[0].content, content);
        assert_eq!(blocks[0].success, Some(true));
    }

    #[test]
    fn legacy_shell_success_followed_by_failure_splits_at_second_exit_status() {
        let content = "EXIT_CODE: 0\nSTDOUT:\nok\nSTDERR:\n\nEXIT_CODE: 1\nSTDOUT:\n\nSTDERR:\nquota denied tenant violet";
        let mut blocks = vec![legacy_tool_block(content)];

        assert!(migrate_legacy_error_blocks(&mut blocks));

        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].block_type, BlockType::ToolResult);
        assert_eq!(blocks[0].success, Some(true));
        assert_eq!(blocks[1].block_type, BlockType::ToolError);
        assert_eq!(blocks[1].success, Some(false));
        assert!(blocks[1].content.starts_with("EXIT_CODE: 1"));
        assert_eq!(
            blocks
                .iter()
                .map(|block| block.content.as_str())
                .collect::<String>(),
            content
        );
    }

    #[test]
    fn legacy_known_failure_shapes_become_typed_errors() {
        for content in [
            "Syntax Error in tool call: malformed JSON",
            "LSP error: connection closed tenant violet",
            "[find_symbol fallback]\nERROR: search failed tenant violet",
            "--- Page 1 Error: PDF is encrypted and requires a password ---",
            "file.rs:1:match\nSTDERR: rg: private: Permission denied",
        ] {
            let mut blocks = vec![legacy_tool_block(content)];
            assert!(migrate_legacy_error_blocks(&mut blocks), "{content}");
            assert_eq!(blocks.last().unwrap().block_type, BlockType::ToolError);
            assert_eq!(blocks.last().unwrap().success, Some(false));
            let reconstructed = blocks
                .iter()
                .map(|block| block.content.as_str())
                .collect::<String>();
            assert_eq!(reconstructed, content);
        }
    }

    #[test]
    fn legacy_empty_session_provider_error_is_removed_exactly() {
        let obsolete = format!(
            "\n{} ERROR: {LEGACY_EMPTY_SESSION_NOTICE}\n",
            crate::icons::WARNING
        );
        let mut obsolete_block = legacy_tool_block(&obsolete);
        obsolete_block.block_type = BlockType::ProviderError;
        let mut real_block = obsolete_block.clone();
        real_block.content = format!("{obsolete} from provider");
        let mut blocks = vec![obsolete_block, real_block.clone()];

        assert!(migrate_legacy_error_blocks(&mut blocks));
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].content, real_block.content);
        assert_eq!(blocks[0].block_type, BlockType::ProviderError);
    }

    #[test]
    fn interrupted_context_repairs_gain_one_idempotent_tool_error_block() {
        let repair = crate::context::InterruptedToolCallRepair {
            function_name: "calculate".to_string(),
            arguments: serde_json::json!({
                "expression": "2 + 2",
                "description": "Calculate"
            }),
        };
        let mut call = legacy_tool_block(
            "call:calculate{\"description\":\"Calculate\",\"expression\":\"2 + 2\"}",
        );
        call.block_type = BlockType::ToolCall;
        call.title = Some("Calculate".to_string());
        let mut blocks = vec![call];

        assert!(reconcile_interrupted_tool_error_blocks(
            &mut blocks,
            std::slice::from_ref(&repair)
        ));
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[1].block_type, BlockType::ToolError);
        assert_eq!(blocks[1].success, Some(false));
        assert_eq!(blocks[1].content, crate::context::INTERRUPTED_TOOL_RESULT);
        assert!(!reconcile_interrupted_tool_error_blocks(
            &mut blocks,
            std::slice::from_ref(&repair)
        ));
        assert_eq!(blocks.len(), 2);
    }

    #[test]
    fn interrupted_context_repair_preserves_an_existing_terminal_block() {
        let repair = crate::context::InterruptedToolCallRepair {
            function_name: "calculate".to_string(),
            arguments: serde_json::json!({
                "expression": "2 + 2",
                "description": "Calculate"
            }),
        };
        let mut call = legacy_tool_block(
            "call:calculate{\"description\":\"Calculate\",\"expression\":\"2 + 2\"}",
        );
        call.block_type = BlockType::ToolCall;
        call.title = Some("Calculate".to_string());
        let mut result = legacy_tool_block("4");
        result.title = Some("Calculate".to_string());
        let mut blocks = vec![call, result];

        assert!(!reconcile_interrupted_tool_error_blocks(
            &mut blocks,
            &[repair]
        ));
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[1].block_type, BlockType::ToolResult);
    }

    #[test]
    fn clear_ui_restores_pending_tail_autoscroll() {
        let mut app = App::new(&Config::default());
        app.scroll = 37;
        app.auto_scroll = false;
        app.total_line_count = 500;
        app.output_state.select(Some(123));

        app.clear_ui_preserving_context();

        assert_eq!(app.blocks.len(), 1);
        assert_eq!(app.scroll, 0);
        assert!(app.auto_scroll);
        assert_eq!(app.total_line_count, 0);
        assert_eq!(app.output_state.selected(), None);
    }
}
