use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::Style,
    text::{Line, Span},
    widgets::{Block as UIBlock, Borders, Clear, Paragraph},
};

use crate::{app::App, icons, ui::centered_rect};

pub fn render_compaction_popup(f: &mut Frame, app: &App) {
    let popup = match app.compaction_popup.as_ref() {
        Some(p) => p,
        None => return,
    };

    let area = centered_rect(72, 65, f.area());
    f.render_widget(Clear, area);

    let footer_height: u16 = if popup.done { 2 } else { 0 };
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(footer_height)])
        .split(area);
    let content_area = layout[0];

    // Inner dimensions (subtract 2 for top+bottom border, 2 for left+right border)
    let visible_height = content_area.height.saturating_sub(2) as usize;
    let width = content_area.width.saturating_sub(2) as usize;

    // Word-wrap content into display lines (char-aware, no byte slicing)
    let mut display_lines: Vec<String> = Vec::new();
    for raw_line in popup.content.lines() {
        let chars: Vec<char> = raw_line.chars().collect();
        if chars.len() <= width {
            display_lines.push(raw_line.to_string());
        } else {
            let mut pos = 0;
            while pos < chars.len() {
                let end = (pos + width).min(chars.len());
                display_lines.push(chars[pos..end].iter().collect());
                pos = end;
            }
        }
    }

    let total_lines = display_lines.len();
    let max_scroll = total_lines.saturating_sub(visible_height);
    // usize::MAX is our sentinel for "follow bottom"; clamp handles it.
    let scroll = popup.scroll.min(max_scroll);

    let scroll_hint = if total_lines > visible_height {
        format!(
            " [{}/{}]",
            (scroll + visible_height).min(total_lines),
            total_lines
        )
    } else {
        String::new()
    };

    let title = if popup.done {
        format!(" ✓ Compaction Complete{} ", scroll_hint)
    } else {
        format!(" {} Compacting session…{} ", icons::SPINNER[0], scroll_hint)
    };

    let block = UIBlock::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().bg(app.theme.terminal_bg))
        .border_style(Style::default().fg(app.theme.thought_fg));

    let visible: Vec<Line> = display_lines
        .iter()
        .skip(scroll)
        .take(visible_height)
        .map(|l| Line::from(Span::raw(l.clone())))
        .collect();

    f.render_widget(
        Paragraph::new(visible)
            .block(block)
            .style(Style::default().fg(app.theme.output_fg)),
        content_area,
    );

    if popup.done {
        f.render_widget(
            Paragraph::new("(↑↓ / PgUp / PgDn) Scroll    (Enter / Esc) Close")
                .block(
                    UIBlock::default()
                        .borders(Borders::TOP)
                        .style(Style::default().bg(app.theme.terminal_bg)),
                )
                .style(Style::default().fg(app.theme.system_fg)),
            layout[1],
        );
    }
}
