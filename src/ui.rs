use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block as UIBlock, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap, Scrollbar, ScrollbarOrientation, ScrollbarState},
};
use crate::app::{App, BlockType, RenderBlock};
use crate::icons;
use crate::markdown;

fn render_json_highlighted(json_val: &serde_json::Value) -> Text<'static> {
    let pretty = match serde_json::to_string_pretty(json_val) {
        Ok(s) => s,
        Err(_) => format!("{:?}", json_val),
    };

    let mut lines = Vec::new();
    for line in pretty.lines() {
        let mut spans = Vec::new();
        let trimmed = line.trim_start();
        let indent = &line[..line.len() - trimmed.len()];
        
        if !indent.is_empty() {
            spans.push(Span::raw(indent.to_string()));
        }

        if trimmed.starts_with('"') {
            if let Some(colon_pos) = trimmed.find(':') {
                // It's a key
                let key = &trimmed[..colon_pos];
                let rest = &trimmed[colon_pos..];
                spans.push(Span::styled(key.to_string(), Style::default().fg(Color::Cyan)));
                
                // Colorize the value part
                let value_part = rest.trim_start_matches(':').trim();
                spans.push(Span::raw(": "));
                if value_part.starts_with('"') {
                    spans.push(Span::styled(value_part.to_string(), Style::default().fg(Color::Yellow)));
                } else if value_part == "true" || value_part == "false" || value_part == "null" {
                    spans.push(Span::styled(value_part.to_string(), Style::default().fg(Color::LightRed)));
                } else if value_part.chars().next().map_or(false, |c| c.is_ascii_digit() || c == '-') {
                    spans.push(Span::styled(value_part.to_string(), Style::default().fg(Color::LightMagenta)));
                } else {
                    spans.push(Span::raw(value_part.to_string()));
                }
            } else {
                // Just a string (maybe in an array)
                spans.push(Span::styled(trimmed.to_string(), Style::default().fg(Color::Yellow)));
            }
        } else if trimmed == "{" || trimmed == "}" || trimmed == "[" || trimmed == "]" || trimmed == "}," || trimmed == "]," {
            spans.push(Span::styled(trimmed.to_string(), Style::default().fg(Color::Gray)));
        } else {
            spans.push(Span::raw(trimmed.to_string()));
        }
        lines.push(Line::from(spans));
    }
    Text::from(lines)
}

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
    
    let input_height = (((app.input.len() + prefix_len) as u16 / inner_width.max(1)) + 3).min(10);
    
    let left_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(input_height),
            Constraint::Length(2),
        ].as_ref())
        .split(main_layout[0]);

    let title = if app.show_approval_prompt { format!("{} Approval Required", icons::WARNING) } 
                else if app.is_executing_tool { format!("{} {} Executing Tool...", icons::TOOL_SPINNER[app.tool_spinner_index], icons::COMMAND) }
                else if app.is_processing { format!("{} {} Lethetic Intelligence Engine Processing...", icons::SPINNER[app.spinner_index], icons::PROCESSING) } 
                else { format!("{} Output", icons::OUTPUT) };

    let terminal_width = left_layout[0].width.saturating_sub(2) as usize;
    let terminal_height = left_layout[0].height.saturating_sub(2) as usize;
    
    if terminal_width != app.last_rendered_width {
        for block in &mut app.blocks {
            block.cached_lines = None;
        }
        app.last_rendered_width = terminal_width;
    }

    // --- VIRTUALIZATION LOGIC START ---
    // Instead of rendering all lines, we first count them and then only render the ones that would be visible.
    let mut total_lines = 0;
    let mut block_line_counts = Vec::with_capacity(app.blocks.len());

    for block in &mut app.blocks {
        let count = if let Some(ref cached) = block.cached_lines {
            cached.len()
        } else {
            let rendered = render_block_to_lines(block, terminal_width, &app.theme);
            let len = rendered.len();
            block.cached_lines = Some(rendered);
            len
        };
        block_line_counts.push(count);
        total_lines += count;
    }
    app.total_line_count = total_lines;

    let mut selected_line = app.output_state.selected().unwrap_or(0);
    if app.auto_scroll && total_lines > 0 {
        selected_line = total_lines.saturating_sub(1);
        app.output_state.select(Some(selected_line));
    }
    
    // Calculate the window of lines to actually create ListItems for.
    let half_height = terminal_height / 2;
    let mut start_line = selected_line.saturating_sub(half_height);
    
    // Ensure we fill the terminal as much as possible if we are near the end
    if start_line + terminal_height > total_lines {
        start_line = total_lines.saturating_sub(terminal_height);
    }
    
    let end_line = (start_line + terminal_height + 2).min(total_lines);

    let mut list_items = Vec::new();
    let mut current_line_idx = 0;

    for (block_idx, count) in block_line_counts.iter().enumerate() {
        let block_end = current_line_idx + count;
        
        // If this block is within or partially within our visible window
        if block_end > start_line && current_line_idx < end_line {
            if let Some(ref lines) = app.blocks[block_idx].cached_lines {
                for (i, line) in lines.iter().enumerate() {
                    let absolute_idx = current_line_idx + i;
                    if absolute_idx >= start_line && absolute_idx < end_line {
                        let rendered_line = line.clone();
                        list_items.push(ListItem::new(rendered_line));
                    }
                }
            }
        }
        current_line_idx = block_end;
    }

    // Adjust the list state to point to the correct relative item in our virtualized list
    let mut virtual_state = ListState::default();
    let relative_selected = selected_line.saturating_sub(start_line);
    virtual_state.select(Some(relative_selected));

    let output_block = UIBlock::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(if app.is_output_focused { Style::default().fg(app.theme.highlight_fg) } else { Style::default() });

    f.render_stateful_widget(
        List::new(list_items).block(output_block),
        left_layout[0],
        &mut virtual_state
    );

    // Render Scrollbar
    f.render_stateful_widget(
        Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓")),
        left_layout[0],
        &mut ScrollbarState::new(total_lines.saturating_sub(terminal_height)).position(start_line),
    );
    // --- VIRTUALIZATION LOGIC END ---
    
    let input_style = Style::default().bg(Color::Rgb(20, 20, 30)).fg(app.theme.input_fg);
    let input_title = if app.is_asking_user {
        format!("{} Waiting for your answer...", icons::WARNING)
    } else {
        format!("{} Input", icons::INPUT)
    };
    let input_block = UIBlock::default()
        .title(input_title)
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

    if app.show_session_manager {
        let area = centered_rect(80, 80, f.area());
        f.render_widget(Clear, area);
        let items: Vec<ListItem> = app.session_files.iter().map(|f| {
            let name = std::path::Path::new(f).file_name().unwrap_or_default().to_string_lossy();
            ListItem::new(name.to_string())
        }).collect();
        
        let block = UIBlock::default()
            .title(format!("{} Session Manager", icons::COMMAND))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        
        let inner_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(3)])
            .split(area);

        f.render_stateful_widget(
            List::new(items)
                .block(block)
                .highlight_style(Style::default().add_modifier(Modifier::BOLD).fg(app.theme.highlight_fg))
                .highlight_symbol("> "),
            inner_layout[0],
            &mut app.session_list_state
        );
        
        let help_text = "(Enter) Resume | (N) New | (D) Delete | (X) Wipe All | (Esc) Close";
        f.render_widget(Paragraph::new(help_text).block(UIBlock::default().borders(Borders::TOP)).style(Style::default().fg(Color::DarkGray)), inner_layout[1]);
    }

    if app.show_approval_prompt {
        let area = centered_rect(70, 60, f.area());
        f.render_widget(Clear, area);
        if let Some(tc) = &app.pending_tool_call {
            let json_text = render_json_highlighted(&tc.function.arguments);

            let mut display_text = Text::from(vec![
                Line::from(vec![
                    Span::styled("Tool: ", Style::default().add_modifier(Modifier::BOLD)),
                    Span::styled(&tc.function.name, Style::default().fg(Color::LightGreen)),
                ]),
                Line::from("Params:"),
            ]);

            // Add JSON lines with possible truncation
            let max_lines = 15;
            let mut lines_added = 0;
            for line in json_text.lines {
                if lines_added >= max_lines {
                    display_text.lines.push(Line::from(Span::styled("... [Truncated for display]", Style::default().fg(Color::DarkGray))));
                    break;
                }
                display_text.lines.push(line);
                lines_added += 1;
            }

            display_text.lines.push(Line::from(""));
            display_text.lines.push(Line::from(vec![
                Span::styled("(A)", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                Span::raw("lways Allow | "),
                Span::styled("(O)", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                Span::raw("nce | "),
                Span::styled("(D)", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                Span::raw("eny"),
            ]));

            let block = UIBlock::default()
                .title(format!("{} Security Confirmation", icons::WARNING))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow));

            f.render_widget(Paragraph::new(display_text).block(block).wrap(Wrap { trim: false }), area);
        } else {
            app.show_approval_prompt = false;
        }
    }
    if app.show_prompt_editor {
        let full_area = centered_rect(80, 80, f.area());
        f.render_widget(Clear, full_area);
        
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(0)].as_ref())
            .split(full_area);

        let header_block = UIBlock::default()
            .title(format!("{} System Prompt Editor", icons::MODEL))
            .borders(Borders::ALL)
            .style(if app.is_editing_prompt { Style::default().fg(Color::Yellow) } else { Style::default() });
        
        let instructions = if app.is_editing_prompt { 
            format!("EDITING MODE | Cursor: {} | (ESC) Finish", app.prompt_cursor_pos) 
        } else { 
            "(M)odify | (S)ave & Apply | (UP/DN) Scroll | (ESC) Close".to_string() 
        };
        f.render_widget(Paragraph::new(instructions).block(header_block), chunks[0]);

        let editor_block = UIBlock::default()
            .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM);
        
        let mut display_spans = Vec::new();
        if app.is_editing_prompt {
            let mut current_pos = 0;
            let mut cursor_seen = false;
            
            for c in app.system_prompt.chars() {
                if current_pos == app.prompt_cursor_pos {
                    display_spans.push(Span::styled(c.to_string(), Style::default().add_modifier(Modifier::REVERSED).fg(Color::Yellow)));
                    cursor_seen = true;
                } else {
                    display_spans.push(Span::raw(c.to_string()));
                }
                current_pos += c.len_utf8();
            }
            
            if !cursor_seen {
                display_spans.push(Span::styled("█", Style::default().fg(Color::Yellow)));
            }
        } else {
            display_spans.push(Span::raw(app.system_prompt.clone()));
        }
        
        f.render_widget(
            Paragraph::new(Line::from(display_spans))
                .block(editor_block)
                .wrap(Wrap { trim: false })
                .scroll((app.prompt_scroll as u16, 0)), 
            chunks[1]
        );
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
            Line::from(vec![Span::raw("  CTRL + L  : Clear UI (Keep Context)")]),
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

    let (bg_color, mut header) = match block.block_type {
        BlockType::User => (Color::Rgb(30, 35, 45), Some(format!("{} User Request", icons::INPUT))),
        BlockType::Thought => (Color::Rgb(25, 45, 45), Some(format!("{} Engine Thinking...", icons::PROCESSING))),
        BlockType::Formulating => (Color::Rgb(45, 35, 25), Some(format!("{} Formulating tool request...", icons::SPINNER[0]))),
        BlockType::ToolCall => (Color::Rgb(45, 45, 30), Some(format!("{} Engine Tool Request", icons::COMMAND))),
        BlockType::ToolResult => (Color::Rgb(35, 35, 35), Some(format!("{} Tool Output", icons::SUCCESS))),
        BlockType::Divider => (Color::Reset, None),
        _ => (Color::Reset, None),
    };

    if let Some(ref t) = block.title {
        header = match block.block_type {
            BlockType::ToolCall => Some(format!("{} {}", icons::COMMAND, t)),
            BlockType::ToolResult => Some(format!("{} {}\n, Tool Output:", icons::SUCCESS, t)),
            _ => Some(t.clone()),
        };
    }

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

    let content_lines = wrap_lines(content_lines, width.saturating_sub(2));

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

    let is_final = block.block_type == BlockType::User || block.block_type == BlockType::Divider;
    if is_final {
        lines.push(Line::from(vec![status_block, Span::styled(" ".repeat(width.saturating_sub(2)), base_style)]));
    }

    lines
}

fn wrap_lines(lines: Vec<Line<'static>>, max_width: usize) -> Vec<Line<'static>> {
    if max_width == 0 { return lines; }
    let mut wrapped_lines = Vec::new();
    for line in lines {
        let mut current_line_spans = Vec::new();
        let mut current_width = 0;

        for span in line.spans {
            let text = span.content.to_string();
            let style = span.style;
            
            // Split into words, preserving spaces
            let mut words = Vec::new();
            let mut current_word = String::new();
            for c in text.chars() {
                current_word.push(c);
                if c.is_whitespace() {
                    words.push(current_word);
                    current_word = String::new();
                }
            }
            if !current_word.is_empty() {
                words.push(current_word);
            }

            for word in words {
                let word_width = word.chars().count();
                
                if current_width + word_width <= max_width {
                    current_line_spans.push(Span::styled(word, style));
                    current_width += word_width;
                } else {
                    if !current_line_spans.is_empty() {
                        wrapped_lines.push(Line::from(std::mem::take(&mut current_line_spans)));
                    }
                    
                    if word_width > max_width {
                        // Word is longer than line, split it
                        let mut remaining = word;
                        while remaining.chars().count() > max_width {
                            let head: String = remaining.chars().take(max_width).collect();
                            let tail: String = remaining.chars().skip(max_width).collect();
                            wrapped_lines.push(Line::from(Span::styled(head, style)));
                            remaining = tail;
                        }
                        current_line_spans.push(Span::styled(remaining.clone(), style));
                        current_width = remaining.chars().count();
                    } else {
                        current_line_spans.push(Span::styled(word, style));
                        current_width = word_width;
                    }
                }
            }
        }
        if !current_line_spans.is_empty() {
            wrapped_lines.push(Line::from(current_line_spans));
        }
    }
    wrapped_lines
}

pub fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default().direction(Direction::Vertical).constraints([Constraint::Percentage((100 - percent_y) / 2), Constraint::Percentage(percent_y), Constraint::Percentage((100 - percent_y) / 2)].as_ref()).split(r);
    Layout::default().direction(Direction::Horizontal).constraints([Constraint::Percentage((100 - percent_x) / 2), Constraint::Percentage(percent_x), Constraint::Percentage((100 - percent_x) / 2)].as_ref()).split(popup_layout[1])[1]
}
