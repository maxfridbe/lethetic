use crate::app::{BlockType, RenderBlock};
use crate::icons;
use crate::markdown;
use crate::status_summary::{format_estimated_cost, format_tokens};
use crate::theme::Theme;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
};

pub(super) fn render_json_highlighted(
    json_val: &serde_json::Value,
    theme: &Theme,
) -> Text<'static> {
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
                spans.push(Span::styled(
                    key.to_string(),
                    Style::default().fg(theme.json_key_fg),
                ));

                // Colorize the value part
                let value_part = rest.trim_start_matches(':').trim();
                spans.push(Span::raw(": "));
                if value_part.starts_with('"') {
                    spans.push(Span::styled(
                        value_part.to_string(),
                        Style::default().fg(theme.json_val_fg),
                    ));
                } else if value_part == "true" || value_part == "false" || value_part == "null" {
                    spans.push(Span::styled(
                        value_part.to_string(),
                        Style::default().fg(theme.error_fg),
                    ));
                } else if value_part
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_digit() || c == '-')
                {
                    spans.push(Span::styled(
                        value_part.to_string(),
                        Style::default().fg(theme.thought_fg),
                    ));
                } else {
                    spans.push(Span::raw(value_part.to_string()));
                }
            } else {
                // Just a string (maybe in an array)
                spans.push(Span::styled(
                    trimmed.to_string(),
                    Style::default().fg(theme.json_val_fg),
                ));
            }
        } else if trimmed == "{"
            || trimmed == "}"
            || trimmed == "["
            || trimmed == "]"
            || trimmed == "},"
            || trimmed == "],"
        {
            spans.push(Span::styled(
                trimmed.to_string(),
                Style::default().fg(theme.system_fg),
            ));
        } else {
            spans.push(Span::raw(trimmed.to_string()));
        }
        lines.push(Line::from(spans));
    }
    Text::from(lines)
}

pub(super) fn render_python_call(
    arguments: &serde_json::Value,
    theme: &Theme,
    exact_original: bool,
) -> Vec<Line<'static>> {
    let Some(display) = crate::python::display::PythonCallDisplay::from_arguments(arguments) else {
        return render_json_highlighted(arguments, theme).lines;
    };
    let mut lines = vec![
        Line::from(Span::styled(
            "call:python",
            Style::default().fg(theme.tool_fg),
        )),
        Line::from(vec![
            Span::styled("Description: ", Style::default().fg(theme.system_fg)),
            Span::styled(display.description, Style::default().fg(theme.output_fg)),
        ]),
        Line::from(Span::styled(
            if exact_original {
                "Exact original source:"
            } else {
                "Formatted preview:"
            },
            Style::default()
                .fg(theme.system_fg)
                .add_modifier(Modifier::ITALIC),
        )),
    ];
    let source = if exact_original {
        &display.original
    } else {
        &display.preview.text
    };
    lines.extend(markdown::highlight_source(source, "python", theme).lines);
    lines
}

fn user_block_header(label: String, block: &RenderBlock, include_estimated_cost: bool) -> String {
    let mut header = label;
    if let Some(usage) = block.usage {
        let marker = if usage.breakdown_complete { "" } else { "*" };
        header = format!(
            "{header} (u:{} cr:{} cw:{} o:{} tokens){marker}",
            format_tokens(usage.uncached_input_tokens),
            format_tokens(usage.cache_read_input_tokens),
            format_tokens(usage.cache_creation_input_tokens),
            format_tokens(usage.output_tokens),
        );
    } else if let (Some(prompt), Some(completion)) = (block.prompt_tokens, block.completion_tokens)
    {
        header = format!(
            "{header} (i:{} o:{} tokens)*",
            format_tokens(u64::from(prompt)),
            format_tokens(u64::from(completion))
        );
    }
    if include_estimated_cost && let Some(cost) = &block.estimated_cost {
        header = format!(
            "{header} · EST API-eq turn: {}",
            format_estimated_cost(cost)
        );
    }
    header
}

struct BlockPresentation {
    color: Color,
    background: Color,
    header: Option<String>,
}

pub fn render_block_to_lines(
    block: &RenderBlock,
    width: usize,
    theme: &Theme,
    tool_preview: Option<&str>,
) -> Vec<Line<'static>> {
    render_block_to_lines_with_cost_visibility(block, width, theme, tool_preview, true)
}

pub fn render_block_to_lines_with_cost_visibility(
    block: &RenderBlock,
    width: usize,
    theme: &Theme,
    tool_preview: Option<&str>,
    include_estimated_cost: bool,
) -> Vec<Line<'static>> {
    if block.block_type == BlockType::Divider {
        return vec![Line::from(Span::styled(
            "─".repeat(width),
            Style::default().fg(theme.system_fg),
        ))];
    }

    let presentation = block_presentation(block, theme, include_estimated_cost);
    let status_block = Span::styled("█ ", Style::default().fg(presentation.color));
    let base_style = Style::default()
        .bg(presentation.background)
        .fg(theme.output_fg);
    let mut output = Vec::new();
    if let Some(header) = presentation.header {
        output.push(render_block_header(
            &header,
            width,
            status_block.clone(),
            base_style,
        ));
    }

    let content = render_block_content(block, width, theme, tool_preview, base_style);
    let wrapped = wrap_lines(content, width.saturating_sub(2));
    output.extend(
        wrapped.into_iter().map(|line| {
            decorate_content_line(line, block, width, theme, &status_block, base_style)
        }),
    );
    if block.block_type == BlockType::User {
        output.push(Line::from(vec![
            status_block,
            Span::styled(" ".repeat(width.saturating_sub(2)), base_style),
        ]));
    }
    output
}

fn block_presentation(
    block: &RenderBlock,
    theme: &Theme,
    include_estimated_cost: bool,
) -> BlockPresentation {
    let color = match block.block_type {
        BlockType::User => theme.input_fg,
        BlockType::Thought => theme.thought_fg,
        BlockType::Formulating => theme.warning_fg,
        BlockType::ToolCall => theme.tool_fg,
        BlockType::ToolResult => match block.success {
            Some(true) => theme.success_fg,
            Some(false) => theme.error_fg,
            None => theme.system_fg,
        },
        BlockType::ToolError | BlockType::ProviderError => theme.error_fg,
        BlockType::Divider => theme.system_fg,
        _ => theme.output_fg,
    };
    let (background, default_header) = match block.block_type {
        BlockType::User => (
            theme.input_bg,
            Some(user_block_header(
                format!("{} User Request", icons::INPUT),
                block,
                include_estimated_cost,
            )),
        ),
        BlockType::Thought => (
            theme.thought_bg,
            Some(format!("{} Engine Thinking...", icons::PROCESSING)),
        ),
        BlockType::Formulating => (
            theme.thought_bg,
            Some(format!("{} Formulating tool request...", icons::SPINNER[0])),
        ),
        BlockType::ToolCall => (
            theme.tool_bg,
            Some(format!("{} Engine Tool Request", icons::COMMAND)),
        ),
        BlockType::ToolResult | BlockType::ToolError => (
            theme.terminal_bg,
            Some(format!("{} Agent, Tool Output", icons::SUCCESS)),
        ),
        BlockType::Divider => (Color::Reset, None),
        _ => (Color::Reset, None),
    };
    let header = match (&block.title, &block.block_type) {
        (None, _) | (Some(_), BlockType::ToolCall) => default_header,
        (Some(title), BlockType::ToolResult | BlockType::ToolError) => {
            Some(format!("{} Agent, {}", icons::SUCCESS, title))
        }
        (Some(title), BlockType::User) => Some(user_block_header(
            format!("{} {}", icons::INPUT, title),
            block,
            include_estimated_cost,
        )),
        (Some(title), _) => Some(title.clone()),
    };
    BlockPresentation {
        color,
        background,
        header,
    }
}

fn render_block_header(
    header: &str,
    width: usize,
    status_block: Span<'static>,
    base_style: Style,
) -> Line<'static> {
    let mut spans = vec![
        status_block,
        Span::styled(
            format!(" {} ", header),
            base_style.add_modifier(Modifier::BOLD).fg(Color::White),
        ),
    ];
    let current_len = 2 + header.len() + 2;
    if width > current_len {
        spans.push(Span::styled(" ".repeat(width - current_len), base_style));
    }
    Line::from(spans)
}

fn render_block_content(
    block: &RenderBlock,
    width: usize,
    theme: &Theme,
    tool_preview: Option<&str>,
    base_style: Style,
) -> Vec<Line<'static>> {
    match block.block_type {
        BlockType::Formulating => render_formulating_content(block, width, base_style),
        BlockType::ToolCall => {
            render_tool_call_content(block, width, theme, tool_preview, base_style)
        }
        BlockType::Text
        | BlockType::ProviderError
        | BlockType::ToolResult
        | BlockType::ToolError
        | BlockType::Markdown
        | BlockType::Thought => markdown::render_markdown(&block.content, theme).lines,
        _ if block.content.contains("```") => {
            markdown::render_markdown(&block.content, theme).lines
        }
        _ => render_plain_content(&block.content, base_style),
    }
}

fn render_formulating_content(
    block: &RenderBlock,
    width: usize,
    base_style: Style,
) -> Vec<Line<'static>> {
    let block_lines: Vec<&str> = block.content.lines().collect();
    let last_lines = if block_lines.len() > 3 {
        &block_lines[block_lines.len() - 3..]
    } else {
        &block_lines[..]
    };
    let mut formatted = vec![Line::from(Span::styled(
        "(Engine is preparing the tool payload...)",
        base_style.add_modifier(Modifier::ITALIC),
    ))];
    for index in 0..3 {
        let Some(line_content) = last_lines.get(index) else {
            formatted.push(Line::from(Span::styled("  ", base_style)));
            continue;
        };
        let max_line_len = width.saturating_sub(10);
        let display_line = if line_content.len() > max_line_len {
            format!("  {}...", &line_content[..max_line_len.saturating_sub(3)])
        } else {
            format!("  {}", line_content)
        };
        formatted.push(Line::from(Span::styled(
            display_line,
            base_style.add_modifier(Modifier::DIM),
        )));
    }
    formatted
}

fn render_tool_call_content(
    block: &RenderBlock,
    width: usize,
    theme: &Theme,
    tool_preview: Option<&str>,
    base_style: Style,
) -> Vec<Line<'static>> {
    let Some(brace_pos) = block.content.find('{') else {
        return render_plain_content(&block.content, base_style);
    };
    let func_name = &block.content[..brace_pos];
    let json_part = &block.content[brace_pos..];
    let Ok(arguments) = serde_json::from_str::<serde_json::Value>(json_part) else {
        return render_plain_content(&block.content, base_style);
    };
    let mut formatted = if func_name == "call:python" {
        render_python_call(&arguments, theme, false)
    } else {
        render_json_tool_call(func_name, &arguments, theme)
    };
    if let Some(preview) = tool_preview {
        append_tool_preview(&mut formatted, preview, width, theme);
    }
    formatted
}

fn render_json_tool_call(
    func_name: &str,
    arguments: &serde_json::Value,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let json_text = render_json_highlighted(arguments, theme);
    let mut formatted = Vec::new();
    if let Some(first_line) = json_text.lines.first() {
        let mut spans = vec![Span::styled(
            func_name.to_string(),
            Style::default().fg(theme.tool_fg),
        )];
        spans.extend(first_line.spans.clone());
        formatted.push(Line::from(spans));
    }
    formatted.extend(json_text.lines.iter().skip(1).cloned());
    formatted
}

fn append_tool_preview(
    formatted: &mut Vec<Line<'static>>,
    preview: &str,
    width: usize,
    theme: &Theme,
) {
    let preview_style = Style::default()
        .fg(theme.system_fg)
        .add_modifier(Modifier::ITALIC);
    formatted.push(Line::from(Span::styled(
        "--- Live Output Preview ---",
        preview_style,
    )));
    let lines: Vec<&str> = preview.lines().collect();
    for index in 0..5 {
        let Some(line_content) = lines.get(index) else {
            formatted.push(Line::from(Span::styled(">", preview_style)));
            continue;
        };
        let max_line_len = width.saturating_sub(10);
        let display_line = if line_content.len() > max_line_len {
            format!("> {}...", &line_content[..max_line_len.saturating_sub(3)])
        } else {
            format!("> {}", line_content)
        };
        formatted.push(Line::from(Span::styled(display_line, preview_style)));
    }
}

fn render_plain_content(content: &str, base_style: Style) -> Vec<Line<'static>> {
    content
        .lines()
        .map(|line| Line::from(Span::styled(line.to_string(), base_style)))
        .collect()
}

fn decorate_content_line(
    mut line: Line<'static>,
    block: &RenderBlock,
    width: usize,
    theme: &Theme,
    status_block: &Span<'static>,
    base_style: Style,
) -> Line<'static> {
    for span in &mut line.spans {
        if block.block_type == BlockType::Thought {
            span.style = span
                .style
                .add_modifier(Modifier::ITALIC)
                .fg(theme.thought_fg);
        } else if block.block_type == BlockType::ToolCall && span.style.fg.is_none() {
            span.style = span.style.fg(theme.warning_fg);
        }
    }
    let mut spans = vec![status_block.clone()];
    spans.append(&mut line.spans);
    let current_len = 2 + line.width();
    if width > current_len {
        spans.push(Span::styled(" ".repeat(width - current_len), base_style));
    }
    Line::from(spans)
}

#[derive(Clone, Copy)]
struct WrapIndent {
    width: usize,
    style: Style,
}

fn wrap_lines(lines: Vec<Line<'static>>, max_width: usize) -> Vec<Line<'static>> {
    if max_width == 0 {
        return lines;
    }
    let mut wrapped = Vec::new();
    for line in lines {
        wrap_line(line, max_width, &mut wrapped);
    }
    wrapped
}

fn wrap_line(line: Line<'static>, max_width: usize, wrapped: &mut Vec<Line<'static>>) {
    if line.spans.is_empty() {
        wrapped.push(Line::from(vec![]));
        return;
    }
    let indent = line_number_indent(&line);
    let mut current_spans = Vec::new();
    let mut current_width = 0;
    for span in line.spans {
        let style = span.style;
        for word in split_preserving_whitespace(span.content.as_ref()) {
            push_wrapped_word(
                word,
                style,
                max_width,
                indent,
                &mut current_spans,
                &mut current_width,
                wrapped,
            );
        }
    }
    flush_wrapped_line(&mut current_spans, wrapped);
}

fn line_number_indent(line: &Line<'static>) -> WrapIndent {
    let Some(first_span) = line.spans.first() else {
        return WrapIndent {
            width: 0,
            style: Style::default(),
        };
    };
    let is_numbered = first_span.content.len() >= 7
        && first_span
            .content
            .chars()
            .take(6)
            .all(|character| character.is_whitespace() || character.is_ascii_digit())
        && first_span.content.chars().nth(6) == Some('\t');
    if is_numbered {
        WrapIndent {
            width: 7,
            style: first_span.style,
        }
    } else {
        WrapIndent {
            width: 0,
            style: Style::default(),
        }
    }
}

fn split_preserving_whitespace(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current_word = String::new();
    for character in text.chars() {
        current_word.push(character);
        if character.is_whitespace() {
            words.push(current_word);
            current_word = String::new();
        }
    }
    if !current_word.is_empty() {
        words.push(current_word);
    }
    words
}

fn push_wrapped_word(
    word: String,
    style: Style,
    max_width: usize,
    indent: WrapIndent,
    current_spans: &mut Vec<Span<'static>>,
    current_width: &mut usize,
    wrapped: &mut Vec<Line<'static>>,
) {
    let word_width = word.chars().count();
    if *current_width + word_width <= max_width {
        current_spans.push(Span::styled(word, style));
        *current_width += word_width;
        return;
    }
    flush_wrapped_line(current_spans, wrapped);
    start_continuation_line(indent, current_spans, current_width);
    if word_width + *current_width <= max_width {
        current_spans.push(Span::styled(word, style));
        *current_width += word_width;
        return;
    }
    split_long_word(
        word,
        style,
        max_width,
        indent,
        current_spans,
        current_width,
        wrapped,
    );
}

fn start_continuation_line(
    indent: WrapIndent,
    current_spans: &mut Vec<Span<'static>>,
    current_width: &mut usize,
) {
    if indent.width > 0 {
        current_spans.push(Span::styled(" ".repeat(indent.width), indent.style));
        *current_width = indent.width;
    } else {
        *current_width = 0;
    }
}

fn split_long_word(
    word: String,
    style: Style,
    max_width: usize,
    indent: WrapIndent,
    current_spans: &mut Vec<Span<'static>>,
    current_width: &mut usize,
    wrapped: &mut Vec<Line<'static>>,
) {
    let mut remaining = word;
    let available = max_width.saturating_sub(*current_width);
    if available > 0 {
        let head: String = remaining.chars().take(available).collect();
        let tail: String = remaining.chars().skip(available).collect();
        current_spans.push(Span::styled(head, style));
        flush_wrapped_line(current_spans, wrapped);
        remaining = tail;
    } else {
        flush_wrapped_line(current_spans, wrapped);
    }

    let chunk_size = max_width.saturating_sub(indent.width);
    while chunk_size > 0 && remaining.chars().count() > chunk_size {
        let mut next_line = Vec::new();
        if indent.width > 0 {
            next_line.push(Span::styled(" ".repeat(indent.width), indent.style));
        }
        let head: String = remaining.chars().take(chunk_size).collect();
        let tail: String = remaining.chars().skip(chunk_size).collect();
        next_line.push(Span::styled(head, style));
        wrapped.push(Line::from(next_line));
        remaining = tail;
    }
    if remaining.is_empty() {
        return;
    }
    if indent.width > 0 {
        current_spans.push(Span::styled(" ".repeat(indent.width), indent.style));
    }
    let remaining_width = remaining.chars().count();
    current_spans.push(Span::styled(remaining, style));
    *current_width = indent.width + remaining_width;
}

fn flush_wrapped_line(current_spans: &mut Vec<Span<'static>>, wrapped: &mut Vec<Line<'static>>) {
    if !current_spans.is_empty() {
        wrapped.push(Line::from(std::mem::take(current_spans)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounting::EstimatedCost;

    fn rendered_text(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn user_block_cost_respects_render_visibility_policy() {
        let block = RenderBlock {
            block_type: BlockType::User,
            content: "request".to_string(),
            title: None,
            success: Some(true),
            prompt_tokens: None,
            completion_tokens: None,
            usage: None,
            estimated_cost: Some(EstimatedCost {
                currency: "USD".to_string(),
                nanos: 8_765_432,
                incomplete: false,
                mixed_pricing: false,
                long_context_applied: false,
                pricing_effective_as_of: "2026-01-01".to_string(),
                pricing_valid_through: None,
                provenance_kind: "fixture".to_string(),
            }),
            logical_turn_id: None,
            cached_lines: None,
            cached_line_count: None,
        };
        let theme = Theme::default();

        let visible = rendered_text(&render_block_to_lines_with_cost_visibility(
            &block, 120, &theme, None, true,
        ));
        let hidden = rendered_text(&render_block_to_lines_with_cost_visibility(
            &block, 120, &theme, None, false,
        ));

        assert!(visible.contains("EST API-eq turn:"));
        assert!(!hidden.contains("EST API-eq turn:"));
        assert!(hidden.contains("User Request"));
    }
}
