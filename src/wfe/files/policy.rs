use super::{FileIdentity, FilesError};
use std::collections::HashSet;
use std::fmt;
use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExclusionKind {
    Protected,
    Unsupported,
    Unreadable,
}

#[derive(Clone, Default)]
pub struct DisclosurePolicy {
    protected_paths: Vec<Vec<String>>,
    protected_identities: HashSet<FileIdentity>,
    exact_secrets: Vec<Zeroizing<Vec<u8>>>,
}

impl DisclosurePolicy {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_secret(&mut self, secret: &str) -> Result<(), FilesError> {
        if secret.is_empty() {
            return Ok(());
        }
        if secret.len() > super::MAX_REGISTERED_SECRET_BYTES {
            return Err(FilesError::BadRequest);
        }
        if !self
            .exact_secrets
            .iter()
            .any(|registered| registered.as_slice() == secret.as_bytes())
        {
            self.exact_secrets
                .push(Zeroizing::new(secret.as_bytes().to_vec()));
        }
        Ok(())
    }

    pub(crate) fn protect_components(&mut self, components: Vec<String>) {
        if !self.protected_paths.iter().any(|path| path == &components) {
            self.protected_paths.push(components);
        }
    }

    pub(crate) fn protect_identity(&mut self, identity: FileIdentity) {
        self.protected_identities.insert(identity);
    }

    pub(crate) fn path_is_protected(&self, components: &[&str]) -> bool {
        components
            .iter()
            .any(|component| default_protected_name(component))
            || self.bytes_are_sensitive(components.join("/").as_bytes())
            || self.protected_paths.iter().any(|protected| {
                protected.len() <= components.len()
                    && protected
                        .iter()
                        .zip(components)
                        .all(|(expected, actual)| expected == actual)
            })
    }

    pub(crate) fn root_is_protected(&self, components: &[&str]) -> bool {
        components
            .iter()
            .any(|component| default_protected_name(component))
    }

    pub(crate) fn identity_is_protected(&self, identity: FileIdentity) -> bool {
        self.protected_identities.contains(&identity)
    }

    pub(crate) fn bytes_are_sensitive(&self, bytes: &[u8]) -> bool {
        contains_any_exact(bytes, &self.exact_secrets)
            || has_private_key_marker(bytes)
            || has_known_token(bytes)
            || has_strong_credential_assignment(bytes)
    }
}

impl fmt::Debug for DisclosurePolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DisclosurePolicy")
            .field("protected_paths", &self.protected_paths.len())
            .field("protected_identities", &self.protected_identities.len())
            .field("exact_secrets", &self.exact_secrets.len())
            .finish()
    }
}

fn default_protected_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        ".lethetic"
            | ".git"
            | ".claude"
            | ".ssh"
            | ".gnupg"
            | ".aws"
            | ".azure"
            | ".kube"
            | ".docker"
            | ".password-store"
            | ".netrc"
            | ".npmrc"
            | ".pypirc"
            | ".git-credentials"
            | ".vault-token"
            | ".credentials"
            | ".secrets"
            | "credentials"
            | "credentials.json"
            | "secrets.json"
            | "service-account.json"
            | "id_rsa"
            | "id_dsa"
            | "id_ecdsa"
            | "id_ed25519"
            | "wfe-state.json"
            | "wfe-token"
            | "controller-token"
    ) {
        return true;
    }
    lower == ".env"
        || lower == ".envrc"
        || lower.starts_with(".envrc.")
        || lower.starts_with(".env.")
        || lower.starts_with(".env-")
        || lower.starts_with(".env_")
        || lower.ends_with(".env")
        || lower.ends_with(".p12")
        || lower.ends_with(".pfx")
        || lower.ends_with(".jks")
        || lower.ends_with(".keystore")
        || lower.ends_with(".key")
}

fn contains_any_exact(haystack: &[u8], needles: &[Zeroizing<Vec<u8>>]) -> bool {
    if needles.is_empty() {
        return false;
    }
    haystack.iter().enumerate().any(|(offset, first)| {
        needles.iter().any(|needle| {
            needle.first() == Some(first)
                && haystack[offset..]
                    .get(..needle.len())
                    .is_some_and(|candidate| candidate == needle.as_slice())
        })
    })
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    let Some(first) = needle.first() else {
        return false;
    };
    haystack.iter().enumerate().any(|(offset, candidate)| {
        candidate == first
            && haystack[offset..]
                .get(..needle.len())
                .is_some_and(|candidate| candidate == needle)
    })
}

fn contains_ascii_case_insensitive(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && needle.len() <= haystack.len()
        && haystack.windows(needle.len()).any(|candidate| {
            candidate
                .iter()
                .zip(needle)
                .all(|(left, right)| left.eq_ignore_ascii_case(right))
        })
}

fn has_private_key_marker(bytes: &[u8]) -> bool {
    if !contains_bytes(bytes, b"PRIVATE KEY") {
        return false;
    }
    const MARKERS: &[&[u8]] = &[
        b"-----BEGIN PRIVATE KEY-----",
        b"-----BEGIN ENCRYPTED PRIVATE KEY-----",
        b"-----BEGIN OPENSSH PRIVATE KEY-----",
        b"-----BEGIN RSA PRIVATE KEY-----",
        b"-----BEGIN DSA PRIVATE KEY-----",
        b"-----BEGIN EC PRIVATE KEY-----",
        b"-----BEGIN PGP PRIVATE KEY BLOCK-----",
    ];
    MARKERS.iter().any(|marker| contains_bytes(bytes, marker))
}

fn has_known_token(bytes: &[u8]) -> bool {
    const S_PREFIXES: &[(&[u8], usize)] = &[(b"sk-ant-", 24)];
    const G_PREFIXES: &[(&[u8], usize)] = &[
        (b"github_pat_", 32),
        (b"ghp_", 32),
        (b"gho_", 32),
        (b"ghu_", 32),
        (b"ghs_", 32),
        (b"ghr_", 32),
        (b"glpat-", 20),
    ];
    const X_PREFIXES: &[(&[u8], usize)] = &[
        (b"xoxb-", 24),
        (b"xoxp-", 24),
        (b"xoxa-", 24),
        (b"xoxr-", 24),
    ];
    const A_PREFIXES: &[(&[u8], usize)] = &[(b"AIza", 35)];
    const P_PREFIXES: &[(&[u8], usize)] = &[(b"pypi-AgEI", 40)];
    const N_PREFIXES: &[(&[u8], usize)] = &[(b"npm_", 32)];

    for (offset, byte) in bytes.iter().copied().enumerate() {
        if matches!(byte, b'A') && has_aws_access_key_at(bytes, offset) {
            return true;
        }
        let prefixes = match byte {
            b's' => S_PREFIXES,
            b'g' => G_PREFIXES,
            b'x' => X_PREFIXES,
            b'A' => A_PREFIXES,
            b'p' => P_PREFIXES,
            b'n' => N_PREFIXES,
            _ => continue,
        };
        for (prefix, minimum_tail) in prefixes {
            let Some(candidate) = bytes[offset..].get(..prefix.len()) else {
                continue;
            };
            if candidate != *prefix {
                continue;
            }
            let start = offset + prefix.len();
            let Some(tail) = bytes[start..].get(..*minimum_tail) else {
                continue;
            };
            if tail.iter().all(|byte| token_byte(*byte)) {
                return true;
            }
        }
    }
    false
}

fn token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'/' | b'+' | b'=')
}

fn has_aws_access_key_at(bytes: &[u8], start: usize) -> bool {
    let end = start.saturating_add(20);
    end <= bytes.len()
        && matches!(bytes[start..].get(..4), Some(b"AKIA" | b"ASIA"))
        && bytes[start..end]
            .iter()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        && (start == 0 || !bytes[start - 1].is_ascii_alphanumeric())
        && (end == bytes.len() || !bytes[end].is_ascii_alphanumeric())
}

fn has_strong_credential_assignment(bytes: &[u8]) -> bool {
    const KEYS: &[&[u8]] = &[
        b"api_key",
        b"apikey",
        b"secret_key",
        b"secret_access_key",
        b"access_token",
        b"refresh_token",
        b"auth_token",
        b"client_secret",
        b"private_key",
        b"password",
        b"passwd",
    ];
    bytes.split(|byte| *byte == b'\n').any(|line| {
        (line.contains(&b'=') || line.contains(&b':'))
            && KEYS
                .iter()
                .any(|key| assignment_value(line, key).is_some_and(strong_credential_value))
    })
}

fn assignment_value<'a>(line: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    let line = trim_ascii(line);
    if line.starts_with(b"#") || line.starts_with(b"//") || line.len() < key.len() {
        return None;
    }
    let mut key_at = None;
    for index in 0..=line.len() - key.len() {
        if line[index..index + key.len()]
            .iter()
            .zip(key)
            .all(|(left, right)| left.eq_ignore_ascii_case(right))
        {
            let before_ok =
                index == 0 || matches!(line[index - 1], b'\'' | b'"' | b' ' | b'\t' | b'{' | b',');
            let after = index + key.len();
            let after_ok = after == line.len()
                || matches!(line[after], b'\'' | b'"' | b' ' | b'\t' | b'=' | b':');
            if before_ok && after_ok {
                key_at = Some(after);
                break;
            }
        }
    }
    let mut remainder = &line[key_at?..];
    remainder = trim_ascii_start(remainder);
    if matches!(remainder.first(), Some(b'\'') | Some(b'"')) {
        remainder = &remainder[1..];
        remainder = trim_ascii_start(remainder);
    }
    if !matches!(remainder.first(), Some(b'=') | Some(b':')) {
        return None;
    }
    remainder = trim_ascii_start(&remainder[1..]);
    let quote = remainder
        .first()
        .copied()
        .filter(|byte| matches!(byte, b'\'' | b'"'));
    if quote.is_some() {
        remainder = &remainder[1..];
    }
    let mut value = trim_ascii(remainder);
    if let Some(quote) = quote
        && value.last() == Some(&quote)
    {
        value = trim_ascii(&value[..value.len() - 1]);
    }
    Some(value)
}

fn strong_credential_value(value: &[u8]) -> bool {
    if value.len() < 16
        || value.len() > 8 * 1024
        || value.iter().any(|byte| byte.is_ascii_whitespace())
    {
        return false;
    }
    const PLACEHOLDERS: &[&[u8]] = &[
        b"example",
        b"placeholder",
        b"changeme",
        b"replace_me",
        b"replace-me",
        b"redacted",
        b"not-a-real",
        b"dummy",
        b"your_",
        b"your-",
        b"${",
        b"{{",
    ];
    if PLACEHOLDERS
        .iter()
        .any(|placeholder| contains_ascii_case_insensitive(value, placeholder))
    {
        return false;
    }
    let has_lower = value.iter().any(u8::is_ascii_lowercase);
    let has_upper = value.iter().any(u8::is_ascii_uppercase);
    let has_digit = value.iter().any(u8::is_ascii_digit);
    let has_symbol = value.iter().any(|byte| !byte.is_ascii_alphanumeric());
    usize::from(has_lower)
        + usize::from(has_upper)
        + usize::from(has_digit)
        + usize::from(has_symbol)
        >= 3
}

fn trim_ascii(mut bytes: &[u8]) -> &[u8] {
    bytes = trim_ascii_start(bytes);
    while bytes.last().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

fn trim_ascii_start(mut bytes: &[u8]) -> &[u8] {
    while bytes.first().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[1..];
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_variants_are_protected_as_names() {
        let policy = DisclosurePolicy::new();
        for name in [
            ".env",
            ".env.local",
            ".env-production",
            ".env_test",
            ".envrc",
            ".envrc.local",
            "production.env",
        ] {
            assert!(policy.path_is_protected(&[name]), "{name}");
        }
        assert!(!policy.path_is_protected(&["environment.rs"]));
    }

    #[test]
    fn paths_are_scanned_before_serialization_and_across_components() {
        let mut policy = DisclosurePolicy::new();
        policy.register_secret("quoted-\"credential").unwrap();
        policy
            .register_secret("credential/across-components")
            .unwrap();
        assert!(policy.path_is_protected(&["src", "quoted-\"credential.txt"]));
        assert!(policy.path_is_protected(&["credential", "across-components.txt"]));
        assert!(policy.path_is_protected(&["-----BEGIN OPENSSH PRIVATE KEY-----"]));
        assert!(!policy.path_is_protected(&["src", "ordinary.rs"]));
    }

    #[test]
    fn strong_markers_reject_secrets_without_generic_false_positives() {
        let policy = DisclosurePolicy::new();
        assert!(policy.bytes_are_sensitive(b"-----BEGIN OPENSSH PRIVATE KEY-----\nabc"));
        assert!(policy.bytes_are_sensitive(b"client_secret = 'A9b$very-long-random-value'"));
        assert!(!policy.bytes_are_sensitive(b"let api_key = 'not-a-real-secret-placeholder';"));
        assert!(!policy.bytes_are_sensitive(b"the password field is documented here"));
    }

    #[test]
    fn short_lines_are_safe_and_not_credentials() {
        let policy = DisclosurePolicy::new();
        for content in [b"".as_slice(), b"x", b"api", b"{}", b"\n\n"] {
            assert!(!policy.bytes_are_sensitive(content));
        }
    }

    #[test]
    fn exact_secrets_include_short_registered_values() {
        let mut policy = DisclosurePolicy::new();
        policy.register_secret("s3cr3t").unwrap();
        assert!(policy.bytes_are_sensitive(b"prefix s3cr3t suffix"));
        assert!(!policy.bytes_are_sensitive(b"ordinary content"));
    }
}
