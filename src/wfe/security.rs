//! Strict WFE target parsing, TLS identity handling, and controller-token security.
//!
//! This module deliberately contains no listener, HTTP, WebSocket, proxy, or
//! browser-launching code. It prepares the security material those layers need.

use base64::Engine as _;
use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use rcgen::{
    CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose, PKCS_ECDSA_P256_SHA256, PublicKeyData, SanType, SerialNumber,
};
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
#[cfg(unix)]
use std::fs::{File, OpenOptions};
use std::io::Cursor;
#[cfg(unix)]
use std::io::Read as _;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;
use url::{Host, Origin, Url};
use x509_parser::extensions::GeneralName;
use x509_parser::parse_x509_certificate;
use x509_parser::time::ASN1Time;
use zeroize::{Zeroize, Zeroizing};

/// The simple loopback endpoint shown in CLI help and documentation.
pub const DEFAULT_WFE_TARGET: &str = "https://127.0.0.1:11223";

pub const MIN_TOKEN_DECODED_BYTES: usize = 32;
pub const MAX_CERTIFICATE_FILE_BYTES: usize = 1024 * 1024;
pub const MAX_PRIVATE_KEY_FILE_BYTES: usize = 256 * 1024;
pub const MAX_TOKEN_FILE_BYTES: usize = 4096;

const MAX_MANIFEST_FILE_BYTES: usize = 16 * 1024;
const MAX_CERTIFICATE_CHAIN_LEN: usize = 8;
const PRIVATE_DIRECTORY_MODE: u32 = 0o700;
const PRIVATE_FILE_MODE: u32 = 0o600;
const GENERATED_CERTIFICATE_FILE: &str = "certificate.der";
const GENERATED_PRIVATE_KEY_FILE: &str = "private-key.der";
const GENERATED_MANIFEST_FILE: &str = "manifest.json";
#[cfg(target_os = "linux")]
const GENERATED_IDENTITY_LOCK_FILE: &str = ".generation.lock";
const IP_MANIFEST_VERSION: u32 = 2;
const DNS_MANIFEST_VERSION: u32 = 3;
const GENERATED_ALGORITHM: &str = "ECDSA_P256_SHA256";
const GENERATED_CERTIFICATE_LIFETIME_SECONDS: i64 = 30 * 24 * 60 * 60;
const GENERATED_CERTIFICATE_BACKDATE_SECONDS: i64 = 5 * 60;
const GENERATED_CERTIFICATE_ROTATE_BEFORE_SECONDS: i64 = 7 * 24 * 60 * 60;
const ECDSA_WITH_SHA256_OID: &str = "1.2.840.10045.4.3.2";

/// The sole logical host identity of a validated WFE endpoint.
#[derive(Clone, Debug, Eq, PartialEq)]
enum WfeHostIdentity {
    Ip(IpAddr),
    Dns(String),
}

/// A validated WFE HTTPS target.
///
/// Parsing is intentionally stricter than general URL parsing. The source must
/// consist only of an HTTPS scheme, one exact IP or lowercase DNS identity, and
/// an explicit, non-zero `u16` port.
#[derive(Clone, Eq, PartialEq)]
pub struct WfeTarget {
    source: String,
    url: Url,
    identity: WfeHostIdentity,
    port: u16,
}

impl WfeTarget {
    pub fn parse(value: &str) -> Result<Self, String> {
        parse_target(value)
    }

    pub fn url(&self) -> &Url {
        &self.url
    }

    pub fn ip_literal(&self) -> Option<IpAddr> {
        match self.identity {
            WfeHostIdentity::Ip(ip) => Some(ip),
            WfeHostIdentity::Dns(_) => None,
        }
    }

    pub fn dns_name(&self) -> Option<&str> {
        match &self.identity {
            WfeHostIdentity::Ip(_) => None,
            WfeHostIdentity::Dns(name) => Some(name),
        }
    }

    pub fn literal_socket_addr(&self) -> Option<SocketAddr> {
        self.ip_literal().map(|ip| SocketAddr::new(ip, self.port))
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn as_str(&self) -> &str {
        &self.source
    }

    pub fn origin(&self) -> Origin {
        self.url.origin()
    }

    pub fn canonical_origin(&self) -> String {
        self.origin().ascii_serialization()
    }

    pub fn canonical_authority(&self) -> String {
        self.canonical_origin()
            .strip_prefix("https://")
            .expect("a validated WFE origin always uses HTTPS")
            .to_string()
    }

    pub fn matches_origin_header(&self, candidate: &str) -> bool {
        candidate == self.canonical_origin()
    }

    pub fn matches_host_header(&self, candidate: &str) -> bool {
        candidate == self.canonical_authority()
    }

    /// Checks only scheme, host, and effective port. Paths and other URL
    /// components are intentionally irrelevant to an origin comparison.
    pub fn has_exact_origin(&self, candidate: &Url) -> bool {
        exact_same_origin(&self.url, candidate)
    }

    pub fn has_exact_origin_str(&self, candidate: &str) -> bool {
        Url::parse(candidate)
            .map(|candidate| self.has_exact_origin(&candidate))
            .unwrap_or(false)
    }
}

impl FromStr for WfeTarget {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl fmt::Display for WfeTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.source)
    }
}

impl fmt::Debug for WfeTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WfeTarget")
            .field("url", &self.source)
            .field("identity", &self.identity)
            .field("port", &self.port)
            .finish()
    }
}

pub fn parse_wfe_target(value: &str) -> Result<WfeTarget, String> {
    WfeTarget::parse(value)
}

fn parse_target(value: &str) -> Result<WfeTarget, String> {
    if value.is_empty() {
        return Err("--wfe target cannot be empty".to_string());
    }
    if value
        .chars()
        .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err("--wfe target cannot contain whitespace or control characters".to_string());
    }

    let (scheme, authority) = value
        .split_once("://")
        .ok_or_else(|| "--wfe target must be an HTTPS URL".to_string())?;
    if !scheme.eq_ignore_ascii_case("https") {
        return Err("--wfe target scheme must be https".to_string());
    }
    if authority.is_empty()
        || authority
            .bytes()
            .any(|byte| matches!(byte, b'/' | b'?' | b'#'))
    {
        return Err(
            "--wfe target must not contain credentials, a path, query, or fragment".to_string(),
        );
    }

    let url = Url::parse(value).map_err(|_| "--wfe target is not a valid HTTPS URL".to_string())?;
    if url.scheme() != "https" {
        return Err("--wfe target scheme must be https".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("--wfe target must not contain credentials".to_string());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("--wfe target must not contain a query or fragment".to_string());
    }

    // Preserve the existing IP-literal parser: SocketAddr simultaneously
    // requires an IP literal, IPv6 brackets, and an explicit u16 port.
    if let Ok(bind_addr) = authority.parse::<SocketAddr>() {
        if bind_addr.port() == 0 {
            return Err("--wfe target port must be in 1..=65535".to_string());
        }
        validate_concrete_ip(bind_addr.ip())?;
        let url_ip = match url.host() {
            Some(Host::Ipv4(ip)) => IpAddr::V4(ip),
            Some(Host::Ipv6(ip)) => IpAddr::V6(ip),
            _ => return Err("--wfe target host must be an IP literal".to_string()),
        };
        if url_ip != bind_addr.ip()
            || url.port_or_known_default() != Some(bind_addr.port())
            || url.path() != "/"
        {
            return Err("--wfe target authority is not canonical or complete".to_string());
        }
        return Ok(WfeTarget {
            source: value.to_string(),
            url,
            identity: WfeHostIdentity::Ip(bind_addr.ip()),
            port: bind_addr.port(),
        });
    }

    let (hostname, port_source) = authority.rsplit_once(':').ok_or_else(|| {
        "--wfe target must use an exact IP or lowercase DNS name and an explicit u16 port"
            .to_string()
    })?;
    if hostname.contains(':')
        || hostname.contains('@')
        || hostname.contains('[')
        || hostname.contains(']')
    {
        return Err("--wfe target DNS authority is malformed".to_string());
    }
    validate_dns_name(hostname)?;
    if port_source.is_empty() || !port_source.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("--wfe target port must be an explicit decimal u16".to_string());
    }
    let port = port_source
        .parse::<u16>()
        .ok()
        .filter(|port| *port > 0)
        .ok_or_else(|| "--wfe target port must be in 1..=65535".to_string())?;
    if port_source != port.to_string() {
        return Err("--wfe target DNS port must use canonical decimal notation".to_string());
    }
    match url.host() {
        Some(Host::Domain(parsed)) if parsed == hostname => {}
        _ => {
            return Err(
                "--wfe target DNS name is ambiguous with an IP address or URL alias".to_string(),
            );
        }
    }
    if url.port_or_known_default() != Some(port) || url.path() != "/" {
        return Err("--wfe target authority is not canonical or complete".to_string());
    }

    Ok(WfeTarget {
        source: value.to_string(),
        url,
        identity: WfeHostIdentity::Dns(hostname.to_string()),
        port,
    })
}

fn validate_dns_name(hostname: &str) -> Result<(), String> {
    if hostname.is_empty() || hostname.len() > 253 || !hostname.is_ascii() {
        return Err("--wfe target DNS name must contain 1..=253 ASCII bytes".to_string());
    }
    if !hostname.bytes().any(|byte| byte.is_ascii_lowercase()) {
        return Err("--wfe target DNS name cannot be numeric-only".to_string());
    }
    for label in hostname.split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err("--wfe target DNS labels must contain 1..=63 bytes".to_string());
        }
        if label.starts_with('-')
            || label.ends_with('-')
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(
                "--wfe target DNS names must use canonical lowercase LDH labels".to_string(),
            );
        }
    }
    Ok(())
}

pub(crate) fn validate_concrete_ip(ip: IpAddr) -> Result<(), String> {
    if matches!(ip, IpAddr::V6(ip) if ip.to_ipv4_mapped().is_some()) {
        return Err("--wfe target cannot use an IPv4-mapped IPv6 address".to_string());
    }
    if ip.is_unspecified()
        || ip.is_multicast()
        || matches!(ip, IpAddr::V4(ip) if ip == Ipv4Addr::BROADCAST)
    {
        return Err(
            "--wfe target cannot use a wildcard, broadcast, or multicast IP address".to_string(),
        );
    }
    Ok(())
}

/// Compares exactly the three origin components used by WFE admission.
pub fn exact_same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host().is_some()
        && left.host() == right.host()
        && left.port_or_known_default() == right.port_or_known_default()
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SecurityFileOptions {
    pub cert: Option<PathBuf>,
    pub key: Option<PathBuf>,
    pub token: Option<PathBuf>,
}

impl SecurityFileOptions {
    pub fn new(cert: Option<PathBuf>, key: Option<PathBuf>, token: Option<PathBuf>) -> Self {
        Self { cert, key, token }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SecurityProfile {
    AutomaticGenerated,
    ExplicitFiles,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerAuthenticationMode {
    TokenRequired,
    Disabled,
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct CertificateFingerprint([u8; 32]);

impl CertificateFingerprint {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_hex(self) -> String {
        hex_encode(&self.0)
    }
}

impl fmt::Display for CertificateFingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&hex_encode(&self.0))
    }
}

impl fmt::Debug for CertificateFingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("CertificateFingerprint")
            .field(&hex_encode(&self.0))
            .finish()
    }
}

/// A controller credential whose value and verification digest are scrubbed on
/// drop and omitted from Debug output.
pub struct ControllerSecret {
    value: Zeroizing<String>,
    digest: Zeroizing<[u8; 32]>,
}

impl ControllerSecret {
    fn new(value: String) -> Self {
        let digest = Zeroizing::new(Sha256::digest(value.as_bytes()).into());
        Self {
            value: Zeroizing::new(value),
            digest,
        }
    }

    /// Constant-time digest comparison for an already extracted bearer token.
    pub fn verify(&self, candidate: &str) -> bool {
        if candidate.len() > MAX_TOKEN_FILE_BYTES {
            return false;
        }
        let candidate_digest =
            Zeroizing::new(<[u8; 32]>::from(Sha256::digest(candidate.as_bytes())));
        bool::from(self.digest.as_slice().ct_eq(candidate_digest.as_slice()))
    }

    fn expose(&self) -> &str {
        self.value.as_str()
    }
}

impl fmt::Debug for ControllerSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ControllerSecret(<redacted>)")
    }
}

enum ControllerAuthentication {
    Token(ControllerSecret),
    Disabled,
}

impl ControllerAuthentication {
    fn mode(&self) -> ControllerAuthenticationMode {
        match self {
            Self::Token(_) => ControllerAuthenticationMode::TokenRequired,
            Self::Disabled => ControllerAuthenticationMode::Disabled,
        }
    }
}

impl fmt::Debug for ControllerAuthentication {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Token(secret) => formatter.debug_tuple("Token").field(secret).finish(),
            Self::Disabled => formatter.write_str("Disabled"),
        }
    }
}

/// Fully validated material ready for a TLS listener and controller admission.
pub struct PreparedSecurity {
    target: WfeTarget,
    profile: SecurityProfile,
    server_config: Arc<ServerConfig>,
    certificate_chain: Vec<CertificateDer<'static>>,
    fingerprint: CertificateFingerprint,
    controller_authentication: ControllerAuthentication,
}

impl PreparedSecurity {
    pub fn target(&self) -> &WfeTarget {
        &self.target
    }

    pub fn profile(&self) -> SecurityProfile {
        self.profile
    }

    pub fn server_config(&self) -> Arc<ServerConfig> {
        Arc::clone(&self.server_config)
    }

    pub fn certificate_chain_der(&self) -> &[CertificateDer<'static>] {
        &self.certificate_chain
    }

    pub fn fingerprint(&self) -> CertificateFingerprint {
        self.fingerprint
    }

    pub fn fingerprint_sha256(&self) -> String {
        self.fingerprint.to_hex()
    }

    pub fn controller_authentication_mode(&self) -> ControllerAuthenticationMode {
        self.controller_authentication.mode()
    }

    pub fn verify_controller_token(&self, candidate: &str) -> bool {
        match &self.controller_authentication {
            ControllerAuthentication::Token(secret) => secret.verify(candidate),
            ControllerAuthentication::Disabled => false,
        }
    }

    /// Registers both explicit and generated controller credentials without
    /// exposing explicit file tokens through the host bootstrap accessor.
    pub(super) fn protect_file_credentials(
        &self,
        files: &mut super::files::RootedFiles,
    ) -> Result<(), super::files::FilesError> {
        match &self.controller_authentication {
            ControllerAuthentication::Token(secret) => files.register_secret(secret.expose()),
            ControllerAuthentication::Disabled => Ok(()),
        }
    }

    /// The generated token is available only to trusted host-side bootstrap
    /// code. Explicit file tokens are never exposed through this accessor.
    /// Callers must place it in a URL fragment or an authenticated exchange,
    /// never in a URL query or process argument.
    pub fn bootstrap_token_for_host(&self) -> Option<&str> {
        match (&self.profile, &self.controller_authentication) {
            (SecurityProfile::AutomaticGenerated, ControllerAuthentication::Token(secret)) => {
                Some(secret.expose())
            }
            _ => None,
        }
    }
}

impl fmt::Debug for PreparedSecurity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedSecurity")
            .field("target", &self.target)
            .field("profile", &self.profile)
            .field("fingerprint", &self.fingerprint)
            .field("server_config", &"<configured>")
            .field("controller_authentication", &self.controller_authentication)
            .finish()
    }
}

/// Returns the generated profile's private durable state root without creating
/// or modifying it.
pub fn automatic_security_state_root() -> PathBuf {
    crate::platform::lethetic_state_dir().join("wfe")
}

/// Validates an explicit identity, or loads/generates an exact-target identity.
/// Tests and embedding hosts can supply a private state-root override; normal
/// callers pass `None`.
pub fn prepare_security(
    target: &WfeTarget,
    files: SecurityFileOptions,
    state_root_override: Option<&Path>,
) -> Result<PreparedSecurity, String> {
    prepare_security_with_authentication(
        target,
        files,
        ControllerAuthenticationMode::TokenRequired,
        state_root_override,
    )
}

pub fn prepare_security_with_authentication(
    target: &WfeTarget,
    files: SecurityFileOptions,
    authentication_mode: ControllerAuthenticationMode,
    state_root_override: Option<&Path>,
) -> Result<PreparedSecurity, String> {
    prepare_security_at_with_authentication(
        target,
        files,
        authentication_mode,
        state_root_override,
        SystemTime::now(),
    )
}

#[cfg(test)]
fn prepare_security_at(
    target: &WfeTarget,
    files: SecurityFileOptions,
    state_root_override: Option<&Path>,
    now: SystemTime,
) -> Result<PreparedSecurity, String> {
    prepare_security_at_with_authentication(
        target,
        files,
        ControllerAuthenticationMode::TokenRequired,
        state_root_override,
        now,
    )
}

fn prepare_security_at_with_authentication(
    target: &WfeTarget,
    files: SecurityFileOptions,
    authentication_mode: ControllerAuthenticationMode,
    state_root_override: Option<&Path>,
    now: SystemTime,
) -> Result<PreparedSecurity, String> {
    let SecurityFileOptions { cert, key, token } = files;
    let selection = select_profile(cert, key)?;
    let authentication_selection = select_authentication(
        matches!(&selection, ProfileSelection::Explicit(_)),
        token,
        authentication_mode,
    )?;
    let (profile, material) = match selection {
        ProfileSelection::Automatic => {
            let default_root;
            let state_root = match state_root_override {
                Some(root) => root,
                None => {
                    default_root = automatic_security_state_root();
                    default_root.as_path()
                }
            };
            let material = load_or_generate_material(target, state_root, now)?;
            (SecurityProfile::AutomaticGenerated, material)
        }
        ProfileSelection::Explicit(paths) => {
            let material = load_explicit_material(target, &paths, now)?;
            (SecurityProfile::ExplicitFiles, material)
        }
    };
    let controller_authentication = match authentication_selection {
        AuthenticationSelection::GenerateToken => {
            ControllerAuthentication::Token(generate_controller_secret()?)
        }
        AuthenticationSelection::ExplicitToken(path) => {
            let bytes = read_explicit_private_file(&path, MAX_TOKEN_FILE_BYTES, "token")?;
            ControllerAuthentication::Token(parse_controller_secret(&bytes)?)
        }
        AuthenticationSelection::Disabled => ControllerAuthentication::Disabled,
    };

    let server_config =
        build_server_config(&material.chain, &material.private_key, &target.identity)?;
    Ok(PreparedSecurity {
        target: target.clone(),
        profile,
        server_config,
        certificate_chain: material.chain,
        fingerprint: material.fingerprint,
        controller_authentication,
    })
}

#[derive(Debug)]
struct ExplicitPaths {
    cert: PathBuf,
    key: PathBuf,
}

enum ProfileSelection {
    Automatic,
    Explicit(ExplicitPaths),
}

enum AuthenticationSelection {
    GenerateToken,
    ExplicitToken(PathBuf),
    Disabled,
}

fn select_profile(cert: Option<PathBuf>, key: Option<PathBuf>) -> Result<ProfileSelection, String> {
    match (cert, key) {
        (None, None) => Ok(ProfileSelection::Automatic),
        (Some(cert), Some(key)) => Ok(ProfileSelection::Explicit(ExplicitPaths { cert, key })),
        _ => Err("--wfe-tls-cert and --wfe-tls-key must be supplied together".to_string()),
    }
}

fn select_authentication(
    explicit_tls: bool,
    token: Option<PathBuf>,
    mode: ControllerAuthenticationMode,
) -> Result<AuthenticationSelection, String> {
    match (mode, explicit_tls, token) {
        (ControllerAuthenticationMode::Disabled, _, Some(_)) => {
            Err("--wfe-disable-authtoken conflicts with --wfe-auth-token-file".to_string())
        }
        (ControllerAuthenticationMode::Disabled, _, None) => Ok(AuthenticationSelection::Disabled),
        (ControllerAuthenticationMode::TokenRequired, false, None) => {
            Ok(AuthenticationSelection::GenerateToken)
        }
        (ControllerAuthenticationMode::TokenRequired, false, Some(_)) => {
            Err("--wfe-auth-token-file requires --wfe-tls-cert and --wfe-tls-key".to_string())
        }
        (ControllerAuthenticationMode::TokenRequired, true, Some(path)) => {
            Ok(AuthenticationSelection::ExplicitToken(path))
        }
        (ControllerAuthenticationMode::TokenRequired, true, None) => Err(
            "explicit WFE TLS requires --wfe-auth-token-file or --wfe-disable-authtoken"
                .to_string(),
        ),
    }
}

struct TlsMaterial {
    chain: Vec<CertificateDer<'static>>,
    private_key: TlsPrivateKey,
    fingerprint: CertificateFingerprint,
}

struct TlsPrivateKey {
    key: Zeroizing<PrivateKeyDer<'static>>,
}

impl TlsPrivateKey {
    fn new(key: PrivateKeyDer<'static>) -> Self {
        Self {
            key: Zeroizing::new(key),
        }
    }

    fn from_pkcs8(bytes: Vec<u8>) -> Self {
        Self::new(PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(bytes)))
    }

    fn clone_der(&self) -> PrivateKeyDer<'static> {
        self.key.clone_key()
    }

    fn secret_der(&self) -> &[u8] {
        self.key.secret_der()
    }

    fn as_key_der(&self) -> &PrivateKeyDer<'static> {
        &self.key
    }
}

impl fmt::Debug for TlsPrivateKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TlsPrivateKey(<redacted>)")
    }
}

struct ExactDnsCertResolver {
    hostname: String,
    certified_key: Arc<CertifiedKey>,
}

impl fmt::Debug for ExactDnsCertResolver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExactDnsCertResolver")
            .field("hostname", &self.hostname)
            .field("certified_key", &"<configured>")
            .finish()
    }
}

impl ResolvesServerCert for ExactDnsCertResolver {
    fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        client_hello
            .server_name()
            .is_some_and(|name| name.eq_ignore_ascii_case(&self.hostname))
            .then(|| Arc::clone(&self.certified_key))
    }
}

fn build_server_config(
    chain: &[CertificateDer<'static>],
    key: &TlsPrivateKey,
    identity: &WfeHostIdentity,
) -> Result<Arc<ServerConfig>, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = ServerConfig::builder_with_provider(Arc::clone(&provider))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|_| "could not enable the required TLS protocol version".to_string())?
        .with_no_client_auth();
    let mut config = match identity {
        WfeHostIdentity::Ip(_) => builder
            .with_single_cert(chain.to_vec(), key.clone_der())
            .map_err(|_| "certificate and private key are invalid or do not match".to_string())?,
        WfeHostIdentity::Dns(hostname) => {
            let certified_key =
                CertifiedKey::from_der(chain.to_vec(), key.clone_der(), provider.as_ref())
                    .map_err(|_| {
                        "certificate and private key are invalid or do not match".to_string()
                    })?;
            builder.with_cert_resolver(Arc::new(ExactDnsCertResolver {
                hostname: hostname.clone(),
                certified_key: Arc::new(certified_key),
            }))
        }
    };
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

fn load_explicit_material(
    target: &WfeTarget,
    paths: &ExplicitPaths,
    now: SystemTime,
) -> Result<TlsMaterial, String> {
    let cert_bytes =
        read_explicit_private_file(&paths.cert, MAX_CERTIFICATE_FILE_BYTES, "certificate")?;
    let key_bytes =
        read_explicit_private_file(&paths.key, MAX_PRIVATE_KEY_FILE_BYTES, "private key")?;
    let chain = parse_certificate_chain(&cert_bytes)?;
    validate_certificate_chain(&chain, &target.identity, now)?;
    let private_key = parse_private_key(&key_bytes)?;
    let fingerprint = fingerprint_for(&chain[0]);

    // Constructing the configuration is also the authoritative supported-key
    // and public/private consistency check. Do it before returning so explicit
    // material never reaches a listener partially validated.
    let _ = build_server_config(&chain, &private_key, &target.identity)?;

    Ok(TlsMaterial {
        chain,
        private_key,
        fingerprint,
    })
}

fn parse_certificate_chain(bytes: &[u8]) -> Result<Vec<CertificateDer<'static>>, String> {
    if bytes.is_empty() {
        return Err("certificate file is empty".to_string());
    }

    let mut certificates = Vec::new();
    if contains_pem_marker(bytes) {
        let mut reader = Cursor::new(bytes);
        for item in rustls_pemfile::read_all(&mut reader) {
            let item = item.map_err(|_| "certificate PEM is malformed".to_string())?;
            match item {
                rustls_pemfile::Item::X509Certificate(certificate) => {
                    certificates.push(certificate)
                }
                rustls_pemfile::Item::Pkcs1Key(key) => {
                    let mut key = PrivateKeyDer::Pkcs1(key);
                    key.zeroize();
                    return Err("certificate file contains private-key material".to_string());
                }
                rustls_pemfile::Item::Pkcs8Key(key) => {
                    let mut key = PrivateKeyDer::Pkcs8(key);
                    key.zeroize();
                    return Err("certificate file contains private-key material".to_string());
                }
                rustls_pemfile::Item::Sec1Key(key) => {
                    let mut key = PrivateKeyDer::Sec1(key);
                    key.zeroize();
                    return Err("certificate file contains private-key material".to_string());
                }
                _ => {
                    return Err("certificate file contains a non-certificate PEM block".to_string());
                }
            }
            if certificates.len() > MAX_CERTIFICATE_CHAIN_LEN {
                return Err(format!(
                    "certificate chain exceeds the {MAX_CERTIFICATE_CHAIN_LEN}-certificate limit"
                ));
            }
        }
    } else {
        certificates.push(CertificateDer::from(bytes.to_vec()));
    }

    if certificates.is_empty() {
        return Err("certificate file contains no certificate".to_string());
    }
    for certificate in &certificates {
        parse_single_certificate(certificate)?;
    }
    Ok(certificates)
}

fn parse_private_key(bytes: &[u8]) -> Result<TlsPrivateKey, String> {
    if bytes.is_empty() {
        return Err("private key file is empty".to_string());
    }

    if contains_pem_marker(bytes) {
        let mut reader = Cursor::new(bytes);
        let mut selected: Option<TlsPrivateKey> = None;
        for item in rustls_pemfile::read_all(&mut reader) {
            let item = item.map_err(|_| "private key PEM is malformed".to_string())?;
            let key = match item {
                rustls_pemfile::Item::Pkcs1Key(key) => Some(PrivateKeyDer::Pkcs1(key)),
                rustls_pemfile::Item::Pkcs8Key(key) => Some(PrivateKeyDer::Pkcs8(key)),
                rustls_pemfile::Item::Sec1Key(key) => Some(PrivateKeyDer::Sec1(key)),
                _ => return Err("private key file contains a non-key PEM block".to_string()),
            };
            if let Some(key) = key {
                if selected.is_some() {
                    let mut key = key;
                    key.zeroize();
                    return Err("private key file must contain exactly one key".to_string());
                }
                selected = Some(TlsPrivateKey::new(key));
            }
        }
        return selected.ok_or_else(|| "private key file contains no private key".to_string());
    }

    let key = PrivateKeyDer::try_from(bytes)
        .map_err(|_| "private key DER is malformed or unsupported".to_string())?;
    Ok(TlsPrivateKey::new(key.clone_key()))
}

fn contains_pem_marker(bytes: &[u8]) -> bool {
    bytes
        .windows(b"-----BEGIN".len())
        .any(|window| window == b"-----BEGIN")
}

#[derive(Clone, Copy, Debug)]
struct CertificateDetails {
    not_before_unix: i64,
    not_after_unix: i64,
}

fn validate_certificate_chain(
    chain: &[CertificateDer<'static>],
    expected_identity: &WfeHostIdentity,
    now: SystemTime,
) -> Result<CertificateDetails, String> {
    if chain.is_empty() {
        return Err("certificate chain is empty".to_string());
    }
    let now = asn1_time(now)?;

    for certificate in chain {
        let parsed = parse_single_certificate(certificate)?;
        if !parsed.validity().is_valid_at(now) {
            return Err("certificate is not valid at the current time".to_string());
        }
    }

    let leaf = parse_single_certificate(&chain[0])?;
    // Force duplicate-extension detection even for extensions not otherwise
    // interpreted below.
    leaf.extensions_map()
        .map_err(|_| "certificate contains duplicate or malformed extensions".to_string())?;

    let basic_constraints = leaf
        .basic_constraints()
        .map_err(|_| "certificate Basic Constraints extension is malformed".to_string())?
        .ok_or_else(|| "certificate must explicitly contain CA=false".to_string())?;
    if basic_constraints.value.ca {
        return Err("certificate must be a non-CA leaf".to_string());
    }

    let extended_key_usage = leaf
        .extended_key_usage()
        .map_err(|_| "certificate Extended Key Usage extension is malformed".to_string())?
        .ok_or_else(|| "certificate must contain a server-auth EKU".to_string())?;
    if !extended_key_usage.value.server_auth {
        return Err("certificate must contain a server-auth EKU".to_string());
    }

    let subject_alt_name = leaf
        .subject_alternative_name()
        .map_err(|_| "certificate Subject Alternative Name extension is malformed".to_string())?
        .ok_or_else(|| "certificate must contain exactly one identity SAN".to_string())?;
    let san_matches = subject_alt_name.value.general_names.len() == 1
        && match (expected_identity, &subject_alt_name.value.general_names[0]) {
            (WfeHostIdentity::Ip(expected_ip), GeneralName::IPAddress(bytes)) => {
                ip_san_matches(bytes, *expected_ip)
            }
            (WfeHostIdentity::Dns(expected_name), GeneralName::DNSName(candidate)) => {
                candidate.eq_ignore_ascii_case(expected_name)
            }
            _ => false,
        };
    if !san_matches {
        return Err(match expected_identity {
            WfeHostIdentity::Ip(_) => {
                "certificate SAN must be exactly the WFE target IP".to_string()
            }
            WfeHostIdentity::Dns(_) => {
                "certificate SAN must be exactly the WFE target DNS name".to_string()
            }
        });
    }

    Ok(CertificateDetails {
        not_before_unix: leaf.validity().not_before.timestamp(),
        not_after_unix: leaf.validity().not_after.timestamp(),
    })
}

fn parse_single_certificate<'a>(
    certificate: &'a CertificateDer<'a>,
) -> Result<x509_parser::certificate::X509Certificate<'a>, String> {
    let (remainder, parsed) = parse_x509_certificate(certificate.as_ref())
        .map_err(|_| "certificate DER is malformed".to_string())?;
    if !remainder.is_empty() {
        return Err("certificate DER contains trailing data".to_string());
    }
    Ok(parsed)
}

fn ip_san_matches(bytes: &[u8], expected_ip: IpAddr) -> bool {
    match expected_ip {
        IpAddr::V4(ip) => bytes == ip.octets(),
        IpAddr::V6(ip) => bytes == ip.octets(),
    }
}

fn fingerprint_for(certificate: &CertificateDer<'_>) -> CertificateFingerprint {
    CertificateFingerprint(Sha256::digest(certificate.as_ref()).into())
}

fn parse_controller_secret(bytes: &[u8]) -> Result<ControllerSecret, String> {
    let line_feeds = bytes.iter().filter(|byte| **byte == b'\n').count();
    if line_feeds > 1 || (line_feeds == 1 && bytes.last() != Some(&b'\n')) {
        return Err("token may contain at most one trailing LF".to_string());
    }
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    if bytes.is_empty() {
        return Err("token is empty".to_string());
    }
    let value = std::str::from_utf8(bytes)
        .map_err(|_| "token must be UTF-8 URL-safe base64".to_string())?;

    let decoded = URL_SAFE_NO_PAD
        .decode(value)
        .or_else(|_| URL_SAFE.decode(value))
        .map_err(|_| "token must be URL-safe base64".to_string())?;
    let decoded = Zeroizing::new(decoded);
    if decoded.len() < MIN_TOKEN_DECODED_BYTES {
        return Err(format!(
            "token must decode to at least {MIN_TOKEN_DECODED_BYTES} bytes"
        ));
    }

    Ok(ControllerSecret::new(value.to_string()))
}

fn generate_controller_secret() -> Result<ControllerSecret, String> {
    let mut random = Zeroizing::new([0_u8; MIN_TOKEN_DECODED_BYTES]);
    getrandom::fill(random.as_mut())
        .map_err(|_| "operating-system randomness is unavailable".to_string())?;
    Ok(ControllerSecret::new(
        URL_SAFE_NO_PAD.encode(random.as_ref()),
    ))
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct GeneratedIpManifest {
    version: u32,
    target_ip: String,
    algorithm: String,
    certificate_sha256: String,
    not_before_unix: i64,
    not_after_unix: i64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct GeneratedDnsManifest {
    version: u32,
    identity: GeneratedDnsManifestIdentity,
    algorithm: String,
    certificate_sha256: String,
    not_before_unix: i64,
    not_after_unix: i64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum GeneratedDnsManifestIdentity {
    #[serde(rename = "dns")]
    Dns { hostname: String },
}

fn generated_identity_directory_name(identity: &WfeHostIdentity) -> String {
    match identity {
        WfeHostIdentity::Ip(IpAddr::V4(ip)) => format!("ipv4-{}", hex_encode(&ip.octets())),
        WfeHostIdentity::Ip(IpAddr::V6(ip)) => format!("ipv6-{}", hex_encode(&ip.octets())),
        WfeHostIdentity::Dns(hostname) => {
            format!("dns-{}", hex_encode(&Sha256::digest(hostname.as_bytes())))
        }
    }
}

fn generated_identity_state_root(state_root: &Path, identity: &WfeHostIdentity) -> PathBuf {
    state_root.join(generated_identity_directory_name(identity))
}

struct GeneratedIdentityLock {
    #[cfg(target_os = "linux")]
    _file: File,
}

#[cfg(target_os = "linux")]
fn acquire_generated_identity_lock(identity_root: &Path) -> Result<GeneratedIdentityLock, String> {
    let file = crate::platform::open_lock_file_nofollow(
        identity_root,
        &[],
        GENERATED_IDENTITY_LOCK_FILE,
        PRIVATE_FILE_MODE,
    )
    .map_err(|error| format!("could not safely open generated TLS lock: {error}"))?;
    validate_open_private_file(
        &file,
        &identity_root.join(GENERATED_IDENTITY_LOCK_FILE),
        true,
        "generated TLS lock",
    )?;
    loop {
        match rustix::fs::flock(&file, rustix::fs::FlockOperation::LockExclusive) {
            Ok(()) => break,
            Err(rustix::io::Errno::INTR) => continue,
            Err(error) => {
                return Err(format!(
                    "could not acquire the generated TLS identity lock: {error}"
                ));
            }
        }
    }
    Ok(GeneratedIdentityLock { _file: file })
}

#[cfg(not(target_os = "linux"))]
fn acquire_generated_identity_lock(identity_root: &Path) -> Result<GeneratedIdentityLock, String> {
    let _ = identity_root;
    Err("automatic WFE security locking requires Linux no-follow filesystem support".to_string())
}

fn load_or_generate_material(
    target: &WfeTarget,
    state_root: &Path,
    now: SystemTime,
) -> Result<TlsMaterial, String> {
    ensure_private_state_root(state_root)?;
    let identity_root = generated_identity_state_root(state_root, &target.identity);
    ensure_private_state_root(&identity_root)?;
    let _generation_lock = acquire_generated_identity_lock(&identity_root)?;

    if let Ok(material) = load_reusable_material(target, &identity_root, now) {
        return Ok(material);
    }

    let (material, details) = generate_material(target, now)?;
    store_generated_material(target, &identity_root, &material, details)?;
    Ok(material)
}

fn load_reusable_material(
    target: &WfeTarget,
    state_root: &Path,
    now: SystemTime,
) -> Result<TlsMaterial, String> {
    let manifest_bytes =
        read_generated_state_file(state_root, GENERATED_MANIFEST_FILE, MAX_MANIFEST_FILE_BYTES)?;
    let (algorithm, certificate_sha256, not_before_unix, not_after_unix) = match &target.identity {
        WfeHostIdentity::Ip(expected_ip) => {
            let manifest: GeneratedIpManifest = serde_json::from_slice(&manifest_bytes)
                .map_err(|_| "generated TLS manifest is malformed".to_string())?;
            if manifest.version != IP_MANIFEST_VERSION
                || manifest.target_ip != expected_ip.to_string()
            {
                return Err("generated TLS manifest binding changed".to_string());
            }
            (
                manifest.algorithm,
                manifest.certificate_sha256,
                manifest.not_before_unix,
                manifest.not_after_unix,
            )
        }
        WfeHostIdentity::Dns(expected_hostname) => {
            let manifest: GeneratedDnsManifest = serde_json::from_slice(&manifest_bytes)
                .map_err(|_| "generated TLS manifest is malformed".to_string())?;
            let GeneratedDnsManifestIdentity::Dns { hostname } = manifest.identity;
            if manifest.version != DNS_MANIFEST_VERSION || hostname != *expected_hostname {
                return Err("generated TLS manifest binding changed".to_string());
            }
            (
                manifest.algorithm,
                manifest.certificate_sha256,
                manifest.not_before_unix,
                manifest.not_after_unix,
            )
        }
    };
    if algorithm != GENERATED_ALGORITHM {
        return Err("generated TLS manifest binding changed".to_string());
    }

    let cert_bytes = read_generated_state_file(
        state_root,
        GENERATED_CERTIFICATE_FILE,
        MAX_CERTIFICATE_FILE_BYTES,
    )?;
    let key_bytes = read_generated_state_file(
        state_root,
        GENERATED_PRIVATE_KEY_FILE,
        MAX_PRIVATE_KEY_FILE_BYTES,
    )?;
    let chain = parse_certificate_chain(&cert_bytes)?;
    if chain.len() != 1 {
        return Err("generated identity must contain exactly one certificate".to_string());
    }
    let private_key = parse_private_key(&key_bytes)?;
    let details = validate_certificate_chain(&chain, &target.identity, now)?;
    let fingerprint = fingerprint_for(&chain[0]);
    let now_unix = system_time_to_unix(now)?;
    if details.not_before_unix != not_before_unix
        || details.not_after_unix != not_after_unix
        || fingerprint.to_hex() != certificate_sha256
        || details.not_after_unix.saturating_sub(now_unix)
            <= GENERATED_CERTIFICATE_ROTATE_BEFORE_SECONDS
    {
        return Err("generated TLS manifest no longer matches reusable material".to_string());
    }

    validate_generated_leaf(&chain[0], &private_key)?;
    let _ = build_server_config(&chain, &private_key, &target.identity)?;
    Ok(TlsMaterial {
        chain,
        private_key,
        fingerprint,
    })
}

fn generate_material(
    target: &WfeTarget,
    now: SystemTime,
) -> Result<(TlsMaterial, CertificateDetails), String> {
    let now_unix = system_time_to_unix(now)?;
    let not_before_unix = now_unix
        .checked_sub(GENERATED_CERTIFICATE_BACKDATE_SECONDS)
        .ok_or_else(|| "certificate validity start is out of range".to_string())?;
    let not_after_unix = now_unix
        .checked_add(GENERATED_CERTIFICATE_LIFETIME_SECONDS)
        .ok_or_else(|| "certificate validity end is out of range".to_string())?;

    let mut serial = [0_u8; 16];
    getrandom::fill(&mut serial)
        .map_err(|_| "operating-system randomness is unavailable".to_string())?;
    serial[0] &= 0x7f;
    serial[0] |= 0x01;

    let mut distinguished_name = DistinguishedName::new();
    distinguished_name.push(DnType::CommonName, "Lethetic WFE");
    let mut parameters = CertificateParams::default();
    parameters.not_before = ASN1Time::from_timestamp(not_before_unix)
        .map_err(|_| "certificate validity start is out of range".to_string())?
        .to_datetime();
    parameters.not_after = ASN1Time::from_timestamp(not_after_unix)
        .map_err(|_| "certificate validity end is out of range".to_string())?
        .to_datetime();
    parameters.serial_number = Some(SerialNumber::from_slice(&serial));
    parameters.subject_alt_names = vec![match &target.identity {
        WfeHostIdentity::Ip(ip) => SanType::IpAddress(*ip),
        WfeHostIdentity::Dns(hostname) => SanType::DnsName(
            hostname
                .as_str()
                .try_into()
                .map_err(|_| "validated WFE DNS identity cannot be encoded as a SAN".to_string())?,
        ),
    }];
    parameters.distinguished_name = distinguished_name;
    parameters.is_ca = IsCa::ExplicitNoCa;
    parameters.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    parameters.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];

    let key_pair = Zeroizing::new(
        KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
            .map_err(|_| "could not generate an ECDSA P-256 private key".to_string())?,
    );
    let certificate = parameters
        .self_signed(&*key_pair)
        .map_err(|_| "could not generate the self-signed certificate".to_string())?;
    let private_key = TlsPrivateKey::from_pkcs8(key_pair.serialize_der());
    let chain = vec![certificate.der().clone()];
    let details = validate_certificate_chain(&chain, &target.identity, now)?;
    validate_generated_leaf(&chain[0], &private_key)?;
    let fingerprint = fingerprint_for(&chain[0]);

    Ok((
        TlsMaterial {
            chain,
            private_key,
            fingerprint,
        },
        details,
    ))
}

fn validate_generated_leaf(
    certificate: &CertificateDer<'_>,
    private_key: &TlsPrivateKey,
) -> Result<(), String> {
    let parsed = parse_single_certificate(certificate)?;
    if parsed.subject() != parsed.issuer() {
        return Err("generated certificate is not self-issued".to_string());
    }
    if parsed.signature_algorithm.algorithm.to_id_string() != ECDSA_WITH_SHA256_OID {
        return Err("generated certificate does not use ECDSA with SHA-256".to_string());
    }
    parsed
        .verify_signature(None)
        .map_err(|_| "generated certificate self-signature is invalid".to_string())?;

    let key_pair = Zeroizing::new(
        KeyPair::try_from(private_key.as_key_der())
            .map_err(|_| "generated private key is not ECDSA P-256 PKCS#8".to_string())?,
    );
    if !key_pair.is_compatible(&PKCS_ECDSA_P256_SHA256)
        || key_pair.subject_public_key_info() != parsed.public_key().raw
    {
        return Err("generated certificate and ECDSA P-256 key do not match".to_string());
    }
    Ok(())
}

fn store_generated_material(
    target: &WfeTarget,
    state_root: &Path,
    material: &TlsMaterial,
    details: CertificateDetails,
) -> Result<(), String> {
    let manifest_bytes = match &target.identity {
        WfeHostIdentity::Ip(ip) => serde_json::to_vec_pretty(&GeneratedIpManifest {
            version: IP_MANIFEST_VERSION,
            target_ip: ip.to_string(),
            algorithm: GENERATED_ALGORITHM.to_string(),
            certificate_sha256: material.fingerprint.to_hex(),
            not_before_unix: details.not_before_unix,
            not_after_unix: details.not_after_unix,
        }),
        WfeHostIdentity::Dns(hostname) => serde_json::to_vec_pretty(&GeneratedDnsManifest {
            version: DNS_MANIFEST_VERSION,
            identity: GeneratedDnsManifestIdentity::Dns {
                hostname: hostname.clone(),
            },
            algorithm: GENERATED_ALGORITHM.to_string(),
            certificate_sha256: material.fingerprint.to_hex(),
            not_before_unix: details.not_before_unix,
            not_after_unix: details.not_after_unix,
        }),
    }
    .map_err(|_| "could not serialize generated TLS manifest".to_string())?;

    // The manifest is the commit marker and is deliberately replaced last.
    write_generated_state_file(
        state_root,
        GENERATED_CERTIFICATE_FILE,
        material.chain[0].as_ref(),
    )?;
    write_generated_state_file(
        state_root,
        GENERATED_PRIVATE_KEY_FILE,
        material.private_key.secret_der(),
    )?;
    write_generated_state_file(state_root, GENERATED_MANIFEST_FILE, &manifest_bytes)?;
    Ok(())
}

fn ensure_private_state_root(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        crate::platform::ensure_private_directory_durable(path, PRIVATE_DIRECTORY_MODE).map_err(
            |error| {
                format!(
                    "could not secure WFE state root {}: {error}",
                    path.display()
                )
            },
        )?;
        validate_private_directory(path)?;
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = path;
        Err(
            "automatic WFE security storage requires Linux no-follow filesystem support"
                .to_string(),
        )
    }
}

#[cfg(unix)]
fn validate_private_directory(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| {
            format!(
                "could not open private directory {}: {error}",
                path.display()
            )
        })?;
    let metadata = directory.metadata().map_err(|error| {
        format!(
            "could not inspect private directory {}: {error}",
            path.display()
        )
    })?;
    if !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o7777 != PRIVATE_DIRECTORY_MODE
    {
        return Err(format!(
            "private directory {} must be owner-controlled mode 0700",
            path.display()
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_private_directory(path: &Path) -> Result<(), String> {
    let _ = path;
    Err("private WFE storage requires Unix no-follow filesystem support".to_string())
}

fn write_generated_state_file(path: &Path, file_name: &str, bytes: &[u8]) -> Result<(), String> {
    crate::platform::atomic_write_nofollow(path, &[], file_name, bytes, PRIVATE_FILE_MODE)
        .map_err(|error| format!("could not atomically store {file_name}: {error}"))?;
    harden_generated_state_file(&path.join(file_name))
}

fn read_generated_state_file(
    path: &Path,
    file_name: &str,
    max_bytes: usize,
) -> Result<Zeroizing<Vec<u8>>, String> {
    let bytes = crate::platform::read_file_nofollow_bounded(path, &[], file_name, max_bytes)
        .map_err(|error| format!("could not safely read {file_name}: {error}"))?
        .ok_or_else(|| format!("generated state file {file_name} is missing"))?;
    validate_private_file_metadata(&path.join(file_name), true, file_name)?;
    Ok(Zeroizing::new(bytes))
}

#[cfg(unix)]
fn harden_generated_state_file(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| {
            format!(
                "could not reopen generated state {}: {error}",
                path.display()
            )
        })?;
    validate_open_private_file(&file, path, false, "generated state")?;
    file.set_permissions(std::fs::Permissions::from_mode(PRIVATE_FILE_MODE))
        .map_err(|error| format!("could not set mode 0600 on {}: {error}", path.display()))?;
    file.sync_all()
        .map_err(|error| format!("could not sync permissions on {}: {error}", path.display()))?;
    validate_open_private_file(&file, path, true, "generated state")?;
    Ok(())
}

#[cfg(not(unix))]
fn harden_generated_state_file(path: &Path) -> Result<(), String> {
    let _ = path;
    Err("private WFE storage requires Unix no-follow filesystem support".to_string())
}

#[cfg(unix)]
fn read_explicit_private_file(
    path: &Path,
    max_bytes: usize,
    label: &str,
) -> Result<Zeroizing<Vec<u8>>, String> {
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| {
            format!(
                "could not safely open {label} file {}: {error}",
                path.display()
            )
        })?;
    let before = validate_open_private_file(&file, path, false, label)?;
    if before.len() > max_bytes as u64 {
        return Err(format!(
            "{label} file {} exceeds the {max_bytes}-byte limit",
            path.display()
        ));
    }

    let mut bytes = Zeroizing::new(Vec::with_capacity(before.len() as usize));
    (&mut file)
        .take(max_bytes.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("could not read {label} file {}: {error}", path.display()))?;
    if bytes.len() > max_bytes {
        return Err(format!(
            "{label} file {} exceeds the {max_bytes}-byte limit",
            path.display()
        ));
    }

    let after = validate_open_private_file(&file, path, false, label)?;
    use std::os::unix::fs::MetadataExt;
    if before.dev() != after.dev()
        || before.ino() != after.ino()
        || before.uid() != after.uid()
        || before.nlink() != after.nlink()
        || before.mode() != after.mode()
        || before.len() != after.len()
        || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec()
        || before.ctime() != after.ctime()
        || before.ctime_nsec() != after.ctime_nsec()
    {
        return Err(format!(
            "{label} file {} changed while being read",
            path.display()
        ));
    }
    Ok(bytes)
}

#[cfg(not(unix))]
fn read_explicit_private_file(
    path: &Path,
    max_bytes: usize,
    label: &str,
) -> Result<Zeroizing<Vec<u8>>, String> {
    let _ = (path, max_bytes, label);
    Err("explicit WFE credentials require Unix no-follow filesystem support".to_string())
}

#[cfg(unix)]
fn validate_private_file_metadata(
    path: &Path,
    exact_mode: bool,
    label: &str,
) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;

    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| format!("could not inspect {label} file {}: {error}", path.display()))?;
    validate_open_private_file(&file, path, exact_mode, label).map(|_| ())
}

#[cfg(not(unix))]
fn validate_private_file_metadata(
    path: &Path,
    exact_mode: bool,
    label: &str,
) -> Result<(), String> {
    let _ = (path, exact_mode, label);
    Err("private WFE files require Unix no-follow filesystem support".to_string())
}

#[cfg(unix)]
fn validate_open_private_file(
    file: &File,
    path: &Path,
    exact_mode: bool,
    label: &str,
) -> Result<std::fs::Metadata, String> {
    use std::os::unix::fs::MetadataExt;

    let metadata = file
        .metadata()
        .map_err(|error| format!("could not inspect {label} file {}: {error}", path.display()))?;
    let permission_bits = metadata.mode() & 0o7777;
    let safe_mode = if exact_mode {
        permission_bits == PRIVATE_FILE_MODE
    } else {
        permission_bits & 0o077 == 0
            && permission_bits & 0o7000 == 0
            && permission_bits & 0o400 != 0
    };
    if !metadata.is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.nlink() != 1
        || !safe_mode
    {
        return Err(format!(
            "{label} file {} must be a single-link owner-controlled regular private file",
            path.display()
        ));
    }
    Ok(metadata)
}

fn asn1_time(time: SystemTime) -> Result<ASN1Time, String> {
    ASN1Time::from_timestamp(system_time_to_unix(time)?)
        .map_err(|_| "current time cannot be represented in an X.509 certificate".to_string())
}

fn system_time_to_unix(time: SystemTime) -> Result<i64, String> {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => {
            i64::try_from(duration.as_secs()).map_err(|_| "system time is out of range".to_string())
        }
        Err(error) => {
            let seconds = i64::try_from(error.duration().as_secs())
                .map_err(|_| "system time is out of range".to_string())?;
            seconds
                .checked_neg()
                .ok_or_else(|| "system time is out of range".to_string())
        }
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

#[cfg(test)]
mod tests;
