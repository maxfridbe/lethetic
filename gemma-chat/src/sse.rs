/// Parse Server-Sent Events from a raw line.
/// Returns `Some(data)` for a `data:` field carrying a non-terminal payload.
/// Per the SSE grammar, the single space after the colon is optional.
/// Returns `None` for terminal, comment, event-name, retry, or empty lines.
pub fn parse_sse_line(line: &str) -> Option<&str> {
    let line = line.trim();
    if line.is_empty() || line.starts_with(':') {
        return None;
    }
    if let Some(data) = line.strip_prefix("data:") {
        let data = data.strip_prefix(' ').unwrap_or(data);
        if data == "[DONE]" {
            return None;
        }
        return Some(data);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_line_is_none() {
        assert!(parse_sse_line("").is_none());
        assert!(parse_sse_line("   ").is_none());
    }

    #[test]
    fn comment_line_is_none() {
        assert!(parse_sse_line(": keep-alive").is_none());
    }

    #[test]
    fn event_name_line_is_none() {
        assert!(parse_sse_line("event: response.output_text.delta").is_none());
    }

    #[test]
    fn done_sentinel_is_none() {
        assert!(parse_sse_line("data: [DONE]").is_none());
        assert!(parse_sse_line("data:[DONE]").is_none());
    }

    #[test]
    fn data_line_returns_json() {
        let line = r#"data: {"choices":[{"delta":{"content":"hello"}}]}"#;
        assert_eq!(
            parse_sse_line(line),
            Some(r#"{"choices":[{"delta":{"content":"hello"}}]}"#)
        );
        let line = r#"data:{"choices":[{"delta":{"content":"hello"}}]}"#;
        assert_eq!(
            parse_sse_line(line),
            Some(r#"{"choices":[{"delta":{"content":"hello"}}]}"#)
        );
    }

    #[test]
    fn data_line_with_leading_whitespace() {
        let line = r#"  data: {"id":"x"}"#;
        assert_eq!(parse_sse_line(line), Some(r#"{"id":"x"}"#));
    }
}
