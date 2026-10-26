use lethetic_network::{Grant, NetworkMode};
use serde_json::json;

#[tokio::test]
async fn tor_grants_cannot_fall_back_to_loopback_or_system_proxy() {
    let directory =
        tempfile::tempdir_in(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target"))
            .unwrap();
    let grant = Grant::open_mode(NetworkMode::Tor, Some(directory.path()))
        .await
        .unwrap();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    for target in [
        "http://127.0.0.1:8080/",
        "http://[::1]/",
        "http://localhost/",
        "http://printer.local/",
    ] {
        let response = client
            .post(&grant.route.endpoint)
            .bearer_auth(&grant.route.token)
            .json(&json!({"url": target, "method":"GET", "headers":{}}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
        assert!(response.text().await.unwrap().contains("Tor-only"));
    }
    assert!(Grant::open_mode(NetworkMode::Tor, None).await.is_err());
}

/// Explicit opt-in integration check; normal CI must not depend on the public Tor network.
#[tokio::test]
#[ignore = "requires public Tor connectivity"]
async fn public_tor_exit_reports_tor() {
    let directory =
        tempfile::tempdir_in(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target"))
            .unwrap();
    let grant = Grant::open_mode(NetworkMode::Tor, Some(directory.path()))
        .await
        .unwrap();
    let response = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(75))
        .build()
        .unwrap()
        .post(&grant.route.endpoint)
        .bearer_auth(&grant.route.token)
        .json(&json!({"url":"https://check.torproject.org/api/ip", "method":"GET", "headers":{}}))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let text = response.text().await.unwrap();
    assert_eq!(status, 200, "{text}");
    let reply: serde_json::Value = serde_json::from_str(&text).unwrap();
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(reply["bodyBase64"].as_str().unwrap())
        .unwrap();
    let result: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(result["IsTor"], true);
}
