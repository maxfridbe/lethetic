use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block as UIBlock, Borders, Clear, List, ListItem, Paragraph, Wrap},
};
use crate::app::{App, BlockType, RenderBlock};
use crate::icons;
use crate::markdown;

#[derive(Clone, PartialEq, Debug)]
pub struct Theme {
    pub name: String,
    pub output_fg: Color,
    pub input_fg: Color,
    pub highlight_fg: Color,
}

impl Theme {
    pub fn default() -> Self {
        Self {
            name: "Default".to_string(),
            output_fg: Color::Green,
            input_fg: Color::Yellow,
            highlight_fg: Color::Cyan,
        }
    }

    pub fn all() -> Vec<Self> {
        vec![
            Self::default(),
            Self { name: "Matrix".to_string(), output_fg: Color::Green, input_fg: Color::LightGreen, highlight_fg: Color::White },
            Self { name: "Cyberpunk".to_string(), output_fg: Color::Magenta, input_fg: Color::LightCyan, highlight_fg: Color::LightMagenta },
            Self { name: "Ocean".to_string(), output_fg: Color::Blue, input_fg: Color::Cyan, highlight_fg: Color::White },
            Self { name: "Sunset".to_string(), output_fg: Color::Red, input_fg: Color::LightYellow, highlight_fg: Color::Yellow },
            Self { name: "Forest".to_string(), output_fg: Color::DarkGray, input_fg: Color::Green, highlight_fg: Color::LightGreen },
            Self { name: "Lavender".to_string(), output_fg: Color::Magenta, input_fg: Color::White, highlight_fg: Color::LightMagenta },
            Self { name: "Mono".to_string(), output_fg: Color::White, input_fg: Color::Gray, highlight_fg: Color::DarkGray },
            Self { name: "Gold".to_string(), output_fg: Color::Yellow, input_fg: Color::White, highlight_fg: Color::LightYellow },
            Self { name: "Deep Sea".to_string(), output_fg: Color::DarkGray, input_fg: Color::Blue, highlight_fg: Color::LightBlue },
        ]
    }
}

pub fn ui(f: &mut ratatui::Frame, app: &mut App) {
    let main_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(if app.show_debug { [Constraint::Percentage(50), Constraint::Percentage(50)] } else { [Constraint::Percentage(100), Constraint::Min(0)] }.as_ref())
        .split(f.area());

    let inner_width = main_layout[0].width.saturating_sub(4);
    let prefix_len = 2; // "> "
    
    let left_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(((app.input.len() + prefix_len) as u16 / inner_width.max(1)) + 3),
            Constraint::Length(2),
        ].as_ref())
        .split(main_layout[0]);

    let title = if app.show_approval_prompt { format!("{} Approval Required", icons::WARNING) } 
                else if app.is_processing { format!("{} {} Lethetic Intelligence Engine Processing...", icons::SPINNER[app.spinner_index], icons::PROCESSING) } 
                else { format!("{} Output", icons::OUTPUT) };

    let terminal_width = left_layout[0].width.saturating_sub(2) as usize;
    
    if terminal_width != app.last_rendered_width {
        for block in &mut app.blocks {
            block.cached_lines = None;
        }
        app.last_rendered_width = terminal_width;
    }

    let mut list_items = Vec::new();
    let mut total_lines = 0;

    for block in app.blocks.iter_mut() {
        let lines = if let Some(ref cached) = block.cached_lines {
            cached.clone()
        } else {
            let rendered = render_block_to_lines(block, terminal_width, &app.theme);
            block.cached_lines = Some(rendered.clone());
            rendered
        };
        
        for mut line in lines {
            if app.output_state.selected() == Some(total_lines) && app.is_output_focused {
                for span in line.spans.iter_mut() {
                    span.style = span.style.add_modifier(Modifier::REVERSED);
                }
            }
            list_items.push(ListItem::new(line));
            total_lines += 1;
        }
    }
    app.total_line_count = total_lines;

    let output_block = UIBlock::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(if app.is_output_focused { Style::default().fg(app.theme.highlight_fg) } else { Style::default() });

    f.render_stateful_widget(
        List::new(list_items).block(output_block),
        left_layout[0],
        &mut app.output_state
    );
    
    let input_style = Style::default().bg(Color::Rgb(20, 20, 30)).fg(app.theme.input_fg);
    let input_block = UIBlock::default()
        .title(format!("{} Input", icons::INPUT))
        .borders(Borders::ALL)
        .style(if !app.is_output_focused { input_style.fg(app.theme.highlight_fg) } else { input_style });
    
    let prefix = Span::styled("> ", Style::default().fg(app.theme.highlight_fg).add_modifier(Modifier::BOLD));
    let input_text = Line::from(vec![prefix, Span::raw(&app.input)]);
    f.render_widget(Paragraph::new(input_text).block(input_block).wrap(Wrap { trim: false }), left_layout[1]);

    if !app.is_output_focused {
        let cursor_x = left_layout[1].x + 1 + prefix_len as u16 + (app.cursor_pos as u16 % inner_width.max(1));
        let cursor_y = left_layout[1].y + 1 + (app.cursor_pos as u16 / inner_width.max(1));
        f.set_cursor_position((cursor_x, cursor_y));
    }

    let status_text = vec![
        Line::from(vec![
            Span::styled(format!("{} T/s: ", icons::TOKENS), Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{:.2} ", app.tokens_per_s), Style::default().fg(Color::Cyan)),
            Span::styled(format!("| {} Model: ", icons::MODEL), Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{} ", app.model_name), Style::default().fg(Color::Green)),
            Span::styled(format!("| {} Server: ", icons::SERVER), Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{} ", app.server_url), Style::default().fg(Color::Yellow)),
            Span::styled(format!("| {} Context: ", icons::TOKENS), Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{}/{} ", app.context_manager.get_token_count(), app.max_tokens), Style::default().fg(Color::Cyan)),
            Span::styled("| Mem: ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{}MB ", app.memory_usage), Style::default().fg(Color::Magenta)),
        ]),
        Line::from(vec![
            Span::styled(format!("{} Path: ", icons::PATH), Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{} ", app.current_dir), Style::default().fg(Color::LightBlue)),
            Span::styled(format!("| {} Git: ", icons::GIT), Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{} ", app.git_status), Style::default().fg(if app.git_status.contains("dirty") { Color::Red } else { Color::Green })),
            Span::styled(format!(" | (TAB/F10) Mouse: {} ", if app.mouse_enabled { "ON" } else { "OFF" }), Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC)),
        ]),
    ];
    f.render_widget(Paragraph::new(status_text).wrap(Wrap { trim: true }), left_layout[2]);

    if app.show_debug {
        let items: Vec<ListItem> = app.debug_log.iter().rev().take(50).map(|s| ListItem::new(s.as_str())).collect();
        f.render_widget(List::new(items).block(UIBlock::default().title(format!("{} Debugger", icons::DEBUG)).borders(Borders::ALL)).style(Style::default().fg(Color::Gray)), main_layout[1]);
    }

    if app.show_palette {
        let area = centered_rect(60, 25, f.area());
        f.render_widget(Clear, area);
        let items: Vec<ListItem> = app.palette_items.iter().map(|i| ListItem::new(i.as_str())).collect();
        f.render_stateful_widget(List::new(items).block(UIBlock::default().title(format!("{} Command Palette", icons::COMMAND)).borders(Borders::ALL)).highlight_style(Style::default().add_modifier(Modifier::BOLD).fg(app.theme.highlight_fg)).highlight_symbol("> "), area, &mut app.palette_state);
    }

    if app.show_theme_menu {
        let area = centered_rect(60, 60, f.area());
        f.render_widget(Clear, area);
        let items: Vec<ListItem> = app.themes.iter().map(|t| ListItem::new(t.name.as_str())).collect();
        f.render_stateful_widget(List::new(items).block(UIBlock::default().title(format!("{} Themes", icons::THEME)).borders(Borders::ALL)).highlight_style(Style::default().add_modifier(Modifier::BOLD).fg(app.theme.highlight_fg)).highlight_symbol("> "), area, &mut app.theme_state);
    }

    if app.show_approval_prompt {
        let area = centered_rect(70, 50, f.area());
        f.render_widget(Clear, area);
        if let Some(tc) = &app.pending_tool_call {
            let args_str = format!("{}", tc.function.arguments);
            let display_args = if args_str.len() > 500 {
                format!("{}... [Truncated for display]", &args_str[..500])
            } else {
                args_str
            };
            
            let text = format!("Tool: {}\nParams:\n{}\n\n(A)lways Allow | (O)nce | (D)eny", tc.function.name, display_args);
            f.render_widget(Paragraph::new(text).block(UIBlock::default().title(format!("{} Security Confirmation", icons::WARNING)).borders(Borders::ALL)).style(Style::default().fg(Color::Red)).wrap(Wrap { trim: false }), area);
        } else {
            app.show_approval_prompt = false;
        }
    }

    if app.show_prompt_editor {
        let area = centered_rect(80, 80, f.area());
        f.render_widget(Clear, area);
        let editor_block = UIBlock::default()
            .title(format!("{} System Prompt Editor", icons::MODEL))
            .borders(Borders::ALL)
            .style(if app.is_editing_prompt { Style::default().fg(Color::Yellow) } else { Style::default() });
        
        let instructions = if app.is_editing_prompt { "EDITING MODE: (ESC) to finish" } else { "(M)odify | (S)ave & Apply | (ESC) Close" };
        let content = format!("{}\n\n---\n{}", instructions, app.system_prompt);
        
        f.render_widget(Paragraph::new(content).block(editor_block).wrap(Wrap { trim: false }), area);
    }

    if app.show_cleanup_prompt {
        let area = centered_rect(50, 20, f.area());
        f.render_widget(Clear, area);
        let text = format!("{} Found existing debug files in .lethetic/\nWould you like to clear them?\n\n(Y)es | (N)o", icons::DEBUG);
        f.render_widget(Paragraph::new(text).block(UIBlock::default().title("Startup Cleanup").borders(Borders::ALL)).style(Style::default().fg(Color::Yellow)), area);
    }

    if app.show_hotkeys {
        let area = centered_rect(70, 70, f.area());
        f.render_widget(Clear, area);
        let hotkeys_text = vec![
            Line::from(vec![Span::styled("Navigation", Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan))]),
            Line::from(vec![Span::raw("  TAB       : Toggle Focus between Input and Output")]),
            Line::from(vec![Span::raw("  UP / DOWN : Scroll Output (when focused)")]),
            Line::from(vec![Span::raw("  PGUP/PGDN : Fast Scroll Output")]),
            Line::from(vec![Span::raw("  ESC       : Open Command Palette / Stop Output")]),
            Line::from(vec![]),
            Line::from(vec![Span::styled("Global Toggles", Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan))]),
            Line::from(vec![Span::raw("  F12       : Toggle Debugger Pane")]),
            Line::from(vec![Span::raw("  F10       : Toggle Mouse (for terminal selection)")]),
            Line::from(vec![Span::raw("  CTRL + P  : Command Palette")]),
            Line::from(vec![]),
            Line::from(vec![Span::styled("General", Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan))]),
            Line::from(vec![Span::raw("  ENTER     : Send Prompt / Confirm Selection")]),
            Line::from(vec![Span::raw("  CTRL + C  : Stop Output (1st) / Quit (2nd)")]),
            Line::from(vec![]),
            Line::from(vec![Span::styled("Press ESC or ENTER to close", Style::default().add_modifier(Modifier::ITALIC).fg(Color::DarkGray))]),
        ];
        f.render_widget(Paragraph::new(hotkeys_text).block(UIBlock::default().title(format!("{} Hotkeys Reference", icons::COMMAND)).borders(Borders::ALL)).wrap(Wrap { trim: false }), area);
    }
}

fn render_block_to_lines(block: &RenderBlock, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let block_color = if block.block_type == BlockType::Thought {
        Color::Cyan
    } else {
        match block.success {
            Some(true) => Color::Green,
            Some(false) => Color::Red,
            None => Color::Gray,
        }
    };
    
    let status_block = Span::styled("█ ", Style::default().fg(block_color));

    let (bg_color, header) = match block.block_type {
        BlockType::User => (Color::Rgb(30, 35, 45), Some(format!("{} User Request", icons::INPUT))),
        BlockType::Thought => (Color::Rgb(25, 45, 45), Some(format!("{} Engine Thinking...", icons::PROCESSING))),
        BlockType::Formulating => (Color::Rgb(45, 35, 25), Some(format!("{} Formulating tool request...", icons::SPINNER[0]))),
        BlockType::ToolCall => (Color::Rgb(45, 45, 30), Some(format!("{} Engine Tool Request", icons::COMMAND))),
        BlockType::ToolResult => (Color::Rgb(35, 35, 35), Some(format!("{} Tool Output", icons::SUCCESS))),
        BlockType::Divider => (Color::Reset, None),
        _ => (Color::Reset, None),
    };

    let base_style = Style::default().bg(bg_color).fg(theme.output_fg);

    if block.block_type == BlockType::Divider {
        lines.push(Line::from(vec![
            Span::styled("─".repeat(width), Style::default().fg(Color::DarkGray))
        ]));
        return lines;
    }

    if let Some(h) = header {
        let mut header_spans = vec![
            status_block.clone(),
            Span::styled(format!(" {} ", h), base_style.add_modifier(Modifier::BOLD).fg(Color::White)),
        ];
        let current_len = 2 + h.len() + 2;
        if width > current_len {
            header_spans.push(Span::styled(" ".repeat(width - current_len), base_style));
        }
        lines.push(Line::from(header_spans));
    }

    // Advanced rendering for Markdown or specialized blocks
    let content_lines = if block.block_type == BlockType::Formulating {
        let lines: Vec<&str> = block.content.lines().collect();
        if lines.is_empty() {
            vec![Line::from(Span::styled("(Engine is preparing the tool payload...)", base_style.add_modifier(Modifier::ITALIC)))]
        } else {
            let last_lines = if lines.len() > 5 {
                &lines[lines.len() - 5..]
            } else {
                &lines[..]
            };
            
            let mut formatted_lines = vec![Line::from(Span::styled("(Engine is preparing the tool payload...)", base_style.add_modifier(Modifier::ITALIC)))];
            for line in last_lines {
                formatted_lines.push(Line::from(Span::styled(format!("  {}", line), base_style.add_modifier(Modifier::DIM))));
            }
            formatted_lines
        }
    } else if block.block_type == BlockType::Markdown || block.content.contains("```") {
        markdown::render_markdown(&block.content, base_style).lines
    } else {
        block.content.lines().map(|l| Line::from(Span::styled(l.to_string(), base_style))).collect()
    };

    for mut line in content_lines {
        let mut spans = vec![status_block.clone()];
        
        for span in line.spans.iter_mut() {
            if block.block_type == BlockType::Thought {
                span.style = span.style.add_modifier(Modifier::ITALIC).fg(Color::Cyan);
            } else if block.block_type == BlockType::ToolCall {
                span.style = span.style.fg(Color::Yellow);
            }
        }
        
        spans.append(&mut line.spans);
        
        let current_len = 2 + line.width();
        if width > current_len {
            spans.push(Span::styled(" ".repeat(width - current_len), base_style));
        }
        lines.push(Line::from(spans));
    }

    let is_final = block.block_type == BlockType::User || block.block_type == BlockType::Divider || !block.content.ends_with(' ');
    if is_final {
        lines.push(Line::from(vec![status_block, Span::styled(" ".repeat(width.saturating_sub(2)), base_style)]));
    }

    lines
}

pub fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default().direction(Direction::Vertical).constraints([Constraint::Percentage((100 - percent_y) / 2), Constraint::Percentage(percent_y), Constraint::Percentage((100 - percent_y) / 2)].as_ref()).split(r);
    Layout::default().direction(Direction::Horizontal).constraints([Constraint::Percentage((100 - percent_x) / 2), Constraint::Percentage(percent_x), Constraint::Percentage((100 - percent_x) / 2)].as_ref()).split(popup_layout[1])[1]
}
