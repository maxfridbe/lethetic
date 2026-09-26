//! Embedded HTTPS/WSS service for the optional browser mirror.

mod files;

use super::actor::{ConnectionId, SubmitError};
use super::contracts::{
    CommandError, CommandErrorCode, CommandResult, ICommandRequest, ICommandResponse,
    IProtocolHello, IServerMessage, IStatePatch, IStateSnapshot, MAX_COMMAND_MESSAGE_BYTES,
    MAX_SERVER_MESSAGE_BYTES, ProtocolCapabilities, StateChange, WebCommand,
};
use super::files::RootedFiles;
use super::runtime::{
    WfeConnectedEvent, WfeConnectionEvent, WfeDisconnectCategory, WfeDisconnectedEvent,
    WfeFrontendHandle,
};
use super::security::{
    ControllerAuthenticationMode, MAX_TOKEN_FILE_BYTES, PreparedSecurity, SecurityProfile,
    WfeTarget, validate_concrete_ip,
};
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade, close_code};
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_TYPE, COOKIE, HOST, ORIGIN, RETRY_AFTER, SET_COOKIE,
};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Router, body::Body};
use axum_server::accept::Accept;
use axum_server::tls_rustls::RustlsConfig;
use cookie::{Cookie, SameSite};
use futures_util::stream::FuturesUnordered;
use futures_util::{FutureExt, StreamExt};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::fmt;
use std::future::Future;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

pub const WFE_TLS_CONNECTION_CAPACITY: usize = 16;
pub const WFE_AUTHENTICATED_CLIENT_CAPACITY: usize = 8;
pub const WFE_AUTH_CONCURRENCY: usize = 4;
pub const WFE_AUTH_BODY_BYTES: usize = 8 * 1024;
pub const WFE_INCOMING_MESSAGE_BYTES: usize = MAX_COMMAND_MESSAGE_BYTES;
pub const WFE_OUTGOING_MESSAGE_BYTES: usize = MAX_SERVER_MESSAGE_BYTES;
pub const WFE_CONNECTION_PENDING_RESPONSES: usize = 64;

const WFE_SOCKET_WRITE_BUFFER_BYTES: usize = 64 * 1024;
const WFE_SOCKET_MAX_WRITE_BUFFER_BYTES: usize = 8 * 1024 * 1024;
const WFE_AUTH_BODY_TIMEOUT: Duration = Duration::from_secs(5);
const WFE_SEND_TIMEOUT: Duration = Duration::from_secs(10);
const WFE_TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const WFE_HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);
const WFE_GRACEFUL_SHUTDOWN: Duration = Duration::from_secs(5);
const WFE_AUTH_WINDOW: Duration = Duration::from_secs(60);
const WFE_AUTH_ATTEMPTS_PER_PEER: usize = 8;
const WFE_AUTH_ATTEMPTS_GLOBAL: usize = 64;
const WFE_AUTH_PEER_CAPACITY: usize = 128;
#[cfg(not(test))]
const WFE_RTT_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(test)]
const WFE_RTT_TIMEOUT: Duration = Duration::from_secs(1);
const WFE_RTT_NONCE_BYTES: usize = 16;
const WFE_COOKIE_NAME: &str = "__Host-lethetic-wfe";
const WFE_COOKIE_RANDOM_BYTES: usize = 32;
const WFE_WEBSOCKET_PROTOCOL_PREFIX: &str = "lethetic-wfe-v1.";
const WFE_DNS_ANSWER_CAP: usize = 32;
#[cfg(not(test))]
const WFE_DNS_LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(test)]
const WFE_DNS_LOOKUP_TIMEOUT: Duration = Duration::from_millis(100);

static WFE_WEBSOCKET_PROTOCOL_HEADER: HeaderName =
    HeaderName::from_static("x-lethetic-websocket-protocol");
static CONTENT_SECURITY_POLICY: HeaderName = HeaderName::from_static("content-security-policy");
static X_CONTENT_TYPE_OPTIONS: HeaderName = HeaderName::from_static("x-content-type-options");
static REFERRER_POLICY: HeaderName = HeaderName::from_static("referrer-policy");
static PERMISSIONS_POLICY: HeaderName = HeaderName::from_static("permissions-policy");
static X_FRAME_OPTIONS: HeaderName = HeaderName::from_static("x-frame-options");
static CROSS_ORIGIN_OPENER_POLICY: HeaderName =
    HeaderName::from_static("cross-origin-opener-policy");
static CROSS_ORIGIN_RESOURCE_POLICY: HeaderName =
    HeaderName::from_static("cross-origin-resource-policy");

/// One immutable startup resolution of the logical WFE target.
#[derive(Clone, Eq, PartialEq)]
pub struct WfeBindPlan {
    target: WfeTarget,
    addresses: Box<[SocketAddr]>,
}

impl WfeBindPlan {
    pub fn target(&self) -> &WfeTarget {
        &self.target
    }

    pub fn addresses(&self) -> &[SocketAddr] {
        &self.addresses
    }
}

impl fmt::Debug for WfeBindPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WfeBindPlan")
            .field("target", &self.target)
            .field("addresses", &self.addresses)
            .finish()
    }
}

pub async fn resolve_target(target: &WfeTarget) -> Result<WfeBindPlan, String> {
    resolve_target_with(target, |hostname, port| async move {
        let addresses = tokio::net::lookup_host((hostname.as_str(), port))
            .await
            .map_err(|error| format!("could not resolve WFE DNS target: {error}"))?;
        Ok(addresses.take(WFE_DNS_ANSWER_CAP + 1).collect())
    })
    .await
}

async fn resolve_target_with<F, Fut>(target: &WfeTarget, lookup: F) -> Result<WfeBindPlan, String>
where
    F: FnOnce(String, u16) -> Fut,
    Fut: Future<Output = Result<Vec<SocketAddr>, String>>,
{
    if let Some(address) = target.literal_socket_addr() {
        return Ok(WfeBindPlan {
            target: target.clone(),
            addresses: vec![address].into_boxed_slice(),
        });
    }

    let hostname = target
        .dns_name()
        .expect("a validated non-IP WFE target has a DNS identity")
        .to_string();
    let answers = timeout(WFE_DNS_LOOKUP_TIMEOUT, lookup(hostname, target.port()))
        .await
        .map_err(|_| {
            format!(
                "WFE DNS resolution exceeded the {} second deadline",
                WFE_DNS_LOOKUP_TIMEOUT.as_secs_f64()
            )
        })??;
    bind_plan_from_answers(target, answers)
}

fn bind_plan_from_answers(
    target: &WfeTarget,
    answers: Vec<SocketAddr>,
) -> Result<WfeBindPlan, String> {
    if answers.len() > WFE_DNS_ANSWER_CAP {
        return Err(format!(
            "WFE DNS resolution exceeded the {WFE_DNS_ANSWER_CAP}-answer limit"
        ));
    }
    let mut unique = BTreeSet::new();
    for mut address in answers {
        validate_concrete_ip(address.ip()).map_err(|error| {
            format!("WFE DNS resolution returned a prohibited address: {error}")
        })?;
        address.set_port(target.port());
        unique.insert(address);
    }
    if unique.is_empty() {
        return Err("WFE DNS resolution returned no addresses".to_string());
    }
    Ok(WfeBindPlan {
        target: target.clone(),
        addresses: unique.into_iter().collect::<Vec<_>>().into_boxed_slice(),
    })
}

/// Host-only information to display before entering terminal raw mode.
///
/// Debug is deliberately not implemented because the automatic bootstrap URL
/// contains a controller credential.
pub struct WfeHostInfo {
    target: String,
    listener_addresses: Box<[SocketAddr]>,
    fingerprint_sha256: String,
    bootstrap_url: Option<Zeroizing<String>>,
    profile: SecurityProfile,
    authentication_mode: ControllerAuthenticationMode,
}

impl WfeHostInfo {
    pub fn target(&self) -> &str {
        &self.target
    }

    pub fn listener_addresses(&self) -> &[SocketAddr] {
        &self.listener_addresses
    }

    pub fn fingerprint_sha256(&self) -> &str {
        &self.fingerprint_sha256
    }

    pub fn profile(&self) -> SecurityProfile {
        self.profile
    }

    pub fn authentication_mode(&self) -> ControllerAuthenticationMode {
        self.authentication_mode
    }

    pub fn take_bootstrap_url(&mut self) -> Option<Zeroizing<String>> {
        self.bootstrap_url.take()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WfeServerStatus {
    Running,
    Stopped { error: Option<String> },
}

pub struct WfeServerHandle {
    handles: Vec<axum_server::Handle<SocketAddr>>,
    cancellation: CancellationToken,
    status: watch::Receiver<WfeServerStatus>,
    join: Option<JoinHandle<Result<(), String>>>,
}

impl WfeServerHandle {
    pub fn is_finished(&self) -> bool {
        self.join.as_ref().is_none_or(JoinHandle::is_finished)
    }

    pub fn status_receiver(&self) -> watch::Receiver<WfeServerStatus> {
        self.status.clone()
    }

    /// Waits for a server that has terminated independently of application
    /// shutdown. Normal callers use `shutdown` after the sole application actor
    /// exits.
    pub async fn wait(&mut self) -> Result<(), String> {
        let join = self
            .join
            .take()
            .ok_or_else(|| "WFE server completion was already consumed".to_string())?;
        flatten_server_join(join.await)
    }

    pub async fn shutdown(mut self) -> Result<(), String> {
        self.cancellation.cancel();
        for handle in &self.handles {
            handle.graceful_shutdown(Some(WFE_GRACEFUL_SHUTDOWN));
        }
        let Some(mut join) = self.join.take() else {
            return Ok(());
        };
        match timeout(WFE_GRACEFUL_SHUTDOWN + Duration::from_secs(1), &mut join).await {
            Ok(result) => flatten_server_join(result),
            Err(_) => {
                for handle in &self.handles {
                    handle.shutdown();
                }
                join.abort();
                let _ = join.await;
                Err("WFE server group exceeded its bounded shutdown deadline".to_string())
            }
        }
    }
}

impl Drop for WfeServerHandle {
    fn drop(&mut self) {
        self.cancellation.cancel();
        for handle in &self.handles {
            handle.shutdown();
        }
        if let Some(join) = self.join.take() {
            join.abort();
        }
    }
}

fn flatten_server_join(
    result: Result<Result<(), String>, tokio::task::JoinError>,
) -> Result<(), String> {
    match result {
        Ok(result) => result,
        Err(error) if error.is_cancelled() => Err("WFE server task was cancelled".to_string()),
        Err(error) => Err(format!("WFE server task failed: {error}")),
    }
}

#[derive(Default)]
pub struct WfeServerOptions {
    pub files: Option<RootedFiles>,
}

pub async fn start(
    security: PreparedSecurity,
    bind_plan: WfeBindPlan,
    frontend: WfeFrontendHandle,
) -> Result<(WfeServerHandle, WfeHostInfo), String> {
    start_with_options(security, bind_plan, frontend, WfeServerOptions::default()).await
}

/// Binds and starts every pinned HTTPS/WSS listener. All binds and TLS server
/// setup complete before any accept future is polled, so a partial listener set
/// can never become the running service.
pub async fn start_with_options(
    security: PreparedSecurity,
    bind_plan: WfeBindPlan,
    frontend: WfeFrontendHandle,
    options: WfeServerOptions,
) -> Result<(WfeServerHandle, WfeHostInfo), String> {
    if security.target() != bind_plan.target() {
        return Err("WFE bind plan does not match the prepared TLS identity".to_string());
    }
    let target = security.target().clone();
    let bootstrap_url = security
        .bootstrap_token_for_host()
        .map(|token| Zeroizing::new(format!("{}/#token={token}", target.as_str())));
    let host_info = WfeHostInfo {
        target: target.as_str().to_string(),
        listener_addresses: bind_plan.addresses.clone(),
        fingerprint_sha256: security.fingerprint_sha256(),
        bootstrap_url,
        profile: security.profile(),
        authentication_mode: security.controller_authentication_mode(),
    };

    let mut raw_listeners = Vec::with_capacity(bind_plan.addresses.len());
    for address in bind_plan.addresses.iter().copied() {
        let listener = std::net::TcpListener::bind(address)
            .map_err(|error| format!("could not bind WFE HTTPS listener at {address}: {error}"))?;
        listener.set_nonblocking(true).map_err(|error| {
            format!("could not make WFE listener at {address} nonblocking: {error}")
        })?;
        raw_listeners.push((address, listener));
    }

    let cancellation = CancellationToken::new();
    let state = Arc::new(ServerState::new_with_options(
        security,
        frontend,
        cancellation.clone(),
        options,
    )?);
    let router = build_router(Arc::clone(&state));
    let connection_limit = Arc::new(Semaphore::new(WFE_TLS_CONNECTION_CAPACITY));
    let mut handles = Vec::with_capacity(raw_listeners.len());
    let mut listeners = FuturesUnordered::<
        Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'static>>,
    >::new();

    for (address, listener) in raw_listeners {
        let tls = RustlsConfig::from_config(state.security.server_config());
        let handle = axum_server::Handle::new();
        let mut server = axum_server::from_tcp_rustls(listener, tls)
            .map_err(|error| format!("could not configure WFE TLS listener at {address}: {error}"))?
            .map(|acceptor| {
                TlsConnectionLimit::new(
                    acceptor.handshake_timeout(WFE_TLS_HANDSHAKE_TIMEOUT),
                    Arc::clone(&connection_limit),
                )
            })
            .handle(handle.clone());
        server
            .http_builder()
            .http1()
            .timer(hyper_util::rt::TokioTimer::new())
            .header_read_timeout(WFE_HEADER_READ_TIMEOUT)
            .max_headers(64)
            .max_buf_size(64 * 1024);

        let serve = server.serve(
            router
                .clone()
                .into_make_service_with_connect_info::<SocketAddr>(),
        );
        listeners.push(Box::pin(async move {
            match std::panic::AssertUnwindSafe(serve).catch_unwind().await {
                Ok(result) => result
                    .map_err(|error| format!("WFE HTTPS listener at {address} stopped: {error}")),
                Err(_) => Err(format!("WFE HTTPS listener task at {address} panicked")),
            }
        }));
        handles.push(handle);
    }

    let task_cancellation = cancellation.clone();
    let coordinator_handles = handles.clone();
    let (status_sender, status) = watch::channel(WfeServerStatus::Running);
    let join = tokio::spawn(async move {
        let first = listeners.next().await;
        let stopping_normally = task_cancellation.is_cancelled();
        let mut failure = match first {
            Some(Ok(())) if stopping_normally => None,
            Some(Ok(())) => Some("WFE HTTPS listener stopped unexpectedly".to_string()),
            Some(Err(error)) => Some(error),
            None => Some("WFE HTTPS listener group was empty".to_string()),
        };

        task_cancellation.cancel();
        for handle in &coordinator_handles {
            handle.graceful_shutdown(Some(WFE_GRACEFUL_SHUTDOWN));
        }
        let drain = async {
            while let Some(result) = listeners.next().await {
                if let Err(error) = result
                    && failure.is_none()
                {
                    failure = Some(error);
                }
            }
        };
        if timeout(WFE_GRACEFUL_SHUTDOWN, drain).await.is_err() {
            for handle in &coordinator_handles {
                handle.shutdown();
            }
            if failure.is_none() {
                failure = Some("WFE listener group exceeded its shutdown deadline".to_string());
            }
        }

        status_sender.send_replace(WfeServerStatus::Stopped {
            error: failure.clone(),
        });
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    });

    Ok((
        WfeServerHandle {
            handles,
            cancellation,
            status,
            join: Some(join),
        },
        host_info,
    ))
}

struct ServerState {
    security: Arc<PreparedSecurity>,
    frontend: WfeFrontendHandle,
    process_cookie: ProcessCookie,
    websocket_protocol: WebSocketProtocol,
    authentication_attempts: tokio::sync::Mutex<AuthAttemptLimiter>,
    authentication_slots: Arc<Semaphore>,
    clients: Arc<Semaphore>,
    next_connection_ordinal: AtomicU64,
    active_clients: AtomicUsize,
    cancellation: CancellationToken,
    content_security_policy: HeaderValue,
    permissions_policy: HeaderValue,
    files: Option<RootedFiles>,
}

impl ServerState {
    #[cfg(test)]
    fn new(
        security: PreparedSecurity,
        frontend: WfeFrontendHandle,
        cancellation: CancellationToken,
    ) -> Result<Self, String> {
        Self::new_with_options(
            security,
            frontend,
            cancellation,
            WfeServerOptions::default(),
        )
    }

    fn new_with_options(
        security: PreparedSecurity,
        frontend: WfeFrontendHandle,
        cancellation: CancellationToken,
        mut options: WfeServerOptions,
    ) -> Result<Self, String> {
        let process_cookie = ProcessCookie::generate()?;
        let websocket_protocol = WebSocketProtocol::generate()?;
        if let Some(files) = options.files.as_mut() {
            security
                .protect_file_credentials(files)
                .map_err(|_| "could not protect WFE file-service credentials".to_string())?;
            for secret in [process_cookie.expose(), websocket_protocol.expose()] {
                files
                    .register_secret(secret)
                    .map_err(|_| "could not protect WFE file-service credentials".to_string())?;
            }
        }
        let files_enabled = options.files.is_some();
        let websocket_origin = security
            .target()
            .canonical_origin()
            .replacen("https://", "wss://", 1);
        let style_source = if files_enabled {
            "'self' 'unsafe-inline'"
        } else {
            "'self'"
        };
        let content_security_policy = HeaderValue::from_str(&format!(
            "default-src 'self'; script-src 'self'; style-src {style_source}; font-src 'self'; worker-src 'self'; connect-src 'self' {websocket_origin}; img-src 'self' data:; base-uri 'none'; form-action 'none'; frame-ancestors 'none'; object-src 'none'"
        ))
        .map_err(|_| "could not construct the fixed WFE content security policy".to_string())?;
        let clipboard_write = if files_enabled { "(self)" } else { "()" };
        let permissions_policy = HeaderValue::from_str(&format!(
            "accelerometer=(), autoplay=(), camera=(), clipboard-read=(), clipboard-write={clipboard_write}, display-capture=(), geolocation=(), gyroscope=(), magnetometer=(), microphone=(), payment=(), usb=()"
        )).map_err(|_| "could not construct the fixed WFE permissions policy".to_string())?;
        Ok(Self {
            security: Arc::new(security),
            frontend,
            process_cookie,
            websocket_protocol,
            authentication_attempts: tokio::sync::Mutex::new(AuthAttemptLimiter::default()),
            authentication_slots: Arc::new(Semaphore::new(WFE_AUTH_CONCURRENCY)),
            clients: Arc::new(Semaphore::new(WFE_AUTHENTICATED_CLIENT_CAPACITY)),
            next_connection_ordinal: AtomicU64::new(1),
            active_clients: AtomicUsize::new(0),
            cancellation,
            content_security_policy,
            permissions_policy,
            files: options.files,
        })
    }
}

fn build_router(state: Arc<ServerState>) -> Router {
    let router = Router::new()
        .route("/", get(index_asset))
        .route("/auth", post(authenticate))
        .route("/auth/session", post(authenticate_session))
        .route("/ws", get(upgrade_websocket));
    let router = if state.files.is_some() {
        files::register(router)
    } else {
        router
    };
    router
        .route("/{*path}", get(static_asset))
        .fallback(not_found)
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            request_guard,
        ))
        .with_state(state)
}

async fn request_guard(
    State(state): State<Arc<ServerState>>,
    request: Request,
    next: Next,
) -> Response {
    if !exact_header_matches(request.headers(), HOST, |value| {
        state.security.target().matches_host_header(value)
    }) {
        return add_security_headers(
            plain_response(
                StatusCode::MISDIRECTED_REQUEST,
                "request authority rejected",
            ),
            &state,
        );
    }
    let response = next.run(request).await;
    add_security_headers(response, &state)
}

fn add_security_headers(mut response: Response, state: &ServerState) -> Response {
    let headers = response.headers_mut();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        CONTENT_SECURITY_POLICY.clone(),
        state.content_security_policy.clone(),
    );
    headers.insert(
        X_CONTENT_TYPE_OPTIONS.clone(),
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        REFERRER_POLICY.clone(),
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(PERMISSIONS_POLICY.clone(), state.permissions_policy.clone());
    headers.insert(X_FRAME_OPTIONS.clone(), HeaderValue::from_static("DENY"));
    headers.insert(
        CROSS_ORIGIN_OPENER_POLICY.clone(),
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        CROSS_ORIGIN_RESOURCE_POLICY.clone(),
        HeaderValue::from_static("same-origin"),
    );
    response
}

async fn index_asset() -> Response {
    serve_embedded_asset("index.html")
}

async fn static_asset(Path(path): Path<String>) -> Response {
    if !valid_asset_path(&path) {
        return not_found().await;
    }
    serve_embedded_asset(&path)
}

fn serve_embedded_asset(path: &str) -> Response {
    let Some(asset) = super::assets::get(path) else {
        return plain_response(StatusCode::NOT_FOUND, "not found");
    };
    let mut response = Response::new(Body::from(asset.bytes));
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(asset.content_type));
    response
}

fn valid_asset_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 512
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.chars().any(char::is_control)
        && path
            .split('/')
            .all(|component| !component.is_empty() && !matches!(component, "." | ".."))
}

async fn not_found() -> Response {
    plain_response(StatusCode::NOT_FOUND, "not found")
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthenticationRequest {
    token: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AuthenticationEndpoint {
    Token,
    Session,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AuthenticationBodyError {
    TooLarge,
    Invalid,
}

async fn read_authentication_body(
    body: Body,
) -> Result<Zeroizing<Vec<u8>>, AuthenticationBodyError> {
    let mut stream = body.into_data_stream();
    let mut bytes = Zeroizing::new(Vec::new());
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| AuthenticationBodyError::Invalid)?;
        let new_len = bytes
            .len()
            .checked_add(chunk.len())
            .ok_or(AuthenticationBodyError::TooLarge)?;
        if new_len > WFE_AUTH_BODY_BYTES {
            return Err(AuthenticationBodyError::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn is_exact_empty_json_object(bytes: &[u8]) -> bool {
    matches!(
        serde_json::from_slice::<serde_json::Value>(bytes),
        Ok(serde_json::Value::Object(object)) if object.is_empty()
    )
}

async fn authenticate(
    State(state): State<Arc<ServerState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
) -> Response {
    authenticate_request(state, peer.ip(), request, AuthenticationEndpoint::Token).await
}

async fn authenticate_session(
    State(state): State<Arc<ServerState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
) -> Response {
    authenticate_request(state, peer.ip(), request, AuthenticationEndpoint::Session).await
}

async fn authenticate_request(
    state: Arc<ServerState>,
    peer_ip: IpAddr,
    request: Request,
    endpoint: AuthenticationEndpoint,
) -> Response {
    let (parts, body) = request.into_parts();
    let headers = parts.headers;
    if !exact_origin(&headers, &state) {
        return plain_response(StatusCode::FORBIDDEN, "request origin rejected");
    }
    if !exact_header_matches(&headers, CONTENT_TYPE, |value| {
        value.eq_ignore_ascii_case("application/json")
    }) {
        return plain_response(StatusCode::UNSUPPORTED_MEDIA_TYPE, "invalid request");
    }
    let _authentication_slot = match state.authentication_slots.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => return rate_limited_response(),
    };
    {
        let mut attempts = state.authentication_attempts.lock().await;
        if !attempts.admit(peer_ip, Instant::now()) {
            return rate_limited_response();
        }
    }
    let body = match timeout(WFE_AUTH_BODY_TIMEOUT, read_authentication_body(body)).await {
        Err(_) => return plain_response(StatusCode::REQUEST_TIMEOUT, "invalid request"),
        Ok(Err(AuthenticationBodyError::TooLarge)) => {
            return plain_response(StatusCode::PAYLOAD_TOO_LARGE, "invalid request");
        }
        Ok(Err(AuthenticationBodyError::Invalid)) => {
            return plain_response(StatusCode::BAD_REQUEST, "invalid request");
        }
        Ok(Ok(body)) => body,
    };

    match endpoint {
        AuthenticationEndpoint::Token => {
            if state.security.controller_authentication_mode()
                != ControllerAuthenticationMode::TokenRequired
            {
                return plain_response(StatusCode::UNAUTHORIZED, "authentication rejected");
            }
            let request: AuthenticationRequest = match serde_json::from_slice(body.as_slice()) {
                Ok(request) => request,
                Err(_) => return plain_response(StatusCode::BAD_REQUEST, "invalid request"),
            };
            let token = Zeroizing::new(request.token);
            if token.len() > MAX_TOKEN_FILE_BYTES
                || !state.security.verify_controller_token(token.as_str())
            {
                return plain_response(StatusCode::UNAUTHORIZED, "authentication rejected");
            }
        }
        AuthenticationEndpoint::Session => {
            if state.security.controller_authentication_mode()
                != ControllerAuthenticationMode::Disabled
            {
                return plain_response(StatusCode::UNAUTHORIZED, "authentication rejected");
            }
            if !is_exact_empty_json_object(body.as_slice()) {
                return plain_response(StatusCode::BAD_REQUEST, "invalid request");
            }
        }
    }

    authenticated_session_response(&state)
}

fn authenticated_session_response(state: &ServerState) -> Response {
    let cookie = Cookie::build((WFE_COOKIE_NAME, state.process_cookie.expose().to_string()))
        .secure(true)
        .http_only(true)
        .same_site(SameSite::Strict)
        .path("/")
        .build();
    let cookie = match HeaderValue::from_str(&cookie.to_string()) {
        Ok(cookie) => cookie,
        Err(_) => return plain_response(StatusCode::INTERNAL_SERVER_ERROR, "server unavailable"),
    };
    let websocket_protocol = match HeaderValue::from_str(state.websocket_protocol.expose()) {
        Ok(protocol) => protocol,
        Err(_) => return plain_response(StatusCode::INTERNAL_SERVER_ERROR, "server unavailable"),
    };
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(SET_COOKIE, cookie);
    response
        .headers_mut()
        .insert(WFE_WEBSOCKET_PROTOCOL_HEADER.clone(), websocket_protocol);
    response
}

fn rate_limited_response() -> Response {
    let mut response = plain_response(StatusCode::TOO_MANY_REQUESTS, "try again later");
    response
        .headers_mut()
        .insert(RETRY_AFTER, HeaderValue::from_static("60"));
    response
}

async fn upgrade_websocket(
    State(state): State<Arc<ServerState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    websocket: WebSocketUpgrade,
) -> Response {
    let peer_ip = peer.ip();
    if !exact_origin(&headers, &state) {
        return plain_response(StatusCode::FORBIDDEN, "request origin rejected");
    }
    if !state.process_cookie.verify_headers(&headers) {
        return plain_response(StatusCode::UNAUTHORIZED, "authentication rejected");
    }
    let selected_protocol = {
        let mut requested = websocket.requested_protocols();
        let Some(candidate) = requested.next() else {
            return plain_response(StatusCode::UNAUTHORIZED, "authentication rejected");
        };
        if requested.next().is_some()
            || !candidate
                .to_str()
                .is_ok_and(|value| state.websocket_protocol.verify(value))
        {
            return plain_response(StatusCode::UNAUTHORIZED, "authentication rejected");
        }
        state.websocket_protocol.expose().to_string()
    };
    let client_permit = match state.clients.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => return plain_response(StatusCode::SERVICE_UNAVAILABLE, "server busy"),
    };
    websocket
        .protocols([selected_protocol])
        .read_buffer_size(16 * 1024)
        .write_buffer_size(WFE_SOCKET_WRITE_BUFFER_BYTES)
        .max_write_buffer_size(WFE_SOCKET_MAX_WRITE_BUFFER_BYTES)
        .max_message_size(WFE_INCOMING_MESSAGE_BYTES)
        .max_frame_size(WFE_INCOMING_MESSAGE_BYTES)
        .accept_unmasked_frames(false)
        .on_upgrade(move |socket| run_websocket(socket, state, client_permit, peer_ip))
}

fn exact_origin(headers: &HeaderMap, state: &ServerState) -> bool {
    exact_header_matches(headers, ORIGIN, |value| {
        state.security.target().matches_origin_header(value)
    })
}

fn exact_header_matches(
    headers: &HeaderMap,
    name: axum::http::header::HeaderName,
    predicate: impl FnOnce(&str) -> bool,
) -> bool {
    let mut values = headers.get_all(name).iter();
    let Some(value) = values.next() else {
        return false;
    };
    if values.next().is_some() {
        return false;
    }
    value.to_str().is_ok_and(predicate)
}

type PendingCommand = Pin<
    Box<
        dyn Future<
                Output = (
                    bool,
                    String,
                    Result<ICommandResponse, oneshot::error::RecvError>,
                ),
            > + Send,
    >,
>;

struct ConnectionTelemetry {
    sender: tokio::sync::mpsc::Sender<WfeConnectionEvent>,
    peer_ip: IpAddr,
    connection_ordinal: u64,
    admitted_active_clients: usize,
    authentication_mode: ControllerAuthenticationMode,
    initial_sequence: u64,
    initial_revision: u64,
    connected_emitted: bool,
}

impl ConnectionTelemetry {
    async fn emit_connected(&mut self, round_trip_time: Option<Duration>) {
        if self.connected_emitted {
            return;
        }
        self.connected_emitted = true;
        let _ = self
            .sender
            .send(WfeConnectionEvent::Connected(WfeConnectedEvent {
                peer_ip: self.peer_ip,
                connection_ordinal: self.connection_ordinal,
                active_clients: self.admitted_active_clients,
                authentication_mode: self.authentication_mode,
                initial_sequence: self.initial_sequence,
                initial_revision: self.initial_revision,
                round_trip_time,
            }))
            .await;
    }

    async fn emit_disconnected(
        mut self,
        active_clients: usize,
        uptime: Duration,
        category: WfeDisconnectCategory,
    ) {
        self.emit_connected(None).await;
        let _ = self
            .sender
            .send(WfeConnectionEvent::Disconnected(WfeDisconnectedEvent {
                peer_ip: self.peer_ip,
                connection_ordinal: self.connection_ordinal,
                active_clients,
                uptime,
                category,
            }))
            .await;
    }
}

async fn emit_disconnected_before_releasing_client(
    telemetry: ConnectionTelemetry,
    active_clients: usize,
    uptime: Duration,
    category: WfeDisconnectCategory,
    client_permit: OwnedSemaphorePermit,
) {
    telemetry
        .emit_disconnected(active_clients, uptime, category)
        .await;
    drop(client_permit);
}

async fn run_websocket(
    socket: WebSocket,
    state: Arc<ServerState>,
    client_permit: OwnedSemaphorePermit,
    peer_ip: IpAddr,
) {
    let started = Instant::now();
    let connection_ordinal = state
        .next_connection_ordinal
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |ordinal| {
            ordinal.checked_add(1)
        })
        .expect("WFE connection ordinal space is exhausted");
    let admitted_active_clients = state.active_clients.fetch_add(1, Ordering::AcqRel) + 1;
    let initial = state.frontend.mirror.latest_snapshot();
    let mut telemetry = ConnectionTelemetry {
        sender: state.frontend.connection_events.clone(),
        peer_ip,
        connection_ordinal,
        admitted_active_clients,
        authentication_mode: state.security.controller_authentication_mode(),
        initial_sequence: initial.sequence,
        initial_revision: initial.revision,
        connected_emitted: false,
    };
    let category = run_websocket_session(socket, &state, &mut telemetry).await;
    let previous_active = state
        .active_clients
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
            active.checked_sub(1)
        })
        .expect("WFE active-client count underflowed");
    emit_disconnected_before_releasing_client(
        telemetry,
        previous_active - 1,
        started.elapsed(),
        category,
        client_permit,
    )
    .await;
}

async fn run_websocket_session(
    mut socket: WebSocket,
    state: &Arc<ServerState>,
    telemetry: &mut ConnectionTelemetry,
) -> WfeDisconnectCategory {
    let connection_id = ConnectionId::random();
    let mut patches = state.frontend.mirror.subscribe_patches();
    let mut snapshots = state.frontend.mirror.subscribe_snapshots();
    let initial = snapshots.borrow_and_update().clone();
    telemetry.initial_sequence = initial.sequence;
    telemetry.initial_revision = initial.revision;
    let hello = match IProtocolHello::new(
        initial.sequence,
        initial.revision,
        ProtocolCapabilities {
            state_patches: true,
            request_replay: true,
            session_names: true,
            exact_tool_approval: true,
            read_only_files: state.files.is_some(),
        },
    ) {
        Ok(hello) => hello,
        Err(_) => return WfeDisconnectCategory::SetupFailed,
    };
    if send_server_message(&mut socket, IServerMessage::Hello { hello })
        .await
        .is_err()
        || send_server_message(
            &mut socket,
            IServerMessage::StateSnapshot {
                snapshot: Box::new((*initial).clone()),
            },
        )
        .await
        .is_err()
    {
        return WfeDisconnectCategory::SendFailed;
    }

    let mut rtt_nonce = [0_u8; WFE_RTT_NONCE_BYTES];
    if getrandom::fill(&mut rtt_nonce).is_err() {
        return WfeDisconnectCategory::SetupFailed;
    }
    let ping_started = Instant::now();
    if !matches!(
        timeout(
            WFE_SEND_TIMEOUT,
            socket.send(Message::Ping(rtt_nonce.to_vec().into())),
        )
        .await,
        Ok(Ok(()))
    ) {
        return WfeDisconnectCategory::SendFailed;
    }
    let mut rtt_deadline = Box::pin(tokio::time::sleep(WFE_RTT_TIMEOUT));
    let mut awaiting_rtt = true;
    let mut disconnect_category = WfeDisconnectCategory::PeerClosed;
    let mut last_sequence = initial.sequence;
    let mut last_revision = initial.revision;
    let mut last_known_snapshot = Some(initial);
    let mut pending = FuturesUnordered::<PendingCommand>::new();

    loop {
        tokio::select! {
            _ = &mut rtt_deadline, if awaiting_rtt => {
                awaiting_rtt = false;
                telemetry.emit_connected(None).await;
            }
            _ = state.cancellation.cancelled() => {
                disconnect_category = WfeDisconnectCategory::ServerShutdown;
                let _ = send_close(&mut socket, close_code::AWAY, "server shutdown").await;
                break;
            }
            incoming = socket.recv() => {
                let Some(incoming) = incoming else {
                    break;
                };
                match incoming {
                    Ok(Message::Text(text)) => {
                        let request: ICommandRequest = match serde_json::from_str(text.as_str()) {
                            Ok(request) => request,
                            Err(_) => {
                                let Some(request_id) = malformed_request_id(text.as_str()) else {
                                    disconnect_category = WfeDisconnectCategory::ProtocolViolation;
                                    let _ = send_close(&mut socket, close_code::POLICY, "invalid command").await;
                                    break;
                                };
                                let response = local_error_response(
                                    request_id,
                                    CommandErrorCode::BadRequest,
                                    "The command payload was invalid",
                                    false,
                                    state.frontend.mirror.latest_snapshot().revision,
                                );
                                if send_server_message(
                                    &mut socket,
                                    IServerMessage::CommandResponse { response },
                                )
                                .await
                                .is_err()
                                {
                                    disconnect_category = WfeDisconnectCategory::SendFailed;
                                    break;
                                }
                                continue;
                            }
                        };
                        let request_snapshot = matches!(&request.command, WebCommand::RequestSnapshot);
                        let request_id = request.id.clone();
                        if pending.len() >= WFE_CONNECTION_PENDING_RESPONSES {
                            let response = local_error_response(
                                request_id,
                                CommandErrorCode::Busy,
                                "Too many commands are awaiting completion",
                                true,
                                state.frontend.mirror.latest_snapshot().revision,
                            );
                            if send_server_message(
                                &mut socket,
                                IServerMessage::CommandResponse { response },
                            )
                            .await
                            .is_err()
                            {
                                disconnect_category = WfeDisconnectCategory::SendFailed;
                                break;
                            }
                            continue;
                        }
                        match state
                            .frontend
                            .commands
                            .try_submit(connection_id.clone(), request)
                        {
                            Ok(receiver) => {
                                pending.push(Box::pin(async move {
                                    (request_snapshot, request_id, receiver.await)
                                }));
                            }
                            Err(SubmitError::Full) => {
                                let response = local_error_response(
                                    request_id,
                                    CommandErrorCode::Busy,
                                    "The remote command mailbox is full",
                                    true,
                                    state.frontend.mirror.latest_snapshot().revision,
                                );
                                if send_server_message(
                                    &mut socket,
                                    IServerMessage::CommandResponse { response },
                                )
                                .await
                                .is_err()
                                {
                                    disconnect_category = WfeDisconnectCategory::SendFailed;
                                    break;
                                }
                            }
                            Err(SubmitError::Closed) => {
                                let response = local_error_response(
                                    request_id,
                                    CommandErrorCode::BackendUnavailable,
                                    "The application command actor is unavailable",
                                    true,
                                    state.frontend.mirror.latest_snapshot().revision,
                                );
                                let _ = send_server_message(
                                    &mut socket,
                                    IServerMessage::CommandResponse { response },
                                )
                                .await;
                                disconnect_category = WfeDisconnectCategory::BackendUnavailable;
                                break;
                            }
                        }
                    }
                    Ok(Message::Binary(_)) => {
                        disconnect_category = WfeDisconnectCategory::ProtocolViolation;
                        let _ = send_close(&mut socket, close_code::UNSUPPORTED, "text commands required").await;
                        break;
                    }
                    Ok(Message::Ping(_)) => {}
                    Ok(Message::Pong(payload)) => {
                        if awaiting_rtt
                            && let Some(round_trip_time) = matching_pong_round_trip_time(
                                &rtt_nonce,
                                payload.as_ref(),
                                ping_started,
                            )
                        {
                            awaiting_rtt = false;
                            telemetry.emit_connected(Some(round_trip_time)).await;
                        }
                    }
                    Ok(Message::Close(_)) => {
                        disconnect_category = WfeDisconnectCategory::PeerClosed;
                        break;
                    }
                    Err(_) => {
                        disconnect_category = WfeDisconnectCategory::TransportError;
                        break;
                    }
                }
            }
            response = pending.next(), if !pending.is_empty() => {
                let Some((request_snapshot, request_id, response)) = response else {
                    continue;
                };
                let response = match response {
                    Ok(response) => response,
                    Err(_) => local_error_response(
                        request_id,
                        CommandErrorCode::BackendUnavailable,
                        "The application command actor stopped before responding",
                        true,
                        state.frontend.mirror.latest_snapshot().revision,
                    ),
                };
                let backend_unavailable = matches!(
                    &response.result,
                    CommandResult::Error { error }
                        if error.code == CommandErrorCode::BackendUnavailable
                );
                if send_server_message(
                    &mut socket,
                    IServerMessage::CommandResponse { response },
                )
                .await
                .is_err()
                {
                    disconnect_category = WfeDisconnectCategory::SendFailed;
                    break;
                }
                if request_snapshot {
                    let snapshot = snapshots.borrow_and_update().clone();
                    if send_snapshot(&mut socket, &snapshot).await.is_err() {
                        disconnect_category = WfeDisconnectCategory::SendFailed;
                        break;
                    }
                    last_sequence = snapshot.sequence;
                    last_revision = snapshot.revision;
                    last_known_snapshot = Some(snapshot);
                }
                if backend_unavailable {
                    disconnect_category = WfeDisconnectCategory::BackendUnavailable;
                    break;
                }
            }
            patch = patches.recv() => {
                match patch {
                    Ok(patch) if patch.sequence <= last_sequence => {}
                    Ok(patch)
                        if patch.sequence == last_sequence.saturating_add(1)
                            && patch.base_revision == last_revision =>
                    {
                        let patched_known = last_known_snapshot
                            .as_ref()
                            .and_then(|known| snapshot_after_patch(known, &patch));
                        if send_server_message(
                            &mut socket,
                            IServerMessage::StatePatch { patch: (*patch).clone() },
                        )
                        .await
                        .is_err()
                        {
                            disconnect_category = WfeDisconnectCategory::SendFailed;
                            break;
                        }
                        last_sequence = patch.sequence;
                        last_revision = patch.revision;
                        let latest = snapshots.borrow_and_update().clone();
                        if latest.sequence == last_sequence && latest.revision == last_revision {
                            if patched_known
                                .as_ref()
                                .is_some_and(|known| known.state == latest.state)
                            {
                                last_known_snapshot = Some(latest);
                            } else {
                                if send_snapshot(&mut socket, &latest).await.is_err() {
                                    disconnect_category = WfeDisconnectCategory::SendFailed;
                                    break;
                                }
                                last_known_snapshot = Some(latest);
                            }
                        } else if latest.sequence > last_sequence || latest.revision > last_revision {
                            if send_snapshot(&mut socket, &latest).await.is_err() {
                                disconnect_category = WfeDisconnectCategory::SendFailed;
                                break;
                            }
                            last_sequence = latest.sequence;
                            last_revision = latest.revision;
                            last_known_snapshot = Some(latest);
                        } else {
                            last_known_snapshot = None;
                        }
                    }
                    Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        let latest = snapshots.borrow_and_update().clone();
                        if send_snapshot(&mut socket, &latest).await.is_err() {
                            disconnect_category = WfeDisconnectCategory::SendFailed;
                            break;
                        }
                        last_sequence = latest.sequence;
                        last_revision = latest.revision;
                        last_known_snapshot = Some(latest);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        disconnect_category = WfeDisconnectCategory::StateChannelClosed;
                        break;
                    }
                }
            }
            changed = snapshots.changed() => {
                if changed.is_err() {
                    disconnect_category = WfeDisconnectCategory::StateChannelClosed;
                    break;
                }
                let latest = snapshots.borrow_and_update().clone();
                let same_counters = latest.sequence == last_sequence
                    && latest.revision == last_revision;
                let volatile_change = same_counters
                    && last_known_snapshot
                        .as_ref()
                        .is_none_or(|known| known.state != latest.state);
                let skipped_semantic_state = latest.sequence > last_sequence.saturating_add(1)
                    || latest.revision > last_revision.saturating_add(1);
                if volatile_change || skipped_semantic_state {
                    if send_snapshot(&mut socket, &latest).await.is_err() {
                        disconnect_category = WfeDisconnectCategory::SendFailed;
                        break;
                    }
                    last_sequence = latest.sequence;
                    last_revision = latest.revision;
                    last_known_snapshot = Some(latest);
                }
            }
        }
    }
    disconnect_category
}

fn matching_pong_round_trip_time(
    expected_nonce: &[u8],
    candidate: &[u8],
    started: Instant,
) -> Option<Duration> {
    (candidate == expected_nonce).then(|| started.elapsed())
}

fn snapshot_after_patch(
    known: &IStateSnapshot,
    patch: &IStatePatch,
) -> Option<Arc<IStateSnapshot>> {
    if patch.sequence != known.sequence.checked_add(1)? || patch.base_revision != known.revision {
        return None;
    }
    let mut state = known.state.clone();
    for change in &patch.changes {
        match change {
            StateChange::Session { value } => state.session = value.clone(),
            StateChange::Blocks { value } => state.blocks = value.clone(),
            StateChange::Activity { value } => state.activity = value.clone(),
            StateChange::PendingApproval { value } => state.pending_approval = value.clone(),
            StateChange::PendingQuestion { value } => state.pending_question = value.clone(),
            StateChange::Commands { value } => state.commands = value.clone(),
            StateChange::Sessions { value } => state.sessions = value.clone(),
            StateChange::Models { value } => state.models = value.clone(),
            StateChange::Themes { value } => state.themes = value.clone(),
            StateChange::Usage { value } => state.usage = value.as_ref().clone(),
            StateChange::Status { value } => state.status = value.clone(),
            StateChange::Debugger { value } => state.debugger = value.clone(),
            StateChange::Overlay { value } => state.overlay = value.clone(),
        }
    }
    IStateSnapshot::new(patch.sequence, patch.revision, state)
        .ok()
        .map(Arc::new)
}

async fn send_snapshot(socket: &mut WebSocket, snapshot: &Arc<IStateSnapshot>) -> Result<(), ()> {
    send_server_message(
        socket,
        IServerMessage::StateSnapshot {
            snapshot: Box::new((**snapshot).clone()),
        },
    )
    .await
}

async fn send_server_message(socket: &mut WebSocket, message: IServerMessage) -> Result<(), ()> {
    let encoded = serde_json::to_string(&message).map_err(|_| ())?;
    if encoded.len() > WFE_OUTGOING_MESSAGE_BYTES {
        return Err(());
    }
    timeout(WFE_SEND_TIMEOUT, socket.send(Message::Text(encoded.into())))
        .await
        .map_err(|_| ())?
        .map_err(|_| ())
}

async fn send_close(socket: &mut WebSocket, code: u16, reason: &'static str) -> Result<(), ()> {
    timeout(
        WFE_SEND_TIMEOUT,
        socket.send(Message::Close(Some(CloseFrame {
            code,
            reason: reason.into(),
        }))),
    )
    .await
    .map_err(|_| ())?
    .map_err(|_| ())
}

fn malformed_request_id(encoded: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(encoded).ok()?;
    let id = value.as_object()?.get("id")?.as_str()?.to_string();
    let validator = ICommandRequest {
        id: id.clone(),
        expected_revision: 0,
        command: WebCommand::RequestSnapshot,
    };
    validator.validate().is_ok().then_some(id)
}

fn local_error_response(
    id: String,
    code: CommandErrorCode,
    message: &'static str,
    retryable: bool,
    revision: u64,
) -> ICommandResponse {
    let response = ICommandResponse {
        id,
        result: CommandResult::Error {
            error: CommandError {
                code,
                message: message.to_string(),
                current_revision: Some(revision),
                retryable,
            },
        },
    };
    debug_assert!(response.validate().is_ok());
    response
}

fn plain_response(status: StatusCode, message: &'static str) -> Response {
    (
        status,
        [(CONTENT_TYPE, "text/plain; charset=utf-8")],
        message,
    )
        .into_response()
}

struct ProcessCookie {
    value: Zeroizing<String>,
    digest: Zeroizing<[u8; 32]>,
}

impl ProcessCookie {
    fn generate() -> Result<Self, String> {
        let mut random = Zeroizing::new([0_u8; WFE_COOKIE_RANDOM_BYTES]);
        getrandom::fill(random.as_mut())
            .map_err(|_| "operating-system randomness is unavailable".to_string())?;
        let value = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            random.as_ref(),
        );
        let digest = Zeroizing::new(Sha256::digest(value.as_bytes()).into());
        Ok(Self {
            value: Zeroizing::new(value),
            digest,
        })
    }

    fn expose(&self) -> &str {
        self.value.as_str()
    }

    fn verify(&self, candidate: &str) -> bool {
        if candidate.len() > 256 {
            return false;
        }
        let digest = Zeroizing::new(<[u8; 32]>::from(Sha256::digest(candidate.as_bytes())));
        bool::from(self.digest.as_slice().ct_eq(digest.as_slice()))
    }

    fn verify_headers(&self, headers: &HeaderMap) -> bool {
        let mut found: Option<Zeroizing<String>> = None;
        for header in headers.get_all(COOKIE).iter() {
            let Ok(value) = header.to_str() else {
                return false;
            };
            for parsed in Cookie::split_parse(value) {
                let Ok(cookie) = parsed else {
                    return false;
                };
                if cookie.name() == WFE_COOKIE_NAME {
                    if found.is_some() {
                        return false;
                    }
                    found = Some(Zeroizing::new(cookie.value().to_string()));
                }
            }
        }
        found.as_deref().is_some_and(|value| self.verify(value))
    }
}

impl fmt::Debug for ProcessCookie {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProcessCookie(<redacted>)")
    }
}

struct WebSocketProtocol {
    value: Zeroizing<String>,
    digest: Zeroizing<[u8; 32]>,
}

impl WebSocketProtocol {
    fn generate() -> Result<Self, String> {
        let mut random = Zeroizing::new([0_u8; WFE_COOKIE_RANDOM_BYTES]);
        getrandom::fill(random.as_mut())
            .map_err(|_| "operating-system randomness is unavailable".to_string())?;
        let encoded = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            random.as_ref(),
        );
        let value = format!("{WFE_WEBSOCKET_PROTOCOL_PREFIX}{encoded}");
        let digest = Zeroizing::new(Sha256::digest(value.as_bytes()).into());
        Ok(Self {
            value: Zeroizing::new(value),
            digest,
        })
    }

    fn expose(&self) -> &str {
        self.value.as_str()
    }

    fn verify(&self, candidate: &str) -> bool {
        if candidate.len() > 256 {
            return false;
        }
        let digest = Zeroizing::new(<[u8; 32]>::from(Sha256::digest(candidate.as_bytes())));
        bool::from(self.digest.as_slice().ct_eq(digest.as_slice()))
    }
}

impl fmt::Debug for WebSocketProtocol {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("WebSocketProtocol(<redacted>)")
    }
}

#[derive(Default)]
struct AuthAttemptLimiter {
    global: VecDeque<Instant>,
    peers: HashMap<IpAddr, VecDeque<Instant>>,
}

impl AuthAttemptLimiter {
    fn admit(&mut self, peer: IpAddr, now: Instant) -> bool {
        purge_attempts(&mut self.global, now);
        self.peers.retain(|_, attempts| {
            purge_attempts(attempts, now);
            !attempts.is_empty()
        });
        if self.global.len() >= WFE_AUTH_ATTEMPTS_GLOBAL {
            return false;
        }
        if !self.peers.contains_key(&peer) && self.peers.len() >= WFE_AUTH_PEER_CAPACITY {
            return false;
        }
        let attempts = self.peers.entry(peer).or_default();
        if attempts.len() >= WFE_AUTH_ATTEMPTS_PER_PEER {
            return false;
        }
        self.global.push_back(now);
        attempts.push_back(now);
        true
    }
}

fn purge_attempts(attempts: &mut VecDeque<Instant>, now: Instant) {
    while attempts
        .front()
        .is_some_and(|attempt| now.saturating_duration_since(*attempt) >= WFE_AUTH_WINDOW)
    {
        attempts.pop_front();
    }
}

#[derive(Clone)]
struct TlsConnectionLimit<A> {
    inner: A,
    permits: Arc<Semaphore>,
}

impl<A> TlsConnectionLimit<A> {
    fn new(inner: A, permits: Arc<Semaphore>) -> Self {
        Self { inner, permits }
    }
}

impl<A, I, S> Accept<I, S> for TlsConnectionLimit<A>
where
    A: Accept<I, S>,
    A::Future: Send + 'static,
    A::Stream: Send + 'static,
    A::Service: Send + 'static,
{
    type Stream = PermitStream<A::Stream>;
    type Service = A::Service;
    type Future =
        Pin<Box<dyn Future<Output = io::Result<(Self::Stream, Self::Service)>> + Send + 'static>>;

    fn accept(&self, stream: I, service: S) -> Self::Future {
        let permit = match self.permits.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                drop((stream, service));
                return Box::pin(async {
                    Err(io::Error::new(
                        io::ErrorKind::ConnectionRefused,
                        "WFE TLS connection limit reached",
                    ))
                });
            }
        };
        let future = self.inner.accept(stream, service);
        Box::pin(async move {
            let (stream, service) = future.await?;
            Ok((
                PermitStream {
                    stream,
                    _permit: permit,
                },
                service,
            ))
        })
    }
}

struct PermitStream<S> {
    stream: S,
    _permit: OwnedSemaphorePermit,
}

impl<S: AsyncRead + Unpin> AsyncRead for PermitStream<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_read(context, buffer)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for PermitStream<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
        buffer: &[u8],
    ) -> std::task::Poll<Result<usize, io::Error>> {
        Pin::new(&mut self.get_mut().stream).poll_write(context, buffer)
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), io::Error>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(context)
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), io::Error>> {
        Pin::new(&mut self.get_mut().stream).poll_shutdown(context)
    }
}

#[cfg(test)]
mod tests;
