//! Revocable loopback HTTP capabilities for the app's WASM runtimes.
//! Outbound requests and explicitly forwarded virtual sites use separate grants.
pub mod diagnostics;
pub mod sites;
pub mod speech;
mod tor;

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NetworkMode {
    #[default]
    Direct,
    Tor,
}
enum Backend {
    Direct(reqwest::Client),
    Tor(tor::TorHttp),
}
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::{net::TcpListener, sync::Semaphore};
use tokio_util::sync::CancellationToken;

pub const MAX_BYTES: usize = 8 * 1024 * 1024;
#[derive(Clone, Serialize)]
pub struct Route {
    pub endpoint: String,
    pub token: String,
}
pub struct Grant {
    pub route: Route,
    cancel: CancellationToken,
    client: Arc<Backend>,
}
impl Drop for Grant {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

struct Network {
    token: String,
    cancel: CancellationToken,
    client: Arc<Backend>,
    slots: Semaphore,
    diagnostics: diagnostics::Diagnostics,
    mode: NetworkMode,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    url: String,
    method: String,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    body_base64: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Reply {
    status: u16,
    headers: BTreeMap<String, String>,
    body_base64: String,
}

impl Grant {
    /// Prepare Tor before execution so short-lived site handlers reuse a ready client.
    pub async fn ready(&self) -> Result<(), String> {
        if let Backend::Tor(tor) = self.client.as_ref() {
            tokio::select! {
                () = self.cancel.cancelled() => return Err("Network permission revoked".into()),
                result = tokio::time::timeout(Duration::from_secs(60), tor.bootstrap()) => {
                    result.map_err(|_| "Tor bootstrap timed out; no direct fallback".to_string())?.map_err(|e| e.1)?;
                }
            }
        }
        Ok(())
    }
    pub async fn open() -> Result<Self, String> {
        Self::open_mode(NetworkMode::Direct, None).await
    }
    pub async fn open_mode(
        mode: NetworkMode,
        tor_directory: Option<&std::path::Path>,
    ) -> Result<Self, String> {
        Self::open_with_diagnostics(mode, tor_directory, diagnostics::Diagnostics::default()).await
    }
    pub async fn open_with_diagnostics(
        mode: NetworkMode,
        tor_directory: Option<&std::path::Path>,
        diagnostics: diagnostics::Diagnostics,
    ) -> Result<Self, String> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|e| e.to_string())?;
        let endpoint = format!(
            "http://{}/relay",
            listener.local_addr().map_err(|e| e.to_string())?
        );
        let token = uuid::Uuid::new_v4().to_string();
        let cancel = CancellationToken::new();
        let client = match mode {
            NetworkMode::Direct => {
                let direct = reqwest::Client::builder()
                    .no_proxy()
                    .redirect(reqwest::redirect::Policy::none())
                    .connect_timeout(Duration::from_secs(15))
                    .timeout(Duration::from_secs(60))
                    .build()
                    .map_err(|e| e.to_string())?;
                Backend::Direct(direct)
            }
            NetworkMode::Tor => Backend::Tor(
                tor::TorHttp::new(tor_directory.ok_or("Tor storage directory is unavailable")?)
                    .await?,
            ),
        };
        let client = Arc::new(client);
        let state = Arc::new(Network {
            token: token.clone(),
            cancel: cancel.clone(),
            client: client.clone(),
            slots: Semaphore::new(8),
            diagnostics,
            mode,
        });
        let app = Router::new()
            .route("/relay", post(relay).options(preflight))
            .layer(DefaultBodyLimit::max(12 * 1024 * 1024))
            .with_state(state);
        let shutdown = cancel.clone();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await;
        });
        Ok(Self {
            route: Route { endpoint, token },
            cancel,
            client,
        })
    }
}
fn cors(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        "access-control-allow-origin",
        "*".parse().expect("constant header"),
    );
    headers.insert(
        "cache-control",
        "no-store".parse().expect("constant header"),
    );
    response
}
async fn preflight() -> Response {
    cors(
        (
            StatusCode::NO_CONTENT,
            [
                ("access-control-allow-methods", "POST, OPTIONS"),
                (
                    "access-control-allow-headers",
                    "authorization, content-type",
                ),
                ("access-control-allow-private-network", "true"),
            ],
        )
            .into_response(),
    )
}
async fn relay(State(state): State<Arc<Network>>, headers: HeaderMap, body: Bytes) -> Response {
    if headers.get("authorization").and_then(|h| h.to_str().ok())
        != Some(format!("Bearer {}", state.token).as_str())
    {
        return cors(StatusCode::UNAUTHORIZED.into_response());
    }
    let start = std::time::Instant::now();
    let mut entry = diagnostics::Entry::new(&body, state.mode);
    let Ok(_slot) = state.slots.try_acquire() else {
        entry.relay_status = 429;
        entry.error = Some("Relay concurrency limit reached (8 requests)".into());
        state.diagnostics.record(entry);
        return cors(
            (
                StatusCode::TOO_MANY_REQUESTS,
                "Relay concurrency limit reached (8 requests)",
            )
                .into_response(),
        );
    };
    let result = tokio::select! {
        () = state.cancel.cancelled() => Err((StatusCode::FORBIDDEN, "Network permission revoked".into())),
        result = tokio::time::timeout(Duration::from_secs(60), forward(&state.client, &body)) => result.unwrap_or_else(|_| Err(upstream("Network request timed out; Tor-only requests never fall back to direct HTTP"))),
    };
    entry.duration_ms = diagnostics::millis(start.elapsed());
    let response = match result {
        Ok(reply) => {
            entry.target_status = Some(reply.status);
            if reply.status >= 400 {
                entry.error = Some(format!(
                    "Destination returned HTTP {}; cause not established",
                    reply.status
                ));
            }
            Json(reply).into_response()
        }
        Err((status, error)) => {
            let message = diagnostics::safe_error(&error, &body);
            entry.relay_status = status.as_u16();
            entry.error = Some(message.clone());
            (status, message).into_response()
        }
    };
    state.diagnostics.record(entry);
    cors(response)
}
type Failure = (StatusCode, String);
fn bad(error: impl std::fmt::Display) -> Failure {
    (StatusCode::BAD_REQUEST, error.to_string())
}
fn upstream(error: impl std::fmt::Display) -> Failure {
    (StatusCode::BAD_GATEWAY, error.to_string())
}
async fn forward(client: &Backend, body: &[u8]) -> Result<Reply, Failure> {
    let Json(request) = Json::<Request>::from_bytes(body).map_err(bad)?;
    let url = reqwest::Url::parse(&request.url).map_err(bad)?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(bad(
            "Only HTTP(S) URLs without embedded credentials or fragments are supported",
        ));
    }
    if !matches!(
        request.method.as_str(),
        "GET" | "HEAD" | "OPTIONS" | "POST" | "PUT" | "PATCH" | "DELETE"
    ) {
        return Err(bad("Unsupported HTTP method"));
    }
    if request.headers.len() > 128
        || request
            .headers
            .iter()
            .map(|(k, v)| k.len() + v.len())
            .sum::<usize>()
            > 32 * 1024
    {
        return Err(bad("Headers exceed 32 KiB"));
    }
    let method = Method::from_bytes(request.method.as_bytes()).map_err(bad)?;
    let mut outgoing = reqwest::Request::new(method, url);
    for (key, value) in request.headers {
        if matches!(
            key.to_ascii_lowercase().as_str(),
            "host"
                | "cookie"
                | "connection"
                | "transfer-encoding"
                | "content-length"
                | "upgrade"
                | "proxy-authorization"
                | "proxy-connection"
        ) {
            continue;
        }
        outgoing.headers_mut().insert(
            reqwest::header::HeaderName::from_bytes(key.as_bytes()).map_err(bad)?,
            reqwest::header::HeaderValue::from_str(&value).map_err(bad)?,
        );
    }
    if let Some(body) = request.body_base64 {
        if body.len() > MAX_BYTES.div_ceil(3) * 4 {
            return Err(bad("Request exceeds 8 MiB"));
        }
        let bytes = STANDARD.decode(body).map_err(bad)?;
        if bytes.len() > MAX_BYTES {
            return Err(bad("Request exceeds 8 MiB"));
        }
        *outgoing.body_mut() = Some(reqwest::Body::from(bytes));
    }
    let mut response = match client {
        Backend::Tor(tor) => return tor.send(outgoing).await,
        Backend::Direct(direct) => direct
            .execute(outgoing)
            .await
            .map_err(|e| upstream(transport_error(&e)))?,
    };
    if response
        .content_length()
        .is_some_and(|size| size > MAX_BYTES as u64)
    {
        return Err(upstream("Response exceeds 8 MiB"));
    }
    let status = response.status().as_u16();
    let mut headers = BTreeMap::new();
    for (key, value) in response.headers() {
        if matches!(
            key.as_str(),
            "set-cookie" | "connection" | "transfer-encoding"
        ) {
            continue;
        }
        if let Ok(value) = value.to_str() {
            headers.insert(key.to_string(), value.to_string());
        }
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| upstream(transport_error(&e)))?
    {
        if chunk.len() > MAX_BYTES - bytes.len() {
            return Err(upstream("Response exceeds 8 MiB"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(Reply {
        status,
        headers,
        body_base64: STANDARD.encode(bytes),
    })
}

fn transport_error(error: &reqwest::Error) -> String {
    let stage = if error.is_timeout() {
        "HTTP transport timeout"
    } else if error.is_connect() {
        "HTTP connection failed (DNS/TCP/TLS)"
    } else {
        "HTTP response transfer failed"
    };
    let mut message = stage.to_owned();
    let mut cause = std::error::Error::source(error);
    for _ in 0..8 {
        let Some(error) = cause else {
            break;
        };
        message.push_str(": ");
        message.push_str(&error.to_string());
        cause = error.source();
    }
    message
}
