use base64::{Engine, engine::general_purpose::STANDARD};
use lethetic_network::speech::{SpeechEvent, SpeechRequest, SpeechStreams};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::oneshot,
};

fn request(url: String) -> SpeechRequest {
    SpeechRequest {
        url,
        headers: BTreeMap::from([
            ("content-type".into(), "application/json".into()),
            ("authorization".into(), "Bearer voice-key".into()),
        ]),
        body: "{\"input\":\"hello\"}".into(),
    }
}
#[tokio::test]
async fn http_stream_starts_before_eof_without_cors_and_cancel_closes_socket() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1/audio/speech", listener.local_addr().unwrap());
    let (closed, closure) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut input = Vec::new();
        while !input.ends_with(b"{\"input\":\"hello\"}") {
            let mut buffer = [0; 1024];
            let n = socket.read(&mut buffer).await.unwrap();
            assert_ne!(n, 0);
            input.extend_from_slice(&buffer[..n]);
        }
        let headers = String::from_utf8(input).unwrap();
        assert!(headers.starts_with("POST /v1/audio/speech"));
        assert!(headers.contains("authorization: Bearer voice-key"));
        assert!(!headers.to_lowercase().contains("cookie:"));
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: audio/pcm\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n\0\0\xFF\x7F\r\n").await.unwrap();
        let mut byte = [0];
        let ended = tokio::time::timeout(Duration::from_secs(3), socket.read(&mut byte))
            .await
            .unwrap();
        assert!(matches!(ended, Ok(0) | Err(_)));
        let _ = closed.send(());
    });
    let streams = Arc::new(SpeechStreams::default());
    streams.start("request".into(), request(url)).unwrap();
    assert!(matches!(
        streams.next("request").await.unwrap(),
        SpeechEvent::Head { status: 200, .. }
    ));
    match streams.next("request").await.unwrap() {
        SpeechEvent::Chunk { data } => assert_eq!(STANDARD.decode(data).unwrap(), [0, 0, 255, 127]),
        _ => panic!("Expected a chunk before the server closes"),
    }
    let reading = {
        let streams = streams.clone();
        tokio::spawn(async move { streams.next("request").await })
    };
    tokio::task::yield_now().await;
    streams.cancel(Some("request")).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(3), reading)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    closure.await.unwrap();
    server.await.unwrap();
}
#[tokio::test]
async fn redirects_are_not_followed_and_credentials_are_not_forwarded() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/speech", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut input = Vec::new();
        while !input.ends_with(b"{\"input\":\"hello\"}") {
            let mut bytes = [0; 4096];
            let count = socket.read(&mut bytes).await.unwrap();
            assert_ne!(count, 0);
            input.extend_from_slice(&bytes[..count]);
        }
        socket.write_all(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/should-not-be-contacted\r\nContent-Length: 0\r\n\r\n").await.unwrap();
    });
    let streams = SpeechStreams::default();
    streams.start("redirect".into(), request(url)).unwrap();
    assert!(matches!(
        streams.next("redirect").await.unwrap(),
        SpeechEvent::Head { status: 302, .. }
    ));
    assert!(matches!(
        streams.next("redirect").await.unwrap(),
        SpeechEvent::Done
    ));
    streams.cancel(None).unwrap();
    server.await.unwrap();
}
#[tokio::test]
async fn only_bounded_speech_posts_and_safe_headers_are_accepted() {
    let streams = SpeechStreams::default();
    for url in [
        "file:///etc/passwd",
        "ftp://example.com",
        "http://user:secret@example.com/speech",
        "http://example.com/#secret",
    ] {
        assert!(streams.start("bad".into(), request(url.into())).is_err());
    }
    let mut invalid = request("http://127.0.0.1:1".into());
    invalid.headers.insert("Cookie".into(), "secret".into());
    assert!(streams.start("bad".into(), invalid).is_err());
    let mut large = request("http://127.0.0.1:1".into());
    large.body = "a".repeat(128 * 1024 + 1);
    assert!(streams.start("bad".into(), large).is_err());
    assert!(streams.next("missing").await.is_err());
    streams.cancel(None).unwrap();
}
