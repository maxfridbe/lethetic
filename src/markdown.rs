use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
};
use pulldown_cmark::{Event, Parser, Tag, CodeBlockKind, TagEnd, Options, HeadingLevel};
use syntect::easy::HighlightLines;
use syntect::parsing::SyntaxSet;
use syntect::highlighting::ThemeSet;
use once_cell::sync::Lazy;

static SYNTAX_SET: Lazy<SyntaxSet> = Lazy::new(SyntaxSet::load_defaults_newlines);
static THEME_SET: Lazy<ThemeSet> = Lazy::new(ThemeSet::load_defaults);

pub fn sniff_for_markdown(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("```") || 
    trimmed.starts_with("#") || 
    trimmed.starts_with("- ") || 
    trimmed.starts_with("* ") ||
    trimmed.contains("| ---") || // Table indicator
    (trimmed.starts_with("[") && trimmed.contains("]("))
}

pub fn render_markdown(content: &str, base_style: Style) -> Text<'static> {
    let mut text = Text::default();
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    
    let parser = Parser::new_ext(content, options);
    
    let mut current_line = Line::default();
    let mut in_code_block: Option<String> = None;
    let mut in_table_header = false;
    let mut current_heading_level: Option<HeadingLevel> = None;
    let mut current_style = base_style;

    for event in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                if !current_line.spans.is_empty() {
                    text.lines.push(std::mem::take(&mut current_line));
                }
                current_heading_level = Some(level);
                let color = match level {
                    HeadingLevel::H1 => Color::Red,
                    HeadingLevel::H2 => Color::Magenta,
                    _ => Color::Yellow,
                };
                let prefix = "#".repeat(level as usize) + " ";
                current_line.spans.push(Span::styled(prefix, Style::default().fg(color).add_modifier(Modifier::BOLD)));
                current_style = Style::default().fg(color).add_modifier(Modifier::BOLD);
            }
            Event::End(TagEnd::Heading(_)) => {
                current_heading_level = None;
                current_style = base_style;
                text.lines.push(std::mem::take(&mut current_line));
            }
            Event::Start(Tag::Paragraph) => {
                if !current_line.spans.is_empty() {
                    text.lines.push(std::mem::take(&mut current_line));
                }
            }
            Event::End(TagEnd::Paragraph) => {
                if !current_line.spans.is_empty() {
                    text.lines.push(std::mem::take(&mut current_line));
                }
            }
            Event::Start(Tag::Strong) => {
                current_style = current_style.add_modifier(Modifier::BOLD);
            }
            Event::End(TagEnd::Strong) => {
                current_style = current_style.remove_modifier(Modifier::BOLD);
            }
            Event::Start(Tag::Emphasis) => {
                current_style = current_style.add_modifier(Modifier::ITALIC);
            }
            Event::End(TagEnd::Emphasis) => {
                current_style = current_style.remove_modifier(Modifier::ITALIC);
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                if !current_line.spans.is_empty() {
                    text.lines.push(std::mem::take(&mut current_line));
                }
                in_code_block = match kind {
                    CodeBlockKind::Fenced(lang) => Some(lang.to_string()),
                    _ => Some("text".to_string()),
                };
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code_block = None;
            }
            // Table handling
            Event::Start(Tag::Table(_)) => {
                if !current_line.spans.is_empty() {
                    text.lines.push(std::mem::take(&mut current_line));
                }
                text.lines.push(Line::from(Span::styled(format!("┌{}", "─".repeat(40)), Style::default().fg(Color::DarkGray))));
            }
            Event::End(TagEnd::Table) => {
                text.lines.push(Line::from(Span::styled(format!("└{}", "─".repeat(40)), Style::default().fg(Color::DarkGray))));
            }
            Event::Start(Tag::TableHead) => {
                in_table_header = true;
            }
            Event::End(TagEnd::TableHead) => {
                in_table_header = false;
                text.lines.push(Line::from(Span::styled(format!("├{}", "─".repeat(40)), Style::default().fg(Color::DarkGray))));
            }
            Event::Start(Tag::TableRow) => {
                current_line.spans.push(Span::styled("│ ", Style::default().fg(Color::DarkGray)));
            }
            Event::End(TagEnd::TableRow) => {
                current_line.spans.push(Span::styled(" │", Style::default().fg(Color::DarkGray)));
                text.lines.push(std::mem::take(&mut current_line));
            }
            Event::Start(Tag::TableCell) => {
                if !current_line.spans.is_empty() && current_line.spans.last().unwrap().content != "│ " {
                    current_line.spans.push(Span::styled(" ║ ", Style::default().fg(Color::DarkGray)));
                }
            }
            Event::End(TagEnd::TableCell) => {}

            Event::Text(t) => {
                if let Some(lang) = &in_code_block {
                    let syntax = SYNTAX_SET.find_syntax_by_token(lang)
                        .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text());
                    let mut h = HighlightLines::new(syntax, &THEME_SET.themes["base16-ocean.dark"]);
                    
                    for line_str in t.lines() {
                        if let Ok(ranges) = h.highlight_line(line_str, &SYNTAX_SET) {
                            let mut spans = Vec::new();
                            for (style, text) in ranges {
                                let fg = Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b);
                                spans.push(Span::styled(text.to_string(), Style::default().fg(fg).bg(Color::Rgb(20, 20, 30))));
                            }
                            text.lines.push(Line::from(spans));
                        }
                    }
                } else {
                    let mut style = current_style;
                    if in_table_header {
                        style = style.add_modifier(Modifier::BOLD).fg(Color::Cyan);
                    }
                    // Handle text within heading
                    if current_heading_level.is_some() {
                        current_line.spans.push(Span::styled(t.to_string(), style));
                    } else {
                        current_line.spans.push(Span::styled(t.to_string(), style));
                    }
                }
            }
            Event::Code(t) => {
                current_line.spans.push(Span::styled(format!(" `{}` ", t), Style::default().fg(Color::Yellow).bg(Color::DarkGray)));
            }
            Event::SoftBreak | Event::HardBreak => {
                text.lines.push(std::mem::take(&mut current_line));
            }
            _ => {}
        }
    }
    
    if !current_line.spans.is_empty() {
        text.lines.push(current_line);
    }

    text
}
