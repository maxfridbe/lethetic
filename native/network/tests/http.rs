use axum::{
    Router,
    body::Bytes,
    http::{HeaderMap, Method, StatusCode},
    routing::any,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use lethetic_network::{Grant, MAX_BYTES};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::{net::TcpListener, sync::Notify};

async fn fixture(app: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (url, task)
}
fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}
fn envelope(url: &str, method: &str) -> Value {
    json!({"url":url,"method":method,"headers":{},"bodyBase64":null})
}

#[tokio::test]
async fn methods_binary_bodies_and_explicit_authorization_work_without_cors() {
    let app = Router::new().route(
        "/",
        any(
            |method: Method, headers: HeaderMap, body: Bytes| async move {
                assert_eq!(headers["authorization"], "Bearer target-key");
                assert!(!headers.contains_key("cookie"));
                ([("x-method", method.to_string())], body)
            },
        ),
    );
    let (url, server) = fixture(app).await;
    let grant = Grant::open().await.unwrap();
    let http = client();
    for method in ["GET", "HEAD", "OPTIONS", "POST", "PUT", "PATCH", "DELETE"] {
        let bytes = if matches!(method, "GET" | "HEAD") {
            &[][..]
        } else {
            &[0, 255, 1, 128][..]
        };
        let response = http.post(&grant.route.endpoint).bearer_auth(&grant.route.token)
            .json(&json!({"url":url,"method":method,"headers":{"Authorization":"Bearer target-key","Cookie":"private"},"bodyBase64":STANDARD.encode(bytes)}))
            .send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["access-control-allow-origin"], "*");
        let value: Value = response.json().await.unwrap();
        assert_eq!(value["status"], 200);
        assert_eq!(value["headers"]["x-method"], method);
        assert_eq!(
            STANDARD
                .decode(value["bodyBase64"].as_str().unwrap())
                .unwrap(),
            bytes
        );
    }
    server.abort();
}

#[tokio::test]
async fn unauthorized_requests_and_non_http_targets_are_denied() {
    let first = Grant::open().await.unwrap();
    let second = Grant::open().await.unwrap();
    let http = client();
    for token in ["", "wrong", &second.route.token] {
        let response = http
            .post(&first.route.endpoint)
            .bearer_auth(token)
            .json(&envelope("https://example.com", "GET"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    for url in [
        "file:///etc/passwd",
        "socks5://127.0.0.1:9050",
        "http://user:password@127.0.0.1/",
        "https://example.com/#fragment",
    ] {
        let response = http
            .post(&first.route.endpoint)
            .bearer_auth(&first.route.token)
            .json(&envelope(url, "GET"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    let preflight = http
        .request(Method::OPTIONS, &first.route.endpoint)
        .send()
        .await
        .unwrap();
    assert_eq!(preflight.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        preflight.headers()["access-control-allow-headers"],
        "authorization, content-type"
    );
}

#[tokio::test]
async fn redirects_are_not_followed_and_large_responses_are_rejected() {
    let app = Router::new()
        .route(
            "/redirect",
            any(|| async {
                (
                    StatusCode::FOUND,
                    [("location", "http://127.0.0.1:1/private")],
                )
            }),
        )
        .route("/large", any(|| async { vec![0_u8; MAX_BYTES + 1] }));
    let (url, server) = fixture(app).await;
    let grant = Grant::open().await.unwrap();
    let http = client();
    let redirect: Value = http
        .post(&grant.route.endpoint)
        .bearer_auth(&grant.route.token)
        .json(&envelope(&(url.clone() + "/redirect"), "GET"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(redirect["status"], 302);
    let large = http
        .post(&grant.route.endpoint)
        .bearer_auth(&grant.route.token)
        .json(&envelope(&(url + "/large"), "GET"))
        .send()
        .await
        .unwrap();
    assert_eq!(large.status(), StatusCode::BAD_GATEWAY);
    server.abort();
}

#[tokio::test]
async fn revocation_cancels_an_in_flight_request() {
    let started = Arc::new(Notify::new());
    let observed = started.clone();
    let (url, server) = fixture(Router::new().route(
        "/",
        any(move || {
            let started = started.clone();
            async move {
                started.notify_one();
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                "late"
            }
        }),
    ))
    .await;
    let grant = Grant::open().await.unwrap();
    let request = client()
        .post(&grant.route.endpoint)
        .bearer_auth(&grant.route.token)
        .json(&envelope(&url, "GET"));
    let pending = tokio::spawn(async move { request.send().await.unwrap() });
    tokio::time::timeout(std::time::Duration::from_secs(5), observed.notified())
        .await
        .unwrap();
    drop(grant);
    let response = tokio::time::timeout(std::time::Duration::from_secs(5), pending)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    server.abort();
}

#[tokio::test]
async fn diagnostics_distinguish_target_rejections_from_relay_failures_and_survive_revocation() {
    use lethetic_network::{NetworkMode, diagnostics::Diagnostics};
    let first = Diagnostics::default();
    let other = Diagnostics::default();
    let grant = Grant::open_with_diagnostics(NetworkMode::Direct, None, first.clone())
        .await
        .unwrap();
    let (url, server) = fixture(Router::new().route(
        "/",
        any(|| async { (StatusCode::FORBIDDEN, "private target body") }),
    ))
    .await;
    let http = client();
    let rejected: Value = http
        .post(&grant.route.endpoint)
        .bearer_auth(&grant.route.token)
        .json(&envelope(&(url.clone() + "/?key=private-query"), "GET"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(rejected["status"], 403);
    server.abort();
    server.await.unwrap_err();
    let response = http
        .post(&grant.route.endpoint)
        .bearer_auth(&grant.route.token)
        .json(&envelope(&(url + "/?key=private-query"), "GET"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let error = response.text().await.unwrap();
    assert!(error.contains("connection failed"), "{error}");
    assert!(error.contains("refused"), "{error}");
    assert!(!error.contains("private-query"));
    drop(grant);
    let log = first.snapshot();
    assert_eq!(log.requests.len(), 2);
    assert_eq!(log.requests[0].target_status, Some(403));
    assert_eq!(log.requests[0].relay_status, 200);
    assert_eq!(log.requests[1].target_status, None);
    assert_eq!(log.requests[1].relay_status, 502);
    assert_eq!(log.requests[1].error.as_deref(), Some(error.as_str()));
    assert!(!serde_json::to_string(&log).unwrap().contains("private"));
    assert!(other.snapshot().requests.is_empty());
}
