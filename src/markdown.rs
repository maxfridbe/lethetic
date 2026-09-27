use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
};
use std::sync::LazyLock;
use syntect::easy::HighlightLines;
use syntect::parsing::SyntaxSet;

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);

/// Code token categories, each drawn with a colour from the app theme so
/// highlighting follows the selected theme (named or RGB colours alike).
#[derive(Clone, Copy)]
enum CodeCategory {
    Plain,
    Comment,
    Keyword,
    StringLiteral,
    Number,
    Function,
    Type,
    Key,
}

const CODE_CATEGORIES: [CodeCategory; 8] = [
    CodeCategory::Plain,
    CodeCategory::Comment,
    CodeCategory::Keyword,
    CodeCategory::StringLiteral,
    CodeCategory::Number,
    CodeCategory::Function,
    CodeCategory::Type,
    CodeCategory::Key,
];

/// Marker bytes: a syntect colour `(index, MARK_G, MARK_B)` stands for
/// `CODE_CATEGORIES[index]` and is replaced by the theme colour when drawn.
const MARK_G: u8 = 0x5a;
const MARK_B: u8 = 0xa5;

impl CodeCategory {
    fn color(self, theme: &crate::ui::Theme) -> Color {
        match self {
            Self::Plain => theme.output_fg,
            Self::Comment => theme.system_fg,
            Self::Keyword => theme.tool_fg,
            Self::StringLiteral => theme.json_val_fg,
            Self::Number => theme.thought_fg,
            Self::Function => theme.highlight_fg,
            Self::Type => theme.warning_fg,
            Self::Key => theme.json_key_fg,
        }
    }

    fn scopes(self) -> &'static str {
        match self {
            Self::Plain => "",
            Self::Comment => "comment, punctuation.definition.comment",
            Self::Keyword => {
                "keyword, storage.modifier, storage.type.function, keyword.operator.word, variable.language"
            }
            Self::StringLiteral => "string, constant.character, punctuation.definition.string",
            Self::Number => "constant.numeric, constant.language, constant.other",
            Self::Function => {
                "entity.name.function, support.function, meta.function-call variable.function, variable.function"
            }
            Self::Type => {
                "entity.name.type, entity.name.class, entity.name.struct, entity.name.enum, support.type, support.class, storage.type"
            }
            Self::Key => {
                "entity.name.tag, entity.other.attribute-name, meta.object-literal.key, support.type.property-name, meta.mapping.key string, variable.other.member"
            }
        }
    }
}

/// A syntect theme whose colours are category markers, built once.
static CATEGORY_THEME: LazyLock<syntect::highlighting::Theme> = LazyLock::new(|| {
    use std::str::FromStr;
    use syntect::highlighting::{
        Color as SyntectColor, ScopeSelectors, StyleModifier, Theme as SyntectTheme, ThemeItem,
        ThemeSettings,
    };
    let marker = |index: usize| SyntectColor {
        r: index as u8,
        g: MARK_G,
        b: MARK_B,
        a: 0xff,
    };
    let scopes = CODE_CATEGORIES
        .iter()
        .enumerate()
        .filter(|(_, category)| !category.scopes().is_empty())
        .filter_map(|(index, category)| {
            Some(ThemeItem {
                scope: ScopeSelectors::from_str(category.scopes()).ok()?,
                style: StyleModifier {
                    foreground: Some(marker(index)),
                    background: None,
                    font_style: None,
                },
            })
        })
        .collect();
    SyntectTheme {
        name: Some("lethetic-app-theme".to_string()),
        author: None,
        settings: ThemeSettings {
            foreground: Some(marker(0)),
            ..ThemeSettings::default()
        },
        scopes,
    }
});

fn themed_color(color: syntect::highlighting::Color, theme: &crate::ui::Theme) -> Color {
    if color.g == MARK_G && color.b == MARK_B {
        if let Some(category) = CODE_CATEGORIES.get(color.r as usize) {
            return category.color(theme);
        }
    }
    Color::Rgb(color.r, color.g, color.b)
}

/// Force the syntect lazy statics to load. Called from a background thread at
/// startup so the first code-fence render doesn't pay the dump-load cost.
pub fn warm_highlighter() {
    LazyLock::force(&SYNTAX_SET);
    LazyLock::force(&CATEGORY_THEME);
}

/// Line-oriented highlighting for formats syntect does not bundle: TOML,
/// INI/conf, `.env` and Dockerfile. Same theme categories as syntect output.
fn highlight_config_like(
    source: &str,
    dockerfile: bool,
    theme: &crate::ui::Theme,
) -> Text<'static> {
    let style = |category: CodeCategory| {
        let style = Style::default()
            .fg(category.color(theme))
            .bg(theme.terminal_bg);
        if matches!(category, CodeCategory::Comment) {
            style.add_modifier(Modifier::ITALIC)
        } else {
            style
        }
    };
    let value_spans = |value: &str, spans: &mut Vec<Span<'static>>| {
        let trimmed = value.trim();
        let category = if trimmed.starts_with('"') || trimmed.starts_with('\'') {
            CodeCategory::StringLiteral
        } else if matches!(trimmed, "true" | "false" | "yes" | "no" | "on" | "off")
            || trimmed.parse::<f64>().is_ok()
        {
            CodeCategory::Number
        } else {
            CodeCategory::Plain
        };
        spans.push(Span::styled(value.to_string(), style(category)));
    };
    let mut text = Text::default();
    for line in source.strip_suffix('\n').unwrap_or(source).split('\n') {
        let mut spans = Vec::new();
        let body = line.trim_start();
        let indent = &line[..line.len() - body.len()];
        spans.push(Span::styled(indent.to_string(), style(CodeCategory::Plain)));
        if body.starts_with('#') || body.starts_with(';') {
            spans.push(Span::styled(body.to_string(), style(CodeCategory::Comment)));
        } else if dockerfile {
            let (word, rest) = body.split_at(body.find(char::is_whitespace).unwrap_or(body.len()));
            if !word.is_empty() && word.chars().all(|c| c.is_ascii_uppercase()) {
                spans.push(Span::styled(word.to_string(), style(CodeCategory::Keyword)));
                value_spans(rest, &mut spans);
            } else {
                value_spans(body, &mut spans);
            }
        } else if body.starts_with('[') {
            spans.push(Span::styled(body.to_string(), style(CodeCategory::Type)));
        } else if let Some(position) = body.find(['=', ':']) {
            let (key, rest) = body.split_at(position);
            spans.push(Span::styled(key.to_string(), style(CodeCategory::Key)));
            spans.push(Span::styled(
                rest[..1].to_string(),
                style(CodeCategory::Plain),
            ));
            value_spans(&rest[1..], &mut spans);
        } else {
            spans.push(Span::styled(body.to_string(), style(CodeCategory::Plain)));
        }
        text.lines.push(Line::from(spans));
    }
    text
}

pub fn highlight_source(source: &str, language: &str, theme: &crate::ui::Theme) -> Text<'static> {
    let language_lower = language.to_lowercase();
    match language_lower.as_str() {
        "toml" | "ini" | "cfg" | "conf" | "env" | "dotenv" | "editorconfig" | "gitconfig" => {
            return highlight_config_like(source, false, theme);
        }
        "dockerfile" | "containerfile" | "docker" => {
            return highlight_config_like(source, true, theme);
        }
        _ => {}
    }
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
        "html" | "htm" | "xml" | "svg" | "css" | "sql" | "diff" | "patch" | "lua" | "rb"
        | "ruby" | "php" | "go" | "java" | "kotlin" | "scala" | "makefile" | "make" => {
            match language_lower.as_str() {
                "ruby" => "rb",
                "kotlin" => "java",
                "makefile" => "make",
                "patch" => "diff",
                "htm" => "html",
                "svg" => "xml",
                other => other,
            }
        }
        "md" | "markdown" => "md",
        other => other,
    };
    let syntax = SYNTAX_SET
        .find_syntax_by_extension(extension)
        .or_else(|| SYNTAX_SET.find_syntax_by_name(language))
        .or_else(|| SYNTAX_SET.find_syntax_by_token(language))
        .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text());
    let mut highlighter = HighlightLines::new(syntax, &CATEGORY_THEME);
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
                    let mut span_style = Style::default()
                        .fg(themed_color(style.foreground, theme))
                        .bg(theme.terminal_bg);
                    if style.foreground.r == 1
                        && style.foreground.g == MARK_G
                        && style.foreground.b == MARK_B
                    {
                        span_style = span_style.add_modifier(Modifier::ITALIC);
                    }
                    spans.push(Span::styled(text.to_string(), span_style));
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

#[cfg(test)]
mod theme_highlight_tests {
    use super::*;

    #[test]
    fn rust_code_uses_the_app_theme_colours() {
        let theme = crate::ui::Theme::default();
        let text = highlight_source(
            "fn main() {\n    // hi\n    let x: u32 = 42;\n    println!(\"hello\");\n}\n",
            "rust",
            &theme,
        );
        let color_of = |needle: &str| {
            text.lines
                .iter()
                .flat_map(|line| line.spans.iter())
                .find(|span| span.content.contains(needle))
                .and_then(|span| span.style.fg)
        };
        assert_eq!(color_of("fn"), Some(theme.tool_fg));
        assert_eq!(color_of("hi"), Some(theme.system_fg));
        assert_eq!(color_of("42"), Some(theme.thought_fg));
        assert_eq!(color_of("hello"), Some(theme.json_val_fg));
        assert_eq!(color_of("main"), Some(theme.highlight_fg));
    }
}

#[cfg(test)]
mod config_highlight_tests {
    use super::*;

    #[test]
    fn toml_yaml_and_dockerfile_use_theme_categories() {
        let theme = crate::ui::Theme::default();
        let color_of = |text: &Text<'static>, needle: &str| {
            text.lines
                .iter()
                .flat_map(|line| line.spans.iter())
                .find(|span| span.content.contains(needle))
                .and_then(|span| span.style.fg)
        };
        let toml = highlight_source(
            "[package]\nname = \"demo\"\n# note\nedition = 2024\n",
            "toml",
            &theme,
        );
        assert_eq!(color_of(&toml, "[package]"), Some(theme.warning_fg));
        assert_eq!(color_of(&toml, "name"), Some(theme.json_key_fg));
        assert_eq!(color_of(&toml, "demo"), Some(theme.json_val_fg));
        assert_eq!(color_of(&toml, "note"), Some(theme.system_fg));
        assert_eq!(color_of(&toml, "2024"), Some(theme.thought_fg));
        let docker = highlight_source("FROM rust:1\nRUN cargo build\n", "dockerfile", &theme);
        assert_eq!(color_of(&docker, "FROM"), Some(theme.tool_fg));
        let yaml = highlight_source("name: demo\ncount: 3\n", "yaml", &theme);
        assert_ne!(color_of(&yaml, "name"), Some(theme.output_fg));
    }
}
