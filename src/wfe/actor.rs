use super::contracts::{ICommandRequest, ICommandResponse};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use tokio::sync::{mpsc, oneshot};

pub const WFE_COMMAND_MAILBOX_CAPACITY: usize = 64;
pub const WFE_REQUEST_REPLAY_CAPACITY: usize = 512;
pub const WFE_REQUEST_REPLAY_BYTE_BUDGET: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConnectionId(String);

impl ConnectionId {
    pub fn new(value: String) -> Result<Self, String> {
        if value.is_empty() || value.len() > 64 {
            return Err("connection ID must contain between 1 and 64 bytes".to_string());
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(
                "connection ID may contain only ASCII letters, digits, '-' and '_'".to_string(),
            );
        }
        Ok(Self(value))
    }

    pub fn random() -> Self {
        Self(format!("client-{}", uuid::Uuid::new_v4().simple()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub struct WfeCommandEnvelope {
    pub connection_id: ConnectionId,
    pub request: ICommandRequest,
    pub response: oneshot::Sender<ICommandResponse>,
}

#[derive(Clone)]
pub struct WfeActorHandle {
    sender: mpsc::Sender<WfeCommandEnvelope>,
}

pub struct WfeActorReceiver {
    receiver: mpsc::Receiver<WfeCommandEnvelope>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitError {
    Full,
    Closed,
}

pub fn command_channel() -> (WfeActorHandle, WfeActorReceiver) {
    let (sender, receiver) = mpsc::channel(WFE_COMMAND_MAILBOX_CAPACITY);
    (WfeActorHandle { sender }, WfeActorReceiver { receiver })
}

impl WfeActorHandle {
    pub fn try_submit(
        &self,
        connection_id: ConnectionId,
        request: ICommandRequest,
    ) -> Result<oneshot::Receiver<ICommandResponse>, SubmitError> {
        let (response, receiver) = oneshot::channel();
        self.sender
            .try_send(WfeCommandEnvelope {
                connection_id,
                request,
                response,
            })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => SubmitError::Full,
                mpsc::error::TrySendError::Closed(_) => SubmitError::Closed,
            })?;
        Ok(receiver)
    }

    pub fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }
}

impl WfeActorReceiver {
    pub async fn recv(&mut self) -> Option<WfeCommandEnvelope> {
        self.receiver.recv().await
    }

    pub fn close(&mut self) {
        self.receiver.close();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestFingerprint([u8; 32]);

impl RequestFingerprint {
    pub fn for_request(request: &ICommandRequest) -> Result<Self, String> {
        let encoded = serde_json::to_vec(request)
            .map_err(|error| format!("could not fingerprint WFE request: {error}"))?;
        Ok(Self(Sha256::digest(encoded).into()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayLookup {
    Miss(RequestFingerprint),
    Replay(ICommandResponse),
    ContentSessionMismatch,
    Conflict,
}

#[derive(Debug, Clone)]
struct ReplayEntry {
    fingerprint: RequestFingerprint,
    response: ICommandResponse,
    content_session_id: Option<String>,
    encoded_bytes: usize,
}

#[derive(Debug)]
pub struct RequestReplayCache {
    capacity: usize,
    byte_budget: usize,
    encoded_bytes: usize,
    entries: HashMap<String, ReplayEntry>,
    order: VecDeque<String>,
}

impl RequestReplayCache {
    pub fn new(capacity: usize) -> Result<Self, String> {
        Self::with_limits(capacity, WFE_REQUEST_REPLAY_BYTE_BUDGET)
    }

    pub fn with_limits(capacity: usize, byte_budget: usize) -> Result<Self, String> {
        if capacity == 0 {
            return Err("request replay cache capacity must be greater than zero".to_string());
        }
        if byte_budget == 0 {
            return Err("request replay cache byte budget must be greater than zero".to_string());
        }
        Ok(Self {
            capacity,
            byte_budget,
            encoded_bytes: 0,
            entries: HashMap::new(),
            order: VecDeque::new(),
        })
    }

    pub fn lookup(
        &mut self,
        request: &ICommandRequest,
        active_session_id: &str,
    ) -> Result<ReplayLookup, String> {
        let fingerprint = RequestFingerprint::for_request(request)?;
        let Some(entry) = self.entries.get(&request.id) else {
            return Ok(ReplayLookup::Miss(fingerprint));
        };
        if entry.fingerprint != fingerprint {
            return Ok(ReplayLookup::Conflict);
        }
        if entry
            .content_session_id
            .as_deref()
            .is_some_and(|session_id| session_id != active_session_id)
        {
            return Ok(ReplayLookup::ContentSessionMismatch);
        }
        let response = entry.response.clone();
        self.touch(&request.id);
        Ok(ReplayLookup::Replay(response))
    }

    pub fn insert(
        &mut self,
        request_id: String,
        fingerprint: RequestFingerprint,
        response: ICommandResponse,
    ) -> Result<(), String> {
        if request_id != response.id {
            return Err("request replay response ID does not match its request ID".to_string());
        }
        response.validate()?;
        if let Some(existing) = self.entries.get(&request_id) {
            if existing.fingerprint != fingerprint || existing.response != response {
                return Err("request ID was reused with different replay data".to_string());
            }
            self.touch(&request_id);
            return Ok(());
        }

        let encoded_bytes = serde_json::to_vec(&response)
            .map_err(|error| format!("could not size WFE replay response: {error}"))?
            .len();
        if encoded_bytes > self.byte_budget {
            return Err("request replay response exceeds the aggregate byte budget".to_string());
        }
        while self.entries.len() >= self.capacity
            || self.encoded_bytes.saturating_add(encoded_bytes) > self.byte_budget
        {
            self.evict_oldest()?;
        }
        let content_session_id = response_content_session_id(&response).map(str::to_string);
        self.encoded_bytes = self.encoded_bytes.saturating_add(encoded_bytes);
        self.order.push_back(request_id.clone());
        self.entries.insert(
            request_id,
            ReplayEntry {
                fingerprint,
                response,
                content_session_id,
                encoded_bytes,
            },
        );
        Ok(())
    }

    fn evict_oldest(&mut self) -> Result<(), String> {
        let Some(evicted) = self.order.pop_front() else {
            return Err("request replay cache order is inconsistent".to_string());
        };
        let entry = self
            .entries
            .remove(&evicted)
            .ok_or_else(|| "request replay cache entries are inconsistent".to_string())?;
        self.encoded_bytes = self.encoded_bytes.saturating_sub(entry.encoded_bytes);
        Ok(())
    }

    fn touch(&mut self, request_id: &str) {
        if let Some(index) = self.order.iter().position(|id| id == request_id) {
            self.order.remove(index);
        }
        self.order.push_back(request_id.to_string());
    }
}

fn response_content_session_id(response: &ICommandResponse) -> Option<&str> {
    let super::contracts::CommandResult::Ok { outcome, .. } = &response.result else {
        return None;
    };
    match outcome {
        super::contracts::CommandOutcome::HistoryEntrySelected { session_id, .. } => {
            Some(session_id)
        }
        _ => None,
    }
}

impl Default for RequestReplayCache {
    fn default() -> Self {
        Self::new(WFE_REQUEST_REPLAY_CAPACITY).expect("the fixed WFE replay capacity is nonzero")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::CommandId;
    use crate::wfe::contracts::{CommandOutcome, CommandResult, WebCommand};

    fn request(id: &str, command_id: CommandId) -> ICommandRequest {
        ICommandRequest {
            id: id.to_string(),
            expected_revision: 0,
            command: WebCommand::InvokeCommand { command_id },
        }
    }

    fn response(id: &str) -> ICommandResponse {
        ICommandResponse {
            id: id.to_string(),
            result: CommandResult::Ok {
                revision: 0,
                outcome: CommandOutcome::Applied,
            },
        }
    }

    fn history_response(id: &str, session_id: &str, content: &str) -> ICommandResponse {
        ICommandResponse {
            id: id.to_string(),
            result: CommandResult::Ok {
                revision: 0,
                outcome: CommandOutcome::HistoryEntrySelected {
                    session_id: session_id.to_string(),
                    entry_id: "history-1".to_string(),
                    editor_content: content.to_string(),
                },
            },
        }
    }

    #[tokio::test]
    async fn bounded_mailbox_preserves_connection_and_request() {
        let (handle, mut receiver) = command_channel();
        let connection = ConnectionId::new("client-1".to_string()).unwrap();
        let response_receiver = handle
            .try_submit(connection.clone(), request("request-1", CommandId::Hotkeys))
            .unwrap();
        let envelope = receiver.recv().await.unwrap();
        assert_eq!(envelope.connection_id, connection);
        assert_eq!(envelope.request.id, "request-1");
        envelope.response.send(response("request-1")).unwrap();
        assert_eq!(response_receiver.await.unwrap().id, "request-1");
    }

    #[test]
    fn replay_cache_replays_exact_requests_and_rejects_conflicts() {
        let mut cache = RequestReplayCache::new(2).unwrap();
        let first_request = request("request-1", CommandId::Hotkeys);
        let ReplayLookup::Miss(fingerprint) = cache.lookup(&first_request, "session-a").unwrap()
        else {
            panic!("first request should miss");
        };
        cache
            .insert(first_request.id.clone(), fingerprint, response("request-1"))
            .unwrap();
        assert!(matches!(
            cache.lookup(&first_request, "session-a").unwrap(),
            ReplayLookup::Replay(_)
        ));
        assert_eq!(
            cache
                .lookup(&request("request-1", CommandId::Themes), "session-a")
                .unwrap(),
            ReplayLookup::Conflict
        );

        let second = request("request-2", CommandId::Hotkeys);
        let ReplayLookup::Miss(second_fingerprint) = cache.lookup(&second, "session-a").unwrap()
        else {
            panic!("second request should miss");
        };
        cache
            .insert(second.id.clone(), second_fingerprint, response("request-2"))
            .unwrap();
        let third = request("request-3", CommandId::Hotkeys);
        let ReplayLookup::Miss(third_fingerprint) = cache.lookup(&third, "session-a").unwrap()
        else {
            panic!("third request should miss");
        };
        cache
            .insert(third.id.clone(), third_fingerprint, response("request-3"))
            .unwrap();
        assert!(matches!(
            cache.lookup(&first_request, "session-a").unwrap(),
            ReplayLookup::Miss(_)
        ));
    }

    #[test]
    fn content_replay_is_scoped_without_invalidating_mutation_replay() {
        const FIRST_SESSION: &str = "11111111-2222-4333-8444-555555555555";
        const SECOND_SESSION: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
        let mut cache = RequestReplayCache::new(4).unwrap();
        let content_request = request("history-request", CommandId::Hotkeys);
        let ReplayLookup::Miss(content_fingerprint) =
            cache.lookup(&content_request, FIRST_SESSION).unwrap()
        else {
            panic!("content request should miss");
        };
        cache
            .insert(
                content_request.id.clone(),
                content_fingerprint,
                history_response("history-request", FIRST_SESSION, "lossless original"),
            )
            .unwrap();
        assert!(matches!(
            cache.lookup(&content_request, SECOND_SESSION).unwrap(),
            ReplayLookup::ContentSessionMismatch
        ));
        assert!(matches!(
            cache.lookup(&content_request, FIRST_SESSION).unwrap(),
            ReplayLookup::Replay(_)
        ));

        let mutation_request = request("mutation-request", CommandId::Themes);
        let ReplayLookup::Miss(mutation_fingerprint) =
            cache.lookup(&mutation_request, FIRST_SESSION).unwrap()
        else {
            panic!("mutation request should miss");
        };
        cache
            .insert(
                mutation_request.id.clone(),
                mutation_fingerprint,
                response("mutation-request"),
            )
            .unwrap();
        assert!(matches!(
            cache.lookup(&mutation_request, SECOND_SESSION).unwrap(),
            ReplayLookup::Replay(_)
        ));
    }

    #[test]
    fn aggregate_encoded_byte_budget_evicts_oldest_replays() {
        let sample_bytes = serde_json::to_vec(&response("first")).unwrap().len();
        let mut cache = RequestReplayCache::with_limits(8, sample_bytes * 2 + 8).unwrap();
        for id in ["first", "second", "third"] {
            let command = request(id, CommandId::Hotkeys);
            let ReplayLookup::Miss(fingerprint) = cache.lookup(&command, "session").unwrap() else {
                panic!("new replay request should miss");
            };
            cache
                .insert(id.to_string(), fingerprint, response(id))
                .unwrap();
        }
        assert!(cache.encoded_bytes <= cache.byte_budget);
        assert!(matches!(
            cache
                .lookup(&request("first", CommandId::Hotkeys), "session")
                .unwrap(),
            ReplayLookup::Miss(_)
        ));
        assert!(matches!(
            cache
                .lookup(&request("third", CommandId::Hotkeys), "session")
                .unwrap(),
            ReplayLookup::Replay(_)
        ));
    }
}
