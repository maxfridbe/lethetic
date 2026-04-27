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
