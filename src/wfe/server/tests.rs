use super::*;

#[cfg(target_os = "linux")]
fn reserve_loopback_listener_pair() -> (
    std::net::TcpListener,
    std::net::TcpListener,
    SocketAddr,
    SocketAddr,
) {
    for _ in 0..128 {
        let first = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = first.local_addr().unwrap().port();
        let first_address = SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), port);
        let second_address =
            SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 2)), port);
        if let Ok(second) = std::net::TcpListener::bind(second_address) {
            return (first, second, first_address, second_address);
        }
    }
    panic!("could not reserve one port on both loopback addresses");
}

#[tokio::test]
async fn target_resolution_is_one_shot_bounded_and_pinned() {
    let literal = WfeTarget::parse("https://127.0.0.1:11223").unwrap();
    let literal_calls = Arc::new(AtomicUsize::new(0));
    let calls = Arc::clone(&literal_calls);
    let literal_plan = resolve_target_with(&literal, move |_, _| {
        calls.fetch_add(1, Ordering::SeqCst);
        async { Ok::<_, String>(Vec::new()) }
    })
    .await
    .unwrap();
    assert_eq!(literal_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        literal_plan.addresses(),
        &["127.0.0.1:11223".parse::<SocketAddr>().unwrap()]
    );

    let dns = WfeTarget::parse("https://brainiac:11223").unwrap();
    let dns_calls = Arc::new(AtomicUsize::new(0));
    let calls = Arc::clone(&dns_calls);
    let plan = resolve_target_with(&dns, move |hostname, port| {
        calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(hostname, "brainiac");
        assert_eq!(port, 11223);
        async {
            Ok::<_, String>(vec![
                "127.0.0.2:1".parse().unwrap(),
                "[::1]:2".parse().unwrap(),
                "127.0.0.1:3".parse().unwrap(),
                "127.0.0.2:4".parse().unwrap(),
            ])
        }
    })
    .await
    .unwrap();
    assert_eq!(dns_calls.load(Ordering::SeqCst), 1);
    let mut expected = vec![
        "127.0.0.1:11223".parse::<SocketAddr>().unwrap(),
        "127.0.0.2:11223".parse().unwrap(),
        "[::1]:11223".parse().unwrap(),
    ];
    expected.sort();
    assert_eq!(plan.addresses(), expected);
    assert_eq!(plan.target(), &dns);

    assert!(bind_plan_from_answers(&dns, Vec::new()).is_err());
    assert!(
        bind_plan_from_answers(
            &dns,
            vec!["127.0.0.1:1".parse().unwrap(), "0.0.0.0:1".parse().unwrap(),],
        )
        .is_err()
    );
    assert!(
        bind_plan_from_answers(
            &dns,
            (0..=WFE_DNS_ANSWER_CAP)
                .map(|index| SocketAddr::new(
                    IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                    index as u16
                ))
                .collect(),
        )
        .is_err()
    );
    assert!(
        resolve_target_with(&dns, |_, _| async {
            Err::<Vec<SocketAddr>, _>("lookup failed".to_string())
        })
        .await
        .is_err()
    );
    assert!(
        resolve_target_with(&dns, |_, _| async {
            std::future::pending::<Result<Vec<SocketAddr>, String>>().await
        })
        .await
        .is_err()
    );
}

#[test]
fn asset_paths_reject_traversal_and_special_forms() {
    assert!(valid_asset_path("src/app.js"));
    for rejected in ["", "/index.html", "../index.html", "a/../b", "a//b", "a\\b"] {
        assert!(!valid_asset_path(rejected), "accepted {rejected:?}");
    }
}

#[test]
fn process_cookie_is_private_exact_and_duplicate_safe() {
    let cookie = ProcessCookie::generate().unwrap();
    let value = cookie.expose().to_string();
    assert!(cookie.verify(&value));
    assert!(!cookie.verify("different"));
    assert!(!format!("{cookie:?}").contains(&value));

    let mut headers = HeaderMap::new();
    headers.insert(
        COOKIE,
        HeaderValue::from_str(&format!("other=ok; {WFE_COOKIE_NAME}={value}")).unwrap(),
    );
    assert!(cookie.verify_headers(&headers));
    headers.insert(
        COOKIE,
        HeaderValue::from_str(&format!(
            "{WFE_COOKIE_NAME}={value}; {WFE_COOKIE_NAME}={value}"
        ))
        .unwrap(),
    );
    assert!(!cookie.verify_headers(&headers));
}

#[test]
fn websocket_protocol_is_independent_private_and_exact() {
    let protocol = WebSocketProtocol::generate().unwrap();
    let value = protocol.expose().to_string();
    assert!(value.starts_with(WFE_WEBSOCKET_PROTOCOL_PREFIX));
    assert!(protocol.verify(&value));
    assert!(!protocol.verify("lethetic-wfe-v1.different"));
    assert!(!format!("{protocol:?}").contains(&value));
}

#[test]
fn websocket_rtt_accepts_only_the_matching_ping_nonce() {
    let started = Instant::now();
    assert!(matching_pong_round_trip_time(b"expected", b"different", started).is_none());
    assert!(matching_pong_round_trip_time(b"expected", b"expected", started).is_some());
}

#[tokio::test]
async fn lifecycle_backpressure_retains_the_authenticated_client_slot() {
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    let queued = WfeConnectionEvent::Connected(WfeConnectedEvent {
        peer_ip: "127.0.0.1".parse().unwrap(),
        connection_ordinal: 1,
        active_clients: 1,
        authentication_mode: ControllerAuthenticationMode::TokenRequired,
        initial_sequence: 0,
        initial_revision: 0,
        round_trip_time: Some(Duration::from_millis(1)),
    });
    sender.send(queued).await.unwrap();
    let slots = Arc::new(Semaphore::new(1));
    let permit = slots.clone().acquire_owned().await.unwrap();
    let telemetry = ConnectionTelemetry {
        sender,
        peer_ip: "127.0.0.1".parse().unwrap(),
        connection_ordinal: 1,
        admitted_active_clients: 1,
        authentication_mode: ControllerAuthenticationMode::TokenRequired,
        initial_sequence: 0,
        initial_revision: 0,
        connected_emitted: true,
    };
    let finishing = tokio::spawn(emit_disconnected_before_releasing_client(
        telemetry,
        0,
        Duration::from_secs(1),
        WfeDisconnectCategory::PeerClosed,
        permit,
    ));
    tokio::task::yield_now().await;
    assert!(slots.clone().try_acquire_owned().is_err());

    assert!(receiver.recv().await.is_some());
    timeout(Duration::from_secs(1), finishing)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        receiver.recv().await,
        Some(WfeConnectionEvent::Disconnected(_))
    ));
    assert!(slots.try_acquire_owned().is_ok());
}

#[tokio::test]
async fn authentication_body_reader_is_bounded_and_session_body_is_exact() {
    let body = read_authentication_body(Body::from(vec![b'a'; WFE_AUTH_BODY_BYTES]))
        .await
        .unwrap();
    assert_eq!(body.len(), WFE_AUTH_BODY_BYTES);
    assert_eq!(
        read_authentication_body(Body::from(vec![b'a'; WFE_AUTH_BODY_BYTES + 1])).await,
        Err(AuthenticationBodyError::TooLarge)
    );

    assert!(is_exact_empty_json_object(br#"{}"#));
    for rejected in [
        b"".as_slice(),
        b"null".as_slice(),
        b"[]".as_slice(),
        br#"{"unexpected":true}"#.as_slice(),
    ] {
        assert!(
            !is_exact_empty_json_object(rejected),
            "accepted {}",
            String::from_utf8_lossy(rejected)
        );
    }
}

#[test]
fn authentication_attempts_are_bounded_per_peer_and_globally() {
    let mut limiter = AuthAttemptLimiter::default();
    let now = Instant::now();
    let peer: IpAddr = "127.0.0.1".parse().unwrap();
    for _ in 0..WFE_AUTH_ATTEMPTS_PER_PEER {
        assert!(limiter.admit(peer, now));
    }
    assert!(!limiter.admit(peer, now));
    assert!(limiter.admit(peer, now + WFE_AUTH_WINDOW));
}

#[test]
fn malformed_commands_with_valid_ids_receive_correlated_errors() {
    assert_eq!(
        malformed_request_id(r#"{"id":"request-1","unexpected":true}"#),
        Some("request-1".to_string())
    );
    assert!(malformed_request_id(r#"{"unexpected":true}"#).is_none());
    assert!(malformed_request_id(r#"{"id":"bad id"}"#).is_none());
    assert!(malformed_request_id("not-json").is_none());
}

#[test]
fn snapshot_after_patch_tracks_only_state_sent_to_the_client() {
    let app = crate::app::App::new(&crate::config::Config::default());
    let initial_state = crate::wfe::presentation::project_app(
        &app,
        crate::wfe::presentation::ProjectionContext::default(),
    );
    let known = IStateSnapshot::new(0, 0, initial_state.clone()).unwrap();
    let mut semantic = initial_state;
    semantic.debugger.open = false;
    let patch = IStatePatch::between(1, 0, 1, &known.state, &semantic).unwrap();
    let patched = snapshot_after_patch(&known, &patch).unwrap();
    assert_eq!(patched.state, semantic);

    let mut volatile = semantic;
    volatile.status.memory_mebibytes = "999".to_string();
    assert_ne!(patched.state, volatile);
}

#[test]
fn local_errors_are_valid_bounded_contract_messages() {
    let response = local_error_response(
        "request-1".to_string(),
        CommandErrorCode::Busy,
        "server busy",
        true,
        7,
    );
    response.validate().unwrap();
    assert_eq!(response.id, "request-1");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn exact_hostname_tls_http_and_wss_use_every_pinned_listener() {
    use futures_util::{SinkExt as _, StreamExt as _};
    use rustls::pki_types::CertificateDer;
    use tokio_tungstenite::Connector;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;

    let (first_reservation, second_reservation, first_address, second_address) =
        reserve_loopback_listener_pair();
    let port = first_address.port();

    let target = WfeTarget::parse(&format!("https://brainiac:{port}")).unwrap();
    let bind_plan = bind_plan_from_answers(
        &target,
        vec![
            SocketAddr::new(second_address.ip(), 1),
            SocketAddr::new(first_address.ip(), 2),
        ],
    )
    .unwrap();
    let expected_addresses = bind_plan.addresses().to_vec();
    let temporary = tempfile::tempdir().unwrap();
    let security = super::super::security::prepare_security(
        &target,
        super::super::security::SecurityFileOptions::default(),
        Some(&temporary.path().join("wfe-state")),
    )
    .unwrap();
    let token = security.bootstrap_token_for_host().unwrap().to_string();
    let certificate_der = security.certificate_chain_der()[0].as_ref().to_vec();
    let app = crate::app::App::new(&crate::config::Config::default());
    let (frontend, mut runtime) =
        crate::wfe::runtime::WfeRuntime::new(&app, vec![token.clone()]).unwrap();
    drop(first_reservation);
    drop(second_reservation);
    let (server, mut host_info) = start(security, bind_plan, frontend).await.unwrap();
    assert_eq!(host_info.target(), target.as_str());
    assert_eq!(host_info.listener_addresses(), expected_addresses);
    assert_eq!(
        host_info.take_bootstrap_url().unwrap().as_str(),
        format!("{}/#token={token}", target.as_str())
    );

    let client_for = |name: &str, address: SocketAddr| {
        reqwest::Client::builder()
            .no_proxy()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .add_root_certificate(reqwest::Certificate::from_der(&certificate_der).unwrap())
            .resolve(name, address)
            .build()
            .unwrap()
    };
    for address in &expected_addresses {
        let client = client_for("brainiac", *address);
        let response = client
            .get(format!("{}/", target.as_str()))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{address}");
        let csp = response
            .headers()
            .get(CONTENT_SECURITY_POLICY.clone())
            .unwrap()
            .to_str()
            .unwrap();
        assert!(csp.contains(&format!("wss://brainiac:{port}")));
    }

    let client = client_for("brainiac", first_address);
    let wrong_host = client
        .get(format!("{}/", target.as_str()))
        .header(HOST, first_address.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(wrong_host.status(), StatusCode::MISDIRECTED_REQUEST);
    let wrong_origin = client
        .post(format!("{}/auth", target.as_str()))
        .header(ORIGIN, format!("https://{}", first_address))
        .json(&serde_json::json!({ "token": &token }))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong_origin.status(), StatusCode::FORBIDDEN);

    let authenticated = client
        .post(format!("{}/auth", target.as_str()))
        .header(ORIGIN, target.canonical_origin())
        .json(&serde_json::json!({ "token": &token }))
        .send()
        .await
        .unwrap();
    assert_eq!(authenticated.status(), StatusCode::NO_CONTENT);
    let websocket_protocol = authenticated
        .headers()
        .get(WFE_WEBSOCKET_PROTOCOL_HEADER.clone())
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let set_cookie = authenticated
        .headers()
        .get(SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(!set_cookie.contains("Domain="));
    let cookie_pair = set_cookie.split(';').next().unwrap().to_string();

    let direct_ip = reqwest::Client::builder()
        .no_proxy()
        .https_only(true)
        .timeout(Duration::from_secs(5))
        .add_root_certificate(reqwest::Certificate::from_der(&certificate_der).unwrap())
        .build()
        .unwrap();
    assert!(
        direct_ip
            .get(format!("https://{first_address}/"))
            .send()
            .await
            .is_err()
    );
    let alias_client = client_for("brainiac-alias", first_address);
    assert!(
        alias_client
            .get(format!("https://brainiac-alias:{port}/"))
            .send()
            .await
            .is_err(),
        "TLS unexpectedly admitted an alternate DNS identity"
    );
    let trailing_dot_client = client_for("brainiac.", first_address);
    match trailing_dot_client
        .get(format!("https://brainiac.:{port}/"))
        .send()
        .await
    {
        Err(_) => {}
        Ok(response) => assert_eq!(response.status(), StatusCode::MISDIRECTED_REQUEST),
    }

    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from(certificate_der.clone()))
        .unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let client_tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = Connector::Rustls(Arc::new(client_tls));
    let mut request = format!("wss://brainiac:{port}/ws")
        .into_client_request()
        .unwrap();
    request.headers_mut().insert(
        ORIGIN,
        HeaderValue::from_str(&target.canonical_origin()).unwrap(),
    );
    request
        .headers_mut()
        .insert(COOKIE, HeaderValue::from_str(&cookie_pair).unwrap());
    request.headers_mut().insert(
        axum::http::header::SEC_WEBSOCKET_PROTOCOL,
        HeaderValue::from_str(&websocket_protocol).unwrap(),
    );
    let stream = tokio::net::TcpStream::connect(second_address)
        .await
        .unwrap();
    let (mut socket, response) =
        tokio_tungstenite::client_async_tls_with_config(request, stream, None, Some(connector))
            .await
            .unwrap();
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
    for expected in ["hello", "state_snapshot"] {
        let message = timeout(Duration::from_secs(5), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let tokio_tungstenite::tungstenite::Message::Text(message) = message else {
            panic!("{expected} was not text");
        };
        let parsed = serde_json::from_str::<IServerMessage>(message.as_str()).unwrap();
        assert_eq!(
            match parsed {
                IServerMessage::Hello { .. } => "hello",
                IServerMessage::StateSnapshot { .. } => "state_snapshot",
                _ => "other",
            },
            expected
        );
    }
    let ping = timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let tokio_tungstenite::tungstenite::Message::Ping(payload) = ping else {
        panic!("RTT probe was not a Ping");
    };
    socket
        .send(tokio_tungstenite::tungstenite::Message::Pong(payload))
        .await
        .unwrap();
    let connected = timeout(Duration::from_secs(5), runtime.recv_connection_event())
        .await
        .unwrap()
        .unwrap();
    let WfeConnectionEvent::Connected(connected) = connected else {
        panic!("hostname WSS did not emit a connected event");
    };
    assert_eq!(connected.peer_ip, IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));

    socket.close(None).await.unwrap();
    drop(client);
    drop(runtime);
    server.shutdown().await.unwrap();
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn multi_address_bind_failure_rolls_back_every_earlier_listener() {
    let (first_reservation, _occupied, first_address, occupied_address) =
        reserve_loopback_listener_pair();
    let port = first_address.port();
    let target = WfeTarget::parse(&format!("https://brainiac:{port}")).unwrap();
    let bind_plan = bind_plan_from_answers(
        &target,
        vec![
            SocketAddr::new(first_address.ip(), 1),
            SocketAddr::new(occupied_address.ip(), 2),
        ],
    )
    .unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let security = super::super::security::prepare_security(
        &target,
        super::super::security::SecurityFileOptions::default(),
        Some(&temporary.path().join("wfe-state")),
    )
    .unwrap();
    let token = security.bootstrap_token_for_host().unwrap().to_string();
    let app = crate::app::App::new(&crate::config::Config::default());
    let (frontend, runtime) = crate::wfe::runtime::WfeRuntime::new(&app, vec![token]).unwrap();
    drop(first_reservation);
    let error = match start(security, bind_plan, frontend).await {
        Ok(_) => panic!("partially occupied listener group unexpectedly started"),
        Err(error) => error,
    };
    assert!(error.contains(&occupied_address.to_string()), "{error}");
    let reclaimed = std::net::TcpListener::bind(first_address).unwrap();
    drop(reclaimed);
    drop(runtime);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn local_https_auth_and_wss_are_exact_and_state_only_follows_authentication() {
    use futures_util::{SinkExt as _, StreamExt as _};
    use rustls::pki_types::CertificateDer;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio_tungstenite::Connector;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;

    let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = reservation.local_addr().unwrap();
    drop(reservation);
    let target = super::super::security::WfeTarget::parse(&format!("https://{address}")).unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let state_root = temporary.path().join("wfe-state");
    let security = super::super::security::prepare_security(
        &target,
        super::super::security::SecurityFileOptions::default(),
        Some(&state_root),
    )
    .unwrap();
    let token = security.bootstrap_token_for_host().unwrap().to_string();
    let certificate_der = security.certificate_chain_der()[0].as_ref().to_vec();
    let mut app = crate::app::App::new(&crate::config::Config::default());
    let active_session_id = app.session_id.clone();
    let (frontend, mut runtime) =
        crate::wfe::runtime::WfeRuntime::new(&app, vec![token.clone()]).unwrap();
    let bind_plan = resolve_target(&target).await.unwrap();
    let (server, mut host_info) = start(security, bind_plan, frontend).await.unwrap();
    assert_eq!(host_info.profile(), SecurityProfile::AutomaticGenerated);
    assert_eq!(
        host_info.authentication_mode(),
        ControllerAuthenticationMode::TokenRequired
    );
    let bootstrap_url = host_info.take_bootstrap_url().unwrap();
    assert_eq!(
        bootstrap_url.as_str(),
        format!("{}/#token={token}", target.as_str())
    );

    let client = reqwest::Client::builder()
        .no_proxy()
        .tls_danger_accept_invalid_certs(true)
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let base = target.as_str();
    let index = client.get(format!("{base}/")).send().await.unwrap();
    assert_eq!(index.status(), StatusCode::OK);
    assert_eq!(
        index.headers().get(X_CONTENT_TYPE_OPTIONS.clone()).unwrap(),
        "nosniff"
    );
    let csp = index
        .headers()
        .get(CONTENT_SECURITY_POLICY.clone())
        .unwrap()
        .to_str()
        .unwrap();
    assert!(csp.contains("default-src 'self'"));
    assert!(csp.contains("script-src 'self'"));
    assert!(!csp.contains("'unsafe-inline'"));
    let index_body = index.text().await.unwrap();
    assert!(index_body.contains("<title>Lethetic</title>"));
    assert!(!index_body.contains("http-equiv=\"Content-Security-Policy\""));
    assert!(!index_body.contains(&active_session_id));

    let mut plaintext = tokio::net::TcpStream::connect(address).await.unwrap();
    plaintext
        .write_all(b"GET / HTTP/1.1\r\nHost: invalid\r\n\r\n")
        .await
        .unwrap();
    let mut plaintext_response = [0_u8; 16];
    let plaintext_read = timeout(
        Duration::from_secs(2),
        plaintext.read(&mut plaintext_response),
    )
    .await;
    if let Ok(Ok(count)) = plaintext_read {
        assert!(
            !plaintext_response[..count].starts_with(b"HTTP/"),
            "TLS listener returned a plaintext HTTP response"
        );
    }
    drop(plaintext);

    let wrong_host = client
        .get(format!("{base}/"))
        .header(HOST, "127.0.0.1:1")
        .send()
        .await
        .unwrap();
    assert_eq!(wrong_host.status(), StatusCode::MISDIRECTED_REQUEST);

    let traversal = client
        .get(format!("{base}/%2e%2e/Cargo.toml"))
        .send()
        .await
        .unwrap();
    assert_eq!(traversal.status(), StatusCode::NOT_FOUND);

    let missing_origin = client
        .post(format!("{base}/auth"))
        .json(&serde_json::json!({ "token": &token }))
        .send()
        .await
        .unwrap();
    assert_eq!(missing_origin.status(), StatusCode::FORBIDDEN);
    assert!(missing_origin.headers().get(SET_COOKIE).is_none());
    assert!(
        missing_origin
            .headers()
            .get(WFE_WEBSOCKET_PROTOCOL_HEADER.clone())
            .is_none()
    );
    assert!(
        missing_origin
            .headers()
            .get(axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none()
    );

    let null_origin = client
        .post(format!("{base}/auth"))
        .header(ORIGIN, "null")
        .json(&serde_json::json!({ "token": &token }))
        .send()
        .await
        .unwrap();
    assert_eq!(null_origin.status(), StatusCode::FORBIDDEN);
    assert!(null_origin.headers().get(SET_COOKIE).is_none());
    assert!(
        null_origin
            .headers()
            .get(WFE_WEBSOCKET_PROTOCOL_HEADER.clone())
            .is_none()
    );
    assert!(
        null_origin
            .headers()
            .get(axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none()
    );

    let wrong_origin = client
        .post(format!("{base}/auth"))
        .header(ORIGIN, "https://127.0.0.2:9")
        .json(&serde_json::json!({ "token": &token }))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong_origin.status(), StatusCode::FORBIDDEN);
    assert!(wrong_origin.headers().get(SET_COOKIE).is_none());
    assert!(
        wrong_origin
            .headers()
            .get(WFE_WEBSOCKET_PROTOCOL_HEADER.clone())
            .is_none()
    );
    assert!(
        wrong_origin
            .headers()
            .get(axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none()
    );

    let authenticated = client
        .post(format!("{base}/auth"))
        .header(ORIGIN, target.canonical_origin())
        .json(&serde_json::json!({ "token": &token }))
        .send()
        .await
        .unwrap();
    assert_eq!(authenticated.status(), StatusCode::NO_CONTENT);
    assert!(
        authenticated
            .headers()
            .get(axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none()
    );
    let authenticated_again = client
        .post(format!("{base}/auth"))
        .header(ORIGIN, target.canonical_origin())
        .json(&serde_json::json!({ "token": &token }))
        .send()
        .await
        .unwrap();
    assert_eq!(authenticated_again.status(), StatusCode::NO_CONTENT);
    let tokenless_route = client
        .post(format!("{base}/auth/session"))
        .header(ORIGIN, target.canonical_origin())
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(tokenless_route.status(), StatusCode::UNAUTHORIZED);
    assert!(tokenless_route.headers().get(SET_COOKIE).is_none());
    assert!(
        tokenless_route
            .headers()
            .get(WFE_WEBSOCKET_PROTOCOL_HEADER.clone())
            .is_none()
    );
    let websocket_protocol = authenticated
        .headers()
        .get(WFE_WEBSOCKET_PROTOCOL_HEADER.clone())
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(websocket_protocol.starts_with(WFE_WEBSOCKET_PROTOCOL_PREFIX));
    let set_cookie = authenticated
        .headers()
        .get(SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(set_cookie.contains("Secure"));
    assert!(set_cookie.contains("HttpOnly"));
    assert!(set_cookie.contains("SameSite=Strict"));
    assert!(set_cookie.contains("Path=/"));
    assert!(!set_cookie.contains("Domain="));
    let cookie_pair = set_cookie.split(';').next().unwrap().to_string();

    let mut roots = rustls::RootCertStore::empty();
    roots.add(CertificateDer::from(certificate_der)).unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let client_tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = Connector::Rustls(Arc::new(client_tls));

    let mut unauthenticated_request = format!("wss://{address}/ws").into_client_request().unwrap();
    unauthenticated_request.headers_mut().insert(
        ORIGIN,
        HeaderValue::from_str(&target.canonical_origin()).unwrap(),
    );
    let unauthenticated = tokio_tungstenite::connect_async_tls_with_config(
        unauthenticated_request,
        None,
        false,
        Some(connector.clone()),
    )
    .await
    .unwrap_err();
    match unauthenticated {
        tokio_tungstenite::tungstenite::Error::Http(response) => {
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
        error => panic!("unexpected unauthenticated WSS result: {error}"),
    }

    let mut cross_origin_request = format!("wss://{address}/ws").into_client_request().unwrap();
    cross_origin_request
        .headers_mut()
        .insert(ORIGIN, HeaderValue::from_static("https://127.0.0.2:9"));
    cross_origin_request
        .headers_mut()
        .insert(COOKIE, HeaderValue::from_str(&cookie_pair).unwrap());
    let cross_origin = tokio_tungstenite::connect_async_tls_with_config(
        cross_origin_request,
        None,
        false,
        Some(connector.clone()),
    )
    .await
    .unwrap_err();
    match cross_origin {
        tokio_tungstenite::tungstenite::Error::Http(response) => {
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }
        error => panic!("unexpected cross-origin WSS result: {error}"),
    }

    let mut cookie_only_request = format!("wss://{address}/ws").into_client_request().unwrap();
    cookie_only_request.headers_mut().insert(
        ORIGIN,
        HeaderValue::from_str(&target.canonical_origin()).unwrap(),
    );
    cookie_only_request
        .headers_mut()
        .insert(COOKIE, HeaderValue::from_str(&cookie_pair).unwrap());
    let cookie_only = tokio_tungstenite::connect_async_tls_with_config(
        cookie_only_request,
        None,
        false,
        Some(connector.clone()),
    )
    .await
    .unwrap_err();
    match cookie_only {
        tokio_tungstenite::tungstenite::Error::Http(response) => {
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
        error => panic!("unexpected cookie-only WSS result: {error}"),
    }

    let mut wrong_protocol = websocket_protocol.clone();
    let final_character = wrong_protocol.pop().unwrap();
    wrong_protocol.push(if final_character == 'A' { 'B' } else { 'A' });
    let mut wrong_protocol_request = format!("wss://{address}/ws").into_client_request().unwrap();
    wrong_protocol_request.headers_mut().insert(
        ORIGIN,
        HeaderValue::from_str(&target.canonical_origin()).unwrap(),
    );
    wrong_protocol_request
        .headers_mut()
        .insert(COOKIE, HeaderValue::from_str(&cookie_pair).unwrap());
    wrong_protocol_request.headers_mut().insert(
        axum::http::header::SEC_WEBSOCKET_PROTOCOL,
        HeaderValue::from_str(&wrong_protocol).unwrap(),
    );
    let wrong_protocol_result = tokio_tungstenite::connect_async_tls_with_config(
        wrong_protocol_request,
        None,
        false,
        Some(connector.clone()),
    )
    .await
    .unwrap_err();
    match wrong_protocol_result {
        tokio_tungstenite::tungstenite::Error::Http(response) => {
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
        error => panic!("unexpected wrong-protocol WSS result: {error}"),
    }

    let mut request = format!("wss://{address}/ws").into_client_request().unwrap();
    request.headers_mut().insert(
        ORIGIN,
        HeaderValue::from_str(&target.canonical_origin()).unwrap(),
    );
    request
        .headers_mut()
        .insert(COOKIE, HeaderValue::from_str(&cookie_pair).unwrap());
    request.headers_mut().insert(
        axum::http::header::SEC_WEBSOCKET_PROTOCOL,
        HeaderValue::from_str(&websocket_protocol).unwrap(),
    );
    let (mut socket, upgrade_response) =
        tokio_tungstenite::connect_async_tls_with_config(request, None, false, Some(connector))
            .await
            .unwrap();
    assert_eq!(
        upgrade_response
            .headers()
            .get(axum::http::header::SEC_WEBSOCKET_PROTOCOL)
            .unwrap(),
        websocket_protocol.as_str()
    );
    let first = timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let second = timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let tokio_tungstenite::tungstenite::Message::Text(first) = first else {
        panic!("hello was not text");
    };
    let tokio_tungstenite::tungstenite::Message::Text(second) = second else {
        panic!("snapshot was not text");
    };
    assert!(matches!(
        serde_json::from_str::<IServerMessage>(first.as_str()).unwrap(),
        IServerMessage::Hello { .. }
    ));
    assert!(matches!(
        serde_json::from_str::<IServerMessage>(second.as_str()).unwrap(),
        IServerMessage::StateSnapshot { .. }
    ));
    let ping = timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let tokio_tungstenite::tungstenite::Message::Ping(ping_payload) = ping else {
        panic!("RTT probe was not a WebSocket Ping");
    };
    socket
        .send(tokio_tungstenite::tungstenite::Message::Pong(ping_payload))
        .await
        .unwrap();
    let connected = timeout(Duration::from_secs(5), runtime.recv_connection_event())
        .await
        .unwrap()
        .unwrap();
    let WfeConnectionEvent::Connected(connected) = connected else {
        panic!("first lifecycle event was not connected");
    };
    assert_eq!(connected.peer_ip, IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
    assert_eq!(connected.connection_ordinal, 1);
    assert_eq!(connected.active_clients, 1);
    assert_eq!(
        connected.authentication_mode,
        ControllerAuthenticationMode::TokenRequired
    );
    assert_eq!(connected.initial_sequence, 0);
    assert_eq!(connected.initial_revision, 0);
    assert!(connected.round_trip_time.is_some());

    let snapshot_request = ICommandRequest {
        id: "snapshot-request".to_string(),
        expected_revision: 0,
        command: WebCommand::RequestSnapshot,
    };
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&snapshot_request).unwrap().into(),
        ))
        .await
        .unwrap();
    let envelope = timeout(Duration::from_secs(5), runtime.recv())
        .await
        .unwrap()
        .unwrap();
    let admitted = runtime.admit(&app, envelope).unwrap();
    runtime.publish(&app).unwrap();
    runtime
        .complete(
            admitted,
            Ok(crate::wfe::contracts::CommandOutcome::SnapshotQueued),
        )
        .unwrap();
    let response_message = timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let snapshot_message = timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let tokio_tungstenite::tungstenite::Message::Text(response_message) = response_message else {
        panic!("command response was not text");
    };
    let tokio_tungstenite::tungstenite::Message::Text(snapshot_message) = snapshot_message else {
        panic!("requested snapshot was not text");
    };
    assert!(matches!(
        serde_json::from_str::<IServerMessage>(response_message.as_str()).unwrap(),
        IServerMessage::CommandResponse { .. }
    ));
    assert!(matches!(
        serde_json::from_str::<IServerMessage>(snapshot_message.as_str()).unwrap(),
        IServerMessage::StateSnapshot { .. }
    ));

    app.add_segment(
        "server-pushed-state".to_string(),
        crate::app::BlockType::Text,
    );
    assert!(runtime.publish(&app).unwrap());
    let pushed = timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let tokio_tungstenite::tungstenite::Message::Text(pushed) = pushed else {
        panic!("state patch was not text");
    };
    assert!(matches!(
        serde_json::from_str::<IServerMessage>(pushed.as_str()).unwrap(),
        IServerMessage::StatePatch { .. }
    ));
    socket
        .send(tokio_tungstenite::tungstenite::Message::Binary(
            vec![0_u8].into(),
        ))
        .await
        .unwrap();
    let closed = timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let tokio_tungstenite::tungstenite::Message::Close(Some(frame)) = closed else {
        panic!("binary command did not close the connection");
    };
    assert_eq!(
        frame.code,
        tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Unsupported
    );
    let disconnected = timeout(Duration::from_secs(5), runtime.recv_connection_event())
        .await
        .unwrap()
        .unwrap();
    let WfeConnectionEvent::Disconnected(disconnected) = disconnected else {
        panic!("second lifecycle event was not disconnected");
    };
    assert_eq!(
        disconnected.peer_ip,
        IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
    );
    assert_eq!(disconnected.connection_ordinal, 1);
    assert_eq!(disconnected.active_clients, 0);
    assert_eq!(
        disconnected.category,
        WfeDisconnectCategory::ProtocolViolation
    );
    assert!(disconnected.uptime > Duration::ZERO);

    drop(socket);

    drop(client);
    drop(runtime);
    let mut status = server.status_receiver();
    server.shutdown().await.unwrap();
    status.changed().await.unwrap();
    assert_eq!(*status.borrow(), WfeServerStatus::Stopped { error: None });
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn tokenless_session_route_is_strict_repeatable_and_upgrades_wss() {
    use futures_util::StreamExt as _;
    use rustls::pki_types::CertificateDer;
    use tokio_tungstenite::Connector;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;

    let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = reservation.local_addr().unwrap();
    drop(reservation);
    let target = super::super::security::WfeTarget::parse(&format!("https://{address}")).unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let security = super::super::security::prepare_security_with_authentication(
        &target,
        super::super::security::SecurityFileOptions::default(),
        ControllerAuthenticationMode::Disabled,
        Some(&temporary.path().join("wfe-state")),
    )
    .unwrap();
    let certificate_der = security.certificate_chain_der()[0].as_ref().to_vec();
    let app = crate::app::App::new(&crate::config::Config::default());
    let (frontend, mut runtime) = crate::wfe::runtime::WfeRuntime::new(&app, Vec::new()).unwrap();
    let bind_plan = resolve_target(&target).await.unwrap();
    let (server, mut host_info) = start(security, bind_plan, frontend).await.unwrap();
    assert_eq!(host_info.profile(), SecurityProfile::AutomaticGenerated);
    assert_eq!(
        host_info.authentication_mode(),
        ControllerAuthenticationMode::Disabled
    );
    assert!(host_info.take_bootstrap_url().is_none());

    let client = reqwest::Client::builder()
        .no_proxy()
        .tls_danger_accept_invalid_certs(true)
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let base = target.as_str();
    let token_route = client
        .post(format!("{base}/auth"))
        .header(ORIGIN, target.canonical_origin())
        .json(&serde_json::json!({ "token": "not-used" }))
        .send()
        .await
        .unwrap();
    assert_eq!(token_route.status(), StatusCode::UNAUTHORIZED);
    assert!(token_route.headers().get(SET_COOKIE).is_none());

    for rejected in ["", "null", "[]", r#"{"unexpected":true}"#] {
        let response = client
            .post(format!("{base}/auth/session"))
            .header(ORIGIN, target.canonical_origin())
            .header(CONTENT_TYPE, "application/json")
            .body(rejected)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{rejected:?}");
        assert!(response.headers().get(SET_COOKIE).is_none());
    }

    let authenticated = client
        .post(format!("{base}/auth/session"))
        .header(ORIGIN, target.canonical_origin())
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(authenticated.status(), StatusCode::NO_CONTENT);
    let websocket_protocol = authenticated
        .headers()
        .get(WFE_WEBSOCKET_PROTOCOL_HEADER.clone())
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let set_cookie = authenticated
        .headers()
        .get(SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let cookie_pair = set_cookie.split(';').next().unwrap().to_string();

    let authenticated_again = client
        .post(format!("{base}/auth/session"))
        .header(ORIGIN, target.canonical_origin())
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(authenticated_again.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        authenticated_again
            .headers()
            .get(WFE_WEBSOCKET_PROTOCOL_HEADER.clone())
            .unwrap(),
        websocket_protocol.as_str()
    );

    let mut roots = rustls::RootCertStore::empty();
    roots.add(CertificateDer::from(certificate_der)).unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let client_tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = Connector::Rustls(Arc::new(client_tls));
    let mut request = format!("wss://{address}/ws").into_client_request().unwrap();
    request.headers_mut().insert(
        ORIGIN,
        HeaderValue::from_str(&target.canonical_origin()).unwrap(),
    );
    request
        .headers_mut()
        .insert(COOKIE, HeaderValue::from_str(&cookie_pair).unwrap());
    request.headers_mut().insert(
        axum::http::header::SEC_WEBSOCKET_PROTOCOL,
        HeaderValue::from_str(&websocket_protocol).unwrap(),
    );
    let (mut socket, _) =
        tokio_tungstenite::connect_async_tls_with_config(request, None, false, Some(connector))
            .await
            .unwrap();
    for expected in ["hello", "state_snapshot"] {
        let message = timeout(Duration::from_secs(5), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let tokio_tungstenite::tungstenite::Message::Text(message) = message else {
            panic!("{expected} was not text");
        };
        let parsed = serde_json::from_str::<IServerMessage>(message.as_str()).unwrap();
        assert_eq!(
            match parsed {
                IServerMessage::Hello { .. } => "hello",
                IServerMessage::StateSnapshot { .. } => "state_snapshot",
                _ => "other",
            },
            expected
        );
    }
    let connected = timeout(Duration::from_secs(3), runtime.recv_connection_event())
        .await
        .unwrap()
        .unwrap();
    let WfeConnectionEvent::Connected(connected) = connected else {
        panic!("first tokenless lifecycle event was not connected");
    };
    assert_eq!(
        connected.authentication_mode,
        ControllerAuthenticationMode::Disabled
    );
    assert_eq!(connected.peer_ip, IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
    assert_eq!(connected.active_clients, 1);
    assert!(connected.round_trip_time.is_none());

    let ping = timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(
        ping,
        tokio_tungstenite::tungstenite::Message::Ping(_)
    ));
    socket.close(None).await.unwrap();
    let disconnected = timeout(Duration::from_secs(5), runtime.recv_connection_event())
        .await
        .unwrap()
        .unwrap();
    let WfeConnectionEvent::Disconnected(disconnected) = disconnected else {
        panic!("second tokenless lifecycle event was not disconnected");
    };
    assert_eq!(
        disconnected.connection_ordinal,
        connected.connection_ordinal
    );
    assert_eq!(disconnected.active_clients, 0);
    assert_eq!(disconnected.category, WfeDisconnectCategory::PeerClosed);

    drop(socket);
    drop(client);
    drop(runtime);
    server.shutdown().await.unwrap();
}
