use super::contracts::*;
use crate::accounting::{AccountingTotals, EstimatedCost, Usage};
use crate::app::{App, BlockType, RenderBlock};
use crate::commands::CommandId;
use crate::config::ConnectionKind;
use crate::ui::Theme;
use ratatui::style::Color;
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt::Write as _;

mod redaction;
mod tool_calls;

use redaction::{RedactionOutcome, Redactor, truncate_utf8};
use tool_calls::approval_safe_tool_call_content;

pub const MAX_WEB_BLOCKS: usize = 200;
pub const MAX_WEB_BLOCK_CONTENT_BYTES: usize = 64 * 1024;
pub const MAX_WEB_BLOCK_TOTAL_BYTES: usize = 384 * 1024;
pub const MAX_WEB_BLOCK_TITLE_BYTES: usize = 256;
pub const MAX_WEB_SESSIONS: usize = 100;
pub const MAX_WEB_MODELS: usize = 256;
pub const MAX_WEB_DIAGNOSTICS: usize = super::diagnostics::OPERATIONAL_DIAGNOSTIC_CAPACITY;
pub const MAX_WEB_DIAGNOSTIC_BYTES: usize = 1024;
pub const MAX_WEB_APPROVAL_PREVIEW_BYTES: usize = 16 * 1024;
pub const MAX_WEB_APPROVAL_DESCRIPTION_BYTES: usize = 256;
pub const MAX_WEB_QUESTIONS: usize = 16;
pub const MAX_WEB_QUESTION_OPTIONS: usize = 8;
pub const MAX_WEB_SYSTEM_PROMPT_EDITOR_BYTES: usize = 64 * 1024;
pub const QUESTION_PREVIEW_UNAVAILABLE: &str =
    "Question details are unavailable in this remote view. Cancel the turn to continue.";

#[derive(Clone, Default)]
pub struct ProjectionContext {
    pub pending_approval: Option<PendingApprovalView>,
    pub pending_question: Option<PendingQuestionView>,
    pub panel_data: Option<PanelDataView>,
    pub diagnostics: Vec<DiagnosticView>,
    pub diagnostics_omitted_before: u32,
    pub additional_sensitive_values: Vec<String>,
}

pub fn project_app(app: &App, context: ProjectionContext) -> WebAppSnapshot {
    let ProjectionContext {
        pending_approval,
        pending_question,
        panel_data,
        diagnostics,
        diagnostics_omitted_before,
        additional_sensitive_values,
    } = context;
    let redactor = Redactor::for_app(app, additional_sensitive_values);
    let pending_approval = pending_approval
        .and_then(|approval| sanitize_approval(approval, &redactor, &app.session_id));
    let pending_question = pending_question
        .and_then(|question| sanitize_question(question, &redactor, &app.session_id));

    let debugger = project_debugger(app, diagnostics, diagnostics_omitted_before, &redactor);
    let include_estimated_costs = app.config.estimate_cost.unwrap_or(true);
    WebAppSnapshot {
        session: project_session_header(app, &redactor),
        blocks: project_blocks(&app.blocks, &redactor, include_estimated_costs),
        activity: project_activity(app, pending_approval.is_some(), pending_question.is_some()),
        pending_approval,
        pending_question,
        commands: project_commands(app, &redactor),
        sessions: project_sessions(app, &redactor),
        models: project_models(app, &redactor),
        themes: project_themes(&app.themes, &app.theme.name),
        usage: UsageSummaryView {
            latest_turn: project_accounting_totals(
                &app.accounting.latest_logical_turn,
                &redactor,
                include_estimated_costs,
            ),
            session: project_accounting_totals(
                &app.accounting.session,
                &redactor,
                include_estimated_costs,
            ),
        },
        status: project_status(app, &redactor),
        debugger,
        overlay: project_overlay(app, panel_data, &redactor),
    }
}

pub(crate) fn approval_preview(tool_name: &str, arguments: &serde_json::Value) -> String {
    if tool_name == "python" {
        arguments["code"].as_str().unwrap_or("").to_string()
    } else {
        serde_json::to_string_pretty(arguments).unwrap_or_default()
    }
}

pub fn theme_catalog() -> Vec<ThemeView> {
    project_themes(&Theme::all(), "")
}

pub fn theme_id(name: &str) -> String {
    let mut id = String::with_capacity(name.len());
    let mut separator = false;
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            if separator && !id.is_empty() {
                id.push('-');
            }
            id.push(character.to_ascii_lowercase());
            separator = false;
        } else {
            separator = true;
        }
    }
    if id.is_empty() {
        "theme".to_string()
    } else {
        id
    }
}

pub fn opaque_choice_id(namespace: &str, components: &[&str]) -> String {
    let namespace = namespace
        .bytes()
        .filter(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        .take(24)
        .map(char::from)
        .collect::<String>();
    let namespace = if namespace.is_empty() {
        "choice"
    } else {
        namespace.as_str()
    };
    let mut hash = Sha256::new();
    hash.update(b"lethetic-wfe-choice-v1\0");
    hash.update(namespace.as_bytes());
    for component in components {
        hash.update(b"\0");
        hash.update(component.as_bytes());
    }
    let digest = hash.finalize();
    let mut id = String::with_capacity(namespace.len() + 1 + digest.len() * 2);
    id.push_str(namespace);
    id.push('-');
    for byte in digest {
        write!(id, "{byte:02x}").expect("writing to a String cannot fail");
    }
    id
}

pub fn model_choice_id(connection_id: &str, model_id: &str) -> String {
    opaque_choice_id("model", &[connection_id, model_id])
}

pub(crate) fn system_prompt_editor_projection_is_lossy(
    app: &App,
    additional_sensitive_values: &[String],
) -> bool {
    app.show_prompt_editor
        && Redactor::for_app(app, additional_sensitive_values.to_vec())
            .redact_and_truncate(&app.system_prompt, MAX_WEB_SYSTEM_PROMPT_EDITOR_BYTES)
            .1
}

pub(crate) fn history_entry_is_losslessly_viewable(
    app: &App,
    value: &str,
    additional_sensitive_values: &[String],
) -> bool {
    if value.len() > MAX_PROMPT_BYTES {
        return false;
    }
    let projected = Redactor::for_app(app, additional_sensitive_values.to_vec())
        .redact_and_truncate_detailed(value, MAX_PROMPT_BYTES);
    !projected.redacted && !projected.truncated && projected.text == value
}

fn project_session_header(app: &App, redactor: &Redactor) -> SessionHeaderView {
    let session_id = canonical_session_id(&app.session_id).unwrap_or_default();
    let display_name = app
        .display_name
        .as_deref()
        .map(|name| redactor.redact_and_truncate(name, 256).0)
        .filter(|name| !name.is_empty());
    let fallback_label = app
        .session_summaries
        .iter()
        .find(|summary| summary.session_id == app.session_id)
        .map(|summary| redactor.redact_and_truncate(&summary.fallback_label, 128).0)
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| {
            let abbreviated = session_id.get(..8).unwrap_or(session_id.as_str());
            if abbreviated.is_empty() {
                "Untitled session".to_string()
            } else {
                format!("Session {abbreviated}")
            }
        });
    SessionHeaderView {
        session_id,
        display_name,
        fallback_label,
    }
}

fn project_commands(app: &App, redactor: &Redactor) -> Vec<crate::commands::CommandView> {
    CommandId::ALL
        .into_iter()
        .map(|command| {
            let mut view = app.command_view(command);
            view.label = redactor.redact_and_truncate(&view.label, 256).0;
            view.disabled_reason = view
                .disabled_reason
                .as_deref()
                .map(|reason| redactor.redact_and_truncate(reason, 256).0);
            view
        })
        .collect()
}

fn project_sessions(app: &App, redactor: &Redactor) -> SessionListView {
    let mut sessions = Vec::new();
    let mut seen = HashSet::new();
    for summary in &app.session_summaries {
        if sessions.len() == MAX_WEB_SESSIONS {
            break;
        }
        let Some(session_id) = canonical_session_id(&summary.session_id) else {
            continue;
        };
        if !seen.insert(session_id.clone()) {
            continue;
        }
        sessions.push(SessionChoiceView {
            selected: session_id == app.session_id,
            session_id,
            display_name: summary
                .display_name
                .as_deref()
                .map(|name| redactor.redact_and_truncate(name, 256).0)
                .filter(|name| !name.is_empty()),
            fallback_label: redactor.redact_and_truncate(&summary.fallback_label, 128).0,
        });
    }

    if !sessions.iter().any(|session| session.selected)
        && sessions.len() == MAX_WEB_SESSIONS
        && let Some(current) = app
            .session_summaries
            .iter()
            .find(|summary| summary.session_id == app.session_id)
        && let Some(session_id) = canonical_session_id(&current.session_id)
    {
        sessions.pop();
        sessions.push(SessionChoiceView {
            session_id,
            display_name: current
                .display_name
                .as_deref()
                .map(|name| redactor.redact_and_truncate(name, 256).0)
                .filter(|name| !name.is_empty()),
            fallback_label: redactor.redact_and_truncate(&current.fallback_label, 128).0,
            selected: true,
        });
    }

    SessionListView {
        has_more: app.session_summaries.len() > sessions.len(),
        sessions,
    }
}

fn project_models(app: &App, redactor: &Redactor) -> Vec<ModelChoiceView> {
    let active_connection = app.config.active_server.as_deref();
    let duplicate_counts = app.available_models.iter().fold(
        std::collections::HashMap::<&str, usize>::new(),
        |mut counts, choice| {
            *counts.entry(choice.model_id.as_str()).or_default() += 1;
            counts
        },
    );
    let mut occurrence = std::collections::HashMap::<&str, usize>::new();

    app.available_models
        .iter()
        .take(MAX_WEB_MODELS)
        .map(|choice| {
            let number = occurrence.entry(choice.model_id.as_str()).or_default();
            *number += 1;
            let model_label = safe_model_label(&choice.model_id);
            let (model_name, _) = redactor.redact_and_truncate(&model_label, 256);
            let mut label = model_name.clone();
            if duplicate_counts
                .get(choice.model_id.as_str())
                .copied()
                .unwrap_or_default()
                > 1
            {
                label.push_str(&format!(" (source {})", *number));
            }
            if !choice.available {
                label.push_str(" (unavailable)");
            }
            let (label, _) = truncate_utf8(&label, 320);
            ModelChoiceView {
                model_id: model_choice_id(&choice.connection_id, &choice.model_id),
                label,
                model_name,
                transport: match choice.kind {
                    ConnectionKind::OpenAiChatCompletions => {
                        ModelTransportView::OpenAiChatCompletions
                    }
                    ConnectionKind::ClaudeCodeProxy => ModelTransportView::ClaudeCodeProxy,
                },
                available: choice.available,
                selected: choice.model_id == app.model_name
                    && active_connection.map_or_else(
                        || choice.url == app.server_url,
                        |active| active == choice.connection_id,
                    ),
            }
        })
        .collect()
}

fn project_themes(themes: &[Theme], selected_name: &str) -> Vec<ThemeView> {
    themes
        .iter()
        .map(|theme| ThemeView {
            theme_id: theme_id(&theme.name),
            name: theme.name.clone(),
            colors: ThemeColorsView {
                output_fg: color_to_css(theme.output_fg),
                input_fg: color_to_css(theme.input_fg),
                highlight_fg: color_to_css(theme.highlight_fg),
                system_fg: color_to_css(theme.system_fg),
                thought_fg: color_to_css(theme.thought_fg),
                tool_fg: color_to_css(theme.tool_fg),
                success_fg: color_to_css(theme.success_fg),
                error_fg: color_to_css(theme.error_fg),
                warning_fg: color_to_css(theme.warning_fg),
                json_key_fg: color_to_css(theme.json_key_fg),
                json_val_fg: color_to_css(theme.json_val_fg),
                input_bg: color_to_css(theme.input_bg),
                thought_bg: color_to_css(theme.thought_bg),
                tool_bg: color_to_css(theme.tool_bg),
                terminal_bg: color_to_css(theme.terminal_bg),
            },
            selected: theme.name == selected_name,
        })
        .collect()
}

fn legacy_text_error_projection(content: &str) -> Option<String> {
    let (marker_offset, kind) = crate::app::legacy_text_error_marker(content)?;
    let mut projection = content[..marker_offset].trim_end().to_string();
    if !projection.is_empty() {
        projection.push_str("\n\n");
    }
    projection.push_str(match kind {
        crate::app::LegacyTextErrorKind::Provider => "Provider request failed.",
        crate::app::LegacyTextErrorKind::Tool => "Tool execution failed.",
    });
    Some(projection)
}

fn project_blocks(
    blocks: &[RenderBlock],
    redactor: &Redactor,
    include_estimated_costs: bool,
) -> BlockListView {
    let available = blocks.len().min(MAX_WEB_BLOCKS);
    let start = blocks.len().saturating_sub(available);
    let mut projected = Vec::with_capacity(available);
    let mut remaining = MAX_WEB_BLOCK_TOTAL_BYTES;
    let mut omitted_for_budget = 0usize;
    let mut any_size_limited = false;

    for block in blocks[start..].iter().rev() {
        if remaining == 0 {
            omitted_for_budget += 1;
            continue;
        }
        let title_limit = remaining.min(MAX_WEB_BLOCK_TITLE_BYTES);
        let (title, title_loss) = match block.title.as_deref() {
            Some(title) => {
                let outcome = redactor.redact_and_truncate_detailed(title, title_limit);
                remaining = remaining.saturating_sub(outcome.text.len());
                let loss = ProjectionLossView {
                    filtered: false,
                    redacted: outcome.redacted,
                    truncation: outcome
                        .truncated
                        .then_some(ProjectionTruncationKind::SizeLimit),
                };
                any_size_limited |= loss.truncation.is_some();
                (Some(outcome.text), loss)
            }
            None => (None, ProjectionLossView::complete()),
        };
        let legacy_text_projection = if block.block_type == BlockType::Text {
            legacy_text_error_projection(&block.content)
        } else {
            None
        };
        let (content_source, content_filtered, source_truncation, needs_redaction) =
            if let Some(projection) = legacy_text_projection {
                (Cow::Owned(projection), true, None, true)
            } else {
                match block.block_type {
                    BlockType::ToolCall => {
                        let projection = approval_safe_tool_call_content(&block.content);
                        (
                            Cow::Owned(projection.content),
                            projection.filtered,
                            projection.truncation,
                            true,
                        )
                    }
                    BlockType::ProviderError => {
                        (Cow::Borrowed("Provider request failed."), true, None, false)
                    }
                    BlockType::ToolError => {
                        (Cow::Borrowed("Tool execution failed."), true, None, false)
                    }
                    BlockType::ToolResult if block.success == Some(false) => {
                        (Cow::Borrowed("Tool execution failed."), true, None, false)
                    }
                    _ => (Cow::Borrowed(block.content.as_str()), false, None, true),
                }
            };
        let content_limit = remaining.min(if block.block_type == BlockType::ToolCall {
            MAX_WEB_APPROVAL_PREVIEW_BYTES
        } else {
            MAX_WEB_BLOCK_CONTENT_BYTES
        });
        let RedactionOutcome {
            text: content,
            redacted: content_redacted,
            truncated: content_truncated,
        } = if needs_redaction {
            redactor.redact_and_truncate_detailed(&content_source, content_limit)
        } else {
            let (text, truncated) = truncate_utf8(&content_source, content_limit);
            RedactionOutcome {
                text,
                redacted: false,
                truncated,
            }
        };
        let content_loss = ProjectionLossView {
            filtered: content_filtered,
            redacted: content_redacted,
            truncation: source_truncation
                .or_else(|| content_truncated.then_some(ProjectionTruncationKind::SizeLimit)),
        };
        any_size_limited |= matches!(
            content_loss.truncation,
            Some(ProjectionTruncationKind::SizeLimit)
        );
        remaining = remaining.saturating_sub(content.len());
        let tool = project_tool_block(
            &block.block_type,
            block.title.as_deref(),
            &content,
            content_loss,
            redactor,
        );
        let (content, content_loss) = if tool.is_some() {
            (String::new(), ProjectionLossView::complete())
        } else {
            (content, content_loss)
        };
        projected.push(RenderBlockView {
            kind: block_kind(&block.block_type),
            content,
            content_loss,
            tool,
            title,
            title_loss,
            success: block.success,
            usage: block.usage.as_ref().map(project_usage),
            estimated_cost: if include_estimated_costs {
                block
                    .estimated_cost
                    .as_ref()
                    .map(|cost| project_cost(cost, redactor))
            } else {
                None
            },
            duration_label: crate::status_summary::block_duration_label(block),
        });
    }
    projected.reverse();

    let mut omitted = start.saturating_add(omitted_for_budget);
    if omitted > 0 || any_size_limited {
        if projected.len() == MAX_WEB_BLOCKS {
            projected.remove(0);
            omitted = omitted.saturating_add(1);
        }
        projected.insert(
            0,
            RenderBlockView {
                kind: BlockKind::Truncation,
                content: if omitted > 0 {
                    format!(
                        "{omitted} earlier chat blocks are not included in this mirror snapshot."
                    )
                } else {
                    "Some chat content was shortened for the mirror snapshot.".to_string()
                },
                content_loss: ProjectionLossView::complete(),
                tool: None,
                title: Some("Remote view truncated".to_string()),
                title_loss: ProjectionLossView::complete(),
                success: None,
                usage: None,
                estimated_cost: None,
                duration_label: None,
            },
        );
    }

    BlockListView {
        blocks: projected,
        omitted_before: u32::try_from(omitted).unwrap_or(u32::MAX),
        truncated: omitted > 0 || any_size_limited,
    }
}

fn project_tool_block(
    kind: &BlockType,
    title: Option<&str>,
    content: &str,
    payload_loss: ProjectionLossView,
    redactor: &Redactor,
) -> Option<ToolBlockView> {
    match kind {
        BlockType::ToolCall => {
            let body = content.strip_prefix("call:").unwrap_or(content);
            let (tool_name, payload) = body
                .find('{')
                .map(|index| (&body[..index], &body[index..]))
                .unwrap_or((body, ""));
            let tool_name = redactor.redact_and_truncate(tool_name.trim(), 128).0;
            Some(ToolBlockView {
                kind: ToolBlockKind::Call,
                tool_name: if tool_name.is_empty() {
                    "tool".to_string()
                } else {
                    tool_name
                },
                payload: payload.to_string(),
                payload_loss,
            })
        }
        BlockType::ToolResult | BlockType::ToolError => {
            let name = title
                .and_then(|title| title.split([':', ' ']).find(|part| !part.is_empty()))
                .unwrap_or("tool");
            let name = redactor.redact_and_truncate(name, 128).0;
            Some(ToolBlockView {
                kind: ToolBlockKind::Result,
                tool_name: if name.is_empty() {
                    "tool".to_string()
                } else {
                    name
                },
                payload: content.to_string(),
                payload_loss,
            })
        }
        _ => None,
    }
}

fn block_kind(kind: &BlockType) -> BlockKind {
    match kind {
        BlockType::Text | BlockType::ProviderError => BlockKind::Text,
        BlockType::User => BlockKind::User,
        BlockType::Thought => BlockKind::Thought,
        BlockType::Markdown => BlockKind::Markdown,
        BlockType::ToolCall => BlockKind::ToolCall,
        BlockType::ToolResult | BlockType::ToolError => BlockKind::ToolResult,
        BlockType::Divider => BlockKind::Divider,
        BlockType::Formulating => BlockKind::Formulating,
    }
}

fn project_usage(usage: &Usage) -> UsageView {
    UsageView {
        uncached_input_tokens: usage.uncached_input_tokens.to_string(),
        cache_read_input_tokens: usage.cache_read_input_tokens.to_string(),
        cache_creation_input_tokens: usage.cache_creation_input_tokens.to_string(),
        output_tokens: usage.output_tokens.to_string(),
        total_input_tokens: usage.total_input().to_string(),
        total_tokens: usage.total_tokens().to_string(),
        breakdown_complete: usage.breakdown_complete,
    }
}

fn project_cost(cost: &EstimatedCost, redactor: &Redactor) -> CostView {
    let currency = redactor.redact_and_truncate(&cost.currency, 16).0;
    let mut display_cost = cost.clone();
    display_cost.currency.clone_from(&currency);
    CostView {
        display: crate::status_summary::format_estimated_cost(&display_cost),
        currency,
        nanos: cost.nanos.to_string(),
        incomplete: cost.incomplete,
        mixed_pricing: cost.mixed_pricing,
        long_context_applied: cost.long_context_applied,
        pricing_effective_as_of: redactor
            .redact_and_truncate(&cost.pricing_effective_as_of, 16)
            .0,
        pricing_valid_through: cost
            .pricing_valid_through
            .as_deref()
            .map(|date| redactor.redact_and_truncate(date, 16).0),
        provenance_kind: redactor.redact_and_truncate(&cost.provenance_kind, 64).0,
    }
}

fn project_accounting_totals(
    totals: &AccountingTotals,
    redactor: &Redactor,
    include_estimated_cost: bool,
) -> AccountingTotalsView {
    AccountingTotalsView {
        usage: project_usage(&totals.usage),
        estimated_cost: if include_estimated_cost {
            crate::status_summary::accounting_cost(totals)
                .as_ref()
                .map(|cost| project_cost(cost, redactor))
        } else {
            None
        },
        request_count: totals.request_count.to_string(),
        long_context_request_count: totals.long_context_request_count.to_string(),
        unpriced_request_count: totals.unpriced_request_count.to_string(),
        incomplete_usage_request_count: totals.incomplete_usage_request_count.to_string(),
    }
}

fn project_activity(
    app: &App,
    has_pending_approval: bool,
    has_pending_question: bool,
) -> ActivityView {
    let kind = if app.is_loading_session {
        ActivityKind::LoadingSession
    } else if has_pending_approval || app.show_approval_prompt {
        ActivityKind::AwaitingApproval
    } else if app.lsp_install_in_progress {
        ActivityKind::ManagingLsp
    } else if app.is_executing_tool {
        ActivityKind::ExecutingTool
    } else if has_pending_question || app.is_asking_user {
        ActivityKind::AwaitingAnswer
    } else if app.is_processing {
        ActivityKind::Processing
    } else {
        ActivityKind::Idle
    };
    let progress_percent = app.is_loading_session.then(|| {
        let progress = if app.load_progress.is_finite() {
            app.load_progress.clamp(0.0, 100.0)
        } else {
            0.0
        };
        progress.round() as u8
    });
    let may_cancel = matches!(
        kind,
        ActivityKind::AwaitingApproval
            | ActivityKind::ExecutingTool
            | ActivityKind::AwaitingAnswer
            | ActivityKind::Processing
            | ActivityKind::ManagingLsp
    );
    let cancel_id = may_cancel
        .then(|| app.live_cancellation_id().map(str::to_string))
        .flatten();
    ActivityView {
        kind,
        fully_idle: app.is_fully_idle(),
        cancellable: cancel_id.is_some(),
        cancel_id,
        progress_percent,
    }
}

fn browser_safe_stop_reason(app: &App) -> (&'static str, bool) {
    let original = app.stop_reason.trim();
    let safe = match project_activity(app, app.show_approval_prompt, app.is_asking_user).kind {
        ActivityKind::LoadingSession => "Loading session.",
        ActivityKind::AwaitingApproval => "Awaiting tool approval.",
        ActivityKind::ExecutingTool => "Executing tool.",
        ActivityKind::AwaitingAnswer => "Awaiting user answer.",
        ActivityKind::Processing => "Provider processing.",
        ActivityKind::ManagingLsp => "Managing LSP.",
        ActivityKind::Idle => idle_browser_stop_reason(original),
    };
    (safe, safe != original)
}

fn idle_browser_stop_reason(value: &str) -> &'static str {
    match value {
        "Ready" => "Ready",
        "Cancelled by user" => "Cancelled by user",
        "Python setup cancelled" => "Python setup cancelled",
        "Python backend probe complete" => "Python backend probe complete",
        "Session deleted" => "Session deleted",
        "Termination requested; shutting down safely…" => {
            "Termination requested; shutting down safely…"
        }
        _ if value.starts_with("Response complete") => "Response complete.",
        _ if value.starts_with("Resumed session") => "Session resumed.",
        _ if value.starts_with("Session ") && value.ends_with(" is already active") => {
            "Session is already active."
        }
        _ if value.starts_with("Model:") => "Model changed.",
        _ if value.starts_with("→ Tool dispatched:") => "Tool dispatched.",
        _ if value.starts_with("→ Loop") => "Loop detected.",
        _ if value.starts_with("⚠ Context saturated") => "Context saturated.",
        _ if value.starts_with("⚠ Persistent loop") => "Persistent loop terminated.",
        _ if value.starts_with("⚠ Minimal response") => "Minimal response received.",
        _ if value.starts_with("✗ Server error:")
            || value.starts_with("✗ Provider")
            || value.starts_with("Provider request") =>
        {
            "Provider request failed."
        }
        _ if value.starts_with("Session ")
            || value.starts_with("✗ Session")
            || value.starts_with("✗ Cannot resolve target session")
            || value.starts_with("✗ Cannot switch sessions")
            || value.starts_with("✗ Cannot lock target session") =>
        {
            "Session operation failed."
        }
        _ if value.starts_with("⚠ Python") || value.starts_with("✗ Python") => {
            "Python operation failed."
        }
        _ if value.starts_with("⏸ Awaiting approval:") => "Awaiting tool approval.",
        _ if value.starts_with("⏸ Waiting for your answer:") => "Awaiting user answer.",
        _ if value.starts_with('✗') => "Operation failed.",
        _ if value.starts_with('⚠') => "Operation warning.",
        _ if value.starts_with('⏸') => "Awaiting user action.",
        _ => "Idle.",
    }
}

fn project_status(app: &App, redactor: &Redactor) -> StatusView {
    use crate::config::{
        AccessMode, NetworkAccess, PythonExecutionTarget, SandboxBackend, ToolProfile,
    };
    use crate::status_summary::{CoarseGitState, ContextUsageSource, StatusSummary};

    let summary = StatusSummary::from_app(app);
    let (safe_stop_reason, stop_reason_filtered) = browser_safe_stop_reason(app);
    let stop_reason = redactor.redact_and_truncate_detailed(safe_stop_reason, 256);
    let stop_reason_loss = ProjectionLossView {
        filtered: stop_reason_filtered,
        redacted: stop_reason.redacted,
        truncation: stop_reason
            .truncated
            .then_some(ProjectionTruncationKind::SizeLimit),
    };
    let provider_transport = match summary.provider_kind {
        ConnectionKind::OpenAiChatCompletions => ModelTransportView::OpenAiChatCompletions,
        ConnectionKind::ClaudeCodeProxy => ModelTransportView::ClaudeCodeProxy,
    };
    let profile = match summary.python.profile {
        ToolProfile::General => PythonProfileView::General,
        ToolProfile::PythonOnly => PythonProfileView::PythonOnly,
    };
    let target = summary.python.target.map(|target| match target {
        PythonExecutionTarget::Host => PythonTargetView::Host,
        PythonExecutionTarget::Sandbox => PythonTargetView::Sandbox,
    });
    let backend = summary.python.backend.map(|backend| match backend {
        SandboxBackend::Bubblewrap => PythonBackendView::Bubblewrap,
        SandboxBackend::Podman => PythonBackendView::Podman,
    });
    let network = summary.python.network.map(|network| match network {
        NetworkAccess::None => NetworkAccessView::None,
        NetworkAccess::Nonlocal => NetworkAccessView::PublicOnly,
        NetworkAccess::Full => NetworkAccessView::Full,
    });
    let workspace_access = summary.python.workspace_access.map(|access| match access {
        AccessMode::ReadOnly => WorkspaceAccessView::ReadOnly,
        AccessMode::ReadWrite => WorkspaceAccessView::ReadWrite,
    });
    let policy_source = match summary.python.policy_source {
        crate::python_policy::PythonPolicySource::Config => PythonPolicySourceView::Config,
        crate::python_policy::PythonPolicySource::Global => PythonPolicySourceView::Global,
        crate::python_policy::PythonPolicySource::Project => PythonPolicySourceView::Project,
        crate::python_policy::PythonPolicySource::OneTime => PythonPolicySourceView::OneTime,
        crate::python_policy::PythonPolicySource::CliLocked => PythonPolicySourceView::CliLocked,
    };
    let context_source = match summary.context_source {
        ContextUsageSource::ServerUsage => ContextUsageSourceView::ServerUsage,
        ContextUsageSource::ServerPrompt => ContextUsageSourceView::ServerPrompt,
        ContextUsageSource::LocalEstimate => ContextUsageSourceView::LocalEstimate,
    };
    let git_state = match summary.git_state {
        CoarseGitState::Clean => GitStateView::Clean,
        CoarseGitState::Dirty => GitStateView::Dirty,
        CoarseGitState::Unknown => GitStateView::Unknown,
    };

    StatusView {
        background_tasks: project_background_tasks(redactor),
        tool_use: app.tool_use_summary().unwrap_or_default(),
        stop_reason: stop_reason.text,
        stop_reason_loss,
        model_label: redactor
            .redact_and_truncate(&safe_model_label(&summary.model_label), 256)
            .0,
        provider_label: redactor.redact_and_truncate(&summary.provider_label, 128).0,
        provider_transport,
        python: PythonIsolationView {
            profile,
            target,
            backend,
            network,
            workspace_access,
            grant_count: u16::try_from(summary.python.grant_count).unwrap_or(u16::MAX),
            policy_source,
            container: project_python_container(app),
        },
        tokens_per_second: summary.tokens_per_second.map(|rate| format!("{rate:.2}")),
        prompt_tokens_per_second: summary
            .prompt_tokens_per_second
            .map(|rate| format!("{rate:.2}")),
        context_tokens: summary.context_tokens.to_string(),
        context_limit_tokens: summary.context_limit_tokens.to_string(),
        context_source,
        request_usage: summary.request_usage.as_ref().map(project_usage),
        memory_mebibytes: summary.memory_mebibytes.to_string(),
        file_count: u32::try_from(summary.file_count).unwrap_or(u32::MAX),
        visible_block_count: u16::try_from(summary.visible_block_count.min(MAX_WEB_BLOCKS))
            .unwrap_or(u16::MAX),
        git_state,
    }
}

/// Running tasks and those finished in the last five minutes, at most 20.
/// Progress and timing are volatile status, so they never advance revisions.
fn project_background_tasks(redactor: &Redactor) -> Vec<BackgroundTaskView> {
    use crate::background::{TaskState, format_duration, recent};
    let tasks = recent(std::time::Duration::from_secs(300));
    let skip = tasks.len().saturating_sub(20);
    tasks
        .into_iter()
        .skip(skip)
        .map(|task| BackgroundTaskView {
            state: match task.state {
                TaskState::Running => BackgroundTaskStateView::Running,
                TaskState::Exited(Some(0)) => BackgroundTaskStateView::Done,
                TaskState::Stopped => BackgroundTaskStateView::Stopped,
                _ => BackgroundTaskStateView::Failed,
            },
            stalled: task.state.is_running() && task.idle >= std::time::Duration::from_secs(60),
            state_label: redactor.redact_and_truncate(&task.state.label(), 128).0,
            progress_percent: task
                .progress
                .map(|fraction| (fraction.clamp(0.0, 1.0) * 100.0).round() as u8),
            progress_label: redactor
                .redact_and_truncate(task.progress_label.as_deref().unwrap_or(""), 128)
                .0,
            elapsed: format_duration(task.elapsed),
            idle: format_duration(task.idle),
            description: redactor.redact_and_truncate(&task.description, 256).0,
            last_line: redactor.redact_and_truncate(task.last_line.trim(), 512).0,
            id: task.id,
        })
        .collect()
}

fn project_python_container(app: &App) -> Option<PythonContainerView> {
    project_python_container_with_identity(app, app.tool_runtime.python_container_identity())
}

fn project_python_container_with_identity(
    app: &App,
    active: Option<crate::python::PythonContainerIdentity>,
) -> Option<PythonContainerView> {
    use crate::config::{PythonExecutionTarget, SandboxBackend, ToolProfile};
    use crate::python::{PythonContainerIdentity, PythonContainerKind};

    if app.config.tool_profile != ToolProfile::PythonOnly
        || app.config.python_runtime.target != Some(PythonExecutionTarget::Sandbox)
        || app.config.python_runtime.sandbox.backend != Some(SandboxBackend::Podman)
    {
        return None;
    }

    if crate::config::is_exact_retained_nonlocal_python_policy(
        app.config.tool_profile,
        &app.config.python_runtime,
    ) {
        let mut expected = app
            .python_runtime_id
            .as_deref()
            .and_then(|runtime_id| PythonContainerIdentity::retained(runtime_id, false))?;
        expected.active = active.as_ref().is_some_and(|identity| {
            identity.kind == PythonContainerKind::Retained && identity.name == expected.name
        });
        return Some(PythonContainerView {
            kind: PythonContainerKindView::Retained,
            name: expected.name,
            active: expected.active,
        });
    }

    active.and_then(|identity| {
        if identity.kind == PythonContainerKind::Transient && identity.active {
            Some(PythonContainerView {
                kind: PythonContainerKindView::Transient,
                name: identity.name,
                active: true,
            })
        } else {
            None
        }
    })
}

fn project_debugger(
    app: &App,
    diagnostics: Vec<DiagnosticView>,
    diagnostics_omitted_before: u32,
    redactor: &Redactor,
) -> DebuggerView {
    let omitted_before = diagnostics_omitted_before.saturating_add(
        u32::try_from(diagnostics.len().saturating_sub(MAX_WEB_DIAGNOSTICS)).unwrap_or(u32::MAX),
    );
    let activity = project_activity(app, app.show_approval_prompt, app.is_asking_user).kind;
    let summary = match activity {
        ActivityKind::Idle => "Actor ready · idle · browser mirror ready",
        ActivityKind::LoadingSession => "Actor ready · loading session · browser mirror ready",
        ActivityKind::AwaitingApproval => "Actor ready · awaiting approval · browser mirror ready",
        ActivityKind::ExecutingTool => "Actor ready · executing tool · browser mirror ready",
        ActivityKind::AwaitingAnswer => "Actor ready · awaiting user answer · browser mirror ready",
        ActivityKind::Processing => "Actor ready · provider processing · browser mirror ready",
        ActivityKind::ManagingLsp => "Actor ready · managing LSP · browser mirror ready",
    };
    DebuggerView {
        open: app.show_debug,
        summary: summary.to_string(),
        entries: sanitize_diagnostics(diagnostics, redactor),
        omitted_before,
    }
}

fn safe_model_label(value: &str) -> String {
    if value.contains("://") {
        return "Remote model".to_string();
    }
    let value = value.replace('\\', "/");
    let label = if value.starts_with('/')
        || value.starts_with("./")
        || value.starts_with("../")
        || value.as_bytes().get(1).is_some_and(|byte| *byte == b':')
    {
        value
            .rsplit('/')
            .find(|part| !part.is_empty())
            .unwrap_or("model")
    } else {
        value.as_str()
    };
    label
        .chars()
        .filter(|character| !character.is_control())
        .collect()
}

fn project_overlay(
    app: &App,
    panel_data: Option<PanelDataView>,
    redactor: &Redactor,
) -> OverlayView {
    let active_panel = if app.show_approval_prompt {
        Some(PanelId::ToolApproval)
    } else if app.is_asking_user {
        Some(PanelId::AskUser)
    } else if app.show_cleanup_prompt || app.show_prompt_save_dialog {
        Some(PanelId::Confirmation)
    } else if app.show_session_name_dialog {
        Some(PanelId::NameSession)
    } else if app.show_hotkeys {
        Some(PanelId::Hotkeys)
    } else if app.show_theme_menu {
        Some(PanelId::Themes)
    } else if app.show_history {
        Some(PanelId::InputHistory)
    } else if app.show_prompt_editor || app.show_prompt_manager {
        Some(PanelId::SystemPrompt)
    } else if app.show_session_manager {
        Some(PanelId::Sessions)
    } else if app.show_latest_files {
        Some(PanelId::LatestFiles)
    } else if app.show_model_switcher {
        Some(PanelId::Models)
    } else if app.show_lsp_manager {
        Some(PanelId::LspServers)
    } else if app.python_setup.is_some() {
        Some(PanelId::AgentMode)
    } else if app.skills_panel.is_some() {
        Some(PanelId::Skills)
    } else if app.show_palette {
        Some(PanelId::CommandPalette)
    } else {
        None
    };
    let active_panel = if matches!(active_panel, Some(PanelId::ToolApproval | PanelId::AskUser)) {
        active_panel
    } else {
        panel_data.as_ref().map(panel_for_data).or(active_panel)
    };
    let data = panel_data
        .and_then(|data| sanitize_panel_data(data, active_panel, redactor, &app.session_id));
    OverlayView { active_panel, data }
}

fn panel_for_data(data: &PanelDataView) -> PanelId {
    match data {
        PanelDataView::Hotkeys { .. } => PanelId::Hotkeys,
        PanelDataView::InputHistory { .. } => PanelId::InputHistory,
        PanelDataView::LatestFiles { .. } => PanelId::LatestFiles,
        PanelDataView::SystemPrompts { .. } => PanelId::SystemPrompt,
        PanelDataView::LspServers { .. } => PanelId::LspServers,
        PanelDataView::LoopModes { .. } => PanelId::LoopDetection,
        PanelDataView::AgentModes { .. } => PanelId::AgentMode,
        PanelDataView::NameSession { .. } => PanelId::NameSession,
        PanelDataView::Confirmation { .. } => PanelId::Confirmation,
        PanelDataView::Skills { .. } => PanelId::Skills,
    }
}

fn sanitize_panel_data(
    mut data: PanelDataView,
    active_panel: Option<PanelId>,
    redactor: &Redactor,
    current_session_id: &str,
) -> Option<PanelDataView> {
    if active_panel != Some(panel_for_data(&data)) {
        return None;
    }
    match &mut data {
        PanelDataView::Hotkeys { shortcuts } => {
            shortcuts.truncate(64);
            for shortcut in shortcuts {
                shortcut.keys = redactor.redact_and_truncate(&shortcut.keys, 64).0;
                shortcut.label = redactor.redact_and_truncate(&shortcut.label, 256).0;
            }
        }
        PanelDataView::InputHistory { entries, .. } => {
            entries.truncate(100);
            for entry in entries {
                if !is_wire_id(&entry.entry_id, MAX_CHOICE_ID_BYTES) {
                    return None;
                }
                entry.label = redactor.redact_and_truncate(&entry.label, 1024).0;
            }
        }
        PanelDataView::LatestFiles { files, .. } => {
            files.truncate(100);
            for file in files {
                if !is_wire_id(&file.file_id, MAX_CHOICE_ID_BYTES) {
                    return None;
                }
                file.label = redactor.redact_and_truncate(&file.label, 256).0;
            }
        }
        PanelDataView::SystemPrompts {
            prompts,
            editor_content,
            content_truncated,
        } => {
            prompts.truncate(100);
            for prompt in prompts {
                if !is_wire_id(&prompt.prompt_id, MAX_CHOICE_ID_BYTES) {
                    return None;
                }
                prompt.label = redactor.redact_and_truncate(&prompt.label, 256).0;
            }
            if let Some(content) = editor_content {
                let (safe, lossy) =
                    redactor.redact_and_truncate(content, MAX_WEB_SYSTEM_PROMPT_EDITOR_BYTES);
                *content = safe;
                *content_truncated |= lossy;
            }
        }
        PanelDataView::LspServers { servers } => {
            servers.truncate(128);
            for server in servers {
                if !is_wire_id(&server.server_id, MAX_CHOICE_ID_BYTES) {
                    return None;
                }
                server.label = redactor.redact_and_truncate(&server.label, 256).0;
                server.allowed_actions.sort_by_key(|action| match action {
                    LspAction::Install => 0,
                    LspAction::Enable => 1,
                    LspAction::Disable => 2,
                    LspAction::CancelInstall => 3,
                });
                server.allowed_actions.dedup();
            }
        }
        PanelDataView::LoopModes { modes } | PanelDataView::AgentModes { modes } => {
            modes.truncate(32);
            for mode in modes {
                if !is_wire_id(&mode.mode_id, MAX_CHOICE_ID_BYTES) {
                    return None;
                }
                mode.label = redactor.redact_and_truncate(&mode.label, 256).0;
                mode.disabled_reason = mode
                    .disabled_reason
                    .as_deref()
                    .map(|reason| redactor.redact_and_truncate(reason, 256).0);
            }
        }
        PanelDataView::Skills {
            skills,
            catalog,
            message,
        } => {
            skills.truncate(200);
            for skill in skills {
                if !is_wire_id(&skill.skill_id, MAX_CHOICE_ID_BYTES) {
                    return None;
                }
                skill.name = redactor.redact_and_truncate(&skill.name, 64).0;
                skill.description = redactor.redact_and_truncate(&skill.description, 1024).0;
                skill.source = redactor.redact_and_truncate(&skill.source, 64).0;
            }
            catalog.truncate(64);
            for entry in catalog {
                // Catalog links are fixed repository URLs, never user text.
                if !is_wire_id(&entry.entry_id, MAX_CHOICE_ID_BYTES)
                    || !entry
                        .url
                        .starts_with("https://github.com/anthropics/skills/tree/")
                {
                    return None;
                }
            }
            *message = message
                .as_deref()
                .map(|text| redactor.redact_and_truncate(text, 512).0);
        }
        PanelDataView::NameSession { current_name } => {
            *current_name = current_name
                .as_deref()
                .map(|name| redactor.redact_and_truncate(name, 256).0)
                .filter(|name| !name.is_empty());
        }
        PanelDataView::Confirmation { confirmation } => {
            if !is_wire_id(&confirmation.confirmation_id, MAX_CHOICE_ID_BYTES) {
                return None;
            }
            let session_is_valid = match confirmation.action {
                DestructiveActionView::DeleteSession => confirmation
                    .session_id
                    .as_deref()
                    .is_some_and(|session_id| {
                        canonical_session_id(session_id).as_deref() == Some(session_id)
                    }),
                DestructiveActionView::ClearContext
                | DestructiveActionView::DeletePythonRuntime
                | DestructiveActionView::OverwriteSystemPrompt => confirmation
                    .session_id
                    .as_deref()
                    .is_some_and(|session_id| {
                        canonical_session_id(session_id).as_deref() == Some(current_session_id)
                    }),
                DestructiveActionView::WipeSessions | DestructiveActionView::Quit => {
                    confirmation.session_id.is_none()
                }
            };
            if !session_is_valid {
                return None;
            }
            confirmation.title = redactor.redact_and_truncate(&confirmation.title, 256).0;
            confirmation.message = redactor.redact_and_truncate(&confirmation.message, 1024).0;
        }
    }
    Some(data)
}

fn sanitize_approval(
    mut approval: PendingApprovalView,
    redactor: &Redactor,
    current_session_id: &str,
) -> Option<PendingApprovalView> {
    if canonical_session_id(&approval.session_id).as_deref() != Some(current_session_id)
        || !is_wire_id(&approval.approval_id, MAX_CHOICE_ID_BYTES)
        || !is_display_id(&approval.tool_call_id, MAX_TOOL_CALL_ID_BYTES)
    {
        return None;
    }
    approval.tool_name = redactor.redact_and_truncate(&approval.tool_name, 128).0;
    approval.description = redactor
        .redact_and_truncate(&approval.description, MAX_WEB_APPROVAL_DESCRIPTION_BYTES)
        .0;
    let preview =
        redactor.redact_and_truncate_detailed(&approval.preview, MAX_WEB_APPROVAL_PREVIEW_BYTES);
    approval.preview = preview.text;
    approval.preview_redacted |= preview.redacted;
    approval.preview_truncated |= preview.truncated;
    if approval.preview_redacted || approval.preview_truncated {
        approval.can_view_original = false;
    }
    approval
        .allowed_decisions
        .sort_by_key(|decision| match decision {
            ApprovalDecision::ApproveOnce => 0,
            ApprovalDecision::ApproveAlways => 1,
            ApprovalDecision::Deny => 2,
        });
    approval.allowed_decisions.dedup();
    Some(approval)
}

fn sanitize_question(
    mut question: PendingQuestionView,
    redactor: &Redactor,
    current_session_id: &str,
) -> Option<PendingQuestionView> {
    if canonical_session_id(&question.session_id).as_deref() != Some(current_session_id)
        || !is_wire_id(&question.form_id, MAX_CHOICE_ID_BYTES)
        || !is_display_id(&question.tool_call_id, MAX_TOOL_CALL_ID_BYTES)
    {
        return None;
    }
    let mut content_truncated = question.content_truncated;
    content_truncated |= question.questions.len() > MAX_WEB_QUESTIONS;
    question.questions.truncate(MAX_WEB_QUESTIONS);
    for prompt in &mut question.questions {
        if !is_wire_id(&prompt.question_id, MAX_CHOICE_ID_BYTES) {
            return None;
        }
        let (projected_prompt, prompt_lossy) = redactor.redact_and_truncate(&prompt.prompt, 4096);
        prompt.prompt = projected_prompt;
        content_truncated |= prompt_lossy;
        content_truncated |= prompt.options.len() > MAX_WEB_QUESTION_OPTIONS;
        prompt.options.truncate(MAX_WEB_QUESTION_OPTIONS);
        for option in &mut prompt.options {
            if !is_wire_id(&option.option_id, MAX_CHOICE_ID_BYTES) {
                return None;
            }
            let (label, label_lossy) = redactor.redact_and_truncate(&option.label, 256);
            option.label = label;
            content_truncated |= label_lossy;
            if let Some(description) = option.description.as_deref() {
                let (description, description_lossy) =
                    redactor.redact_and_truncate(description, 256);
                option.description = Some(description);
                content_truncated |= description_lossy;
            }
        }
    }
    question.content_truncated = content_truncated;
    if content_truncated {
        let question_id = question
            .questions
            .first()
            .map(|prompt| prompt.question_id.clone())
            .unwrap_or_else(|| "question-unavailable".to_string());
        question.questions = vec![QuestionPromptView {
            question_id,
            prompt: QUESTION_PREVIEW_UNAVAILABLE.to_string(),
            options: Vec::new(),
            multiple: false,
            allows_other: false,
        }];
    }
    Some(question)
}

fn sanitize_diagnostics(
    diagnostics: Vec<DiagnosticView>,
    redactor: &Redactor,
) -> Vec<DiagnosticView> {
    diagnostics
        .into_iter()
        .take(MAX_WEB_DIAGNOSTICS)
        .map(|diagnostic| DiagnosticView {
            code: diagnostic.code,
            severity: diagnostic.severity,
            message: redactor
                .redact_and_truncate(&diagnostic.message, MAX_WEB_DIAGNOSTIC_BYTES)
                .0,
        })
        .collect()
}

fn canonical_session_id(value: &str) -> Option<String> {
    let uuid = uuid::Uuid::parse_str(value).ok()?;
    let canonical = uuid.hyphenated().to_string();
    (canonical == value).then_some(canonical)
}

fn is_wire_id(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn is_display_id(value: &str, max_bytes: usize) -> bool {
    !value.is_empty() && value.len() <= max_bytes && !value.chars().any(char::is_control)
}

fn color_to_css(color: Color) -> String {
    let (red, green, blue) = match color {
        Color::Reset | Color::Black => (0, 0, 0),
        Color::Red => (128, 0, 0),
        Color::Green => (0, 128, 0),
        Color::Yellow => (128, 128, 0),
        Color::Blue => (0, 0, 128),
        Color::Magenta => (128, 0, 128),
        Color::Cyan => (0, 128, 128),
        Color::Gray => (192, 192, 192),
        Color::DarkGray => (128, 128, 128),
        Color::LightRed => (255, 0, 0),
        Color::LightGreen => (0, 255, 0),
        Color::LightYellow => (255, 255, 0),
        Color::LightBlue => (0, 0, 255),
        Color::LightMagenta => (255, 0, 255),
        Color::LightCyan => (0, 255, 255),
        Color::White => (255, 255, 255),
        Color::Rgb(red, green, blue) => (red, green, blue),
        Color::Indexed(index) => indexed_color(index),
    };
    format!("#{red:02x}{green:02x}{blue:02x}")
}

fn indexed_color(index: u8) -> (u8, u8, u8) {
    const ANSI: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (128, 0, 0),
        (0, 128, 0),
        (128, 128, 0),
        (0, 0, 128),
        (128, 0, 128),
        (0, 128, 128),
        (192, 192, 192),
        (128, 128, 128),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (0, 0, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    if index < 16 {
        return ANSI[index as usize];
    }
    if index < 232 {
        const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
        let cube = index - 16;
        return (
            LEVELS[(cube / 36) as usize],
            LEVELS[((cube % 36) / 6) as usize],
            LEVELS[(cube % 6) as usize],
        );
    }
    let gray = 8 + (index - 232) * 10;
    (gray, gray, gray)
}

#[cfg(test)]
mod tests;
