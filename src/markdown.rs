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

pub fn render_markdown(content: &str, theme: &crate::ui::Theme) -> Text<'static> {
    let mut text = Text::default();
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    
    let parser = Parser::new_ext(content, options);
    
    let mut current_line = Line::default();
    let mut in_code_block: Option<String> = None;
    let mut in_table_header = false;
    let base_style = Style::default().fg(theme.output_fg);
    let mut current_style = base_style;

    for event in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                if !current_line.spans.is_empty() {
                    text.lines.push(std::mem::take(&mut current_line));
                }
                let color = match level {
                    HeadingLevel::H1 => theme.error_fg,
                    HeadingLevel::H2 => theme.thought_fg,
                    _ => theme.warning_fg,
                };
                let prefix = "#".repeat(level as usize) + " ";
                current_line.spans.push(Span::styled(prefix, Style::default().fg(color).add_modifier(Modifier::BOLD)));
                current_style = Style::default().fg(color).add_modifier(Modifier::BOLD);
            }
            Event::End(TagEnd::Heading(_)) => {
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
                text.lines.push(Line::from(Span::styled(format!("┌{}", "─".repeat(40)), Style::default().fg(theme.system_fg))));
            }
            Event::End(TagEnd::Table) => {
                text.lines.push(Line::from(Span::styled(format!("└{}", "─".repeat(40)), Style::default().fg(theme.system_fg))));
            }
            Event::Start(Tag::TableHead) => {
                in_table_header = true;
            }
            Event::End(TagEnd::TableHead) => {
                in_table_header = false;
                text.lines.push(Line::from(Span::styled(format!("├{}", "─".repeat(40)), Style::default().fg(theme.system_fg))));
            }
            Event::Start(Tag::TableRow) => {
                current_line.spans.push(Span::styled("│ ", Style::default().fg(theme.system_fg)));
            }
            Event::End(TagEnd::TableRow) => {
                current_line.spans.push(Span::styled(" │", Style::default().fg(theme.system_fg)));
                text.lines.push(std::mem::take(&mut current_line));
            }
            Event::Start(Tag::TableCell) => {
                if !current_line.spans.is_empty() && current_line.spans.last().unwrap().content != "│ " {
                    current_line.spans.push(Span::styled(" ║ ", Style::default().fg(theme.system_fg)));
                }
            }
            Event::End(TagEnd::TableCell) => {}

            Event::Text(t) => {
                if let Some(lang) = &in_code_block {
                    let lang_lower = lang.to_lowercase();
                    let ext = match lang_lower.as_str() {
                        "sh" | "shell" | "bash" | "zsh" | "fish" => "sh",
                        "rs" | "rust"                             => "rs",
                        "cs" | "csharp" | "c#"                   => "cs",
                        "js" | "javascript"                       => "js",
                        "ts" | "tsx" | "typescript"               => "js", // syntect has no TS syntax; JS grammar covers it
                        "py" | "python"                           => "py",
                        "cpp" | "c++" | "cc"                      => "cpp",
                        "json"                                    => "json",
                        "toml"                                    => "toml",
                        "yaml" | "yml"                            => "yaml",
                        "md" | "markdown"                         => "md",
                        other                                     => other,
                    };
                    let syntax = SYNTAX_SET.find_syntax_by_extension(ext)
                        .or_else(|| SYNTAX_SET.find_syntax_by_name(lang))
                        .or_else(|| SYNTAX_SET.find_syntax_by_token(lang))
                        .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text());
                    let mut h = HighlightLines::new(syntax, &THEME_SET.themes["base16-ocean.dark"]);
                    
                    for line_str in t.lines() {
                        // Check if the line starts with a 6-char number prefix + tab (from read_file)
                        if line_str.len() >= 7 && line_str.chars().take(6).all(|c| c.is_whitespace() || c.is_ascii_digit()) && line_str.chars().nth(6) == Some('\t') {
                            let (prefix, code) = line_str.split_at(7);
                            let mut spans = Vec::new();
                            
                            // Add dimmed line number
                            spans.push(Span::styled(prefix.to_string(), Style::default().fg(theme.system_fg).add_modifier(Modifier::DIM)));
                            
                            // Highlight the rest of the code
                            if let Ok(ranges) = h.highlight_line(code, &SYNTAX_SET) {
                                for (style, text) in ranges {
                                    let fg = Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b);
                                    spans.push(Span::styled(text.to_string(), Style::default().fg(fg).bg(theme.terminal_bg)));
                                }
                            } else {
                                spans.push(Span::styled(code.to_string(), Style::default().fg(theme.output_fg).bg(theme.terminal_bg)));
                            }
                            text.lines.push(Line::from(spans));
                        } else {
                            if let Ok(ranges) = h.highlight_line(line_str, &SYNTAX_SET) {
                                let mut spans = Vec::new();
                                for (style, text) in ranges {
                                    let fg = Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b);
                                    spans.push(Span::styled(text.to_string(), Style::default().fg(fg).bg(theme.terminal_bg)));
                                }
                                text.lines.push(Line::from(spans));
                            }
                        }
                    }
                } else {
                    let mut style = current_style;
                    if in_table_header {
                        style = style.add_modifier(Modifier::BOLD).fg(theme.highlight_fg);
                    }
                    current_line.spans.push(Span::styled(t.to_string(), style));
                }
            }
            Event::Code(t) => {
                current_line.spans.push(Span::styled(format!(" `{}` ", t), Style::default().fg(theme.warning_fg).bg(theme.terminal_bg)));
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
