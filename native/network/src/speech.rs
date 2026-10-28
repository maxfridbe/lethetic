//! Bounded, cancellable speech POST streams for the trusted native UI.
use base64::{Engine, engine::general_purpose::STANDARD};
use reqwest::{
    Client, Url,
    header::{HeaderMap, HeaderName, HeaderValue},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{Mutex as AsyncMutex, mpsc},
    task::JoinHandle,
};

const IDLE: Duration = Duration::from_secs(60);
const CHUNK: usize = 64 * 1024;
#[derive(Deserialize)]
pub struct SpeechRequest {
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub body: String,
}
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SpeechEvent {
    Head {
        status: u16,
        headers: BTreeMap<String, String>,
    },
    Chunk {
        data: String,
    },
    Done,
    Error {
        message: String,
    },
}
struct Session {
    receiver: AsyncMutex<mpsc::Receiver<SpeechEvent>>,
    task: JoinHandle<()>,
}
impl Drop for Session {
    fn drop(&mut self) {
        self.task.abort();
    }
}
#[derive(Default)]
pub struct SpeechStreams {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
}
impl SpeechStreams {
    pub fn start(&self, id: String, request: SpeechRequest) -> Result<(), String> {
        if id.is_empty() || id.len() > 64 {
            return Err("Invalid speech request ID".into());
        }
        let (url, headers) = validate(&request)?;
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(IDLE)
            .read_timeout(IDLE)
            .build()
            .map_err(|_| "Could not initialize native speech HTTP client")?;
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| "Speech state unavailable")?;
        if sessions.contains_key(&id) || sessions.len() >= 4 {
            return Err("Too many active speech requests or duplicate ID".into());
        }
        let (sender, receiver) = mpsc::channel(2);
        let task = tokio::spawn(async move {
            if let Err(message) = stream(client, url, headers, request.body, &sender).await {
                let _ =
                    tokio::time::timeout(IDLE, sender.send(SpeechEvent::Error { message })).await;
            }
        });
        sessions.insert(
            id,
            Arc::new(Session {
                receiver: AsyncMutex::new(receiver),
                task,
            }),
        );
        Ok(())
    }
    pub async fn next(&self, id: &str) -> Result<SpeechEvent, String> {
        let session = self
            .sessions
            .lock()
            .map_err(|_| "Speech state unavailable")?
            .get(id)
            .cloned()
            .ok_or("Speech request stopped")?;
        let event = session
            .receiver
            .lock()
            .await
            .recv()
            .await
            .ok_or("Speech stream closed")?;
        Ok(event)
    }
    pub fn cancel(&self, id: Option<&str>) -> Result<(), String> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| "Speech state unavailable")?;
        if let Some(id) = id {
            if let Some(session) = sessions.remove(id) {
                session.task.abort();
            }
        } else {
            for (_, session) in sessions.drain() {
                session.task.abort();
            }
        }
        Ok(())
    }
}
fn validate(request: &SpeechRequest) -> Result<(Url, HeaderMap), String> {
    let url = Url::parse(&request.url).map_err(|_| "Invalid speech URL")?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "Speech requires an HTTP(S) URL without embedded credentials or fragments".into(),
        );
    }
    if request.body.len() > 128 * 1024 || request.url.len() > 4096 || request.headers.len() > 4 {
        return Err("Speech request too large".into());
    }
    let mut headers = HeaderMap::new();
    for (name, value) in &request.headers {
        if !["content-type", "accept", "authorization", "xi-api-key"]
            .contains(&name.to_ascii_lowercase().as_str())
            || value.len() > 8192
        {
            return Err("Unsupported speech request header".into());
        }
        headers.insert(
            HeaderName::from_bytes(name.as_bytes()).map_err(|_| "Invalid speech header")?,
            HeaderValue::from_str(value).map_err(|_| "Invalid speech header")?,
        );
    }
    Ok((url, headers))
}
async fn send(sender: &mpsc::Sender<SpeechEvent>, event: SpeechEvent) -> Result<(), String> {
    tokio::time::timeout(IDLE, sender.send(event))
        .await
        .map_err(|_| "Speech reader stopped consuming audio")?
        .map_err(|_| "Speech reader closed".into())
}
async fn stream(
    client: Client,
    url: Url,
    headers: HeaderMap,
    body: String,
    sender: &mpsc::Sender<SpeechEvent>,
) -> Result<(), String> {
    let mut response = client.post(url).headers(headers).body(body).send().await
        .map_err(|_| "Native speech connection failed or timed out; check hostname, port and server availability")?;
    let status = response.status();
    let mut headers = BTreeMap::new();
    for name in ["content-type", "content-length"] {
        if let Some(value) = response.headers().get(name).and_then(|v| v.to_str().ok()) {
            headers.insert(name.into(), value.into());
        }
    }
    send(
        sender,
        SpeechEvent::Head {
            status: status.as_u16(),
            headers,
        },
    )
    .await?;
    if status.is_success() {
        while let Some(bytes) = response
            .chunk()
            .await
            .map_err(|_| "Native speech stream failed or timed out")?
        {
            for chunk in bytes.chunks(CHUNK) {
                send(
                    sender,
                    SpeechEvent::Chunk {
                        data: STANDARD.encode(chunk),
                    },
                )
                .await?;
            }
        }
    }
    send(sender, SpeechEvent::Done).await
}
