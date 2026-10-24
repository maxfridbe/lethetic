//! Retry classification and backoff shared by native and browser hosts.
use std::time::Duration;

pub const DEFAULT_RETRIES: u32 = 2;

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
        "provider stream ended",
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
    if error.starts_with("Provider stopped because")
        || error.starts_with("Provider blocked")
        || error.starts_with("Provider error")
        || error.starts_with("Provider could not complete")
    {
        return false;
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
