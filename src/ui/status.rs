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
    let rows = status_rows(app, area.width);
    f.render_widget(Paragraph::new(rows).wrap(Wrap { trim: true }), area);
}

/// Rows the status area needs at `width`, so nothing is cut off.
pub(super) fn status_height(app: &App, width: u16) -> u16 {
    let width = usize::from(width.max(1));
    status_rows(app, width as u16)
        .iter()
        .map(|row| row.width().div_ceil(width).max(1))
        .sum::<usize>()
        .try_into()
        .unwrap_or(u16::MAX)
}

/// Packs each logical status line into rows no wider than `width`. Items
/// (a `| Label: value` pair) are never split; a new row starts instead.
fn status_rows(app: &App, width: u16) -> Vec<Line<'static>> {
    let width = usize::from(width.max(1));
    let mut rows = Vec::new();
    for line in status_lines(app) {
        let mut items: Vec<Vec<Span<'static>>> = Vec::new();
        for span in line.spans {
            let starts_item = span.content.starts_with("| ") || span.content.starts_with(" · ");
            match items.last_mut() {
                Some(item) if !starts_item => item.push(span),
                _ => items.push(vec![span]),
            }
        }
        let mut row: Vec<Span<'static>> = Vec::new();
        let mut row_width = 0;
        for mut item in items {
            let item_width: usize = item.iter().map(Span::width).sum();
            if row_width > 0 && row_width + item_width > width {
                rows.push(Line::from(std::mem::take(&mut row)));
                row_width = 0;
                // A continuation row does not start with a separator.
                if let Some(first) = item.first_mut() {
                    let trimmed = first
                        .content
                        .trim_start_matches("| ")
                        .trim_start_matches(" · ")
                        .to_string();
                    first.content = trimmed.into();
                }
            }
            row_width += item.iter().map(Span::width).sum::<usize>();
            row.extend(item);
        }
        if !row.is_empty() {
            rows.push(Line::from(row));
        }
    }
    rows
}

fn status_lines(app: &App) -> Vec<Line<'static>> {
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

    let times = app.session_times;
    let mut line2_spans = line2_spans;
    for (label, ms) in [
        ("| ⏱ EngTime: ", times.engine_ms),
        ("| ToolTime: ", times.tool_ms),
        ("| IdleTime: ", times.idle_ms),
    ] {
        line2_spans.push(Span::styled(label, Style::default().fg(app.theme.system_fg)));
        line2_spans.push(Span::styled(
            format!("{} ", crate::status_summary::format_compact_duration(ms)),
            Style::default().fg(app.theme.thought_fg),
        ));
    }

    let mut status_text = vec![Line::from(spans), Line::from(line2_spans)];
    if let Some(summary) = app.tool_use_summary() {
        // One item per tool kind, so a long list wraps between items.
        let mut tool_spans = vec![Span::styled(
            format!("{} Tool use: ", icons::COMMAND),
            Style::default().fg(app.theme.system_fg),
        )];
        for (index, part) in summary.split(", ").enumerate() {
            tool_spans.push(Span::styled(
                if index == 0 {
                    part.to_string()
                } else {
                    format!(" · {part}")
                },
                Style::default().fg(app.theme.output_fg),
            ));
        }
        status_text.push(Line::from(tool_spans));
    }
    if let Some(line) = super::background_tasks::status_line(app) {
        status_text.push(line);
    }
    status_text
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

#[cfg(test)]
mod status_layout_tests {
    use super::*;

    #[test]
    fn status_rows_fit_the_width_and_the_area_grows_to_hold_them() {
        let mut app = App::new(&crate::config::Config::default());
        app.tool_use_counts.insert("run_shell_command".to_string(), 87);
        app.tool_use_counts.insert("edit".to_string(), 10);
        app.session_times = crate::status_summary::SessionTimes {
            engine_ms: 33 * 60_000,
            tool_ms: 2 * 60_000,
            idle_ms: 3 * 3_600_000,
        };
        let wide = status_height(&app, 1000);
        let narrow = status_height(&app, 60);
        assert!(narrow > wide, "narrow {narrow} vs wide {wide}");
        let rows = status_rows(&app, 60);
        let text: String = rows
            .iter()
            .map(|row| row.spans.iter().map(|span| span.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("EngTime: 33m"), "{text}");
        assert!(text.contains("ToolTime: 2m"));
        assert!(text.contains("IdleTime: 3h 00m"));
        for row in &rows {
            // Only a single item longer than the width may overflow.
            assert!(row.width() <= 60 || row.spans.len() <= 2, "{row:?}");
        }
        assert!(
            !rows.iter().any(|row| row
                .spans
                .first()
                .is_some_and(|span| span.content.starts_with("| "))),
            "continuation rows drop the separator"
        );
    }

    #[test]
    fn session_time_goes_to_the_bucket_the_app_is_in() {
        let mut app = App::new(&crate::config::Config::default());
        let back = |app: &mut App| {
            app.session_times_tick =
                Some(std::time::Instant::now() - std::time::Duration::from_secs(2));
        };
        back(&mut app);
        app.accrue_session_time();
        assert!(app.session_times.idle_ms >= 2000);
        app.is_processing = true;
        back(&mut app);
        app.accrue_session_time();
        assert!(app.session_times.engine_ms >= 2000);
        app.is_executing_tool = true;
        back(&mut app);
        app.accrue_session_time();
        assert!(app.session_times.tool_ms >= 2000);
        assert_eq!(crate::status_summary::format_compact_duration(45_000), "45s");
        assert_eq!(crate::status_summary::format_compact_duration(33 * 60_000 + 5_000), "33m");
        assert_eq!(
            crate::status_summary::format_compact_duration(3 * 3_600_000 + 300_000),
            "3h 05m"
        );
    }
}
