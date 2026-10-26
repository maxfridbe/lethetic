use base64::{Engine, engine::general_purpose::STANDARD};
use lethetic_network::sites::{SiteGrant, SiteResponse};
use std::collections::BTreeMap;
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}
async fn authorize(grant: &SiteGrant) -> (String, String) {
    let reply = client().get(&grant.route.url).send().await.unwrap();
    assert_eq!(reply.status(), 303);
    let location = reply.headers()["location"].to_str().unwrap().to_owned();
    assert!(!location.contains("__lethetic_site"));
    let cookie = reply.headers()["set-cookie"].to_str().unwrap();
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"));
    (location, cookie.split(';').next().unwrap().to_owned())
}
#[tokio::test]
async fn real_site_http_forwards_methods_binary_headers_and_routes() {
    let grant = SiteGrant::open(0, false).await.unwrap();
    let (url, cookie) = authorize(&grant).await;
    for method in ["GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "HEAD"] {
        let body = vec![0, 255, 13, 10];
        let target = format!("{url}api/test?q=1");
        let request = client()
            .request(method.parse().unwrap(), target)
            .header("Cookie", &cookie)
            .header("X-Test", "yes")
            .body(body.clone());
        let pending = tokio::spawn(async move { request.send().await.unwrap() });
        let incoming = grant.bridge.next().await.unwrap();
        assert_eq!(incoming.method, method);
        assert_eq!(incoming.path, "/api/test?q=1");
        assert_eq!(STANDARD.decode(&incoming.body).unwrap(), body);
        assert_eq!(incoming.headers["x-test"], ["yes"]);
        assert!(!incoming.headers.contains_key("cookie"));
        grant
            .bridge
            .reply(
                &incoming.id,
                SiteResponse {
                    status: 200,
                    headers: BTreeMap::from([(
                        "content-type".into(),
                        vec!["application/octet-stream".into()],
                    )]),
                    body: STANDARD.encode(&body),
                },
            )
            .unwrap();
        let response = pending.await.unwrap();
        assert_eq!(response.status(), 200);
        assert!(
            response.headers()["content-security-policy"]
                .to_str()
                .unwrap()
                .contains("connect-src 'self';")
        );
        assert_eq!(
            response.bytes().await.unwrap().as_ref(),
            if method == "HEAD" { &[][..] } else { &body }
        );
    }
}
#[tokio::test]
async fn forwarded_sites_reject_unauthorized_cross_origin_and_rebinding_requests() {
    let grant = SiteGrant::open(0, true).await.unwrap();
    let (url, cookie) = authorize(&grant).await;
    assert_eq!(client().get(&url).send().await.unwrap().status(), 403);
    assert_eq!(
        client()
            .get(&url)
            .header("Cookie", &cookie)
            .header("Origin", "https://evil.example")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        client()
            .get(&url)
            .header("Cookie", &cookie)
            .header("Host", "evil.example")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        client()
            .get(&url)
            .header("Cookie", &cookie)
            .header("Sec-Fetch-Site", "cross-site")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert!(SiteGrant::open(grant.route.port, false).await.is_err());
    assert!(SiteGrant::open(80, false).await.is_err());
}
#[tokio::test]
async fn stopping_site_cancels_pending_http_and_native_poll() {
    let grant = SiteGrant::open(0, false).await.unwrap();
    let (url, cookie) = authorize(&grant).await;
    let response = tokio::spawn(async move {
        client()
            .get(url)
            .header("Cookie", cookie)
            .send()
            .await
            .unwrap()
    });
    let incoming = grant.bridge.next().await.unwrap();
    let bridge = grant.bridge.clone();
    drop(grant);
    assert_eq!(response.await.unwrap().status(), 504);
    assert!(bridge.next().await.is_err());
    assert!(
        bridge
            .reply(
                &incoming.id,
                SiteResponse {
                    status: 200,
                    headers: BTreeMap::new(),
                    body: String::new()
                }
            )
            .is_err()
    );
}
