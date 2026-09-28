use crate::app::App;
use crate::icons;
use crate::status_summary::{
    CoarseGitState, ContextUsageSource, StatusSummary, format_estimated_cost, format_tokens,
};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block as UIBlock, Borders, List, ListItem, Paragraph, Wrap},
};

pub(super) fn render_processing(f: &mut ratatui::Frame, app: &App, area: Rect) {
    let processing_text = if app.show_approval_prompt {
        vec![Line::from(vec![Span::styled(
            format!(
                "  {} {} Awaiting Permission For Tool Call...",
                icons::SPINNER[app.spinner_index],
                icons::WARNING
            ),
            Style::default()
                .fg(app.theme.warning_fg)
                .add_modifier(Modifier::BOLD),
        )])]
    } else if app.is_executing_tool {
        let preview = if app.tool_output_preview.is_empty() {
            "Executing Tool...".to_string()
        } else {
            let first_line = app.tool_output_preview.lines().next().unwrap_or("...");
            if first_line.len() > 50 {
                format!("{}...", &first_line[..47])
            } else {
                first_line.to_string()
            }
        };
        vec![Line::from(vec![Span::styled(
            format!(
                "  {} {} {}",
                icons::TOOL_SPINNER[app.tool_spinner_index],
                icons::COMMAND,
                preview
            ),
            Style::default().fg(app.theme.tool_fg),
        )])]
    } else if app.is_asking_user {
        vec![Line::from(vec![Span::styled(
            format!(
                "  {} {} Waiting for User Input...",
                icons::SPINNER[app.spinner_index],
                icons::INPUT
            ),
            Style::default().fg(app.theme.warning_fg),
        )])]
    } else if app.is_processing {
        vec![Line::from(vec![Span::styled(
            format!(
                "  {} {} Lethetic Intelligence Engine Processing...",
                icons::SPINNER[app.spinner_index],
                icons::PROCESSING
            ),
            Style::default().fg(app.theme.warning_fg),
        )])]
    } else {
        let reason = &app.stop_reason;
        let color = if reason.starts_with('⚠') || reason.starts_with('✗') {
            app.theme.warning_fg
        } else if reason.starts_with('⏸') || reason.starts_with('→') {
            app.theme.highlight_fg
        } else {
            app.theme.system_fg
        };
        vec![Line::from(vec![Span::styled(
            format!("  {} {}", icons::SUCCESS, reason),
            Style::default().fg(color),
        )])]
    };
    f.render_widget(Paragraph::new(processing_text), area);
}

pub(super) fn render_status(f: &mut ratatui::Frame, app: &App, area: Rect) {
    let summary = StatusSummary::from_app(app);
    let git_color = match summary.git_state {
        CoarseGitState::Clean => app.theme.success_fg,
        CoarseGitState::Dirty => app.theme.error_fg,
        CoarseGitState::Unknown => app.theme.warning_fg,
    };
    let line2_spans = vec![
        Span::styled(
            format!("{} Path: ", icons::PATH),
            Style::default().fg(app.theme.system_fg),
        ),
        Span::styled(
            format!("{} ", app.current_dir),
            Style::default().fg(app.theme.tool_fg),
        ),
        Span::styled(
            format!("| {} Git: ", icons::GIT),
            Style::default().fg(app.theme.system_fg),
        ),
        Span::styled(
            format!("{} ", app.git_status),
            Style::default().fg(git_color),
        ),
    ];

    let context_source = match summary.context_source {
        ContextUsageSource::ServerUsage => "server usage",
        ContextUsageSource::ServerPrompt => "server prompt",
        ContextUsageSource::LocalEstimate => "local estimate",
    };
    let request_usage = summary
        .request_usage
        .as_ref()
        .map(|usage| {
            let marker = if usage.breakdown_complete { "" } else { "*" };
            format!(
                "{} (u: {}, cr: {}, cw: {}, out: {}){marker} ",
                format_tokens(usage.total_tokens()),
                format_tokens(usage.uncached_input_tokens),
                format_tokens(usage.cache_read_input_tokens),
                format_tokens(usage.cache_creation_input_tokens),
                format_tokens(usage.output_tokens),
            )
        })
        .unwrap_or_else(|| "- ".to_string());
    let rate = |value: Option<f64>| {
        value
            .map(|value| format!("{value:.1}"))
            .unwrap_or_else(|| "-".to_string())
    };

    let mut spans = vec![
        Span::styled(
            format!("{} tg: ", icons::TOKENS),
            Style::default().fg(app.theme.system_fg),
        ),
        Span::styled(
            format!("{} ", rate(summary.tokens_per_second)),
            Style::default().fg(app.theme.thought_fg),
        ),
        Span::styled("pp: ", Style::default().fg(app.theme.system_fg)),
        Span::styled(
            format!("{} ", rate(summary.prompt_tokens_per_second)),
            Style::default().fg(app.theme.thought_fg),
        ),
        Span::styled(
            format!("| {} Model: ", icons::MODEL),
            Style::default().fg(app.theme.system_fg),
        ),
        Span::styled(
            format!("{} ", summary.model_label),
            Style::default().fg(app.theme.success_fg),
        ),
        Span::styled(
            format!("| {} Connection: ", icons::SERVER),
            Style::default().fg(app.theme.system_fg),
        ),
        Span::styled(
            format!("{} ({:?}) ", summary.provider_label, summary.provider_kind),
            Style::default().fg(app.theme.warning_fg),
        ),
        Span::styled("| Mode: ", Style::default().fg(app.theme.system_fg)),
        Span::styled(
            format!("{} ", summary.python.display()),
            Style::default().fg(app.theme.highlight_fg),
        ),
        Span::styled(
            format!("| {} Context: ", icons::TOKENS),
            Style::default().fg(app.theme.system_fg),
        ),
        Span::styled(
            format!(
                "{}/{} ({context_source}) ",
                format_tokens(summary.context_tokens),
                format_tokens(summary.context_limit_tokens)
            ),
            Style::default().fg(app.theme.thought_fg),
        ),
        Span::styled("| Tokens Used: ", Style::default().fg(app.theme.system_fg)),
        Span::styled(request_usage, Style::default().fg(app.theme.thought_fg)),
    ];

    if app.config.estimate_cost.unwrap_or(true) {
        if let Some(cost) = summary.latest_turn_cost.as_ref() {
            spans.push(Span::styled(
                "| API-Eq turn: ",
                Style::default().fg(app.theme.system_fg),
            ));
            spans.push(Span::styled(
                format!("{} ", format_estimated_cost(cost)),
                Style::default().fg(app.theme.thought_fg),
            ));
        }
        if let Some(cost) = summary.session_cost.as_ref() {
            spans.push(Span::styled(
                "| API-Eq sess: ",
                Style::default().fg(app.theme.system_fg),
            ));
            spans.push(Span::styled(
                format!("{} ", format_estimated_cost(cost)),
                Style::default().fg(app.theme.thought_fg),
            ));
        }
    }

    spans.push(Span::styled(
        "| Mem: ",
        Style::default().fg(app.theme.system_fg),
    ));
    spans.push(Span::styled(
        format!("{}MB ", summary.memory_mebibytes),
        Style::default().fg(app.theme.thought_fg),
    ));
    if let Some(bytes) = app.lethetic_dir_bytes {
        spans.push(Span::styled(
            "| .lethetic: ",
            Style::default().fg(app.theme.system_fg),
        ));
        spans.push(Span::styled(
            format!("{} ", crate::status_summary::format_bytes(bytes)),
            Style::default().fg(app.theme.thought_fg),
        ));
    }
    spans.push(Span::styled(
        "| Files: ",
        Style::default().fg(app.theme.system_fg),
    ));
    spans.push(Span::styled(
        format!("{} ", summary.file_count),
        Style::default().fg(app.theme.thought_fg),
    ));
    spans.push(Span::styled(
        "| Blocks: ",
        Style::default().fg(app.theme.system_fg),
    ));
    spans.push(Span::styled(
        format!("{} ", summary.visible_block_count),
        Style::default().fg(app.theme.thought_fg),
    ));

    let mut status_text = vec![Line::from(spans), Line::from(line2_spans)];
    if let Some(summary) = app.tool_use_summary() {
        status_text.push(Line::from(vec![
            Span::styled(
                format!("{} Tool use: ", icons::COMMAND),
                Style::default().fg(app.theme.system_fg),
            ),
            Span::styled(summary, Style::default().fg(app.theme.output_fg)),
        ]));
    }
    f.render_widget(Paragraph::new(status_text).wrap(Wrap { trim: true }), area);
}

pub(super) fn render_debug(f: &mut ratatui::Frame, app: &App, area: Rect) {
    if app.show_debug {
        let items: Vec<ListItem> = app
            .debug_log
            .iter()
            .rev()
            .take(50)
            .map(|s| ListItem::new(s.as_str()))
            .collect();
        f.render_widget(
            List::new(items)
                .block(
                    UIBlock::default()
                        .title(format!("{} Debugger", icons::DEBUG))
                        .borders(Borders::ALL),
                )
                .style(Style::default().fg(app.theme.system_fg)),
            area,
        );
    }
}

/// One line under the input while remote control is running.
pub(super) fn render_remote_control(f: &mut ratatui::Frame, app: &App, area: Rect) {
    let Some(target) = app.remote_control_target.as_deref() else {
        return;
    };
    if area.height == 0 {
        return;
    }
    let label = Style::default().fg(app.theme.system_fg);
    let value = Style::default().fg(app.theme.output_fg);
    let auth = if app.remote_control_open {
        Span::styled(
            "OPEN (no token)",
            Style::default()
                .fg(app.theme.error_fg)
                .add_modifier(ratatui::style::Modifier::BOLD),
        )
    } else {
        Span::styled("token", Style::default().fg(app.theme.success_fg))
    };
    let clients = match (app.remote_control_clients, &app.remote_control_last_peer) {
        (0, _) => "no browsers".to_string(),
        (1, Some(peer)) => format!("1 browser ({peer})"),
        (count, Some(peer)) => format!("{count} browsers (latest {peer})"),
        (count, None) => format!("{count} browsers"),
    };
    let mut spans = vec![
        Span::styled(format!("{} Remote control: ", icons::SERVER), label),
        Span::styled(
            target.to_string(),
            Style::default().fg(app.theme.highlight_fg),
        ),
        Span::styled(" | auth: ", label),
        auth,
        Span::styled(" | files: ", label),
        Span::styled(
            if app.remote_control_files {
                "shared"
            } else {
                "off"
            },
            value,
        ),
        Span::styled(" | ", label),
        Span::styled(clients, value),
    ];
    spans.push(Span::styled(
        if app.remote_control_locked {
            " | set by --rc"
        } else {
            " | Ctrl+P → Remote Control: stop"
        },
        label,
    ));
    f.render_widget(
        ratatui::widgets::Paragraph::new(ratatui::text::Line::from(spans))
            .style(Style::default().bg(app.theme.terminal_bg)),
        area,
    );
}

#[cfg(test)]
mod remote_control_line_tests {
    use crate::app::App;
    use crate::config::Config;
    use ratatui::{Terminal, backend::TestBackend};

    fn screen(app: &mut App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(160, 30)).unwrap();
        terminal.draw(|frame| crate::ui::ui(frame, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn status_line_appears_only_while_remote_control_runs() {
        let mut app = App::new(&Config::default());
        app.show_session_manager = false;
        assert!(!screen(&mut app).contains("Remote control:"));
        app.remote_control_target = Some("https://brainiac:11223".into());
        app.remote_control_open = true;
        app.remote_control_clients = 2;
        app.remote_control_last_peer = Some("100.64.0.7".into());
        let text = screen(&mut app);
        assert!(
            text.contains("Remote control: https://brainiac:11223"),
            "{text}"
        );
        assert!(text.contains("auth: OPEN (no token)"));
        assert!(text.contains("files: off"));
        assert!(text.contains("2 browsers (latest 100.64.0.7)"));
    }
}
