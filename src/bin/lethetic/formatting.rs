use std::io;

use lethetic::wfe::runtime::WfeConnectionEvent;
use lethetic::wfe::security::ControllerAuthenticationMode;

pub(crate) fn format_wfe_listener_addresses(addresses: &[std::net::SocketAddr]) -> String {
    addresses
        .iter()
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn format_wfe_connection_event(event: &WfeConnectionEvent) -> String {
    match event {
        WfeConnectionEvent::Connected(event) => {
            let family = match event.peer_ip {
                std::net::IpAddr::V4(_) => "IPv4",
                std::net::IpAddr::V6(_) => "IPv6",
            };
            let authentication = match event.authentication_mode {
                ControllerAuthenticationMode::TokenRequired => "token-required",
                ControllerAuthenticationMode::Disabled => "tokenless",
            };
            let rtt = event
                .round_trip_time
                .map(|duration| format!("{:.3}ms", duration.as_secs_f64() * 1_000.0))
                .unwrap_or_else(|| "unavailable".to_string());
            format!(
                "WFE connection #{} connected ip={} family={} active={} auth={} sequence={} revision={} rtt={}",
                event.connection_ordinal,
                event.peer_ip,
                family,
                event.active_clients,
                authentication,
                event.initial_sequence,
                event.initial_revision,
                rtt,
            )
        }
        WfeConnectionEvent::Disconnected(event) => format!(
            "WFE connection #{} disconnected ip={} active={} uptime={:.3}s category={}",
            event.connection_ordinal,
            event.peer_ip,
            event.active_clients,
            event.uptime.as_secs_f64(),
            event.category.as_str(),
        ),
    }
}

pub(crate) fn validate_wfe_bootstrap_acknowledgement(acknowledgement: &str) -> io::Result<()> {
    if acknowledgement.ends_with('\n') {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "stdin closed before secure WFE startup was acknowledged",
        ))
    }
}
/// Heuristic analysis of why the engine stopped.
/// Covers ~100 distinct outcomes by examining token counts, content, and state flags.
pub(crate) fn classify_done_reason(
    completion_tokens: Option<u32>,
    prompt_tokens: Option<u32>,
    content: &str,
    tool_processed: bool,
    max_tokens: usize,
) -> String {
    let comp = completion_tokens.unwrap_or(0) as usize;
    let prompt = prompt_tokens.unwrap_or(0) as usize;

    // Text after the thinking block (what the user actually saw)
    let text = if let Some(pos) = content.rfind("</think>") {
        content[pos + "</think>".len()..].trim()
    } else {
        content.trim()
    };

    // ── Errors / degenerate cases ─────────────────────────────────────────────
    if comp == 0 {
        return "⚠ Empty response — model produced no tokens".to_string();
    }
    if comp <= 5 && !tool_processed {
        if prompt > 0 && prompt as f64 / max_tokens as f64 > 0.88 {
            return format!(
                "⚠ Context saturated ({:.0}% full) — model emitted immediate EOS",
                prompt as f64 / max_tokens as f64 * 100.0
            );
        }
        if text.is_empty() {
            return format!(
                "⚠ Near-empty response ({} tokens) — possible prompt/template mismatch",
                comp
            );
        }
    }

    // ── Tool dispatched ───────────────────────────────────────────────────────
    if tool_processed {
        return "Tool dispatched → awaiting result".to_string();
    }

    // ── Context pressure ─────────────────────────────────────────────────────
    let ctx_pct = if prompt > 0 {
        prompt as f64 / max_tokens as f64 * 100.0
    } else {
        0.0
    };
    if ctx_pct > 90.0 {
        return format!("⚠ Context {:.0}% full — consider /new to reset", ctx_pct);
    }

    // ── Response length heuristics ────────────────────────────────────────────
    let word_count = text.split_whitespace().count();
    if comp < 20 && word_count < 5 {
        return format!(
            "⚠ Minimal response ({} tokens, {} words) — model may be confused",
            comp, word_count
        );
    }

    // ── Normal completion ─────────────────────────────────────────────────────
    if ctx_pct > 70.0 {
        format!(
            "Response complete ({} tokens, context {:.0}% full)",
            comp, ctx_pct
        )
    } else {
        format!("Response complete ({} tokens)", comp)
    }
}

/// Returns true when the model wrote its intention in text without issuing a tool call.
/// Looks at the text portion only (after any </think> block) to avoid false positives
/// from reasoning content.
pub(crate) fn looks_like_intention_without_action(content: &str) -> bool {
    // Extract text after the thought block
    let text = if let Some(pos) = content.rfind("</think>") {
        &content[pos + "</think>".len()..]
    } else {
        content
    };
    let text = text.trim();

    // Only flag short responses — a long response is likely a legitimate answer
    if text.is_empty() || text.len() > 400 {
        return false;
    }

    let lower = text.to_lowercase();
    let intent_phrases = [
        "let's read",
        "let me read",
        "i will read",
        "i'll read",
        "let's write",
        "let me write",
        "i will write",
        "i'll write",
        "let's run",
        "let me run",
        "i will run",
        "i'll run",
        "let's search",
        "let me search",
        "i will search",
        "let's call",
        "let me call",
        "i will call",
        "i'll call",
        "now i'll",
        "now let's",
        "i need to call",
        "i should call",
        "i'm going to call",
        "i will use",
        "i'll use",
    ];
    intent_phrases.iter().any(|p| lower.contains(p))
}

pub(crate) fn truncate_chars_with_ellipsis(
    value: &str,
    maximum_chars: usize,
    prefix_chars: usize,
) -> String {
    if value.chars().count() > maximum_chars {
        format!("{}…", value.chars().take(prefix_chars).collect::<String>())
    } else {
        value.to_string()
    }
}

pub(crate) fn print_accounting_estimates(
    turn: &lethetic::accounting::AccountingTotals,
    session: &lethetic::accounting::AccountingTotals,
) {
    let reported = session
        .estimated_cost
        .as_ref()
        .is_some_and(|cost| cost.provenance_kind == "provider_reported");
    if reported {
        println!(
            "\nCost turn: {}\nCost session: {}\n(Charged amount reported by the provider.)",
            format_accounting_estimate(turn),
            format_accounting_estimate(session),
        );
        return;
    }
    println!(
        "\nEST API-eq turn: {}\nEST API-eq session: {}\n(API-equivalent estimate; OAuth/Codex subscription or credit billing may differ.)",
        format_accounting_estimate(turn),
        format_accounting_estimate(session),
    );
}

fn format_accounting_estimate(totals: &lethetic::accounting::AccountingTotals) -> String {
    let Some(mut cost) = totals.estimated_cost.clone() else {
        return if totals.request_count == 0 {
            "unavailable*".to_string()
        } else {
            "unpriced*".to_string()
        };
    };
    if totals.unpriced_request_count > 0 || totals.incomplete_usage_request_count > 0 {
        cost.incomplete = true;
    }
    let amount = if cost.currency == "USD" {
        format!("${:.6}", cost.amount())
    } else {
        format!("{:.6} {}", cost.amount(), cost.currency)
    };
    let incomplete = if cost.incomplete { '*' } else { '\0' };
    let stale = if cost.is_stale_on(chrono::Local::now().date_naive()) {
        '†'
    } else {
        '\0'
    };
    format!("{amount}{incomplete}{stale}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn multibyte_truncation_preserves_utf8_boundaries_and_threshold() {
        let rendered = truncate_chars_with_ellipsis(&"界".repeat(81), 80, 77);
        assert_eq!(rendered.chars().count(), 78);
        assert!(rendered.ends_with('…'));
        assert_eq!(
            truncate_chars_with_ellipsis(&"界".repeat(80), 80, 77),
            "界".repeat(80)
        );
        assert_eq!(truncate_chars_with_ellipsis("éclair", 80, 77), "éclair");
    }

    #[test]
    fn wfe_connection_telemetry_is_stable_and_secret_free() {
        let connected = WfeConnectionEvent::Connected(lethetic::wfe::runtime::WfeConnectedEvent {
            peer_ip: "2001:db8::7".parse().unwrap(),
            connection_ordinal: 12,
            active_clients: 2,
            authentication_mode: ControllerAuthenticationMode::Disabled,
            initial_sequence: 9,
            initial_revision: 4,
            round_trip_time: Some(Duration::from_micros(1_250)),
        });
        assert_eq!(
            format_wfe_connection_event(&connected),
            "WFE connection #12 connected ip=2001:db8::7 family=IPv6 active=2 auth=tokenless sequence=9 revision=4 rtt=1.250ms"
        );

        let disconnected =
            WfeConnectionEvent::Disconnected(lethetic::wfe::runtime::WfeDisconnectedEvent {
                peer_ip: "2001:db8::7".parse().unwrap(),
                connection_ordinal: 12,
                active_clients: 1,
                uptime: Duration::from_millis(2_500),
                category: lethetic::wfe::runtime::WfeDisconnectCategory::PeerClosed,
            });
        assert_eq!(
            format_wfe_connection_event(&disconnected),
            "WFE connection #12 disconnected ip=2001:db8::7 active=1 uptime=2.500s category=peer-closed"
        );
    }
}
