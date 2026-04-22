use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block as UIBlock, Borders, Clear, List, ListItem, Paragraph, Wrap},
};
use crate::app::{App, BlockType};
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
                else if app.is_processing { format!("{} {} Lethetic Engine Processing...", icons::SPINNER[app.spinner_index], icons::PROCESSING) } 
                else { format!("{} Output", icons::OUTPUT) };

    let mut text = Text::default();
    let terminal_width = left_layout[0].width.saturating_sub(4);

    for block in &app.blocks {
        let base_style = match block.block_type {
            BlockType::User => Style::default().fg(app.theme.highlight_fg).add_modifier(Modifier::BOLD),
            BlockType::Thought => Style::default().fg(Color::Cyan).add_modifier(Modifier::ITALIC),
            BlockType::ToolCall => Style::default().fg(Color::Yellow).bg(Color::Rgb(40, 40, 60)),
            BlockType::ToolResult => Style::default().bg(Color::Rgb(30, 30, 30)),
            _ => Style::default().fg(app.theme.output_fg),
        };

        if block.block_type == BlockType::Markdown {
            let md_text = markdown::render_markdown(&block.content, Style::default().fg(app.theme.output_fg));
            for line in md_text.lines {
                text.lines.push(line);
            }
        } else {
            for line_content in block.content.lines() {
                let mut line_style = base_style;
                if block.block_type == BlockType::ToolResult {
                    if line_content.starts_with('+') { line_style = line_style.fg(Color::LightGreen); }
                    else if line_content.starts_with('-') { line_style = line_style.fg(Color::LightRed); }
                    else if line_content.starts_with("STDOUT:") || line_content.starts_with("STDERR:") { line_style = line_style.fg(Color::Cyan).add_modifier(Modifier::BOLD); }
                    else { line_style = line_style.fg(Color::Gray); }
                }
                
                let padded_line = format!("{:width$}", line_content, width = terminal_width as usize);
                text.lines.push(Line::from(Span::styled(padded_line, line_style)));
            }
        }
    }

    let area = left_layout[0];
    let widget_height = area.height.saturating_sub(2);
    let total_lines = text.lines.iter().map(|line| {
        let width = line.width() as u16;
        if width == 0 { 1 } else { (width.saturating_sub(1) / terminal_width.max(1)) + 1 }
    }).sum::<u16>();

    if app.auto_scroll {
        app.scroll = if total_lines > widget_height { total_lines - widget_height } else { 0 };
    } else if app.scroll >= total_lines.saturating_sub(widget_height) {
        app.auto_scroll = true;
    }

    f.render_widget(Paragraph::new(text).block(UIBlock::default().title(title).borders(Borders::ALL)).wrap(Wrap { trim: false }).scroll((app.scroll, 0)), left_layout[0]);
    
    let input_style = Style::default().bg(Color::Rgb(20, 20, 30)).fg(app.theme.input_fg);
    let input_block = UIBlock::default().title(format!("{} Input", icons::INPUT)).borders(Borders::ALL).style(input_style);
    let prefix = Span::styled("> ", Style::default().fg(app.theme.highlight_fg).add_modifier(Modifier::BOLD));
    let input_text = Line::from(vec![prefix, Span::raw(&app.input)]);
    f.render_widget(Paragraph::new(input_text).block(input_block).wrap(Wrap { trim: false }), left_layout[1]);

    let cursor_x = left_layout[1].x + 1 + prefix_len as u16 + (app.input.len() as u16 % inner_width.max(1));
    let cursor_y = left_layout[1].y + 1 + (app.input.len() as u16 / inner_width.max(1));
    f.set_cursor_position((cursor_x, cursor_y));

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
            Span::styled(format!("{} ", app.cwd), Style::default().fg(Color::LightBlue)),
            Span::styled(format!("| {} Git: ", icons::GIT), Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{} ", app.git_status), Style::default().fg(if app.git_status.contains("dirty") { Color::Red } else { Color::Green })),
        ]),
    ];
    f.render_widget(Paragraph::new(status_text).wrap(Wrap { trim: true }), left_layout[2]);

    if app.show_debug {
        let items: Vec<ListItem> = app.debug_log.iter().rev().take(50).map(|s| ListItem::new(s.as_str())).collect();
        f.render_widget(List::new(items).block(UIBlock::default().title(format!("{} Debugger", icons::DEBUG)).borders(Borders::ALL)).style(Style::default().fg(Color::Gray)), main_layout[1]);
    }

    if app.show_palette {
        let area = centered_rect(60, 20, f.area());
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
        let area = centered_rect(60, 40, f.area());
        f.render_widget(Clear, area);
        let tc = app.pending_tool_call.as_ref().unwrap();
        let text = format!("Tool: {}\nParams: {}\n\n(A)lways Allow | (O)nce | (D)eny", tc.function.name, tc.function.arguments);
        f.render_widget(Paragraph::new(text).block(UIBlock::default().title(format!("{} Security Confirmation", icons::WARNING)).borders(Borders::ALL)).style(Style::default().fg(Color::Red)), area);
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
}

pub fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default().direction(Direction::Vertical).constraints([Constraint::Percentage((100 - percent_y) / 2), Constraint::Percentage(percent_y), Constraint::Percentage((100 - percent_y) / 2)].as_ref()).split(r);
    Layout::default().direction(Direction::Horizontal).constraints([Constraint::Percentage((100 - percent_x) / 2), Constraint::Percentage(percent_x), Constraint::Percentage((100 - percent_x) / 2)].as_ref()).split(popup_layout[1])[1]
}
