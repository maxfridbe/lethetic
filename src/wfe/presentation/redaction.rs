use crate::app::App;

#[derive(Clone)]
pub(super) struct RedactionOutcome {
    pub(super) text: String,
    pub(super) redacted: bool,
    pub(super) truncated: bool,
}

pub(super) struct Redactor {
    values: Vec<String>,
    /// Identifies this sensitive-value set in the memo below.
    values_fingerprint: u64,
}

/// Redaction results memoized per (sensitive-value set, text, limit). The
/// browser projection re-runs on every publish over the whole transcript,
/// and old blocks never change, so each text is scanned once.
const REDACTION_MEMO_CAPACITY: usize = 4096;

thread_local! {
    static REDACTION_MEMO: std::cell::RefCell<std::collections::HashMap<(u64, u64, usize), RedactionOutcome>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

fn fingerprint<T: std::hash::Hash + ?Sized>(value: &T) -> u64 {
    use std::hash::{BuildHasher, Hasher};
    static STATE: std::sync::LazyLock<std::collections::hash_map::RandomState> =
        std::sync::LazyLock::new(std::collections::hash_map::RandomState::new);
    let mut hasher = STATE.build_hasher();
    value.hash(&mut hasher);
    hasher.finish()
}

impl Redactor {
    pub(super) fn for_app(app: &App, additional: Vec<String>) -> Self {
        let mut raw_values = additional;
        if let Some(api_key) = &app.config.api_key {
            raw_values.push(api_key.clone());
        }
        raw_values.extend(
            app.config
                .model_servers
                .iter()
                .filter_map(|server| server.api_key.clone()),
        );
        raw_values.push(app.server_url.clone());
        raw_values.push(app.config.server_url.clone());
        raw_values.extend(
            app.config
                .model_servers
                .iter()
                .map(|server| server.url.clone()),
        );
        // Filesystem paths are not secrets: the browser shows them as-is.
        if let Some(runtime_id) = &app.python_runtime_id {
            raw_values.push(runtime_id.clone());
        }

        let mut values = Vec::new();
        for value in raw_values {
            add_sensitive_fragments(&mut values, &value, false);
        }
        add_sensitive_fragments(&mut values, &app.system_prompt, true);
        values.sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
        values.dedup();
        let values_fingerprint = fingerprint(&values);
        Self {
            values,
            values_fingerprint,
        }
    }

    pub(super) fn redact_and_truncate(&self, value: &str, max_bytes: usize) -> (String, bool) {
        let outcome = self.redact_and_truncate_detailed(value, max_bytes);
        let lossy = outcome.redacted || outcome.truncated;
        (outcome.text, lossy)
    }

    pub(super) fn redact_and_truncate_detailed(
        &self,
        value: &str,
        max_bytes: usize,
    ) -> RedactionOutcome {
        let key = (self.values_fingerprint, fingerprint(value), max_bytes);
        if let Some(hit) = REDACTION_MEMO.with(|memo| memo.borrow().get(&key).cloned()) {
            return hit;
        }
        let outcome = self.redact_and_truncate_uncached(value, max_bytes);
        REDACTION_MEMO.with(|memo| {
            let mut memo = memo.borrow_mut();
            if memo.len() >= REDACTION_MEMO_CAPACITY {
                memo.clear();
            }
            memo.insert(key, outcome.clone());
        });
        outcome
    }

    fn redact_and_truncate_uncached(&self, value: &str, max_bytes: usize) -> RedactionOutcome {
        if max_bytes == 0 {
            return RedactionOutcome {
                text: String::new(),
                redacted: false,
                truncated: !value.is_empty(),
            };
        }
        let lookahead = self
            .values
            .first()
            .map_or(0, |sensitive| sensitive.len())
            .min(4096);
        let preliminary_limit = max_bytes.saturating_add(lookahead);
        let (preliminary, preliminary_truncated) = truncate_utf8(value, preliminary_limit);
        let mut redacted = preliminary;
        let mut redaction_changed = false;
        for sensitive in &self.values {
            if redacted.contains(sensitive) {
                let replaced = redacted.replace(sensitive, "[REDACTED]");
                redaction_changed |= replaced != redacted;
                redacted = replaced;
            }
        }
        let scrubbed = scrub_sensitive_patterns(&redacted);
        redaction_changed |= scrubbed != redacted;
        let (text, final_truncated) = truncate_utf8(&scrubbed, max_bytes);
        RedactionOutcome {
            text,
            redacted: redaction_changed,
            truncated: preliminary_truncated || final_truncated,
        }
    }
}

fn add_sensitive_fragments(values: &mut Vec<String>, value: &str, include_excerpts: bool) {
    let value = value.trim();
    if value.len() < 4 {
        return;
    }
    if value.len() <= 4096 {
        values.push(value.to_string());
    }
    if !include_excerpts {
        if value.len() > 4096 {
            values.extend(character_windows(value, 128, 64, 128));
        }
        return;
    }
    values.extend(
        value
            .lines()
            .map(str::trim)
            .filter(|line| line.len() >= 16 && line.len() <= 4096)
            .take(256)
            .map(str::to_string),
    );
    values.extend(character_windows(value, 64, 32, 512));
}

fn character_windows(value: &str, width: usize, step: usize, limit: usize) -> Vec<String> {
    let characters = value.chars().collect::<Vec<_>>();
    if characters.len() < width {
        return Vec::new();
    }
    let maximum_start = characters.len() - width;
    let step = step.max(maximum_start.saturating_div(limit.max(1)).max(1));
    (0..=maximum_start)
        .step_by(step)
        .take(limit)
        .map(|start| characters[start..start + width].iter().collect())
        .collect()
}

fn scrub_sensitive_patterns(value: &str) -> String {
    use std::sync::OnceLock;

    fn expression(cell: &'static OnceLock<regex::Regex>, pattern: &str) -> &'static regex::Regex {
        cell.get_or_init(|| regex::Regex::new(pattern).expect("fixed redaction regex must compile"))
    }

    static SECRET: OnceLock<regex::Regex> = OnceLock::new();
    static BEARER: OnceLock<regex::Regex> = OnceLock::new();
    static JWT: OnceLock<regex::Regex> = OnceLock::new();
    static URL: OnceLock<regex::Regex> = OnceLock::new();
    static PODMAN_CONTAINER_ID: OnceLock<regex::Regex> = OnceLock::new();

    let secret = expression(
        &SECRET,
        r#"(?i)\b(api[_-]?key|authorization|access[_-]?token|refresh[_-]?token|session[_-]?(?:token|key|secret)|controller[_-]?(?:token|key)|client[_-]?secret|private[_-]?key|password|passwd|secret|cookie)\b\s*[:=]\s*[\"']?[A-Za-z0-9_./+=:@~-]{4,}[\"']?"#,
    );
    let bearer = expression(&BEARER, r"(?i)\bbearer\s+[A-Za-z0-9._~+/=-]{4,}");
    let jwt = expression(
        &JWT,
        r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}",
    );
    let url = expression(&URL, r#"(?i)\b(?:https?|wss?|file)://[^\s\"'<>]+"#);
    let podman_container_id = expression(
        &PODMAN_CONTAINER_ID,
        r"\b(Podman container)\s+[0-9A-Fa-f]{12,128}\b",
    );

    let value = bearer.replace_all(value, "Bearer [REDACTED]");
    let value = secret.replace_all(&value, "$1=[REDACTED]");
    let value = jwt.replace_all(&value, "[REDACTED-CREDENTIAL]");
    let value = url.replace_all(&value, "[REDACTED-URL]");
    let value = podman_container_id.replace_all(&value, "$1 [REDACTED-CONTAINER-ID]");
    scrub_opaque_candidates(&value)
}

fn scrub_opaque_candidates(value: &str) -> String {
    fn candidate_byte(byte: u8) -> bool {
        byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'+' | b'/' | b'-')
    }

    let bytes = value.as_bytes();
    let mut scrubbed = String::with_capacity(value.len());
    let mut copied_until = 0usize;
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if !candidate_byte(bytes[cursor]) {
            cursor += 1;
            continue;
        }
        let start = cursor;
        while cursor < bytes.len() && candidate_byte(bytes[cursor]) {
            cursor += 1;
        }
        let candidate = &value[start..cursor];
        if should_redact_opaque_candidate(candidate) {
            scrubbed.push_str(&value[copied_until..start]);
            scrubbed.push_str("[REDACTED-OPAQUE]");
            copied_until = cursor;
        }
    }
    if copied_until == 0 {
        return value.to_string();
    }
    scrubbed.push_str(&value[copied_until..]);
    scrubbed
}

fn should_redact_opaque_candidate(candidate: &str) -> bool {
    if recognized_credential_prefix(candidate) {
        return candidate.len() >= 16;
    }
    // Paths are shown as-is: judge each segment on its own so a genuine
    // high-entropy token inside a path is still caught.
    if candidate.starts_with('/') || candidate.matches('/').count() >= 2 {
        return candidate
            .split('/')
            .filter(|segment| !segment.is_empty())
            .any(should_redact_opaque_candidate);
    }
    if candidate.len() < 32
        || is_canonical_uuid(candidate)
        || is_ordinary_hex_digest(candidate)
        || is_lowercase_snake_identifier(candidate)
        || is_valid_operational_container_name(candidate)
    {
        return false;
    }

    let mut classes = [false; 4];
    for byte in candidate.bytes() {
        match byte {
            b'a'..=b'z' => classes[0] = true,
            b'A'..=b'Z' => classes[1] = true,
            b'0'..=b'9' => classes[2] = true,
            _ => classes[3] = true,
        }
    }
    let class_count = classes.into_iter().filter(|present| *present).count();
    let entropy = ascii_entropy(candidate);
    (classes[3] && class_count >= 2 && entropy >= 3.5)
        || (class_count >= 3 && entropy >= 3.8)
        || (classes[2] && class_count >= 2 && entropy >= 4.0)
}

fn recognized_credential_prefix(candidate: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "sk-",
        "sk_",
        "ghp_",
        "gho_",
        "ghu_",
        "ghs_",
        "ghr_",
        "github_pat_",
        "glpat-",
        "xoxb-",
        "xoxp-",
        "xoxa-",
        "xoxr-",
        "ya29-",
    ];
    PREFIXES.iter().any(|prefix| candidate.starts_with(prefix))
        || candidate.starts_with("AIza")
        || candidate.starts_with("AKIA")
        || candidate.starts_with("ASIA")
}

fn is_valid_operational_container_name(candidate: &str) -> bool {
    if crate::python::PythonContainerIdentity::transient(candidate).is_some() {
        return true;
    }
    candidate
        .strip_prefix("lethetic-python-")
        .and_then(|runtime_id| crate::python::PythonContainerIdentity::retained(runtime_id, false))
        .is_some_and(|identity| identity.name == candidate)
}

fn is_canonical_uuid(candidate: &str) -> bool {
    candidate.len() == 36
        && candidate
            .bytes()
            .enumerate()
            .all(|(index, byte)| match index {
                8 | 13 | 18 | 23 => byte == b'-',
                _ => byte.is_ascii_hexdigit(),
            })
}

fn is_ordinary_hex_digest(candidate: &str) -> bool {
    (32..=128).contains(&candidate.len())
        && candidate.len().is_multiple_of(2)
        && candidate.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_lowercase_snake_identifier(candidate: &str) -> bool {
    candidate.contains('_')
        && candidate
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        && candidate
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte == b'_')
}

fn ascii_entropy(candidate: &str) -> f64 {
    let mut frequencies = [0usize; 128];
    for byte in candidate.bytes() {
        frequencies[usize::from(byte)] += 1;
    }
    let length = candidate.len() as f64;
    frequencies
        .into_iter()
        .filter(|count| *count > 0)
        .map(|count| {
            let probability = count as f64 / length;
            -probability * probability.log2()
        })
        .sum()
}

pub(super) fn truncate_utf8(value: &str, max_bytes: usize) -> (String, bool) {
    if value.len() <= max_bytes {
        return (value.to_string(), false);
    }
    const MARKER: &str = "… [truncated]";
    if max_bytes < MARKER.len() {
        let mut end = max_bytes;
        while end > 0 && !MARKER.is_char_boundary(end) {
            end -= 1;
        }
        return (MARKER[..end].to_string(), true);
    }
    let mut end = max_bytes - MARKER.len();
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    let mut truncated = String::with_capacity(max_bytes);
    truncated.push_str(&value[..end]);
    truncated.push_str(MARKER);
    (truncated, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn ordinary_identifiers_uuids_and_hashes_are_not_opaque_secrets() {
        let harmless = concat!(
            "harmless_keyword_assignment_identifier = True\n",
            "550e8400-e29b-41d4-a716-446655440000\n",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n",
            "lethetic-python-550e8400-e29b-41d4-a716-446655440000\n",
            "lethetic-python-transient-123-4"
        );
        assert_eq!(scrub_sensitive_patterns(harmless), harmless);
    }

    #[test]
    fn podman_notice_context_redacts_container_ids_without_hiding_other_digests() {
        let container_id = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let notice = format!(
            "Podman container {container_id} resumed; network: none; mounted R/W cwd: /private/work"
        );
        let scrubbed = scrub_sensitive_patterns(&notice);
        assert!(!scrubbed.contains(container_id));
        assert!(scrubbed.contains("Podman container [REDACTED-CONTAINER-ID]"));
        assert!(scrubbed.contains("/private/work"), "paths are not redacted");
        assert_eq!(scrub_sensitive_patterns(container_id), container_id);
    }

    #[test]
    fn contextual_and_recognized_credentials_are_redacted() {
        let source = concat!(
            "api_key = abcdefghijklmnopqrstuvwxyz012345\n",
            "Authorization: Bearer abCdEf0123456789abCdEf0123456789\n",
            "ghp_abCdEf0123456789abCdEf0123456789\n",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abCdEf0123456789abCdEf"
        );
        let scrubbed = scrub_sensitive_patterns(source);
        assert!(!scrubbed.contains("abcdefghijklmnopqrstuvwxyz012345"));
        assert!(!scrubbed.contains("abCdEf0123456789abCdEf0123456789"));
        assert!(!scrubbed.contains("eyJhbGci"));
        assert!(scrubbed.contains("[REDACTED]"));
        assert!(scrubbed.contains("[REDACTED-OPAQUE]"));
        assert!(scrubbed.contains("[REDACTED-CREDENTIAL]"));
    }

    #[test]
    fn filesystem_paths_are_shown_unchanged() {
        for text in [
            "  File \"/home/example/.venv/lib/python3.13/site-packages/pkg/main.py\", line 1, in run",
            "wrote /var/home/user/Dev/test/src/main.rs",
            "C:\\Users\\me\\project\\main.rs",
        ] {
            assert_eq!(scrub_sensitive_patterns(text), text);
        }
    }

    #[test]
    fn exact_sensitive_provenance_overrides_generic_safe_shapes() {
        let mut app = App::new(&Config::default());
        let runtime_id = "550e8400-e29b-41d4-a716-446655440000";
        let explicit_secret = "ordinary_lowercase_snake_identifier_value";
        app.python_runtime_id = Some(runtime_id.to_string());
        let redactor = Redactor::for_app(&app, vec![explicit_secret.to_string()]);
        let outcome = redactor.redact_and_truncate_detailed(
            &format!(
                "runtime={runtime_id} secret={explicit_secret} safe=another_lowercase_snake_identifier"
            ),
            1024,
        );
        assert!(outcome.redacted);
        assert!(!outcome.truncated);
        assert!(!outcome.text.contains(runtime_id));
        assert!(!outcome.text.contains(explicit_secret));
        assert!(outcome.text.contains("another_lowercase_snake_identifier"));
    }

    #[test]
    fn mixed_high_entropy_candidates_are_redacted_but_low_entropy_text_is_not() {
        let secret = "aZ3mP8qR2vN7cK4xT9bL5sD1fH6jW0uE";
        let low_entropy = "abcabcabcabcabcabcabcabcabcabcabcabc";
        let scrubbed = scrub_sensitive_patterns(&format!("{secret}\n{low_entropy}"));
        assert!(!scrubbed.contains(secret));
        assert!(scrubbed.contains(low_entropy));
    }
}
