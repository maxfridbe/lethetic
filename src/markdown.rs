use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
};
use std::sync::LazyLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEME_SET: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

/// Force the syntect lazy statics to load. Called from a background thread at
/// startup so the first code-fence render doesn't pay the dump-load cost.
pub fn warm_highlighter() {
    LazyLock::force(&SYNTAX_SET);
    LazyLock::force(&THEME_SET);
}

pub fn highlight_source(source: &str, language: &str, theme: &crate::ui::Theme) -> Text<'static> {
    let language_lower = language.to_lowercase();
    let extension = match language_lower.as_str() {
        "sh" | "shell" | "bash" | "zsh" | "fish" => "sh",
        "rs" | "rust" => "rs",
        "cs" | "csharp" | "c#" => "cs",
        "js" | "javascript" => "js",
        "ts" | "tsx" | "typescript" => "js",
        "py" | "python" => "py",
        "cpp" | "c++" | "cc" => "cpp",
        "json" => "json",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "md" | "markdown" => "md",
        other => other,
    };
    let syntax = SYNTAX_SET
        .find_syntax_by_extension(extension)
        .or_else(|| SYNTAX_SET.find_syntax_by_name(language))
        .or_else(|| SYNTAX_SET.find_syntax_by_token(language))
        .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text());
    let mut highlighter = HighlightLines::new(syntax, &THEME_SET.themes["base16-ocean.dark"]);
    let mut highlighted = Text::default();

    let source = source.strip_suffix('\n').unwrap_or(source);
    for line in source.split('\n') {
        let numbered = line.len() >= 7
            && line
                .chars()
                .take(6)
                .all(|character| character.is_whitespace() || character.is_ascii_digit())
            && line.chars().nth(6) == Some('\t');
        let (prefix, code) = if numbered {
            let (prefix, code) = line.split_at(7);
            (Some(prefix), code)
        } else {
            (None, line)
        };
        let mut spans = Vec::new();
        if let Some(prefix) = prefix {
            spans.push(Span::styled(
                prefix.to_string(),
                Style::default()
                    .fg(theme.system_fg)
                    .add_modifier(Modifier::DIM),
            ));
        }
        match highlighter.highlight_line(code, &SYNTAX_SET) {
            Ok(ranges) => {
                for (style, text) in ranges {
                    let foreground =
                        Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b);
                    spans.push(Span::styled(
                        text.to_string(),
                        Style::default().fg(foreground).bg(theme.terminal_bg),
                    ));
                }
            }
            Err(_) => spans.push(Span::styled(
                code.to_string(),
                Style::default().fg(theme.output_fg).bg(theme.terminal_bg),
            )),
        }
        highlighted.lines.push(Line::from(spans));
    }
    highlighted
}

/// Render buffered table rows as box-drawn lines with columns padded to equal width.
fn render_table(
    rows: &[Vec<Line<'static>>],
    has_header: bool,
    theme: &crate::ui::Theme,
) -> Vec<Line<'static>> {
    let border = Style::default().fg(theme.system_fg);
    let ncols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if ncols == 0 {
        return Vec::new();
    }

    let mut widths = vec![1usize; ncols];
    for row in rows {
        for (c, cell) in row.iter().enumerate() {
            widths[c] = widths[c].max(cell.width());
        }
    }

    let rule = |left: &str, mid: &str, right: &str| -> Line<'static> {
        let mut s = String::from(left);
        for (i, w) in widths.iter().enumerate() {
            if i > 0 {
                s.push_str(mid);
            }
            s.push_str(&"─".repeat(w + 2));
        }
        s.push_str(right);
        Line::from(Span::styled(s, border))
    };

    let mut out = vec![rule("┌", "┬", "┐")];
    for (r, row) in rows.iter().enumerate() {
        let mut spans = Vec::new();
        for (c, width) in widths.iter().enumerate() {
            spans.push(Span::styled("│ ", border));
            let cell_width = row.get(c).map(|cell| cell.width()).unwrap_or(0);
            if let Some(cell) = row.get(c) {
                spans.extend(cell.spans.iter().cloned());
            }
            spans.push(Span::raw(" ".repeat(width - cell_width + 1)));
        }
        spans.push(Span::styled("│", border));
        out.push(Line::from(spans));
        if has_header && r == 0 && rows.len() > 1 {
            out.push(rule("├", "┼", "┤"));
        }
    }
    out.push(rule("└", "┴", "┘"));
    out
}

pub fn render_markdown(content: &str, theme: &crate::ui::Theme) -> Text<'static> {
    let mut text = Text::default();
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);

    let parser = Parser::new_ext(content, options);

    let mut current_line = Line::default();
    let mut in_code_block: Option<String> = None;
    let base_style = Style::default().fg(theme.output_fg);
    let mut current_style = base_style;

    // Table state: cells are buffered until End(Table) so columns can be
    // measured and padded to equal width.
    let mut in_table = false;
    let mut in_table_header = false;
    let mut table_has_header = false;
    let mut table_rows: Vec<Vec<Line<'static>>> = Vec::new();
    let mut table_row: Vec<Line<'static>> = Vec::new();
    let mut table_cell = Line::default();

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
                current_line.spans.push(Span::styled(
                    prefix,
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ));
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
            // Table handling: buffer rows, render aligned on End(Table)
            Event::Start(Tag::Table(_)) => {
                if !current_line.spans.is_empty() {
                    text.lines.push(std::mem::take(&mut current_line));
                }
                in_table = true;
                table_has_header = false;
                table_rows.clear();
            }
            Event::End(TagEnd::Table) => {
                in_table = false;
                text.lines
                    .extend(render_table(&table_rows, table_has_header, theme));
            }
            Event::Start(Tag::TableHead) => {
                in_table_header = true;
                table_row.clear();
            }
            Event::End(TagEnd::TableHead) => {
                in_table_header = false;
                table_has_header = true;
                table_rows.push(std::mem::take(&mut table_row));
            }
            Event::Start(Tag::TableRow) => {
                table_row.clear();
            }
            Event::End(TagEnd::TableRow) => {
                table_rows.push(std::mem::take(&mut table_row));
            }
            Event::Start(Tag::TableCell) => {
                table_cell = Line::default();
            }
            Event::End(TagEnd::TableCell) => {
                table_row.push(std::mem::take(&mut table_cell));
            }

            Event::Text(t) => {
                if let Some(language) = &in_code_block {
                    text.lines
                        .extend(highlight_source(&t, language, theme).lines);
                } else {
                    let mut style = current_style;
                    if in_table_header {
                        style = style.add_modifier(Modifier::BOLD).fg(theme.highlight_fg);
                    }
                    let span = Span::styled(t.to_string(), style);
                    if in_table {
                        table_cell.spans.push(span);
                    } else {
                        current_line.spans.push(span);
                    }
                }
            }
            Event::Code(t) => {
                let style = Style::default().fg(theme.warning_fg).bg(theme.terminal_bg);
                if in_table {
                    // No outer padding inside cells — it would skew column widths
                    table_cell
                        .spans
                        .push(Span::styled(format!("`{}`", t), style));
                } else {
                    current_line
                        .spans
                        .push(Span::styled(format!(" `{}` ", t), style));
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if in_table {
                    table_cell.spans.push(Span::raw(" "));
                } else {
                    text.lines.push(std::mem::take(&mut current_line));
                }
            }
            _ => {}
        }
    }

    if !current_line.spans.is_empty() {
        text.lines.push(current_line);
    }

    text
}
