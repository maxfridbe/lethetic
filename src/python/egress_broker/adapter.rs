use super::network_policy::validate_public_hostname;
use super::peer::validate_unix_socket_path_length;
use super::protocol::{
    BROKER_PROTOCOL_VERSION, BrokerRequest, BrokerRequestKind, BrokerResponse, MAX_HTTP_HEAD_BYTES,
    PROTOCOL_TIMEOUT, ProxyKind, read_frame, validate_capability, validate_runtime_id, write_frame,
};
use super::relay::{MAX_TUNNEL_DURATION, copy_with_idle_counted, relay_bidirectional};
use base64::Engine as _;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UnixStream};
use tokio::sync::Semaphore;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

pub const DEFAULT_ADAPTER_PORT: u16 = 18_080;

const MAX_EARLY_DATA_BYTES: usize = 16 * 1024;
const MAX_HTTP_HEADERS: usize = 100;
const MAX_HEADER_LINE_BYTES: usize = 8 * 1024;
const MAX_CONNECTIONS: usize = 32;

#[derive(Clone)]
pub struct AdapterConfig {
    pub socket_path: PathBuf,
    pub runtime_id: String,
    pub capability: String,
    pub listen: SocketAddr,
    pub max_connections: usize,
}

impl AdapterConfig {
    pub fn validate(&self) -> Result<(), String> {
        validate_runtime_id(&self.runtime_id)?;
        validate_capability(&self.capability)?;
        if !self.socket_path.is_absolute() {
            return Err("adapter broker socket path must be absolute".to_string());
        }
        validate_unix_socket_path_length(&self.socket_path)?;
        if !self.listen.ip().is_loopback() {
            return Err("egress adapter may listen only on loopback".to_string());
        }
        if self.max_connections == 0 || self.max_connections > 256 {
            return Err("adapter max_connections must be between 1 and 256".to_string());
        }
        Ok(())
    }
}

impl Default for AdapterConfig {
    fn default() -> Self {
        Self {
            socket_path: PathBuf::from("/run/lethetic-egress/broker.sock"),
            runtime_id: String::new(),
            capability: String::new(),
            listen: SocketAddr::from((Ipv4Addr::LOCALHOST, DEFAULT_ADAPTER_PORT)),
            max_connections: MAX_CONNECTIONS,
        }
    }
}

pub async fn open_broker_readiness_connection(
    socket_path: &Path,
    runtime_id: &str,
    capability: &str,
) -> Result<UnixStream, String> {
    let mut broker = timeout(PROTOCOL_TIMEOUT, UnixStream::connect(socket_path))
        .await
        .map_err(|_| "egress broker startup probe timed out".to_string())?
        .map_err(|error| format!("egress broker startup probe failed: {error}"))?;
    let request = BrokerRequest {
        version: BROKER_PROTOCOL_VERSION,
        runtime_id: runtime_id.to_string(),
        capability: capability.to_string(),
        kind: BrokerRequestKind::Probe,
        proxy_head_base64: None,
        buffered_after_head: false,
    };
    write_frame(&mut broker, &request).await?;
    let response = timeout(
        PROTOCOL_TIMEOUT,
        read_frame::<_, BrokerResponse>(&mut broker),
    )
    .await
    .map_err(|_| "egress broker startup response timed out".to_string())??;
    if response.version != BROKER_PROTOCOL_VERSION
        || !response.allowed
        || response.code != "ready"
        || response.proxy_kind.is_some()
    {
        return Err("egress broker rejected the startup probe".to_string());
    }
    Ok(broker)
}

pub async fn probe_broker(
    socket_path: &Path,
    runtime_id: &str,
    capability: &str,
) -> Result<(), String> {
    let broker = open_broker_readiness_connection(socket_path, runtime_id, capability).await?;
    drop(broker);
    Ok(())
}

pub async fn run_adapter(config: AdapterConfig, cancel: CancellationToken) -> Result<(), String> {
    run_adapter_inner(config, cancel, None).await
}

pub async fn run_adapter_with_readiness(
    config: AdapterConfig,
    cancel: CancellationToken,
    ready: tokio::sync::oneshot::Sender<SocketAddr>,
) -> Result<(), String> {
    run_adapter_inner(config, cancel, Some(ready)).await
}

async fn run_adapter_inner(
    config: AdapterConfig,
    cancel: CancellationToken,
    ready: Option<tokio::sync::oneshot::Sender<SocketAddr>>,
) -> Result<(), String> {
    config.validate()?;
    probe_broker(&config.socket_path, &config.runtime_id, &config.capability).await?;
    let listener = TcpListener::bind(config.listen)
        .await
        .map_err(|error| format!("could not bind loopback egress adapter: {error}"))?;
    if let Some(ready) = ready {
        let address = listener
            .local_addr()
            .map_err(|error| format!("could not inspect loopback egress adapter: {error}"))?;
        ready
            .send(address)
            .map_err(|_| "egress adapter readiness receiver was dropped".to_string())?;
    }
    let semaphore = Arc::new(Semaphore::new(config.max_connections));
    let connection_shutdown = CancellationToken::new();
    let mut connections = tokio::task::JoinSet::new();
    let mut fatal_error = None;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            joined = connections.join_next(), if !connections.is_empty() => {
                match joined {
                    Some(Ok(Ok(()))) | None => {}
                    Some(Ok(Err(error))) => {
                        eprintln!("Lethetic egress adapter connection failed closed: {error}");
                    }
                    Some(Err(_)) => {
                        fatal_error = Some("egress adapter connection task panicked".to_string());
                        break;
                    }
                }
            }
            accepted = listener.accept() => {
                let (stream, peer) = match accepted {
                    Ok(accepted) => accepted,
                    Err(error) => {
                        fatal_error = Some(format!("adapter accept failed: {error}"));
                        break;
                    }
                };
                if !peer.ip().is_loopback() {
                    drop(stream);
                    continue;
                }
                let permit = match semaphore.clone().try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        drop(stream);
                        continue;
                    }
                };
                let socket_path = config.socket_path.clone();
                let runtime_id = config.runtime_id.clone();
                let capability = config.capability.clone();
                let shutdown = connection_shutdown.clone();
                connections.spawn(async move {
                    let _permit = permit;
                    let result = handle_adapter_connection(
                        stream,
                        &socket_path,
                        &runtime_id,
                        &capability,
                        shutdown,
                    )
                    .await;
                    if let Err(error) = &result {
                        eprintln!("Lethetic egress adapter connection failed closed: {error}");
                    }
                    result
                });
            }
        }
    }
    connection_shutdown.cancel();
    let drain = async { while connections.join_next().await.is_some() {} };
    if timeout(Duration::from_secs(5), drain).await.is_err() {
        connections.abort_all();
        while connections.join_next().await.is_some() {}
    }
    match fatal_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProxyIntent {
    pub(super) kind: ProxyKind,
    pub(super) host: String,
    pub(super) port: u16,
    pub(super) rewritten_head: Option<Vec<u8>>,
}

pub(super) async fn handle_adapter_connection(
    mut client: TcpStream,
    socket_path: &Path,
    runtime_id: &str,
    capability: &str,
    shutdown: CancellationToken,
) -> Result<(), String> {
    let (head, buffered) = tokio::select! {
        _ = shutdown.cancelled() => return Err("egress adapter connection cancelled".to_string()),
        head = read_http_head(&mut client) => match head {
            Ok(head) => head,
            Err(error) => {
                write_http_error(&mut client, 400, "Bad Request").await;
                return Err(error);
            }
        },
    };
    let proxy_head_base64 = base64::engine::general_purpose::STANDARD.encode(&head);
    let mut broker = match timeout(PROTOCOL_TIMEOUT, UnixStream::connect(socket_path)).await {
        Ok(Ok(stream)) => stream,
        _ => {
            write_http_error(&mut client, 502, "Bad Gateway").await;
            return Err("egress broker is unavailable".to_string());
        }
    };
    let request = BrokerRequest {
        version: BROKER_PROTOCOL_VERSION,
        runtime_id: runtime_id.to_string(),
        capability: capability.to_string(),
        kind: BrokerRequestKind::Proxy,
        proxy_head_base64: Some(proxy_head_base64),
        buffered_after_head: !buffered.is_empty(),
    };
    if let Err(error) = write_frame(&mut broker, &request).await {
        write_http_error(&mut client, 502, "Broker Request Failed").await;
        return Err(format!("could not send request to egress broker: {error}"));
    }
    let response = match timeout(
        PROTOCOL_TIMEOUT,
        read_frame::<_, BrokerResponse>(&mut broker),
    )
    .await
    {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => {
            write_http_error(&mut client, 502, "Broker Response Failed").await;
            return Err(format!("could not read egress broker response: {error}"));
        }
        Err(_) => {
            write_http_error(&mut client, 504, "Broker Response Timeout").await;
            return Err("egress broker response timed out".to_string());
        }
    };
    if response.version != BROKER_PROTOCOL_VERSION
        || !response.allowed
        || response.code != "connected"
    {
        write_http_error(&mut client, 403, "Forbidden").await;
        return Ok(());
    }

    match response.proxy_kind {
        Some(ProxyKind::Connect) => {
            client
                .write_all(b"HTTP/1.1 200 Connection Established\r\nConnection: close\r\n\r\n")
                .await
                .map_err(|error| format!("could not acknowledge CONNECT: {error}"))?;
            if !buffered.is_empty() {
                broker
                    .write_all(&buffered)
                    .await
                    .map_err(|error| format!("could not forward buffered TLS bytes: {error}"))?;
            }
            tokio::select! {
                _ = shutdown.cancelled() => {}
                _ = timeout(MAX_TUNNEL_DURATION, relay_bidirectional(&mut client, &mut broker)) => {}
            }
        }
        Some(ProxyKind::Http) => {
            if !buffered.is_empty() {
                return Err("broker allowed a pipelined plain HTTP request".to_string());
            }
            let counter = AtomicU64::new(0);
            tokio::select! {
                _ = shutdown.cancelled() => {}
                _ = timeout(
                    MAX_TUNNEL_DURATION,
                    copy_with_idle_counted(&mut broker, &mut client, &counter),
                ) => {}
            }
        }
        None => return Err("egress broker omitted the authorized proxy kind".to_string()),
    }
    Ok(())
}

async fn read_http_head(stream: &mut TcpStream) -> Result<(Vec<u8>, Vec<u8>), String> {
    timeout(PROTOCOL_TIMEOUT, read_http_head_inner(stream))
        .await
        .map_err(|_| "proxy request header timed out".to_string())?
}

async fn read_http_head_inner(stream: &mut TcpStream) -> Result<(Vec<u8>, Vec<u8>), String> {
    let mut bytes = Vec::with_capacity(1024);
    let mut chunk = [0_u8; 2048];
    loop {
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(|error| format!("could not read proxy request: {error}"))?;
        if read == 0 {
            return Err("proxy client closed before headers completed".to_string());
        }
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(position) = find_header_end(&bytes) {
            let head_length = position + 4;
            if head_length > MAX_HTTP_HEAD_BYTES || bytes.len() - head_length > MAX_EARLY_DATA_BYTES
            {
                return Err("proxy request exceeds the size limit".to_string());
            }
            let buffered = bytes.split_off(head_length);
            return Ok((bytes, buffered));
        }
        if bytes.len() >= MAX_HTTP_HEAD_BYTES {
            return Err("proxy request headers exceed the size limit".to_string());
        }
    }
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

pub(super) fn parse_proxy_request(head: &[u8]) -> Result<ProxyIntent, String> {
    if head.len() > MAX_HTTP_HEAD_BYTES
        || !head.ends_with(b"\r\n\r\n")
        || !head.is_ascii()
        || head.contains(&0)
    {
        return Err("proxy header framing is invalid".to_string());
    }
    let text =
        std::str::from_utf8(head).map_err(|_| "proxy request headers are not UTF-8".to_string())?;
    let mut lines = text[..text.len() - 4].split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| "proxy request line is missing".to_string())?;
    if request_line.len() > MAX_HEADER_LINE_BYTES {
        return Err("proxy request line exceeds the size limit".to_string());
    }
    let parts = request_line.split(' ').collect::<Vec<_>>();
    if parts.len() != 3 || parts.iter().any(|part| part.is_empty()) {
        return Err("proxy request line must contain exactly two spaces".to_string());
    }
    let method = parts[0];
    let target = parts[1];
    let version = parts[2];
    if version != "HTTP/1.1"
        || !is_http_token(method)
        || !target.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
    {
        return Err("proxy method or HTTP version is invalid".to_string());
    }

    let mut headers = Vec::new();
    for line in lines {
        if headers.len() >= MAX_HTTP_HEADERS || line.len() > MAX_HEADER_LINE_BYTES {
            return Err("proxy header count or line size exceeds the limit".to_string());
        }
        if line.starts_with([' ', '\t']) {
            return Err("folded proxy headers are not allowed".to_string());
        }
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| "proxy header is missing ':'".to_string())?;
        if !is_http_token(name) {
            return Err("proxy header name is invalid".to_string());
        }
        let value = value.trim_matches(' ');
        if value.bytes().any(|byte| byte == 0x7f || byte < 0x20) {
            return Err("proxy header value contains control bytes".to_string());
        }
        headers.push((name.to_string(), value.to_string()));
    }
    validate_proxy_headers(&headers)?;

    if method == "CONNECT" {
        let (host, port, explicit_port) = parse_authority(target, 443)?;
        if !explicit_port || port != 443 || target != format!("{host}:443") {
            return Err(
                "CONNECT requires a canonical lowercase hostname and explicit port 443".to_string(),
            );
        }
        validate_public_hostname(&host)?;
        let host_headers = header_values(&headers, "host");
        if host_headers.len() != 1 {
            return Err("CONNECT must contain exactly one Host header".to_string());
        }
        let (header_host, header_port, header_explicit_port) =
            parse_authority(host_headers[0], 443)?;
        if !header_explicit_port
            || header_host != host
            || header_port != port
            || host_headers[0] != format!("{host}:443")
        {
            return Err("CONNECT Host header does not match its authority".to_string());
        }
        return Ok(ProxyIntent {
            kind: ProxyKind::Connect,
            host,
            port,
            rewritten_head: None,
        });
    }

    if !matches!(method, "GET" | "HEAD") {
        return Err("plain HTTP proxy supports only GET and HEAD".to_string());
    }
    if target.contains(['\\', '#']) {
        return Err("plain HTTP proxy target contains ambiguous delimiters".to_string());
    }
    let target_without_scheme = target
        .strip_prefix("http://")
        .ok_or_else(|| "plain proxy target must use canonical lowercase http://".to_string())?;
    let authority_end = target_without_scheme
        .find(['/', '?'])
        .unwrap_or(target_without_scheme.len());
    let raw_authority = &target_without_scheme[..authority_end];
    let (host, port, explicit_port) = parse_authority(raw_authority, 80)?;
    let canonical_authority = if explicit_port {
        format!("{host}:80")
    } else {
        host.clone()
    };
    if raw_authority != canonical_authority || port != 80 {
        return Err("plain proxy target has a noncanonical authority or port".to_string());
    }
    validate_public_hostname(&host)?;
    let host_headers = header_values(&headers, "host");
    if host_headers.len() != 1 {
        return Err("plain proxy request must contain exactly one Host header".to_string());
    }
    let (header_host, header_port, _) = parse_authority(host_headers[0], 80)?;
    if header_host != host || header_port != port {
        return Err("plain proxy Host header does not match the absolute target".to_string());
    }
    let remainder = &target_without_scheme[authority_end..];
    if remainder.starts_with("//") {
        return Err("plain HTTP origin path may not begin with //".to_string());
    }
    let origin = if remainder.is_empty() {
        "/".to_string()
    } else if remainder.starts_with('?') {
        format!("/{remainder}")
    } else {
        remainder.to_string()
    };
    let mut rewritten = format!("{method} {origin} HTTP/1.1\r\nHost: {host}\r\n").into_bytes();
    for (name, value) in headers {
        if name.eq_ignore_ascii_case("host")
            || name.eq_ignore_ascii_case("connection")
            || name.eq_ignore_ascii_case("proxy-connection")
            || name.eq_ignore_ascii_case("keep-alive")
        {
            continue;
        }
        rewritten.extend_from_slice(name.as_bytes());
        rewritten.extend_from_slice(b": ");
        rewritten.extend_from_slice(value.as_bytes());
        rewritten.extend_from_slice(b"\r\n");
    }
    rewritten.extend_from_slice(b"Connection: close\r\n\r\n");
    Ok(ProxyIntent {
        kind: ProxyKind::Http,
        host,
        port,
        rewritten_head: Some(rewritten),
    })
}

fn validate_proxy_headers(headers: &[(String, String)]) -> Result<(), String> {
    for (name, value) in headers {
        if name.eq_ignore_ascii_case("proxy-authorization") {
            return Err("Proxy-Authorization is not accepted".to_string());
        }
        if name.eq_ignore_ascii_case("upgrade")
            || name.eq_ignore_ascii_case("transfer-encoding")
            || name.eq_ignore_ascii_case("trailer")
            || name.eq_ignore_ascii_case("te")
            || name.eq_ignore_ascii_case("expect")
        {
            return Err("upgrade, streaming, and trailer headers are not accepted".to_string());
        }
        if name.eq_ignore_ascii_case("content-length") {
            return Err("proxy request bodies are not accepted".to_string());
        }
        if name.eq_ignore_ascii_case("connection") || name.eq_ignore_ascii_case("proxy-connection")
        {
            let allowed = value.split(',').all(|token| {
                matches!(
                    token.trim().to_ascii_lowercase().as_str(),
                    "close" | "keep-alive"
                )
            });
            if !allowed {
                return Err("custom connection header tokens are not accepted".to_string());
            }
        }
    }
    Ok(())
}

fn header_values<'a>(headers: &'a [(String, String)], name: &str) -> Vec<&'a str> {
    headers
        .iter()
        .filter(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
        .collect()
}

fn parse_authority(value: &str, default_port: u16) -> Result<(String, u16, bool), String> {
    if value.is_empty()
        || value != value.trim()
        || !value.is_ascii()
        || value.contains(['@', '/', '\\', '?', '#', '%', '[', ']'])
    {
        return Err("proxy authority is malformed".to_string());
    }
    let mut pieces = value.split(':');
    let host = pieces.next().unwrap_or_default();
    if host.is_empty() {
        return Err("proxy authority has no hostname".to_string());
    }
    let port = pieces.next();
    let explicit_port = port.is_some();
    let port = match port {
        Some(port) => {
            if port.is_empty()
                || (port.len() > 1 && port.starts_with('0'))
                || !port.bytes().all(|byte| byte.is_ascii_digit())
            {
                return Err("proxy authority port is malformed".to_string());
            }
            port.parse::<u16>()
                .map_err(|_| "proxy authority port is out of range".to_string())?
        }
        None => default_port,
    };
    if pieces.next().is_some() {
        return Err("IPv6 literals and multi-colon authorities are not accepted".to_string());
    }
    Ok((host.to_ascii_lowercase(), port, explicit_port))
}

fn is_http_token(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

async fn write_http_error(stream: &mut TcpStream, status: u16, reason: &str) {
    let response =
        format!("HTTP/1.1 {status} {reason}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n");
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}
