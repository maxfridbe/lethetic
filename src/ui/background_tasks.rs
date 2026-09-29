use crate::app::App;
use crate::background::{self, TaskSnapshot, TaskState};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block as UIBlock, Borders, Paragraph, Wrap},
};
use std::time::Duration;

/// Finished tasks stay on the status line this long.
const STATUS_WINDOW: Duration = Duration::from_secs(300);
/// A running task with no output or file growth for this long looks stalled.
const STALLED_AFTER: Duration = Duration::from_secs(60);

fn state_style(app: &App, task: &TaskSnapshot) -> Style {
    let theme = &app.theme;
    match &task.state {
        TaskState::Running if task.idle >= STALLED_AFTER => Style::default().fg(theme.warning_fg),
        TaskState::Running => Style::default().fg(theme.highlight_fg),
        TaskState::Exited(Some(0)) => Style::default().fg(theme.success_fg),
        TaskState::Stopped => Style::default().fg(theme.system_fg),
        _ => Style::default().fg(theme.error_fg),
    }
}

fn state_mark(task: &TaskSnapshot) -> &'static str {
    match &task.state {
        TaskState::Running => "⏳",
        TaskState::Exited(Some(0)) => "✓",
        TaskState::Stopped => "■",
        _ => "✗",
    }
}

/// `⏳ Background: bg1 [████░░░░] 45% Download model · bg2 ✓ done`.
pub(super) fn status_line(app: &App) -> Option<Line<'static>> {
    let tasks = background::recent(STATUS_WINDOW);
    if tasks.is_empty() {
        return None;
    }
    let theme = &app.theme;
    let mut spans = vec![Span::styled(
        "⏳ Background: ",
        Style::default().fg(theme.system_fg),
    )];
    for (index, task) in tasks.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(" · ", Style::default().fg(theme.system_fg)));
        }
        let style = state_style(app, task);
        spans.push(Span::styled(format!("{} ", task.id), style.add_modifier(Modifier::BOLD)));
        if task.state.is_running() {
            spans.push(Span::styled(
                format!("[{}] ", background::progress_bar(task.progress, 8, task.elapsed)),
                style,
            ));
            if let Some(label) = &task.progress_label {
                spans.push(Span::styled(format!("{label} "), style));
            }
        } else {
            spans.push(Span::styled(format!("{} {} ", state_mark(task), task.state.label()), style));
        }
        spans.push(Span::styled(
            task.description.chars().take(40).collect::<String>(),
            Style::default().fg(theme.output_fg),
        ));
    }
    spans.push(Span::styled(" (F8)", Style::default().fg(theme.system_fg)));
    Some(Line::from(spans))
}

/// Right-hand pane with one card per background task (F8).
pub(super) fn render(f: &mut ratatui::Frame, app: &App, area: Rect) {
    if !app.show_background_tasks || area.width == 0 || area.height == 0 {
        return;
    }
    let theme = &app.theme;
    let tasks = background::list();
    let running = tasks.iter().filter(|task| task.state.is_running()).count();
    let bar_width = usize::from(area.width.saturating_sub(4)).clamp(10, 40);
    let mut lines: Vec<Line> = Vec::new();
    if tasks.is_empty() {
        lines.push(Line::from(Span::styled(
            "No background tasks. The model starts them with the background_task tool.",
            Style::default().fg(theme.system_fg),
        )));
    }
    for task in tasks.iter().rev() {
        let style = state_style(app, task);
        lines.push(Line::from(vec![
            Span::styled(format!("{} {} ", state_mark(task), task.id), style.add_modifier(Modifier::BOLD)),
            Span::styled(task.description.clone(), Style::default().fg(theme.output_fg)),
        ]));
        lines.push(Line::from(vec![
            Span::styled(
                format!("[{}] ", background::progress_bar(task.progress, bar_width, task.elapsed)),
                style,
            ),
            Span::styled(
                task.progress_label.clone().unwrap_or_default(),
                style,
            ),
        ]));
        let mut detail = format!("{} · {}", task.state.label(), background::format_duration(task.elapsed));
        if task.state.is_running() {
            detail.push_str(&format!(" · last activity {} ago", background::format_duration(task.idle)));
            if task.idle >= STALLED_AFTER {
                detail.push_str(" · may be stalled");
            }
        }
        detail.push_str(&format!(" · notify {}", task.notify.label()));
        lines.push(Line::from(Span::styled(detail, Style::default().fg(theme.system_fg))));
        if !task.last_line.trim().is_empty() {
            lines.push(Line::from(Span::styled(
                format!("› {}", task.last_line.trim()),
                Style::default().fg(theme.thought_fg),
            )));
        }
        lines.push(Line::from(Span::styled(
            format!("$ {}", task.command),
            Style::default().fg(theme.system_fg).add_modifier(Modifier::DIM),
        )));
        lines.push(Line::from(""));
    }
    let title = format!("⏳ Background tasks · {running} running of {} (F8)", tasks.len());
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            UIBlock::default()
                .title(title)
                .borders(Borders::ALL)
                .style(Style::default().bg(theme.terminal_bg)),
        ),
        area,
    );
}
