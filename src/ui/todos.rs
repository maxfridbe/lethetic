use crate::app::App;
use crate::todo_store::{TodoPriority, TodoStatus};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block as UIBlock, Borders, Paragraph, Wrap},
};

/// Right-hand pane listing the model's todo items (F9).
pub(super) fn render(f: &mut ratatui::Frame, app: &App, area: Rect) {
    if !app.show_todos || area.width == 0 || area.height == 0 {
        return;
    }
    let theme = &app.theme;
    let todos = &app.todos.todos;
    let active = todos
        .iter()
        .filter(|todo| matches!(todo.status, TodoStatus::Pending | TodoStatus::InProgress))
        .count();
    let mut lines: Vec<Line> = Vec::new();
    if todos.is_empty() {
        lines.push(Line::from(Span::styled(
            "No todos yet. They appear here when the model plans its work.",
            Style::default().fg(theme.system_fg),
        )));
    }
    for todo in todos {
        let (mark, style) = match todo.status {
            TodoStatus::InProgress => (
                "[~]",
                Style::default()
                    .fg(theme.highlight_fg)
                    .add_modifier(Modifier::BOLD),
            ),
            TodoStatus::Pending => ("[ ]", Style::default().fg(theme.output_fg)),
            TodoStatus::Completed => (
                "[x]",
                Style::default()
                    .fg(theme.system_fg)
                    .add_modifier(Modifier::CROSSED_OUT),
            ),
            TodoStatus::Cancelled => (
                "[-]",
                Style::default()
                    .fg(theme.system_fg)
                    .add_modifier(Modifier::CROSSED_OUT | Modifier::DIM),
            ),
        };
        let priority = match todo.priority {
            TodoPriority::High => Span::styled("! ", Style::default().fg(theme.error_fg)),
            TodoPriority::Medium => Span::raw("  "),
            TodoPriority::Low => Span::styled("· ", Style::default().fg(theme.system_fg)),
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{mark} "), style),
            priority,
            Span::styled(todo.content.clone(), style),
        ]));
    }
    let title = format!("󰄲 Todos · {active} active of {} (F9)", todos.len());
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

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crate::config::Config;
    use crate::todo_store::{TodoItem, TodoPriority, TodoSnapshot, TodoStatus};
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn pane_lists_todos_with_status_marks() {
        let mut app = App::new(&Config::default());
        app.show_session_manager = false;
        app.show_todos = true;
        app.todos = TodoSnapshot {
            revision: 2,
            todos: vec![
                TodoItem {
                    id: None,
                    content: "write the kernel".into(),
                    status: TodoStatus::InProgress,
                    priority: TodoPriority::High,
                },
                TodoItem {
                    id: None,
                    content: "add tests".into(),
                    status: TodoStatus::Pending,
                    priority: TodoPriority::Medium,
                },
                TodoItem {
                    id: None,
                    content: "scaffold crate".into(),
                    status: TodoStatus::Completed,
                    priority: TodoPriority::Low,
                },
            ],
        };
        let mut terminal = Terminal::new(TestBackend::new(140, 30)).unwrap();
        terminal
            .draw(|frame| crate::ui::ui(frame, &mut app))
            .unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(screen.contains("Todos · 2 active of 3"), "{screen}");
        assert!(screen.contains("[~] ! write the kernel"));
        assert!(screen.contains("[ ]   add tests"));
        assert!(screen.contains("[x] · scaffold crate"));
    }
}

#[cfg(test)]
mod hotkeys_tests {
    use crate::app::App;
    use crate::config::Config;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn f1_opens_the_full_hotkey_reference() {
        let mut app = App::new(&Config::default());
        app.show_session_manager = false;
        crate::app::handle_key(&mut app, KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
        assert!(app.show_hotkeys);
        let mut terminal = Terminal::new(TestBackend::new(150, 60)).unwrap();
        terminal
            .draw(|frame| crate::ui::ui(frame, &mut app))
            .unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        for needle in [
            "Esc Esc",
            "F9",
            "F10",
            "Ctrl+O",
            "Models: s",
            "Stop the running reply",
        ] {
            assert!(screen.contains(needle), "missing {needle}");
        }
    }
}
