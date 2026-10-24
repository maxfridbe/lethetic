//! Provider failure details shared by native and browser hosts.
use serde_json::Value;

fn bounded(text: &str, limit: usize) -> String {
    let mut chars = text.chars().filter(|c| !c.is_control());
    let mut result: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        result.push('…');
    }
    result
}

/// Explain explicit failure reasons before validating any returned tool calls.
/// Unknown native labels do not override a valid normalized finish reason.
pub fn finish_error(reason: Option<&str>, native: Option<&str>) -> Option<String> {
    for value in [native, reason].into_iter().flatten() {
        let explanation = match value {
            "length" | "max_tokens" | "max_output_tokens" => {
                "Provider stopped because the output token limit was reached. Increase the output token limit or request a smaller response."
            }
            "content_filter" | "safety" | "blocked" | "refusal" => {
                "Provider blocked the response with its content filter. Revise the request or check the provider's policy."
            }
            "error" | "malformed_function_call" => {
                "Provider could not complete the response. Check the provider details in the debug report."
            }
            _ => continue,
        };
        return Some(format!(
            "{explanation} Provider reason: {}.",
            bounded(value, 128)
        ));
    }
    None
}

/// Extract only bounded public error fields; never display arbitrary metadata,
/// upstream raw bodies, request headers, or HTML error pages.
pub fn response_error(value: &Value) -> Option<String> {
    let error = value.get("error").filter(|error| !error.is_null())?;
    let message = error
        .as_str()
        .or_else(|| error.get("message").and_then(Value::as_str));
    let code = error.get("code").or_else(|| error.get("type"));
    let label = match code {
        Some(Value::Number(n)) if n.as_u64().is_some_and(|n| (400..=599).contains(&n)) => {
            format!(" (HTTP {n})")
        }
        Some(Value::Number(n)) => format!(" ({n})"),
        Some(Value::String(code)) => format!(" ({})", bounded(code, 128)),
        _ => String::new(),
    };
    Some(format!(
        "Provider error{label}: {}",
        message
            .map(|s| bounded(s, 2048))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "No error message supplied.".into())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn native_limit_overrides_normalized_tool_completion() {
        let error = finish_error(Some("tool_calls"), Some("max_output_tokens")).unwrap();
        assert!(error.contains("output token limit"));
        assert!(error.contains("max_output_tokens"));
        assert!(finish_error(Some("tool_calls"), Some("completed")).is_none());
    }

    #[test]
    fn error_details_are_bounded_and_exclude_metadata() {
        let error=response_error(&json!({"error":{"code":429,"message":"Rate limit exceeded","metadata":{"Authorization":"secret"}}})).unwrap();
        assert_eq!(error, "Provider error (HTTP 429): Rate limit exceeded");
        assert!(!error.contains("secret"));
        assert!(response_error(&json!({"error":null})).is_none());
        let error = response_error(
            &json!({"error":{"type":"overloaded_error","message":"x".repeat(10000)}}),
        )
        .unwrap();
        assert!(error.len() < 2200);
    }
}
