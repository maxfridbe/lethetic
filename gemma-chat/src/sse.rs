/// Parse Server-Sent Events from a raw line.
/// Returns `Some(data)` when the line is a `data: ...` line with a JSON payload.
/// Returns `None` for comment, event-name, retry, or empty lines.
pub fn parse_sse_line(line: &str) -> Option<&str> {
    let line = line.trim();
    if line.is_empty() || line.starts_with(':') {
        return None;
    }
    if let Some(data) = line.strip_prefix("data: ") {
        if data == "[DONE]" { return None; }
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
    }

    #[test]
    fn data_line_returns_json() {
        let line = r#"data: {"choices":[{"delta":{"content":"hello"}}]}"#;
        assert_eq!(parse_sse_line(line), Some(r#"{"choices":[{"delta":{"content":"hello"}}]}"#));
    }

    #[test]
    fn data_line_with_leading_whitespace() {
        let line = r#"  data: {"id":"x"}"#;
        assert_eq!(parse_sse_line(line), Some(r#"{"id":"x"}"#));
    }
}
