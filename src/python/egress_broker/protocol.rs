use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const BROKER_PROTOCOL_VERSION: u32 = 1;
pub const BROKER_PROTOCOL_ABI: &str = "lethetic-egress-v1";

pub(super) const MAX_FRAME_BYTES: usize = 72 * 1024;
pub(super) const MAX_HTTP_HEAD_BYTES: usize = 32 * 1024;
pub(super) const MAX_RUNTIME_ID_BYTES: usize = 128;
pub(super) const CAPABILITY_BYTES: usize = 32;
pub(super) const PROTOCOL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrokerRequestKind {
    Probe,
    Proxy,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProxyKind {
    Http,
    Connect,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BrokerRequest {
    pub version: u32,
    pub runtime_id: String,
    pub capability: String,
    pub kind: BrokerRequestKind,
    pub proxy_head_base64: Option<String>,
    pub buffered_after_head: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BrokerResponse {
    pub version: u32,
    pub allowed: bool,
    pub code: String,
    pub proxy_kind: Option<ProxyKind>,
}

pub(super) fn validate_runtime_id(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > MAX_RUNTIME_ID_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("invalid runtime ID".to_string());
    }
    Ok(())
}

pub(super) fn validate_capability(value: &str) -> Result<(), String> {
    if value.len() != CAPABILITY_BYTES * 2
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err("broker capability must be 32 lowercase hexadecimal bytes".to_string());
    }
    Ok(())
}

pub(super) fn capability_matches(candidate: &str, expected: &str) -> bool {
    if candidate.len() != expected.len() || validate_capability(candidate).is_err() {
        return false;
    }
    candidate
        .bytes()
        .zip(expected.bytes())
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

pub(super) async fn read_frame<R, T>(reader: &mut R) -> Result<T, String>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let length = reader
        .read_u32()
        .await
        .map_err(|error| format!("could not read broker frame length: {error}"))?
        as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err("broker frame length is invalid".to_string());
    }
    let mut payload = vec![0_u8; length];
    reader
        .read_exact(&mut payload)
        .await
        .map_err(|error| format!("could not read broker frame: {error}"))?;
    serde_json::from_slice(&payload).map_err(|error| format!("invalid broker frame JSON: {error}"))
}

pub(super) async fn write_frame<W, T>(writer: &mut W, value: &T) -> Result<(), String>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let payload = serde_json::to_vec(value)
        .map_err(|error| format!("could not encode broker frame: {error}"))?;
    if payload.is_empty() || payload.len() > MAX_FRAME_BYTES {
        return Err("encoded broker frame length is invalid".to_string());
    }
    writer
        .write_u32(payload.len() as u32)
        .await
        .map_err(|error| format!("could not write broker frame length: {error}"))?;
    writer
        .write_all(&payload)
        .await
        .map_err(|error| format!("could not write broker frame: {error}"))
}
