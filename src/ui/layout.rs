use crate::app::App;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    widgets::Block as UIBlock,
};

#[derive(Clone, Copy, Debug)]
pub(super) struct UiAreas {
    pub(super) output: Rect,
    pub(super) processing: Rect,
    pub(super) input: Rect,
    pub(super) status: Rect,
    pub(super) debug: Rect,
    pub(super) inner_width: u16,
}

pub(super) fn render_background(f: &mut ratatui::Frame, app: &App) {
    f.render_widget(
        UIBlock::default().style(Style::default().bg(app.theme.terminal_bg)),
        f.area(),
    );
}

pub(super) fn calculate_areas(app: &App, area: Rect) -> UiAreas {
    let main_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(
            if app.show_debug {
                [Constraint::Percentage(50), Constraint::Percentage(50)]
            } else {
                [Constraint::Percentage(100), Constraint::Min(0)]
            }
            .as_ref(),
        )
        .split(area);

    let inner_width = main_layout[0].width.saturating_sub(4);
    let prefix_len = 2;
    let input_height = (((app.input.len() + prefix_len) as u16 / inner_width.max(1)) + 3).min(10);
    let left_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Min(0),
                Constraint::Length(1),
                Constraint::Length(input_height),
                Constraint::Length(2),
            ]
            .as_ref(),
        )
        .split(main_layout[0]);

    UiAreas {
        output: left_layout[0],
        processing: left_layout[1],
        input: left_layout[2],
        status: left_layout[3],
        debug: main_layout[1],
        inner_width,
    }
}

pub fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Percentage((100 - percent_y) / 2),
                Constraint::Percentage(percent_y),
                Constraint::Percentage((100 - percent_y) / 2),
            ]
            .as_ref(),
        )
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints(
            [
                Constraint::Percentage((100 - percent_x) / 2),
                Constraint::Percentage(percent_x),
                Constraint::Percentage((100 - percent_x) / 2),
            ]
            .as_ref(),
        )
        .split(popup_layout[1])[1]
}
