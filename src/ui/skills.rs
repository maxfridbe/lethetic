use crate::app::{App, SkillRow};
use crate::icons;
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block as UIBlock, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

use super::centered_rect;

/// The Skills menu (Ctrl+P → Skills).
pub(super) fn render(f: &mut ratatui::Frame, app: &App) {
    let Some(panel) = &app.skills_panel else {
        return;
    };
    let theme = &app.theme;
    let normal = Style::default().fg(theme.output_fg);
    let dim = Style::default().fg(theme.system_fg);
    let on = Style::default().fg(theme.success_fg);
    let highlight = Style::default()
        .fg(theme.highlight_fg)
        .add_modifier(Modifier::BOLD | Modifier::REVERSED);

    let area = centered_rect(80, 80, f.area());
    f.render_widget(Clear, area);
    let outer = UIBlock::default()
        .title(format!("{} Skills", icons::COMMAND))
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.terminal_bg))
        .border_style(Style::default().fg(theme.highlight_fg));
    let inner = outer.inner(area);
    f.render_widget(outer, area);
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(4)])
        .split(inner);

    let mut items: Vec<ListItem> = Vec::new();
    let mut selected_item = 0;
    let installed = panel.installed_count();
    items.push(ListItem::new(Line::from(Span::styled(
        format!("Found skills ({installed}) · Space/Enter turns one on or off"),
        dim.add_modifier(Modifier::BOLD),
    ))));
    if installed == 0 {
        items.push(ListItem::new(Line::from(Span::styled(
            "  none yet: install one below, or add a folder with SKILL.md to .lethetic/skills/",
            dim,
        ))));
    }
    for (index, row) in panel.rows.iter().enumerate() {
        if index == installed {
            items.push(ListItem::new(Line::from("")));
            items.push(ListItem::new(Line::from(Span::styled(
                "Catalog · github.com/anthropics/skills · Enter installs to ~/.config/lethetic/skills · c copies the link",
                dim.add_modifier(Modifier::BOLD),
            ))));
        }
        if index == panel.selected {
            selected_item = items.len();
        }
        let line = match row {
            SkillRow::Installed(skill) => Line::from(vec![
                Span::styled(
                    if skill.enabled { "  [on]  " } else { "  [off] " },
                    if skill.enabled { on } else { dim },
                ),
                Span::styled(format!("{:<18}", skill.name), normal.add_modifier(Modifier::BOLD)),
                Span::styled(format!("{:<28}", skill.source.label()), dim),
                Span::styled(skill.description.chars().take(90).collect::<String>(), normal),
            ]),
            SkillRow::Catalog { entry, installed } => {
                let status = if *installed {
                    "  installed "
                } else if panel.installing.as_deref() == Some(entry.name) {
                    "  installing"
                } else {
                    "  install   "
                };
                Line::from(vec![
                    Span::styled(status, if *installed { on } else { normal }),
                    Span::styled(format!(" {:<18}", entry.name), normal.add_modifier(Modifier::BOLD)),
                    Span::styled(entry.summary, normal),
                    Span::styled(
                        if entry.proprietary { "  (proprietary license)" } else { "" },
                        dim,
                    ),
                ])
            }
        };
        items.push(ListItem::new(line));
    }
    let mut state = ListState::default();
    state.select(Some(selected_item));
    f.render_stateful_widget(List::new(items).highlight_style(highlight), parts[0], &mut state);

    let detail = match panel.rows.get(panel.selected) {
        Some(SkillRow::Installed(skill)) => skill.dir.display().to_string(),
        Some(SkillRow::Catalog { entry, .. }) => entry.url(),
        None => String::new(),
    };
    let mut footer = vec![Line::from(Span::styled(detail, dim))];
    if let Some(message) = &panel.message {
        footer.push(Line::from(Span::styled(message.clone(), normal)));
    }
    footer.push(Line::from(Span::styled(
        "↑↓ move · Space/Enter toggle or install · c copy link · r rescan · Esc close",
        dim,
    )));
    f.render_widget(Paragraph::new(footer).wrap(Wrap { trim: true }), parts[1]);
}
