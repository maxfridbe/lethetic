//! Real loopback HTTP ports backed by a revocable virtual-runtime request queue.
use axum::{
    Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    net::TcpListener,
    sync::{mpsc, oneshot},
};
use tokio_util::sync::CancellationToken;

const REQUEST_LIMIT: usize = 16 * 1024 * 1024;
const RESPONSE_LIMIT: usize = 64 * 1024 * 1024;
#[derive(Clone, Serialize)]
pub struct SiteRoute {
    pub id: String,
    pub port: u16,
    pub url: String,
}
#[derive(Clone, Serialize)]
pub struct SiteRequest {
    pub id: String,
    pub method: String,
    pub path: String,
    pub headers: BTreeMap<String, Vec<String>>,
    pub body: String,
}
#[derive(Deserialize)]
pub struct SiteResponse {
    pub status: u16,
    pub headers: BTreeMap<String, Vec<String>>,
    pub body: String,
}
pub struct SiteGrant {
    pub route: SiteRoute,
    pub bridge: Arc<SiteBridge>,
}
pub struct SiteBridge {
    origin: String,
    secret: String,
    cookie: String,
    network: bool,
    cancel: CancellationToken,
    sender: mpsc::Sender<SiteRequest>,
    receiver: tokio::sync::Mutex<mpsc::Receiver<SiteRequest>>,
    pending: Mutex<HashMap<String, oneshot::Sender<SiteResponse>>>,
}
struct PendingRequest {
    site: Arc<SiteBridge>,
    id: String,
}
impl Drop for PendingRequest {
    fn drop(&mut self) {
        if let Ok(mut pending) = self.site.pending.lock() {
            pending.remove(&self.id);
        }
    }
}
impl Drop for SiteGrant {
    fn drop(&mut self) {
        self.bridge.cancel.cancel();
    }
}
impl SiteGrant {
    pub async fn open(port: u16, network: bool) -> Result<Self, String> {
        if port != 0 && port < 1024 {
            return Err("Choose a port from 1024 to 65535, or 0 for an available port".into());
        }
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let origin = format!("http://127.0.0.1:{port}");
        let secret = uuid::Uuid::new_v4().to_string();
        let route = SiteRoute {
            id: uuid::Uuid::new_v4().to_string(),
            port,
            url: format!("{origin}/?__lethetic_site={secret}"),
        };
        let (sender, receiver) = mpsc::channel(16);
        let bridge = Arc::new(SiteBridge {
            origin,
            secret,
            cookie: format!("lethetic_site_{port}"),
            network,
            cancel: CancellationToken::new(),
            sender,
            receiver: tokio::sync::Mutex::new(receiver),
            pending: Mutex::new(HashMap::new()),
        });
        let app = Router::new()
            .fallback(serve)
            .layer(DefaultBodyLimit::max(REQUEST_LIMIT))
            .with_state(bridge.clone());
        let shutdown = bridge.cancel.clone();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await;
        });
        Ok(Self { route, bridge })
    }
}
impl SiteBridge {
    pub async fn next(&self) -> Result<SiteRequest, String> {
        let mut receiver = self.receiver.lock().await;
        tokio::select! { _=self.cancel.cancelled()=>Err("Forwarding stopped".into()), item=receiver.recv()=>item.ok_or_else(||"Forwarding stopped".into()) }
    }
    pub fn reply(&self, id: &str, reply: SiteResponse) -> Result<(), String> {
        if reply.body.len() > RESPONSE_LIMIT.div_ceil(3) * 4 {
            return Err("Site response exceeds 64 MiB".into());
        }
        if !(200..=599).contains(&reply.status) {
            return Err("Invalid HTTP response status".into());
        }
        self.pending
            .lock()
            .map_err(|e| e.to_string())?
            .remove(id)
            .ok_or("Request expired")?
            .send(reply)
            .map_err(|_| "Request expired".into())
    }
}
fn header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}
fn hop_header(name: &str) -> bool {
    matches!(
        name,
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "content-length"
    )
}
async fn serve(
    State(site): State<Arc<SiteBridge>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    bytes: Bytes,
) -> Response {
    if header(&headers, "host").as_deref() != site.origin.strip_prefix("http://") {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok(mut url) = reqwest::Url::parse(&format!("{}{}", site.origin, uri)) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    // One bootstrap URL establishes an HttpOnly, same-site cookie, then removes
    // the capability from URLs before loading any user content or subresources.
    if method == Method::GET
        && url
            .query_pairs()
            .any(|(k, v)| k == "__lethetic_site" && v == site.secret)
    {
        let pairs = url
            .query_pairs()
            .filter(|(k, _)| k != "__lethetic_site")
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect::<Vec<_>>();
        url.set_query(None);
        if !pairs.is_empty() {
            url.query_pairs_mut().extend_pairs(pairs);
        }
        let mut response = StatusCode::SEE_OTHER.into_response();
        let path = match url.query() {
            Some(query) => format!("{}?{query}", url.path()),
            None => url.path().to_string(),
        };
        if let (Ok(location), Ok(cookie)) = (
            HeaderValue::from_str(&format!("{}{path}", site.origin)),
            HeaderValue::from_str(&format!(
                "{}={}; HttpOnly; SameSite=Strict; Path=/",
                site.cookie, site.secret
            )),
        ) {
            response.headers_mut().insert("location", location);
            response.headers_mut().insert("set-cookie", cookie);
            response
                .headers_mut()
                .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
        }
        return response;
    }
    if header(&headers, "origin").is_some_and(|v| v != site.origin)
        || header(&headers, "sec-fetch-site").is_some_and(|v| v == "cross-site")
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let expected = format!("{}={}", site.cookie, site.secret);
    if !header(&headers, "cookie").is_some_and(|v| v.split(';').any(|c| c.trim() == expected)) {
        return (
            StatusCode::FORBIDDEN,
            "Open this site using Lethetic's Open in browser button.",
        )
            .into_response();
    }
    let id = uuid::Uuid::new_v4().to_string();
    let mut forwarded = BTreeMap::<String, Vec<String>>::new();
    for (name, value) in &headers {
        if hop_header(name.as_str()) || name == "host" {
            continue;
        }
        if let Ok(value) = value.to_str() {
            let value = if name == "cookie" {
                value
                    .split(';')
                    .filter(|v| v.trim() != expected)
                    .collect::<Vec<_>>()
                    .join(";")
            } else {
                value.to_owned()
            };
            if !value.is_empty() {
                forwarded.entry(name.to_string()).or_default().push(value);
            }
        }
    }
    let request = SiteRequest {
        id: id.clone(),
        method: method.to_string(),
        path: uri.to_string(),
        headers: forwarded,
        body: STANDARD.encode(bytes),
    };
    let (send, receive) = oneshot::channel();
    if let Ok(mut pending) = site.pending.lock() {
        if pending.len() >= 16 {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        pending.insert(id.clone(), send);
    } else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let _pending = PendingRequest {
        site: site.clone(),
        id: id.clone(),
    };
    let result = tokio::select! {
        _=site.cancel.cancelled()=>Err("Forwarding stopped"),
        result=tokio::time::timeout(Duration::from_secs(60),async {site.sender.send(request).await.map_err(|_|"Forwarding stopped")?;receive.await.map_err(|_|"Forwarding stopped")})=>result.unwrap_or(Err("The app did not answer within 60 seconds")),
    };
    if let Ok(mut pending) = site.pending.lock() {
        pending.remove(&id);
    }
    let reply = match result {
        Ok(reply) => reply,
        Err(message) => return (StatusCode::GATEWAY_TIMEOUT, message).into_response(),
    };
    let body = match STANDARD.decode(&reply.body) {
        Ok(body) if body.len() <= RESPONSE_LIMIT => body,
        _ => return StatusCode::BAD_GATEWAY.into_response(),
    };
    let mut response = Response::new(if method == Method::HEAD {
        Body::empty()
    } else {
        Body::from(body)
    });
    *response.status_mut() = StatusCode::from_u16(reply.status).unwrap_or(StatusCode::BAD_GATEWAY);
    for (name, values) in reply.headers {
        let lower = name.to_ascii_lowercase();
        if hop_header(&lower)
            || lower.starts_with("access-control-")
            || lower == "content-security-policy"
        {
            continue;
        }
        for value in values {
            if lower == "set-cookie" && value.starts_with(&format!("{}=", site.cookie)) {
                continue;
            }
            if let (Ok(name), Ok(value)) = (
                HeaderName::from_bytes(name.as_bytes()),
                HeaderValue::from_str(&value),
            ) {
                response.headers_mut().append(name, value);
            }
        }
    }
    response
        .headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    let policy = if site.network {
        "default-src 'self' data: blob: http: https:; script-src 'self' 'unsafe-inline' 'unsafe-eval' https:; style-src 'self' 'unsafe-inline' https:; connect-src 'self' http: https:; frame-src 'none'; object-src 'none'; base-uri 'self'"
    } else {
        "default-src 'self' data: blob:; script-src 'self' 'unsafe-inline' 'unsafe-eval'; style-src 'self' 'unsafe-inline'; connect-src 'self'; form-action 'self'; frame-src 'none'; object-src 'none'; base-uri 'self'"
    };
    response
        .headers_mut()
        .insert("content-security-policy", HeaderValue::from_static(policy));
    response
}
