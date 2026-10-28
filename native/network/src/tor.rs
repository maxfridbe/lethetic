//! HTTP/1.1 over Arti streams. Destination names are passed to Tor, never resolved locally.
use super::{Failure, MAX_BYTES, Reply, upstream};
use arti_client::{BootstrapBehavior, TorClient};
use base64::{Engine, engine::general_purpose::STANDARD};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper_util::rt::TokioIo;
use std::{collections::BTreeMap, path::Path, sync::Arc};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_rustls::{TlsConnector, rustls};
use tokio_util::compat::FuturesAsyncReadCompatExt;
use tor_rtcompat::PreferredRuntime;

pub(crate) struct TorHttp {
    client: Arc<TorClient<PreferredRuntime>>,
    tls: Arc<rustls::ClientConfig>,
}
impl TorHttp {
    pub(crate) async fn new(directory: &Path) -> Result<Self, String> {
        let config = arti_client::config::TorClientConfigBuilder::from_directories(
            directory.join("state"),
            directory.join("cache"),
        )
        .build()
        .map_err(|e| e.to_string())?;
        let client = TorClient::builder()
            .config(config)
            .bootstrap_behavior(BootstrapBehavior::Manual)
            .create_unbootstrapped_async()
            .await
            .map_err(|e| format!("Tor initialization failed: {}", error_chain(&e)))?;
        let roots =
            rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
        Ok(Self {
            client,
            tls: Arc::new(tls),
        })
    }
    pub(crate) async fn bootstrap(&self) -> Result<(), Failure> {
        self.client.bootstrap().await.map_err(|error| {
            upstream(format!(
                "Tor bootstrap failed (no direct fallback): {}",
                error_chain(&error)
            ))
        })
    }
    pub(crate) async fn send(&self, request: reqwest::Request) -> Result<Reply, Failure> {
        let host = tor_host(request.url()).map_err(super::bad)?;
        let port = request
            .url()
            .port_or_known_default()
            .ok_or_else(|| super::bad("Missing port"))?;
        self.bootstrap().await?;
        let stream = self
            .client
            .connect((host.as_str(), port))
            .await
            .map_err(|e| {
                upstream(format!(
                    "Tor connection failed (no direct fallback): {}",
                    error_chain(&e)
                ))
            })?
            .compat();
        if request.url().scheme() == "https" {
            let name = rustls::pki_types::ServerName::try_from(host).map_err(super::bad)?;
            let tls = TlsConnector::from(self.tls.clone())
                .connect(name, stream)
                .await
                .map_err(|e| upstream(format!("Tor TLS handshake failed: {e}")))?;
            exchange(tls, request).await
        } else {
            exchange(stream, request).await
        }
    }
}

/// IP literals and local names are deliberately rejected before any Tor bootstrap.
/// Arti applies its own exit/address policy as well. No local DNS lookup occurs.
fn tor_host(url: &reqwest::Url) -> Result<String, &'static str> {
    let host = url
        .host_str()
        .ok_or("Missing destination hostname")?
        .trim_end_matches('.');
    if host.is_empty()
        || host.contains(':')
        || host.parse::<std::net::IpAddr>().is_ok()
        || !host.contains('.')
        || host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
    {
        return Err(
            "Tor-only access requires a public hostname or v3 .onion address; local and IP-literal destinations are blocked",
        );
    }
    Ok(host.to_owned())
}
struct ConnectionTask(tokio::task::JoinHandle<()>);
impl Drop for ConnectionTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn exchange<S>(stream: S, request: reqwest::Request) -> Result<Reply, Failure>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(upstream)?;
    let _connection = ConnectionTask(tokio::spawn(async move {
        let _ = connection.await;
    }));
    let url = request.url();
    let host = url
        .host_str()
        .ok_or_else(|| super::bad("Missing destination hostname"))?;
    let authority = match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    };
    let path = match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_owned(),
    };
    let body = request
        .body()
        .and_then(reqwest::Body::as_bytes)
        .unwrap_or_default();
    let mut outgoing = hyper::Request::builder()
        .method(request.method())
        .uri(path)
        .body(Full::new(Bytes::copy_from_slice(body)))
        .map_err(super::bad)?;
    *outgoing.headers_mut() = request.headers().clone();
    outgoing
        .headers_mut()
        .insert("host", authority.parse().map_err(super::bad)?);
    outgoing.headers_mut().insert(
        "connection",
        hyper::header::HeaderValue::from_static("close"),
    );
    let response = sender.send_request(outgoing).await.map_err(upstream)?;
    let status = response.status().as_u16();
    if response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .is_some_and(|n| n > MAX_BYTES as u64)
    {
        return Err(upstream("Response exceeds 8 MiB"));
    }
    let mut headers = BTreeMap::new();
    for (key, value) in response.headers() {
        if matches!(
            key.as_str(),
            "set-cookie" | "connection" | "transfer-encoding"
        ) {
            continue;
        }
        if let Ok(value) = value.to_str() {
            headers.insert(key.to_string(), value.to_owned());
        }
    }
    let mut body = response.into_body();
    let mut bytes = Vec::new();
    while let Some(frame) = body.frame().await {
        if let Ok(data) = frame.map_err(upstream)?.into_data() {
            if data.len() > MAX_BYTES - bytes.len() {
                return Err(upstream("Response exceeds 8 MiB"));
            }
            bytes.extend_from_slice(&data);
        }
    }
    Ok(Reply {
        status,
        headers,
        body_base64: STANDARD.encode(bytes),
    })
}

fn error_chain(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn tor_names_never_need_local_dns_and_reject_local_targets() {
        for value in [
            "http://127.0.0.1",
            "http://[::1]",
            "http://192.168.1.1",
            "http://localhost",
            "http://printer.local",
            "http://private.internal",
            "http://localhost.",
            "http://2130706433",
        ] {
            assert!(
                tor_host(&reqwest::Url::parse(value).unwrap()).is_err(),
                "{value}"
            );
        }
        assert_eq!(
            tor_host(&reqwest::Url::parse("https://example.org/path").unwrap()).unwrap(),
            "example.org"
        );
    }

    #[tokio::test]
    async fn http_over_an_existing_stream_preserves_method_body_and_does_not_follow_redirects() {
        let (client, mut server) = tokio::io::duplex(4096);
        let peer = tokio::spawn(async move {
            let mut bytes = vec![];
            loop {
                bytes.push(server.read_u8().await.unwrap());
                if bytes.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            let text = String::from_utf8(bytes).unwrap();
            assert!(text.starts_with("POST /api?q=1 HTTP/1.1\r\n"));
            assert!(text.contains("host: example.org\r\n"));
            assert!(text.contains("authorization: Bearer test\r\n"));
            let mut body = [0; 2];
            server.read_exact(&mut body).await.unwrap();
            assert_eq!(body, [0, 255]);
            server.write_all(b"HTTP/1.1 302 Found\r\nLocation: https://elsewhere.test/\r\nSet-Cookie: private=1\r\nContent-Length: 2\r\n\r\n\x00\xff").await.unwrap();
        });
        let mut request = reqwest::Request::new(
            reqwest::Method::POST,
            "https://example.org/api?q=1".parse().unwrap(),
        );
        request
            .headers_mut()
            .insert("authorization", "Bearer test".parse().unwrap());
        *request.body_mut() = Some(reqwest::Body::from(vec![0, 255]));
        let reply = exchange(client, request).await.unwrap();
        assert_eq!(reply.status, 302);
        assert_eq!(reply.headers["location"], "https://elsewhere.test/");
        assert!(!reply.headers.contains_key("set-cookie"));
        assert_eq!(STANDARD.decode(reply.body_base64).unwrap(), [0, 255]);
        peer.await.unwrap();
    }

    #[tokio::test]
    async fn oversized_tor_response_is_rejected_before_collection() {
        let (client, mut server) = tokio::io::duplex(4096);
        let peer = tokio::spawn(async move {
            let mut bytes = vec![];
            while !bytes.ends_with(b"\r\n\r\n") {
                bytes.push(server.read_u8().await.unwrap());
            }
            server
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 999999999\r\n\r\n")
                .await
                .unwrap();
        });
        let request = reqwest::Request::new(
            reqwest::Method::GET,
            "https://example.org/".parse().unwrap(),
        );
        assert!(
            exchange(client, request)
                .await
                .err()
                .unwrap()
                .1
                .contains("8 MiB")
        );
        peer.await.unwrap();
    }
}
