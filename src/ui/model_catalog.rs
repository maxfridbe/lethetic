use crate::app::App;
use crate::icons;
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block as UIBlock, Borders, Clear, List, ListItem, Paragraph},
};

use super::centered_rect;

/// The model picker's "scan for more" view: a filter line over the
/// connection's full catalog.
pub(super) fn render(f: &mut ratatui::Frame, app: &mut App) {
    let theme = app.theme.clone();
    let Some(catalog) = app.model_catalog.as_mut() else {
        return;
    };
    let area = centered_rect(75, 75, f.area());
    f.render_widget(Clear, area);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);

    let filtered = catalog.filtered();
    let shown = filtered.len();
    let status = match (&catalog.models, &catalog.error) {
        (None, _) => "scanning…".to_string(),
        (Some(_), Some(error)) => format!("scan failed: {error}"),
        (Some(_), None) => format!("{shown} of {}", catalog.total()),
    };
    let filter = Paragraph::new(Line::from(vec![
        Span::styled("Filter: ", Style::default().fg(theme.system_fg)),
        Span::raw(catalog.filter.clone()),
        Span::styled("▏", Style::default().fg(theme.highlight_fg)),
        Span::styled(format!("   {status}"), Style::default().fg(theme.system_fg)),
    ]))
    .block(
        UIBlock::default()
            .title(format!(
                "{} Scan {} (type to filter · ↑↓ · Enter: add to picker · Esc: back)",
                icons::MODEL,
                catalog.connection_name
            ))
            .borders(Borders::ALL)
            .style(Style::default().bg(theme.terminal_bg)),
    );
    f.render_widget(filter, rows[0]);

    let items: Vec<ListItem> = filtered
        .iter()
        .map(|model| {
            if model.display_name.is_empty() || model.display_name == model.id {
                ListItem::new(model.id.clone())
            } else {
                ListItem::new(Line::from(vec![
                    Span::raw(model.id.clone()),
                    Span::styled(
                        format!("  {}", model.display_name),
                        Style::default().fg(theme.system_fg),
                    ),
                ]))
            }
        })
        .collect();
    drop(filtered);
    f.render_stateful_widget(
        List::new(items)
            .block(
                UIBlock::default()
                    .borders(Borders::ALL)
                    .style(Style::default().bg(theme.terminal_bg)),
            )
            .highlight_style(
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .fg(theme.highlight_fg),
            )
            .highlight_symbol("> "),
        rows[1],
        &mut catalog.list_state,
    );
}
