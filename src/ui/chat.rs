use crate::app::{App, RenderBlock};
use crate::icons;
use crate::theme::Theme;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block as UIBlock, Borders, List, ListItem, ListState, Paragraph, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Wrap,
    },
};

use super::block::render_block_to_lines_with_cost_visibility;

const PREFIX_LEN: u16 = 2;
const VIEWPORT_OVERSCAN: usize = 2;
const CACHE_MARGIN_SCREENS: usize = 3;

struct PreparedBlocks {
    line_counts: Vec<usize>,
    total_lines: usize,
    live_lines: Option<Vec<Line<'static>>>,
}

#[derive(Clone, Copy)]
struct Viewport {
    selected_line: usize,
    start_line: usize,
    end_line: usize,
}

pub(super) fn render_output(f: &mut ratatui::Frame, app: &mut App, area: Rect) {
    let title = output_title(app);
    let terminal_width = area.width.saturating_sub(2) as usize;
    let terminal_height = area.height.saturating_sub(2) as usize;
    let include_estimated_cost = app.config.estimate_cost.unwrap_or(true);
    invalidate_blocks_for_render_policy(app, terminal_width, include_estimated_cost);

    let prepared = prepare_blocks(app, terminal_width, include_estimated_cost);
    app.total_line_count = prepared.total_lines;
    let viewport = select_viewport(app, prepared.total_lines, terminal_height);
    let list_items = collect_visible_items(
        app,
        &prepared.line_counts,
        prepared.live_lines,
        viewport,
        terminal_width,
        include_estimated_cost,
    );
    evict_offscreen_caches(app, &prepared.line_counts, viewport, terminal_height);
    app.last_output_rect = area;
    app.last_block_line_counts = prepared.line_counts;
    app.last_start_line = viewport.start_line;
    render_output_widgets(f, app, area, title, list_items, viewport, terminal_height);
}

/// Absolute line indices of the header row of every visible block, used for
/// mouse hit-testing of the copy badge.
pub fn block_header_at_line(line_counts: &[usize], abs_line: usize) -> Option<usize> {
    let mut cumulative = 0usize;
    for (index, &count) in line_counts.iter().enumerate() {
        if count == 0 {
            continue;
        }
        if abs_line < cumulative + count {
            return (abs_line == cumulative).then_some(index);
        }
        cumulative += count;
    }
    None
}

fn output_title(app: &App) -> String {
    let base_title = if app.show_approval_prompt {
        format!("{} Approval Required", icons::WARNING)
    } else if app.is_executing_tool {
        format!(
            "{} {} Executing Tool...",
            icons::TOOL_SPINNER[app.tool_spinner_index],
            icons::COMMAND
        )
    } else {
        format!("{} Output", icons::OUTPUT)
    };
    format!("{base_title} · {}", app.current_session_label())
}

fn invalidate_blocks_for_render_policy(
    app: &mut App,
    terminal_width: usize,
    include_estimated_cost: bool,
) {
    if terminal_width == app.last_rendered_width
        && app.last_rendered_cost_visibility == Some(include_estimated_cost)
    {
        return;
    }
    for block in &mut app.blocks {
        block.invalidate();
    }
    app.last_rendered_width = terminal_width;
    app.last_rendered_cost_visibility = Some(include_estimated_cost);
}

fn prepare_blocks(
    app: &mut App,
    terminal_width: usize,
    include_estimated_cost: bool,
) -> PreparedBlocks {
    let num_blocks = app.blocks.len();
    let live_index = (app.is_executing_tool || app.is_processing)
        .then(|| num_blocks.checked_sub(1))
        .flatten();
    let mut line_counts = Vec::with_capacity(num_blocks);
    let mut total_lines = 0;
    let mut live_lines = None;

    let hide_thinking = app.hide_thinking;
    for (index, block) in app.blocks.iter_mut().enumerate() {
        if hide_thinking
            && matches!(
                block.block_type,
                crate::app::BlockType::Thought | crate::app::BlockType::Formulating
            )
        {
            line_counts.push(0);
            continue;
        }
        let is_live = live_index == Some(index);
        let (count, rendered_live_lines) = prepare_block(
            block,
            terminal_width,
            &app.theme,
            is_live,
            app.is_executing_tool
                .then_some(app.tool_output_preview.as_str()),
            include_estimated_cost,
        );
        line_counts.push(count);
        total_lines += count;
        if rendered_live_lines.is_some() {
            live_lines = rendered_live_lines;
        }
    }
    PreparedBlocks {
        line_counts,
        total_lines,
        live_lines,
    }
}

fn prepare_block(
    block: &mut RenderBlock,
    terminal_width: usize,
    theme: &Theme,
    is_live: bool,
    tool_preview: Option<&str>,
    include_estimated_cost: bool,
) -> (usize, Option<Vec<Line<'static>>>) {
    if is_live {
        let rendered = render_block_to_lines_with_cost_visibility(
            block,
            terminal_width,
            theme,
            tool_preview,
            include_estimated_cost,
        );
        return (rendered.len(), Some(rendered));
    }
    if let Some(cached) = &block.cached_lines {
        return (cached.len(), None);
    }
    if let Some(count) = block.cached_line_count {
        return (count, None);
    }
    let rendered = render_block_to_lines_with_cost_visibility(
        block,
        terminal_width,
        theme,
        None,
        include_estimated_cost,
    );
    let count = rendered.len();
    block.cached_line_count = Some(count);
    block.cached_lines = Some(rendered);
    (count, None)
}

fn select_viewport(app: &mut App, total_lines: usize, terminal_height: usize) -> Viewport {
    let mut selected_line = app.output_state.selected().unwrap_or(0);
    if app.auto_scroll && total_lines > 0 {
        selected_line = total_lines.saturating_sub(1);
        app.output_state.select(Some(selected_line));
    }
    let half_height = terminal_height / 2;
    let mut start_line = selected_line.saturating_sub(half_height);
    if start_line + terminal_height > total_lines {
        start_line = total_lines.saturating_sub(terminal_height);
    }
    let end_line = (start_line + terminal_height + VIEWPORT_OVERSCAN).min(total_lines);
    Viewport {
        selected_line,
        start_line,
        end_line,
    }
}

fn collect_visible_items(
    app: &mut App,
    line_counts: &[usize],
    mut live_lines: Option<Vec<Line<'static>>>,
    viewport: Viewport,
    terminal_width: usize,
    include_estimated_cost: bool,
) -> Vec<ListItem<'static>> {
    let num_blocks = line_counts.len();
    let mut list_items = Vec::new();
    let mut block_start = 0;
    for (block_index, count) in line_counts.iter().copied().enumerate() {
        let block_end = block_start + count;
        if block_end <= viewport.start_line || block_start >= viewport.end_line {
            block_start = block_end;
            continue;
        }

        let is_last = block_index == num_blocks.saturating_sub(1);
        let live = is_last.then(|| live_lines.take()).flatten();
        ensure_visible_block_cache(
            app,
            block_index,
            terminal_width,
            live.is_some(),
            include_estimated_cost,
        );
        let lines =
            visible_block_lines(live.as_ref(), app.blocks[block_index].cached_lines.as_ref());
        let from = viewport
            .start_line
            .saturating_sub(block_start)
            .min(lines.len());
        let to = (viewport.end_line - block_start).min(lines.len());
        list_items.extend(lines[from..to].iter().cloned().map(ListItem::new));
        block_start = block_end;
    }
    list_items
}

fn ensure_visible_block_cache(
    app: &mut App,
    block_index: usize,
    terminal_width: usize,
    has_live_lines: bool,
    include_estimated_cost: bool,
) {
    if has_live_lines || app.blocks[block_index].cached_lines.is_some() {
        return;
    }
    let rendered = render_block_to_lines_with_cost_visibility(
        &app.blocks[block_index],
        terminal_width,
        &app.theme,
        None,
        include_estimated_cost,
    );
    app.blocks[block_index].cached_line_count = Some(rendered.len());
    app.blocks[block_index].cached_lines = Some(rendered);
}

fn visible_block_lines<'a>(
    live: Option<&'a Vec<Line<'static>>>,
    cached: Option<&'a Vec<Line<'static>>>,
) -> &'a [Line<'static>] {
    if let Some(live) = live {
        return live;
    }
    cached.map(Vec::as_slice).unwrap_or_default()
}

fn evict_offscreen_caches(
    app: &mut App,
    line_counts: &[usize],
    viewport: Viewport,
    terminal_height: usize,
) {
    let margin = terminal_height * CACHE_MARGIN_SCREENS;
    let keep_from = viewport.start_line.saturating_sub(margin);
    let keep_to = viewport.end_line + margin;
    let last_index = line_counts.len().saturating_sub(1);
    let mut block_start = 0;
    for (block_index, count) in line_counts.iter().copied().enumerate() {
        let block_end = block_start + count;
        let outside_margin = block_end <= keep_from || block_start >= keep_to;
        if block_index != last_index && outside_margin {
            let block = &mut app.blocks[block_index];
            if block.cached_lines.is_some() {
                block.cached_line_count = Some(count);
                block.cached_lines = None;
            }
        }
        block_start = block_end;
    }
}

fn render_output_widgets(
    f: &mut ratatui::Frame,
    app: &App,
    area: Rect,
    title: String,
    list_items: Vec<ListItem<'static>>,
    viewport: Viewport,
    terminal_height: usize,
) {
    let mut virtual_state = ListState::default();
    virtual_state.select(Some(
        viewport.selected_line.saturating_sub(viewport.start_line),
    ));
    let output_block = UIBlock::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(if app.is_output_focused {
            Style::default().fg(app.theme.highlight_fg)
        } else {
            Style::default()
        });
    f.render_stateful_widget(
        List::new(list_items).block(output_block),
        area,
        &mut virtual_state,
    );
    f.render_stateful_widget(
        Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓")),
        area,
        &mut ScrollbarState::new(app.total_line_count.saturating_sub(terminal_height))
            .position(viewport.start_line),
    );
}

pub(super) fn render_input(f: &mut ratatui::Frame, app: &App, area: Rect, inner_width: u16) {
    let input_style = Style::default()
        .bg(app.theme.input_bg)
        .fg(app.theme.input_fg);
    let input_title = if app.is_asking_user {
        format!("{} Waiting for your answer...", icons::WARNING)
    } else {
        format!("{} Input", icons::INPUT)
    };
    let input_block = UIBlock::default()
        .title(input_title)
        .title_top(
            Line::from(Span::styled(
                " Ctrl+P: commands · F9: todos · F10: select text · F12: debug ",
                Style::default().fg(app.theme.system_fg),
            ))
            .right_aligned(),
        )
        .borders(Borders::ALL)
        .style(if !app.is_output_focused {
            input_style.fg(app.theme.highlight_fg)
        } else {
            input_style
        });
    let prefix = Span::styled(
        "> ",
        Style::default()
            .fg(app.theme.highlight_fg)
            .add_modifier(Modifier::BOLD),
    );
    let input_text = Line::from(vec![prefix, Span::raw(&app.input)]);
    f.render_widget(
        Paragraph::new(input_text)
            .block(input_block)
            .wrap(Wrap { trim: false }),
        area,
    );
    if !app.is_output_focused {
        let cursor_x = area.x + 1 + PREFIX_LEN + (app.cursor_pos as u16 % inner_width.max(1));
        let cursor_y = area.y + 1 + (app.cursor_pos as u16 / inner_width.max(1));
        f.set_cursor_position((cursor_x, cursor_y));
    }
}
