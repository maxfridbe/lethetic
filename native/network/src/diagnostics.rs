//! Bounded, in-memory relay diagnostics. No paths, queries, headers or bodies are retained.
use crate::{NetworkMode, Request};
use serde::Serialize;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const MAX_REQUESTS: usize = 200;
#[derive(Clone, Default)]
pub struct Diagnostics(Arc<Mutex<Snapshot>>);
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub requests: VecDeque<Entry>,
    pub omitted_requests: usize,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub started_at_ms: u64,
    pub duration_ms: u64,
    pub hostname: String,
    pub method: String,
    pub mode: NetworkMode,
    pub relay_status: u16,
    pub target_status: Option<u16>,
    pub error: Option<String>,
}
impl Diagnostics {
    pub fn snapshot(&self) -> Snapshot {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub(crate) fn record(&self, entry: Entry) {
        let mut log = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if log.requests.len() == MAX_REQUESTS {
            log.requests.pop_front();
            log.omitted_requests += 1;
        }
        log.requests.push_back(entry);
    }
}
impl Entry {
    pub(crate) fn new(body: &[u8], mode: NetworkMode) -> Self {
        let request = serde_json::from_slice::<Request>(body).ok();
        let hostname = request
            .as_ref()
            .and_then(|r| reqwest::Url::parse(&r.url).ok())
            .and_then(|u| u.host_str().map(str::to_owned))
            .unwrap_or_default();
        let method = request
            .as_ref()
            .map(|r| r.method.as_str())
            .filter(|m| {
                matches!(
                    *m,
                    "GET" | "HEAD" | "OPTIONS" | "POST" | "PUT" | "PATCH" | "DELETE"
                )
            })
            .unwrap_or("invalid")
            .to_owned();
        Self {
            started_at_ms: millis(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default(),
            ),
            duration_ms: 0,
            hostname,
            method,
            mode,
            relay_status: 200,
            target_status: None,
            error: None,
        }
    }
}
pub(crate) fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Errors originate in the transport, never in target response bodies. Scrub any
/// request URL or header values echoed by libraries before displaying or recording.
pub(crate) fn safe_error(error: &str, body: &[u8]) -> String {
    let Ok(request) = serde_json::from_slice::<Request>(body) else {
        return "Invalid relay request".into();
    };
    let mut secrets = vec![request.url.clone()];
    secrets.extend(request.headers.into_values());
    if let Ok(url) = reqwest::Url::parse(&request.url) {
        secrets.extend(url.query_pairs().map(|(_, v)| v.into_owned()));
    }
    secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let mut message = error.to_owned();
    for secret in secrets.iter().filter(|s| !s.is_empty()) {
        message = message.replace(secret, "[REDACTED]");
    }
    message
        .split_whitespace()
        .map(|word| if word.contains("://") { "[URL]" } else { word })
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| {
            !c.is_control() && !matches!(*c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .take(1024)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagnostics_bound_history_and_remove_request_secrets() {
        let body = br#"{"url":"https://example.org/private?key=secret-query","method":"GET","headers":{"authorization":"Bearer secret-header"},"bodyBase64":null}"#;
        let error = safe_error(
            "TLS failed https://example.org/private?key=secret-query Bearer secret-header secret-query",
            body,
        );
        assert!(error.contains("TLS failed"));
        assert!(!error.contains("secret"));
        assert!(!error.contains("/private"));
        let log = Diagnostics::default();
        for _ in 0..205 {
            log.record(Entry::new(body, NetworkMode::Tor));
        }
        let snapshot = log.snapshot();
        assert_eq!(snapshot.omitted_requests, 5);
        assert_eq!(snapshot.requests.len(), MAX_REQUESTS);
        assert_eq!(snapshot.requests[0].hostname, "example.org");
        assert!(!serde_json::to_string(&snapshot).unwrap().contains("secret"));
    }
}
