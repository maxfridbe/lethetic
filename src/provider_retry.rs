//! Automatic retries of a model request that failed for a transient reason.
//!
//! A request is retried when the model server dropped the connection, sent a
//! 5xx or 429 status, or ended its reply without finishing it. Requests
//! rejected for a reason a retry cannot fix (a 4xx status, an invalid tool
//! call, a request-policy error) are not retried. Any partial reply is
//! discarded and the same request is sent again after a short, growing delay.
//!
//! `provider_retries` in the config sets the global count (default 2); the
//! same key on a model server overrides it for that connection; 0 disables.

use std::time::Duration;

/// Retries per failed request when the config does not say.
pub const DEFAULT_RETRIES: u32 = 2;

/// How many times a failed request is retried under `config`.
pub fn retries_for(config: &crate::config::Config) -> u32 {
    config
        .active_model_server()
        .and_then(|server| server.provider_retries)
        .or(config.provider_retries)
        .unwrap_or(DEFAULT_RETRIES)
}

/// Wait before retry `attempt` (1-based): 3 s, 6 s, 12 s, … capped at 30 s.
pub fn delay(attempt: u32) -> Duration {
    let seconds = 3u64.saturating_mul(1u64 << attempt.saturating_sub(1).min(4));
    Duration::from_secs(seconds.min(30))
}

/// True when `error` came from a failure a fresh request may not repeat.
pub fn is_retryable(error: &str) -> bool {
    const TRANSIENT: &[&str] = &[
        "Request failed:",
        "request failed:",
        "is unavailable at",
        "Stream error:",
        "closed the reply before finishing",
        "Provider stream ended without a terminal event",
        "Provider stream ended before terminal usage",
        "did not send terminal usage/framing",
        "error decoding response body",
        "connection",
        "Connection",
        "timed out",
    ];
    if let Some(status) = http_status(error) {
        return status == 429 || status >= 500;
    }
    TRANSIENT.iter().any(|marker| error.contains(marker))
}

/// The HTTP status in `Server 503 Service Unavailable: …` style messages.
fn http_status(error: &str) -> Option<u16> {
    let rest = error
        .strip_prefix("Server ")
        .or_else(|| error.strip_prefix("claude-code-proxy "))
        .or_else(|| error.split("HTTP ").nth(1))?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    (digits.len() == 3).then(|| digits.parse().ok()).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_failures_are_retried_and_rejections_are_not() {
        for retryable in [
            "The model server closed the reply before finishing it (it may have crashed or restarted)",
            "Request failed: error sending request for url (http://hardac:8000/v1/chat/completions)",
            "Stream error: error decoding response body",
            "Server 503 Service Unavailable: loading model",
            "Server 429 Too Many Requests: slow down",
            "claude-code-proxy 529 (overloaded_error): Overloaded",
        ] {
            assert!(is_retryable(retryable), "{retryable}");
        }
        for fatal in [
            "Server 401 Unauthorized: bad key",
            "Server 400 Bad Request: context length exceeded",
            "claude-code-proxy 400 Bad Request (invalid_request_error): bad",
            "Provider returned invalid tool-call arguments",
            "Python-only OpenAI request must disable parallel tools",
        ] {
            assert!(!is_retryable(fatal), "{fatal}");
        }
    }

    #[test]
    fn delays_grow_and_cap() {
        assert_eq!(delay(1), Duration::from_secs(3));
        assert_eq!(delay(2), Duration::from_secs(6));
        assert_eq!(delay(3), Duration::from_secs(12));
        assert_eq!(delay(9), Duration::from_secs(30));
    }

    #[test]
    fn a_model_server_overrides_the_global_count() {
        let mut config: crate::config::Config = serde_yaml::from_str(
            "server_url: http://x/v1/chat/completions\nmodel: m\ncontext_size: 1000\nprovider_retries: 4\nactive_server: local\nmodel_servers:\n  - id: local\n    name: Local\n    url: http://x/v1/chat/completions\n    model: m\n    provider_retries: 0\n  - id: other\n    name: Other\n    url: http://y/v1/chat/completions\n    model: m\n",
        )
        .unwrap();
        assert_eq!(retries_for(&config), 0);
        config.active_server = Some("other".to_string());
        assert_eq!(retries_for(&config), 4);
        config.provider_retries = None;
        assert_eq!(retries_for(&config), DEFAULT_RETRIES);
    }
}
