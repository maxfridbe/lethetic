use lethetic::markdown::render_markdown;
use lethetic::ui::Theme;

#[test]
fn test_code_block_order() {
    let content = "Before code\n```rust\ncode line\n```\nAfter code";
    let theme = Theme::default();
    let text = render_markdown(content, &theme);
    
    let rendered_lines: Vec<String> = text.lines.iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
        .collect();
        
    assert!(rendered_lines.len() >= 3);
    assert_eq!(rendered_lines[0], "Before code");
    assert_eq!(rendered_lines[1], "code line");
    assert_eq!(rendered_lines[2], "After code");
}

#[test]
fn test_heading_order() {
    let content = "Before heading\n# Heading\nAfter heading";
    let theme = Theme::default();
    let text = render_markdown(content, &theme);
    
    let rendered_lines: Vec<String> = text.lines.iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
        .collect();
        
    assert!(rendered_lines.len() >= 3);
    assert_eq!(rendered_lines[0], "Before heading");
    assert_eq!(rendered_lines[1], "# Heading");
    assert_eq!(rendered_lines[2], "After heading");
}

#[test]
fn test_paragraph_split() {
    let content = "Para 1\n\nPara 2";
    let theme = Theme::default();
    let text = render_markdown(content, &theme);
    
    let rendered_lines: Vec<String> = text.lines.iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
        .collect();
        
    assert_eq!(rendered_lines.len(), 2);
    assert_eq!(rendered_lines[0], "Para 1");
    assert_eq!(rendered_lines[1], "Para 2");
}

#[test]
fn test_table_columns_aligned() {
    let content = "\
| Name | Description |
|---|---|
| x | a much longer cell here |
| longer-name | tiny |";
    let theme = Theme::default();
    let text = render_markdown(content, &theme);

    let rendered: Vec<String> = text.lines.iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
        .collect();

    // ┌─┬─┐ / header / ├─┼─┤ / 2 data rows / └─┴─┘
    assert_eq!(rendered.len(), 6, "unexpected table lines: {:#?}", rendered);
    assert!(rendered[0].starts_with('┌') && rendered[0].ends_with('┐'));
    assert!(rendered[2].starts_with('├') && rendered[2].ends_with('┤'));
    assert!(rendered[5].starts_with('└') && rendered[5].ends_with('┘'));

    // Every line is exactly the same display width
    let widths: Vec<usize> = rendered.iter().map(|l| l.chars().count()).collect();
    assert!(widths.iter().all(|w| *w == widths[0]), "ragged table: {:?}\n{:#?}", widths, rendered);

    // Column separators line up vertically on every row
    let sep_positions = |line: &str| -> Vec<usize> {
        line.chars().enumerate().filter(|(_, c)| "│┬┼┴┌┐├┤└┘".contains(*c)).map(|(i, _)| i).collect::<Vec<_>>()
    };
    let expected = sep_positions(&rendered[1]);
    assert_eq!(expected.len(), 3); // left, middle, right
    for line in &rendered {
        assert_eq!(sep_positions(line), expected, "separators misaligned in {:?}", line);
    }
}

#[test]
fn test_table_with_inline_code_and_empty_cells() {
    let content = "\
| Tool | Args |
|---|---|
| `read_file` | path |
| edit | |";
    let theme = Theme::default();
    let text = render_markdown(content, &theme);

    let rendered: Vec<String> = text.lines.iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
        .collect();

    let widths: Vec<usize> = rendered.iter().map(|l| l.chars().count()).collect();
    assert!(widths.iter().all(|w| *w == widths[0]), "ragged table: {:?}\n{:#?}", widths, rendered);
    assert!(rendered.iter().any(|l| l.contains("`read_file`")));
}
