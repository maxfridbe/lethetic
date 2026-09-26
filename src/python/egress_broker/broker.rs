use super::adapter::{ProxyIntent, parse_proxy_request};
use super::network_policy::{
    HostNetworkMonitor, RouteDecision, RouteSnapshot, RouteVerificationFailure,
    collect_dns_answers, validate_dns_answers, verify_kernel_route_with_policy,
};
use super::peer::{
    PeerVerifier, SocketGuard, validate_private_parent, validate_unix_socket_path_length,
};
use super::protocol::{
    BROKER_PROTOCOL_VERSION, BrokerRequest, BrokerRequestKind, BrokerResponse, MAX_HTTP_HEAD_BYTES,
    PROTOCOL_TIMEOUT, ProxyKind, capability_matches, read_frame, validate_capability,
    validate_runtime_id, write_frame,
};
use super::relay::{
    MAX_TUNNEL_DURATION, RelayCounters, copy_with_idle_counted, relay_bidirectional_counted,
};
use base64::Engine as _;
use serde::Serialize;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpStream, UnixListener, UnixStream};
use tokio::sync::{Mutex, Semaphore};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const DNS_TIMEOUT: Duration = Duration::from_secs(10);
pub(super) const MAX_AUDIT_BYTES: u64 = 16 * 1024 * 1024;

static BROKER_REQUEST_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
pub struct BrokerConfig {
    pub socket_path: PathBuf,
    pub audit_path: PathBuf,
    pub runtime_id: String,
    pub capability: String,
    pub expected_peer_uid: u32,
    pub expected_peer_gid: u32,
    pub derive_peer_credentials: bool,
    pub expected_peer_pid: u32,
    pub selinux_labels: Option<crate::python::selinux::SelinuxLabels>,
    pub allow_label_disabled_peer: bool,
    pub max_connections: usize,
}

impl BrokerConfig {
    pub fn validate(&self) -> Result<(), String> {
        validate_runtime_id(&self.runtime_id)?;
        validate_capability(&self.capability)?;
        validate_unix_socket_path_length(&self.socket_path)?;
        validate_private_parent(&self.socket_path)?;
        validate_private_parent(&self.audit_path)?;
        if self.socket_path == self.audit_path {
            return Err("broker socket and audit paths must differ".to_string());
        }
        if self.max_connections == 0 || self.max_connections > 256 {
            return Err("broker max_connections must be between 1 and 256".to_string());
        }
        if self.expected_peer_pid == 0 || self.expected_peer_pid > i32::MAX as u32 {
            return Err("broker expected peer PID is invalid".to_string());
        }
        if self.derive_peer_credentials
            && (self.expected_peer_uid != 0 || self.expected_peer_gid != 0)
        {
            return Err("derived broker peer credentials must use zero placeholders".to_string());
        }
        if let Some(labels) = &self.selinux_labels {
            labels.validate()?;
        }
        if self.allow_label_disabled_peer && self.selinux_labels.is_some() {
            return Err(
                "label-disabled broker peer mode cannot also require SELinux labels".to_string(),
            );
        }
        Ok(())
    }
}

#[derive(Serialize, Debug)]
struct AuditRecord<'a> {
    timestamp: String,
    runtime_id: &'a str,
    request_id: &'a str,
    destination: Option<&'a str>,
    port: Option<u16>,
    decision: &'a str,
    code: &'a str,
    selected_ip: Option<IpAddr>,
    bytes_from_sandbox: u64,
    bytes_to_sandbox: u64,
}

pub(super) struct AuditLog {
    runtime_id: String,
    file: Mutex<tokio::fs::File>,
    bytes: AtomicU64,
    healthy: AtomicBool,
    failed: CancellationToken,
}

impl AuditLog {
    pub(super) fn open(path: &Path, runtime_id: String) -> Result<Self, String> {
        #[cfg(unix)]
        {
            use rustix::fs::{Mode, OFlags};
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            let fd = rustix::fs::open(
                path,
                OFlags::WRONLY
                    | OFlags::CREATE
                    | OFlags::APPEND
                    | OFlags::NOFOLLOW
                    | OFlags::NONBLOCK
                    | OFlags::CLOEXEC,
                Mode::from_raw_mode(0o600),
            )
            .map_err(|error| format!("could not safely open broker audit log: {error}"))?;
            let file = std::fs::File::from(fd);
            let metadata = file
                .metadata()
                .map_err(|error| format!("could not inspect broker audit log: {error}"))?;
            let uid = rustix::process::geteuid().as_raw();
            if !metadata.is_file() || metadata.uid() != uid || metadata.nlink() != 1 {
                return Err("broker audit log must be a singly linked regular file owned by the current user".to_string());
            }
            if metadata.len() >= MAX_AUDIT_BYTES {
                return Err("broker audit log reached its safety limit".to_string());
            }
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|error| format!("could not secure broker audit log: {error}"))?;
            return Ok(Self {
                runtime_id,
                file: Mutex::new(tokio::fs::File::from_std(file)),
                bytes: AtomicU64::new(metadata.len()),
                healthy: AtomicBool::new(true),
                failed: CancellationToken::new(),
            });
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            let _ = runtime_id;
            Err("egress broker audit logging requires Unix".to_string())
        }
    }

    fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::Acquire)
    }

    fn mark_failed(&self) {
        self.healthy.store(false, Ordering::Release);
        self.failed.cancel();
    }

    async fn record(
        &self,
        request_id: &str,
        destination: Option<&str>,
        port: Option<u16>,
        decision: &str,
        code: &str,
        selected_ip: Option<IpAddr>,
        bytes_from_sandbox: u64,
        bytes_to_sandbox: u64,
    ) -> Result<(), String> {
        if !self.is_healthy() {
            return Err("broker audit log is unhealthy".to_string());
        }
        let record = AuditRecord {
            timestamp: chrono::Utc::now().to_rfc3339(),
            runtime_id: &self.runtime_id,
            request_id,
            destination,
            port,
            decision,
            code,
            selected_ip,
            bytes_from_sandbox,
            bytes_to_sandbox,
        };
        let mut encoded = match serde_json::to_vec(&record) {
            Ok(encoded) => encoded,
            Err(error) => {
                self.mark_failed();
                return Err(format!("could not encode broker audit record: {error}"));
            }
        };
        encoded.push(b'\n');
        let result: Result<(), String> = async {
            let mut file = self.file.lock().await;
            if !self.is_healthy() {
                return Err("broker audit log became unhealthy".to_string());
            }
            let current = self.bytes.load(Ordering::Acquire);
            let next = current
                .checked_add(encoded.len() as u64)
                .ok_or_else(|| "broker audit byte count overflowed".to_string())?;
            if next > MAX_AUDIT_BYTES {
                return Err("broker audit log reached its safety limit".to_string());
            }
            file.write_all(&encoded)
                .await
                .map_err(|error| format!("could not write broker audit record: {error}"))?;
            file.flush()
                .await
                .map_err(|error| format!("could not flush broker audit record: {error}"))?;
            self.bytes.store(next, Ordering::Release);
            Ok(())
        }
        .await;
        if let Err(error) = result {
            self.mark_failed();
            return Err(error);
        }
        Ok(())
    }
}

pub(super) fn decode_proxy_head(request: &BrokerRequest) -> Result<Vec<u8>, String> {
    let encoded = request
        .proxy_head_base64
        .as_deref()
        .ok_or_else(|| "proxy_shape".to_string())?;
    let maximum_encoded = MAX_HTTP_HEAD_BYTES.div_ceil(3) * 4;
    if encoded.is_empty() || encoded.len() > maximum_encoded {
        return Err("proxy_shape".to_string());
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| "proxy_shape".to_string())?;
    if decoded.is_empty() || decoded.len() > MAX_HTTP_HEAD_BYTES {
        return Err("proxy_shape".to_string());
    }
    Ok(decoded)
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ValidatedBrokerRequest {
    Readiness,
    Proxy(ProxyIntent),
}

pub(super) fn validate_broker_request(
    request: &BrokerRequest,
    runtime_id: &str,
    capability: &str,
) -> Result<ValidatedBrokerRequest, String> {
    if request.version != BROKER_PROTOCOL_VERSION {
        return Err("protocol_version".to_string());
    }
    if request.runtime_id != runtime_id {
        return Err("runtime_mismatch".to_string());
    }
    if !capability_matches(&request.capability, capability) {
        return Err("capability".to_string());
    }
    match request.kind {
        BrokerRequestKind::Probe => {
            if request.proxy_head_base64.is_none() && !request.buffered_after_head {
                Ok(ValidatedBrokerRequest::Readiness)
            } else {
                Err("probe_shape".to_string())
            }
        }
        BrokerRequestKind::Proxy => {
            let head = decode_proxy_head(request)?;
            let intent = parse_proxy_request(&head).map_err(|_| "proxy_request".to_string())?;
            if intent.kind == ProxyKind::Http && request.buffered_after_head {
                return Err("proxy_pipeline".to_string());
            }
            Ok(ValidatedBrokerRequest::Proxy(intent))
        }
    }
}

pub async fn run_broker(config: BrokerConfig, cancel: CancellationToken) -> Result<(), String> {
    config.validate()?;
    if config.socket_path.exists() {
        return Err(format!(
            "broker socket path already exists: {}",
            config.socket_path.display()
        ));
    }
    let selinux = config
        .selinux_labels
        .clone()
        .map(crate::python::selinux::SelinuxSocketSecurity::prepare)
        .transpose()?;
    let peer = Arc::new(PeerVerifier::new(
        config.expected_peer_uid,
        config.expected_peer_gid,
        config.expected_peer_pid,
        config.derive_peer_credentials,
        selinux.clone(),
        config.allow_label_disabled_peer,
    )?);
    let audit = Arc::new(AuditLog::open(
        &config.audit_path,
        config.runtime_id.clone(),
    )?);
    let (listener, labeled_identity) = match &selinux {
        Some(security) => {
            let (listener, identity) = security.bind_listener(&config.socket_path)?;
            (listener, Some(identity))
        }
        None => (
            UnixListener::bind(&config.socket_path)
                .map_err(|error| format!("could not bind broker socket: {error}"))?,
            None,
        ),
    };
    let _guard = SocketGuard::new(config.socket_path.clone())?;
    _guard.secure_pathname()?;
    if let Some(security) = &selinux {
        security.verify_path_label(&config.socket_path)?;
    }
    if let Some(identity) = labeled_identity {
        identity.disarm();
    }
    let monitor_shutdown = CancellationToken::new();
    let network_changed = CancellationToken::new();
    let monitor = HostNetworkMonitor::bind()?;
    let monitor_task = tokio::spawn(monitor.run(monitor_shutdown.clone(), network_changed.clone()));
    let semaphore = Arc::new(Semaphore::new(config.max_connections));
    let connection_shutdown = CancellationToken::new();
    let shutdown_reason = Arc::new(AtomicU64::new(0));
    let mut connections = tokio::task::JoinSet::new();
    let mut fatal_error = None;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                shutdown_reason.store(1, Ordering::Release);
                break;
            }
            _ = audit.failed.cancelled() => {
                shutdown_reason.store(3, Ordering::Release);
                fatal_error = Some("egress broker audit sink failed".to_string());
                break;
            }
            _ = network_changed.cancelled() => {
                shutdown_reason.store(2, Ordering::Release);
                let _ = audit
                    .record(
                        "broker",
                        None,
                        None,
                        "closed",
                        "route_changed",
                        None,
                        0,
                        0,
                    )
                    .await;
                fatal_error = Some("host network configuration changed; broker restart required".to_string());
                break;
            }
            joined = connections.join_next(), if !connections.is_empty() => {
                if joined.is_some_and(|joined| joined.is_err()) {
                    shutdown_reason.store(4, Ordering::Release);
                    fatal_error = Some("egress broker connection task panicked".to_string());
                    break;
                }
            }
            accepted = listener.accept() => {
                let (stream, _) = match accepted {
                    Ok(accepted) => accepted,
                    Err(error) => {
                        shutdown_reason.store(4, Ordering::Release);
                        fatal_error = Some(format!("broker accept failed: {error}"));
                        break;
                    }
                };
                let permit = match semaphore.clone().try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        if let Err(error) = audit
                            .record("capacity", None, None, "deny", "capacity", None, 0, 0)
                            .await
                        {
                            shutdown_reason.store(3, Ordering::Release);
                            fatal_error = Some(error);
                            break;
                        }
                        drop(stream);
                        continue;
                    }
                };
                let audit = audit.clone();
                let peer = peer.clone();
                let runtime_id = config.runtime_id.clone();
                let capability = config.capability.clone();
                let shutdown = connection_shutdown.clone();
                let route_changed = network_changed.clone();
                let reason = shutdown_reason.clone();
                connections.spawn(async move {
                    let _permit = permit;
                    handle_broker_connection(
                        stream,
                        &runtime_id,
                        &capability,
                        peer,
                        audit,
                        shutdown,
                        route_changed,
                        reason,
                    )
                    .await
                });
            }
        }
    }

    let final_reason = shutdown_reason.load(Ordering::Acquire);
    if audit.is_healthy() {
        let _ = audit
            .record(
                "broker",
                None,
                None,
                "closed",
                broker_shutdown_code(final_reason),
                None,
                0,
                0,
            )
            .await;
    }
    connection_shutdown.cancel();
    monitor_shutdown.cancel();
    let drain = async { while connections.join_next().await.is_some() {} };
    if timeout(Duration::from_secs(15), drain).await.is_err() {
        connections.abort_all();
        while connections.join_next().await.is_some() {}
    }
    let _ = monitor_task.await;
    match fatal_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn next_broker_request_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let counter = BROKER_REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("egress-{:x}-{nanos:x}-{counter:x}", std::process::id())
}

fn broker_shutdown_code(reason: u64) -> &'static str {
    match reason {
        2 => "route_changed",
        3 => "audit_failure",
        4 => "broker_task_failure",
        _ => "broker_shutdown",
    }
}

pub(super) fn classify_tunnel_error(error: &str) -> &'static str {
    if error == "tunnel_timeout" {
        "tunnel_timeout"
    } else if error == "route_changed" {
        "route_changed"
    } else if error == "broker_shutdown" {
        "broker_shutdown"
    } else if error.contains("idle timeout") {
        "idle_timeout"
    } else if error.contains("byte limit") {
        "byte_limit"
    } else if error.contains("read failed") {
        "read_error"
    } else if error == "adapter_response_error" {
        "adapter_response_error"
    } else if error == "upstream_shutdown_error" {
        "upstream_shutdown_error"
    } else if error == "write_error" || error.contains("write failed") {
        "write_error"
    } else if error.contains("shutdown failed") {
        "shutdown_error"
    } else {
        "tunnel_error"
    }
}

pub(super) async fn run_with_policy<T, F>(
    operation: F,
    audit_failed: &CancellationToken,
    route_changed: &CancellationToken,
    shutdown: &CancellationToken,
) -> Result<T, String>
where
    F: std::future::Future<Output = T>,
{
    tokio::select! {
        biased;
        _ = audit_failed.cancelled() => Err("broker audit log failed".to_string()),
        _ = route_changed.cancelled() => Err("host network policy changed".to_string()),
        _ = shutdown.cancelled() => Err("broker connection cancelled".to_string()),
        output = operation => Ok(output),
    }
}

pub(super) async fn write_counted_with_policy<W>(
    writer: &mut W,
    bytes: &[u8],
    counter: &AtomicU64,
    audit_failed: &CancellationToken,
    route_changed: &CancellationToken,
    shutdown: &CancellationToken,
    shutdown_reason: &AtomicU64,
) -> Result<(), String>
where
    W: AsyncWrite + Unpin,
{
    let mut written = 0_usize;
    while written < bytes.len() {
        let write = tokio::select! {
            biased;
            _ = audit_failed.cancelled() => return Err("audit_failure".to_string()),
            _ = route_changed.cancelled() => return Err("route_changed".to_string()),
            _ = shutdown.cancelled() => {
                return Err(broker_shutdown_code(shutdown_reason.load(Ordering::Acquire)).to_string());
            }
            write = writer.write(&bytes[written..]) => write,
        }
        .map_err(|_| "write_error".to_string())?;
        if write == 0 {
            return Err("write_error".to_string());
        }
        written += write;
        counter.store(written as u64, Ordering::Relaxed);
    }
    Ok(())
}

async fn send_broker_grant(
    stream: &mut UnixStream,
    kind: ProxyKind,
    audit_failed: &CancellationToken,
    route_changed: &CancellationToken,
    shutdown: &CancellationToken,
    shutdown_reason: &AtomicU64,
) -> Result<(), String> {
    let response = BrokerResponse {
        version: BROKER_PROTOCOL_VERSION,
        allowed: true,
        code: "connected".to_string(),
        proxy_kind: Some(kind),
    };
    tokio::select! {
        biased;
        _ = audit_failed.cancelled() => Err("audit_failure".to_string()),
        _ = route_changed.cancelled() => Err("route_changed".to_string()),
        _ = shutdown.cancelled() => {
            Err(broker_shutdown_code(shutdown_reason.load(Ordering::Acquire)).to_string())
        }
        result = write_frame(stream, &response) => {
            result.map_err(|_| "adapter_response_error".to_string())
        }
    }
}

pub(super) async fn shutdown_upstream_with_policy<W>(
    writer: &mut W,
    audit_failed: &CancellationToken,
    route_changed: &CancellationToken,
    shutdown: &CancellationToken,
    shutdown_reason: &AtomicU64,
) -> Result<(), String>
where
    W: AsyncWrite + Unpin,
{
    tokio::select! {
        biased;
        _ = audit_failed.cancelled() => Err("audit_failure".to_string()),
        _ = route_changed.cancelled() => Err("route_changed".to_string()),
        _ = shutdown.cancelled() => {
            Err(broker_shutdown_code(shutdown_reason.load(Ordering::Acquire)).to_string())
        }
        result = writer.shutdown() => result.map_err(|_| "upstream_shutdown_error".to_string()),
    }
}

async fn handle_broker_readiness(
    stream: &mut UnixStream,
    audit: &AuditLog,
    request_id: &str,
    shutdown: &CancellationToken,
    route_changed: &CancellationToken,
) -> Result<(), String> {
    if shutdown.is_cancelled() || route_changed.is_cancelled() || !audit.is_healthy() {
        return Err("broker is not healthy for adapter readiness".to_string());
    }
    audit
        .record(request_id, None, None, "allow", "ready", None, 0, 0)
        .await?;
    let response = BrokerResponse {
        version: BROKER_PROTOCOL_VERSION,
        allowed: true,
        code: "ready".to_string(),
        proxy_kind: None,
    };
    tokio::select! {
        biased;
        _ = audit.failed.cancelled() => {
            return Err("broker audit log failed during readiness".to_string());
        }
        _ = route_changed.cancelled() => {
            return Err("host network changed during readiness".to_string());
        }
        _ = shutdown.cancelled() => {
            return Err("broker shut down during readiness".to_string());
        }
        result = write_frame(&mut *stream, &response) => result,
    }?;
    let mut unexpected = [0_u8; 1];
    tokio::select! {
        biased;
        _ = audit.failed.cancelled() => {
            Err("broker audit log failed while readiness lease was open".to_string())
        }
        _ = route_changed.cancelled() => {
            Err("host network changed while readiness lease was open".to_string())
        }
        _ = shutdown.cancelled() => {
            Err("broker shut down while readiness lease was open".to_string())
        }
        result = stream.read(&mut unexpected) => match result {
            Ok(0) => Ok(()),
            Ok(_) => Err("readiness connection sent unexpected trailing data".to_string()),
            Err(error) => Err(format!("readiness connection failed: {error}")),
        },
    }
}

async fn handle_broker_connection(
    mut stream: UnixStream,
    runtime_id: &str,
    capability: &str,
    peer: Arc<PeerVerifier>,
    audit: Arc<AuditLog>,
    shutdown: CancellationToken,
    route_changed: CancellationToken,
    shutdown_reason: Arc<AtomicU64>,
) -> Result<(), String> {
    let request_id = next_broker_request_id();
    if let Err(error) = peer.verify(&stream) {
        audit
            .record(
                &request_id,
                None,
                None,
                "deny",
                "peer_credentials",
                None,
                0,
                0,
            )
            .await?;
        return Err(error);
    }
    if !audit.is_healthy() {
        return Err("broker audit log is unhealthy".to_string());
    }
    let framed = tokio::select! {
        biased;
        _ = audit.failed.cancelled() => return Err("broker audit log failed".to_string()),
        _ = route_changed.cancelled() => {
            let _ = audit
                .record(
                    &request_id,
                    None,
                    None,
                    "deny",
                    "route_changed",
                    None,
                    0,
                    0,
                )
                .await;
            return Err("host network policy changed".to_string());
        },
        _ = shutdown.cancelled() => {
            let _ = audit
                .record(
                    &request_id,
                    None,
                    None,
                    "deny",
                    "broker_shutdown",
                    None,
                    0,
                    0,
                )
                .await;
            return Err("broker connection cancelled".to_string());
        },
        framed = timeout(PROTOCOL_TIMEOUT, read_frame::<_, BrokerRequest>(&mut stream)) => framed,
    };
    let request = match framed {
        Ok(Ok(request)) => request,
        Ok(Err(error)) => {
            audit
                .record(&request_id, None, None, "deny", "protocol", None, 0, 0)
                .await?;
            return Err(error);
        }
        Err(_) => {
            audit
                .record(
                    &request_id,
                    None,
                    None,
                    "deny",
                    "protocol_timeout",
                    None,
                    0,
                    0,
                )
                .await?;
            return Err("broker request timed out".to_string());
        }
    };

    let intent = match validate_broker_request(&request, runtime_id, capability) {
        Ok(intent) => intent,
        Err(code) => {
            let parsed = decode_proxy_head(&request)
                .ok()
                .and_then(|head| parse_proxy_request(&head).ok());
            deny_proxy_request(
                &mut stream,
                &audit,
                &request_id,
                parsed.as_ref().map(|intent| intent.host.as_str()),
                parsed.as_ref().map(|intent| intent.port),
                &code,
            )
            .await?;
            return Ok(());
        }
    };

    let intent = match intent {
        ValidatedBrokerRequest::Readiness => {
            return handle_broker_readiness(
                &mut stream,
                &audit,
                &request_id,
                &shutdown,
                &route_changed,
            )
            .await;
        }
        ValidatedBrokerRequest::Proxy(intent) => intent,
    };

    let initial_routes = tokio::select! {
        biased;
        _ = audit.failed.cancelled() => return Err("broker audit log failed".to_string()),
        _ = route_changed.cancelled() => return Err("host network policy changed".to_string()),
        _ = shutdown.cancelled() => return Err("broker connection cancelled".to_string()),
        routes = RouteSnapshot::load() => routes,
    };
    let initial_routes = match initial_routes {
        Ok(routes) => routes,
        Err(_) => {
            deny_proxy_request(
                &mut stream,
                &audit,
                &request_id,
                Some(&intent.host),
                Some(intent.port),
                "route_policy",
            )
            .await?;
            return Ok(());
        }
    };
    let lookup_host = format!("{}.", intent.host);
    let resolved = tokio::select! {
        biased;
        _ = audit.failed.cancelled() => return Err("broker audit log failed".to_string()),
        _ = route_changed.cancelled() => return Err("host network policy changed".to_string()),
        _ = shutdown.cancelled() => return Err("broker connection cancelled".to_string()),
        resolved = timeout(DNS_TIMEOUT, tokio::net::lookup_host((lookup_host.as_str(), intent.port))) => resolved,
    };
    let answers = match resolved {
        Ok(Ok(answers)) => match collect_dns_answers(answers) {
            Ok(answers) => answers,
            Err(_) => {
                deny_proxy_request(
                    &mut stream,
                    &audit,
                    &request_id,
                    Some(&intent.host),
                    Some(intent.port),
                    "dns_limit",
                )
                .await?;
                return Ok(());
            }
        },
        _ => {
            deny_proxy_request(
                &mut stream,
                &audit,
                &request_id,
                Some(&intent.host),
                Some(intent.port),
                "dns",
            )
            .await?;
            return Ok(());
        }
    };
    let candidates = match validate_dns_answers(answers, &initial_routes) {
        Ok(candidates) => candidates,
        Err(_) => {
            deny_proxy_request(
                &mut stream,
                &audit,
                &request_id,
                Some(&intent.host),
                Some(intent.port),
                "destination_policy",
            )
            .await?;
            return Ok(());
        }
    };
    let selected = candidates[0];
    let initial_fingerprint =
        verify_kernel_route_with_policy(selected, &audit.failed, &route_changed, &shutdown).await;
    let initial_fingerprint = match initial_fingerprint {
        Ok(fingerprint) => fingerprint,
        Err(RouteVerificationFailure::Policy(error)) => return Err(error.to_string()),
        Err(RouteVerificationFailure::Attestation(_)) => {
            deny_proxy_request(
                &mut stream,
                &audit,
                &request_id,
                Some(&intent.host),
                Some(intent.port),
                "route_selection",
            )
            .await?;
            return Ok(());
        }
    };
    let selected_socket = SocketAddr::new(selected, intent.port);
    let connected = run_with_policy(
        timeout(CONNECT_TIMEOUT, TcpStream::connect(selected_socket)),
        &audit.failed,
        &route_changed,
        &shutdown,
    )
    .await?;
    let mut upstream = match connected {
        Ok(Ok(stream)) => stream,
        _ => {
            deny_proxy_request(
                &mut stream,
                &audit,
                &request_id,
                Some(&intent.host),
                Some(intent.port),
                "connect",
            )
            .await?;
            return Ok(());
        }
    };
    let peer_address = upstream
        .peer_addr()
        .map_err(|error| format!("could not verify upstream peer: {error}"))?;
    if peer_address.ip() != selected || peer_address.port() != intent.port {
        deny_proxy_request(
            &mut stream,
            &audit,
            &request_id,
            Some(&intent.host),
            Some(intent.port),
            "peer_mismatch",
        )
        .await?;
        return Ok(());
    }
    let current_routes = tokio::select! {
        biased;
        _ = audit.failed.cancelled() => return Err("broker audit log failed".to_string()),
        _ = route_changed.cancelled() => return Err("host network policy changed".to_string()),
        _ = shutdown.cancelled() => return Err("broker connection cancelled".to_string()),
        routes = RouteSnapshot::load() => routes,
    };
    let current_fingerprint =
        match verify_kernel_route_with_policy(selected, &audit.failed, &route_changed, &shutdown)
            .await
        {
            Ok(fingerprint) => fingerprint,
            Err(RouteVerificationFailure::Policy(error)) => return Err(error.to_string()),
            Err(RouteVerificationFailure::Attestation(_)) => {
                deny_proxy_request(
                    &mut stream,
                    &audit,
                    &request_id,
                    Some(&intent.host),
                    Some(intent.port),
                    "route_policy_changed",
                )
                .await?;
                return Ok(());
            }
        };
    if current_routes
        .as_ref()
        .ok()
        .and_then(|routes| validate_dns_answers(candidates.iter().copied(), routes).ok())
        .is_none()
        || current_routes
            .as_ref()
            .is_ok_and(|routes| routes.classify(selected) != RouteDecision::Allowed)
        || current_fingerprint != initial_fingerprint
    {
        deny_proxy_request(
            &mut stream,
            &audit,
            &request_id,
            Some(&intent.host),
            Some(intent.port),
            "route_policy_changed",
        )
        .await?;
        return Ok(());
    }
    if shutdown.is_cancelled() || route_changed.is_cancelled() {
        return Err("broker connection cancelled".to_string());
    }

    audit
        .record(
            &request_id,
            Some(&intent.host),
            Some(intent.port),
            "allow",
            "connected",
            Some(selected),
            0,
            0,
        )
        .await?;
    if shutdown.is_cancelled() || route_changed.is_cancelled() {
        let code = if route_changed.is_cancelled() {
            "route_changed"
        } else {
            broker_shutdown_code(shutdown_reason.load(Ordering::Acquire))
        };
        audit
            .record(
                &request_id,
                Some(&intent.host),
                Some(intent.port),
                "closed",
                code,
                Some(selected),
                0,
                0,
            )
            .await?;
        return Ok(());
    }

    let counters = RelayCounters::default();
    let close_code = match intent.kind {
        ProxyKind::Http => {
            let mut result = match intent.rewritten_head.as_deref() {
                Some(rewritten) => {
                    write_counted_with_policy(
                        &mut upstream,
                        rewritten,
                        &counters.from_left,
                        &audit.failed,
                        &route_changed,
                        &shutdown,
                        &shutdown_reason,
                    )
                    .await
                }
                None => Err("internal_error".to_string()),
            };
            if result.is_ok() {
                result = shutdown_upstream_with_policy(
                    &mut upstream,
                    &audit.failed,
                    &route_changed,
                    &shutdown,
                    &shutdown_reason,
                )
                .await;
            }
            if result.is_ok() {
                result = send_broker_grant(
                    &mut stream,
                    ProxyKind::Http,
                    &audit.failed,
                    &route_changed,
                    &shutdown,
                    &shutdown_reason,
                )
                .await;
            }
            if result.is_ok() {
                result = tokio::select! {
                    biased;
                    _ = audit.failed.cancelled() => Err("audit_failure".to_string()),
                    _ = route_changed.cancelled() => Err("route_changed".to_string()),
                    _ = shutdown.cancelled() => Err(broker_shutdown_code(shutdown_reason.load(Ordering::Acquire)).to_string()),
                    result = timeout(
                        MAX_TUNNEL_DURATION,
                        copy_with_idle_counted(&mut upstream, &mut stream, &counters.from_right),
                    ) => match result {
                        Ok(result) => result,
                        Err(_) => Err("tunnel_timeout".to_string()),
                    },
                };
            }
            match result {
                Ok(()) => "tunnel_closed",
                Err(error) => classify_tunnel_error(&error),
            }
        }
        ProxyKind::Connect => {
            let mut result = send_broker_grant(
                &mut stream,
                ProxyKind::Connect,
                &audit.failed,
                &route_changed,
                &shutdown,
                &shutdown_reason,
            )
            .await;
            if result.is_ok() {
                result = tokio::select! {
                    biased;
                    _ = audit.failed.cancelled() => Err("audit_failure".to_string()),
                    _ = route_changed.cancelled() => Err("route_changed".to_string()),
                    _ = shutdown.cancelled() => Err(broker_shutdown_code(shutdown_reason.load(Ordering::Acquire)).to_string()),
                    result = timeout(
                        MAX_TUNNEL_DURATION,
                        relay_bidirectional_counted(&mut stream, &mut upstream, &counters),
                    ) => match result {
                        Ok(result) => result,
                        Err(_) => Err("tunnel_timeout".to_string()),
                    },
                };
            }
            match result {
                Ok(()) => "tunnel_closed",
                Err(error) => classify_tunnel_error(&error),
            }
        }
    };
    let (from_sandbox, to_sandbox) = counters.get();
    if !audit.is_healthy() {
        return Err("broker audit log failed after authorization".to_string());
    }
    audit
        .record(
            &request_id,
            Some(&intent.host),
            Some(intent.port),
            "closed",
            close_code,
            Some(selected),
            from_sandbox,
            to_sandbox,
        )
        .await?;
    Ok(())
}

async fn deny_proxy_request(
    stream: &mut UnixStream,
    audit: &AuditLog,
    request_id: &str,
    destination: Option<&str>,
    port: Option<u16>,
    code: &str,
) -> Result<(), String> {
    audit
        .record(request_id, destination, port, "deny", code, None, 0, 0)
        .await?;
    write_frame(
        stream,
        &BrokerResponse {
            version: BROKER_PROTOCOL_VERSION,
            allowed: false,
            code: code.to_string(),
            proxy_kind: None,
        },
    )
    .await
}
