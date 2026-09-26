use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::config::{Config, ConnectionKind};
use crate::context::{ContextManager, ToolCall};
use crate::transport;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GenerateRequest {
    pub model: String,
    pub prompt: String,
    pub raw: bool,
    pub stream: bool,
    pub options: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<String>>,
}

#[derive(Deserialize, Debug)]
pub struct GenerateResponse {
    #[serde(alias = "content", alias = "text")]
    #[serde(default)]
    pub response: String,
    #[serde(alias = "stop")]
    #[serde(default)]
    pub done: bool,
    #[serde(alias = "tokens_evaluated")]
    pub eval_count: Option<u32>,
    pub eval_duration: Option<u64>,
    pub tokens_predicted: Option<u32>,
    pub timings: Option<Timings>,
    pub choices: Option<Vec<Choice>>,
}

#[derive(Deserialize, Debug)]
pub struct Choice {
    pub delta: Option<Delta>,
    pub text: Option<String>,
    pub finish_reason: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct Delta {
    pub content: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct Timings {
    #[serde(alias = "predicted_ms")]
    pub predicted_ms: Option<f64>,
    #[serde(alias = "predicted_per_token_ms")]
    pub predicted_per_token_ms: Option<f64>,
    #[serde(alias = "predicted_per_second")]
    pub predicted_per_second: Option<f64>,
}

/// A model offered by a configured server, as shown in the model switcher.
#[derive(Clone, Debug)]
pub struct ModelChoice {
    pub display: String,
    pub connection_id: String,
    pub kind: ConnectionKind,
    pub url: String,
    pub model_id: String,
    pub available: bool,
}

fn effective_tool_call_id(
    kind: ConnectionKind,
    provider_id: &str,
    arguments: &serde_json::Value,
) -> String {
    if kind.uses_native_tools() {
        return provider_id.to_string();
    }
    arguments["tool_call_id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .unwrap_or(provider_id)
        .to_string()
}

#[derive(Clone, Debug)]
pub enum StreamEvent {
    Chunk(String),
    PreparingToolCall(String),
    ToolCalls {
        calls: Vec<ToolCall>,
        provider_content: Option<Vec<serde_json::Value>>,
    },
    ToolResult {
        id: Option<String>,
        func_name: String,
        result: String,
        cwd: String,
        is_error: bool,
        provenance: crate::tools::ToolOutputProvenance,
    },
    ToolProgress(String),
    TodoUpdated(crate::todo_store::TodoSnapshot),
    LoadProgress(f32, String),
    SessionLoaded {
        dir: String,
        state: crate::app::SessionState,
        #[cfg(target_os = "linux")]
        lease: std::sync::Arc<crate::session_store::SessionLease>,
    },
    SessionLoadFailed(String),
    Done {
        request_id: String,
        completion_tokens: Option<u32>,
        prompt_tokens: Option<u32>,
        usage: Option<crate::accounting::Usage>,
        tg_per_s: Option<f64>,
        pp_per_s: Option<f64>,
        stop_reason: Option<String>,
        provider_content: Option<Vec<serde_json::Value>>,
    },
    Error(String),
    DebugLog(String),
    TokenUpdate(u32, f64),
    ModelsReady(Vec<ModelChoice>),
    /// Streamed progress text for the session compaction popup.
    CompactionChunk(String),
    /// Terminal compaction result; the run loop turns a summary into a new session.
    CompactionFinished {
        source_session_id: String,
        result: Result<String, String>,
    },
    PythonCapabilities(Vec<crate::python::backend::BackendCapability>),
    PythonRuntimeNotice(crate::python::PythonRuntimeNotice),
    PythonNotebookNotice(std::path::PathBuf),
    PythonPolicyPrepared {
        snapshot: crate::python_policy::PythonPolicySnapshot,
        effective_snapshot: crate::python_policy::PythonPolicySnapshot,
        effective_source: crate::python_policy::PythonPolicySource,
        persistence: crate::python_setup::PolicyPersistence,
        expected_revision: Option<crate::python_policy::PolicyRevision>,
        validation: Result<(), String>,
    },
    UsageUpdate {
        request_id: String,
        usage: crate::accounting::Usage,
    },
    RequestStarted(crate::accounting::ProviderRequestAccounting),
    RequestFinished(crate::accounting::ProviderRequestAccounting),
    RequestSettlementFailed {
        request_id: String,
        error: String,
        cancellation_requested: bool,
    },
    PersistRequestCheckpoint {
        checkpoint: ProviderRequestCheckpoint,
        acknowledgement: std::sync::mpsc::SyncSender<Result<(), String>>,
    },
    RequestCancelled {
        request_id: String,
    },
    PythonPullFinished(Result<Vec<crate::python::backend::BackendCapability>, String>),
    #[cfg(target_os = "linux")]
    RuntimeMaintenanceFinished(
        Result<crate::python::retained_runtime::RuntimeMaintenanceReport, String>,
    ),
}

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(0);

fn next_request_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let counter = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("req-{:x}-{nanos:x}-{counter:x}", std::process::id())
}

fn request_accounting(
    config: &Config,
    request_id: &str,
    usage: Option<crate::accounting::Usage>,
    completed: bool,
) -> crate::accounting::ProviderRequestAccounting {
    let estimated_cost = usage.and_then(|usage| {
        config
            .pricing
            .as_ref()
            .and_then(|pricing| pricing.estimate(&config.model, &usage).ok().flatten())
    });
    crate::accounting::ProviderRequestAccounting {
        request_id: request_id.to_string(),
        connection_id: config
            .active_connection_id()
            .unwrap_or(&config.server_url)
            .to_string(),
        model: config.model.clone(),
        usage: usage.unwrap_or_default(),
        usage_reported: usage.is_some(),
        estimated_cost,
        completed,
        in_flight: false,
    }
}

fn request_started_accounting(
    config: &Config,
    request_id: &str,
) -> crate::accounting::ProviderRequestAccounting {
    let mut request = request_accounting(config, request_id, None, false);
    request.in_flight = true;
    request
}

pub fn prepare_provider_request(config: &Config) -> crate::accounting::ProviderRequestAccounting {
    request_started_accounting(config, &next_request_id())
}

#[derive(Debug)]
pub struct PreparedLlmRequest {
    config: Config,
    tool_surface: crate::tools::ToolSurface,
    started: crate::accounting::ProviderRequestAccounting,
    transcript_prefix: Vec<crate::context::Message>,
    raw_prompt: String,
    estimated_context_tokens: usize,
    request: transport::PreparedAgentRequest,
}

impl PreparedLlmRequest {
    pub fn accounting_start(&self) -> &crate::accounting::ProviderRequestAccounting {
        &self.started
    }
}

fn prepare_llm_request_with_start(
    config: &Config,
    context_manager: &ContextManager,
    tool_surface: crate::tools::ToolSurface,
    started: crate::accounting::ProviderRequestAccounting,
) -> Result<PreparedLlmRequest, String> {
    if started != request_started_accounting(config, &started.request_id) {
        return Err("prepared provider request does not match the active connection".to_string());
    }
    let transcript_prefix = context_manager.get_messages().to_vec();
    let api_context = context_manager.prepare_api_context();
    if api_context.estimated_tokens() > context_manager.input_token_budget() {
        return Err(
            "Prepared API context exceeds the configured input token budget; reduce the system prompt or context"
                .to_string(),
        );
    }
    let tools = crate::tools::get_api_tools(config, tool_surface);
    let request = transport::prepare_agent_request(
        config,
        api_context.messages(),
        &tools,
        config.request_output_tokens(),
    )?;

    Ok(PreparedLlmRequest {
        config: config.clone(),
        tool_surface,
        started,
        transcript_prefix,
        raw_prompt: context_manager.get_raw_prompt(),
        estimated_context_tokens: api_context.estimated_tokens(),
        request,
    })
}

pub fn prepare_llm_request(
    config: &Config,
    context_manager: &ContextManager,
    tool_surface: crate::tools::ToolSurface,
) -> Result<PreparedLlmRequest, String> {
    prepare_llm_request_with_start(
        config,
        context_manager,
        tool_surface,
        prepare_provider_request(config),
    )
}

pub fn validate_llm_request(
    config: &Config,
    context_manager: &ContextManager,
    tool_surface: crate::tools::ToolSurface,
) -> Result<(), String> {
    prepare_llm_request(config, context_manager, tool_surface).map(|_| ())
}

pub fn trigger_llm_request(
    client: Client,
    config: Config,
    context_manager: &ContextManager,
    tx: mpsc::UnboundedSender<StreamEvent>,
    token: CancellationToken,
    is_debug: bool,
    session_dir: Option<String>,
) -> Result<String, String> {
    trigger_llm_request_with_surface(
        client,
        config,
        context_manager,
        tx,
        token,
        is_debug,
        session_dir,
        crate::tools::ToolSurface::Interactive,
    )
}

#[derive(Clone, Debug)]
pub struct ProviderRequestCheckpoint {
    pub request: crate::accounting::ProviderRequestAccounting,
    pub transcript: Option<Vec<crate::context::Message>>,
}

impl ProviderRequestCheckpoint {
    fn accounting_only(request: crate::accounting::ProviderRequestAccounting) -> Self {
        Self {
            request,
            transcript: None,
        }
    }
}

pub type RequestAccountingHook =
    std::sync::Arc<dyn Fn(&ProviderRequestCheckpoint) -> Result<(), String> + Send + Sync>;
pub type RequestStartedHook = RequestAccountingHook;

fn invoke_request_checkpoint(
    hook: &Option<RequestAccountingHook>,
    request: &crate::accounting::ProviderRequestAccounting,
    transcript: Option<Vec<crate::context::Message>>,
) -> Result<(), String> {
    if let Some(hook) = hook {
        hook(&ProviderRequestCheckpoint {
            request: request.clone(),
            transcript,
        })?;
    }
    Ok(())
}

fn emit_checkpointed_request_finish_with_fallback(
    tx: &mpsc::UnboundedSender<StreamEvent>,
    settlement_fallback: Option<&mpsc::UnboundedSender<StreamEvent>>,
    hook: &Option<RequestAccountingHook>,
    request: crate::accounting::ProviderRequestAccounting,
    transcript: Option<Vec<crate::context::Message>>,
) -> Result<(), String> {
    invoke_request_checkpoint(hook, &request, transcript)?;
    send_with_settlement_fallback(
        tx,
        settlement_fallback,
        StreamEvent::RequestFinished(request),
    );
    Ok(())
}

fn send_with_settlement_fallback(
    tx: &mpsc::UnboundedSender<StreamEvent>,
    settlement_fallback: Option<&mpsc::UnboundedSender<StreamEvent>>,
    event: StreamEvent,
) -> bool {
    if let Some(settlement_fallback) = settlement_fallback {
        let _ = settlement_fallback.send(event.clone());
    }
    tx.send(event).is_ok()
}

fn emit_request_settlement_failure(
    tx: &mpsc::UnboundedSender<StreamEvent>,
    request_id: &str,
    error: String,
    cancellation_requested: bool,
) {
    emit_request_settlement_failure_with_fallback(
        tx,
        None,
        request_id,
        error,
        cancellation_requested,
    );
}

fn emit_request_settlement_failure_with_fallback(
    tx: &mpsc::UnboundedSender<StreamEvent>,
    settlement_fallback: Option<&mpsc::UnboundedSender<StreamEvent>>,
    request_id: &str,
    error: String,
    cancellation_requested: bool,
) {
    send_with_settlement_fallback(
        tx,
        settlement_fallback,
        StreamEvent::RequestSettlementFailed {
            request_id: request_id.to_string(),
            error,
            cancellation_requested,
        },
    );
}

fn emit_checkpointed_nested_finish(
    tx: Option<&mpsc::UnboundedSender<StreamEvent>>,
    hook: &Option<RequestAccountingHook>,
    request: crate::accounting::ProviderRequestAccounting,
    cancellation_requested: bool,
    failure_context: &str,
) -> Result<(), String> {
    if let Err(error) = invoke_request_checkpoint(hook, &request, None) {
        let error = format!("{failure_context}: {error}");
        if let Some(tx) = tx {
            emit_request_settlement_failure(
                tx,
                &request.request_id,
                error.clone(),
                cancellation_requested,
            );
        }
        return Err(error);
    }
    if let Some(tx) = tx {
        let _ = tx.send(StreamEvent::RequestFinished(request));
    }
    Ok(())
}

enum ProviderStreamStart {
    Ready(Result<transport::EventStream, String>),
    Cancelled,
    ReceiverClosed,
}

enum ProviderStreamPoll {
    Event(Option<transport::StreamEvent>),
    Cancelled,
    ReceiverClosed,
}

async fn poll_provider_stream(
    stream: &mut transport::EventStream,
    token: &CancellationToken,
    tx: &mpsc::UnboundedSender<StreamEvent>,
) -> ProviderStreamPoll {
    tokio::select! {
        biased;
        _ = tx.closed() => ProviderStreamPoll::ReceiverClosed,
        event = stream.next() => ProviderStreamPoll::Event(event),
        _ = token.cancelled() => ProviderStreamPoll::Cancelled,
    }
}

fn reasoning_segment_separator(
    previous_segment: &mut Option<usize>,
    segment_index: Option<usize>,
) -> Option<&'static str> {
    let segment_index = segment_index?;
    let needs_separator = previous_segment.is_some_and(|previous| previous != segment_index);
    *previous_segment = Some(segment_index);
    needs_separator.then_some("\n\n")
}

pub fn trigger_llm_request_with_surface(
    client: Client,
    config: Config,
    context_manager: &ContextManager,
    tx: mpsc::UnboundedSender<StreamEvent>,
    token: CancellationToken,
    is_debug: bool,
    session_dir: Option<String>,
    tool_surface: crate::tools::ToolSurface,
) -> Result<String, String> {
    trigger_llm_request_with_surface_and_start_hook(
        client,
        config,
        context_manager,
        tx,
        token,
        is_debug,
        session_dir,
        tool_surface,
        None,
    )
}

pub fn trigger_llm_request_with_surface_and_start_hook(
    client: Client,
    config: Config,
    context_manager: &ContextManager,
    tx: mpsc::UnboundedSender<StreamEvent>,
    token: CancellationToken,
    is_debug: bool,
    session_dir: Option<String>,
    tool_surface: crate::tools::ToolSurface,
    request_started_hook: Option<RequestStartedHook>,
) -> Result<String, String> {
    trigger_llm_request_with_surface_and_prepared_start_impl(
        client,
        config,
        Some(context_manager),
        tx,
        token,
        is_debug,
        session_dir,
        tool_surface,
        request_started_hook,
        None,
        None,
        None,
    )
}

pub(crate) fn trigger_llm_request_with_surface_and_start_hook_and_settlement_fallback(
    client: Client,
    config: Config,
    context_manager: &ContextManager,
    tx: mpsc::UnboundedSender<StreamEvent>,
    settlement_fallback: Option<mpsc::UnboundedSender<StreamEvent>>,
    token: CancellationToken,
    is_debug: bool,
    session_dir: Option<String>,
    tool_surface: crate::tools::ToolSurface,
    request_started_hook: Option<RequestStartedHook>,
) -> Result<String, String> {
    trigger_llm_request_with_surface_and_prepared_start_impl(
        client,
        config,
        Some(context_manager),
        tx,
        token,
        is_debug,
        session_dir,
        tool_surface,
        request_started_hook,
        None,
        settlement_fallback,
        None,
    )
}

pub fn trigger_llm_request_with_surface_and_prepared_start(
    client: Client,
    config: Config,
    context_manager: &ContextManager,
    tx: mpsc::UnboundedSender<StreamEvent>,
    token: CancellationToken,
    is_debug: bool,
    session_dir: Option<String>,
    tool_surface: crate::tools::ToolSurface,
    started: crate::accounting::ProviderRequestAccounting,
) -> Result<String, String> {
    trigger_llm_request_with_surface_and_prepared_start_and_hook(
        client,
        config,
        context_manager,
        tx,
        token,
        is_debug,
        session_dir,
        tool_surface,
        started,
        None,
    )
}

pub fn trigger_llm_request_with_surface_and_prepared_start_and_hook(
    client: Client,
    config: Config,
    context_manager: &ContextManager,
    tx: mpsc::UnboundedSender<StreamEvent>,
    token: CancellationToken,
    is_debug: bool,
    session_dir: Option<String>,
    tool_surface: crate::tools::ToolSurface,
    started: crate::accounting::ProviderRequestAccounting,
    request_hook: Option<RequestAccountingHook>,
) -> Result<String, String> {
    trigger_llm_request_with_surface_and_prepared_start_impl(
        client,
        config,
        Some(context_manager),
        tx,
        token,
        is_debug,
        session_dir,
        tool_surface,
        request_hook,
        Some(started),
        None,
        None,
    )
}

pub fn trigger_prepared_llm_request_with_hook(
    client: Client,
    prepared: PreparedLlmRequest,
    tx: mpsc::UnboundedSender<StreamEvent>,
    token: CancellationToken,
    is_debug: bool,
    session_dir: Option<String>,
    request_hook: Option<RequestAccountingHook>,
) -> Result<String, String> {
    let config = prepared.config.clone();
    let tool_surface = prepared.tool_surface;
    trigger_llm_request_with_surface_and_prepared_start_impl(
        client,
        config,
        None,
        tx,
        token,
        is_debug,
        session_dir,
        tool_surface,
        request_hook,
        None,
        None,
        Some(prepared),
    )
}

fn trigger_llm_request_with_surface_and_prepared_start_impl(
    client: Client,
    config: Config,
    context_manager: Option<&ContextManager>,
    tx: mpsc::UnboundedSender<StreamEvent>,
    token: CancellationToken,
    _is_debug: bool,
    session_dir: Option<String>,
    tool_surface: crate::tools::ToolSurface,
    request_started_hook: Option<RequestStartedHook>,
    prepared_start: Option<crate::accounting::ProviderRequestAccounting>,
    settlement_fallback: Option<mpsc::UnboundedSender<StreamEvent>>,
    prepared_llm_request: Option<PreparedLlmRequest>,
) -> Result<String, String> {
    let start_was_prepared = prepared_llm_request.is_some() || prepared_start.is_some();
    let prepared = match prepared_llm_request {
        Some(prepared) => {
            if prepared_start.is_some() {
                return Err("provider request supplied two prepared accounting starts".to_string());
            }
            prepared
        }
        None => {
            let context_manager = context_manager
                .ok_or_else(|| "provider request is missing its context snapshot".to_string())?;
            let started = prepared_start.unwrap_or_else(|| prepare_provider_request(&config));
            prepare_llm_request_with_start(&config, context_manager, tool_surface, started)?
        }
    };
    if prepared.tool_surface != tool_surface {
        return Err("prepared provider request does not match the tool surface".to_string());
    }
    let PreparedLlmRequest {
        config,
        started,
        transcript_prefix,
        raw_prompt,
        estimated_context_tokens: ctx_len,
        request: prepared_request,
        ..
    } = prepared;
    let request_id = started.request_id.clone();
    if started != request_started_accounting(&config, &request_id) {
        return Err("prepared provider request does not match the active connection".to_string());
    }

    let log_tx = tx.clone();
    let server_url = config.server_url.clone();
    let prefix = session_dir.unwrap_or_else(|| ".lethetic/".to_string());

    let _ = crate::app::write_session_file(&prefix, "last_raw_prompt.txt", raw_prompt.as_bytes());
    let _ =
        crate::app::write_session_file(&prefix, "last_request.json", prepared_request.body_bytes());
    let _ = crate::app::write_session_file(&prefix, "tokens.jsonl", b"");
    let stream_request_id = request_id.clone();

    if !start_was_prepared && let Some(hook) = &request_started_hook {
        hook(&ProviderRequestCheckpoint::accounting_only(started.clone()))?;
    }
    let _ = send_with_settlement_fallback(
        &tx,
        settlement_fallback.as_ref(),
        StreamEvent::RequestStarted(started),
    );
    tokio::spawn(async move {
        let _ = log_tx.send(StreamEvent::DebugLog(format!(
            "CALL_START|{server_url}|{ctx_len}"
        )));
        let request_start = std::time::Instant::now();
        let append_token = |prefix: &str, value: serde_json::Value| {
            if let Ok(mut line) = serde_json::to_vec(&value) {
                line.push(b'\n');
                let _ = crate::app::append_session_file(prefix, "tokens.jsonl", &line);
            }
        };

        let stream_start = tokio::select! {
            biased;
            _ = log_tx.closed() => ProviderStreamStart::ReceiverClosed,
            result = transport::stream_prepared(
                &client,
                &config,
                prepared_request,
            ) => ProviderStreamStart::Ready(result),
            _ = token.cancelled() => ProviderStreamStart::Cancelled,
        };
        let stream_result = match stream_start {
            ProviderStreamStart::ReceiverClosed => {
                let accounting = request_accounting(&config, &stream_request_id, None, false);
                let cancellation_requested = token.is_cancelled();
                if let Err(error) = emit_checkpointed_request_finish_with_fallback(
                    &log_tx,
                    settlement_fallback.as_ref(),
                    &request_started_hook,
                    accounting,
                    None,
                ) {
                    emit_request_settlement_failure_with_fallback(
                        &log_tx,
                        settlement_fallback.as_ref(),
                        &stream_request_id,
                        format!("provider receiver-closure accounting checkpoint failed: {error}"),
                        cancellation_requested,
                    );
                }
                return;
            }
            ProviderStreamStart::Cancelled => {
                let accounting = request_accounting(&config, &stream_request_id, None, false);
                if let Err(error) = emit_checkpointed_request_finish_with_fallback(
                    &log_tx,
                    settlement_fallback.as_ref(),
                    &request_started_hook,
                    accounting,
                    None,
                ) {
                    emit_request_settlement_failure_with_fallback(
                        &log_tx,
                        settlement_fallback.as_ref(),
                        &stream_request_id,
                        format!("provider cancellation accounting checkpoint failed: {error}"),
                        true,
                    );
                    return;
                }
                let _ = log_tx.send(StreamEvent::RequestCancelled {
                    request_id: stream_request_id,
                });
                return;
            }
            ProviderStreamStart::Ready(result) => result,
        };
        let mut event_stream = match stream_result {
            Ok(stream) => stream,
            Err(error) => {
                let accounting = request_accounting(&config, &stream_request_id, None, false);
                if let Err(checkpoint_error) = emit_checkpointed_request_finish_with_fallback(
                    &log_tx,
                    settlement_fallback.as_ref(),
                    &request_started_hook,
                    accounting,
                    None,
                ) {
                    emit_request_settlement_failure_with_fallback(
                        &log_tx,
                        settlement_fallback.as_ref(),
                        &stream_request_id,
                        format!(
                            "provider startup error accounting checkpoint failed: {checkpoint_error}; original provider error: {error}"
                        ),
                        false,
                    );
                    return;
                }
                let _ = log_tx.send(StreamEvent::Error(error));
                return;
            }
        };

        let mut in_thought_mode = true;
        let mut emitted_think_open = false;
        let mut last_reasoning_segment = None;
        let mut latest_usage = None;
        let mut pending_tool_calls = Vec::new();
        let mut pending_provider_content = None;
        let mut assembled_content = String::new();

        loop {
            let event = match poll_provider_stream(&mut event_stream, &token, &log_tx).await {
                ProviderStreamPoll::ReceiverClosed => {
                    drop(event_stream);
                    let accounting =
                        request_accounting(&config, &stream_request_id, latest_usage, false);
                    let cancellation_requested = token.is_cancelled();
                    if let Err(error) = emit_checkpointed_request_finish_with_fallback(
                        &log_tx,
                        settlement_fallback.as_ref(),
                        &request_started_hook,
                        accounting,
                        None,
                    ) {
                        emit_request_settlement_failure_with_fallback(
                            &log_tx,
                            settlement_fallback.as_ref(),
                            &stream_request_id,
                            format!(
                                "provider receiver-closure accounting checkpoint failed: {error}"
                            ),
                            cancellation_requested,
                        );
                    }
                    return;
                }
                ProviderStreamPoll::Event(Some(event)) => event,
                ProviderStreamPoll::Event(None) => {
                    let accounting =
                        request_accounting(&config, &stream_request_id, latest_usage, false);
                    if let Err(error) = emit_checkpointed_request_finish_with_fallback(
                        &log_tx,
                        settlement_fallback.as_ref(),
                        &request_started_hook,
                        accounting,
                        None,
                    ) {
                        emit_request_settlement_failure_with_fallback(
                            &log_tx,
                            settlement_fallback.as_ref(),
                            &stream_request_id,
                            format!(
                                "unterminated provider stream accounting checkpoint failed: {error}"
                            ),
                            false,
                        );
                        return;
                    }
                    let _ = log_tx.send(StreamEvent::Error(
                        "Provider stream ended without a terminal event".to_string(),
                    ));
                    return;
                }
                ProviderStreamPoll::Cancelled => {
                    drop(event_stream);
                    let accounting =
                        request_accounting(&config, &stream_request_id, latest_usage, false);
                    if let Err(error) = emit_checkpointed_request_finish_with_fallback(
                        &log_tx,
                        settlement_fallback.as_ref(),
                        &request_started_hook,
                        accounting,
                        None,
                    ) {
                        emit_request_settlement_failure_with_fallback(
                            &log_tx,
                            settlement_fallback.as_ref(),
                            &stream_request_id,
                            format!("provider cancellation accounting checkpoint failed: {error}"),
                            true,
                        );
                        return;
                    }
                    let _ = log_tx.send(StreamEvent::RequestCancelled {
                        request_id: stream_request_id,
                    });
                    return;
                }
            };

            match event {
                transport::StreamEvent::ReasoningDelta {
                    text,
                    segment_index,
                } => {
                    let elapsed = request_start.elapsed().as_millis();
                    if !emitted_think_open {
                        emitted_think_open = true;
                        assembled_content.push_str("<think>\n");
                        append_token(
                            &prefix,
                            serde_json::json!({"c": "<think>\n", "t": elapsed, "kind": "synthetic"}),
                        );
                        let _ = log_tx.send(StreamEvent::Chunk("<think>\n".to_string()));
                    }
                    if let Some(separator) =
                        reasoning_segment_separator(&mut last_reasoning_segment, segment_index)
                    {
                        assembled_content.push_str(separator);
                        append_token(
                            &prefix,
                            serde_json::json!({"c": separator, "t": elapsed, "kind": "synthetic"}),
                        );
                        let _ = log_tx.send(StreamEvent::Chunk(separator.to_string()));
                    }
                    assembled_content.push_str(&text);
                    append_token(
                        &prefix,
                        serde_json::json!({"c": text, "t": elapsed, "kind": "reasoning"}),
                    );
                    let _ = log_tx.send(StreamEvent::Chunk(text));
                }
                transport::StreamEvent::TextDelta(text) => {
                    if in_thought_mode {
                        in_thought_mode = false;
                        if emitted_think_open {
                            let elapsed = request_start.elapsed().as_millis();
                            assembled_content.push_str("</think>\n");
                            append_token(
                                &prefix,
                                serde_json::json!({"c": "</think>\n", "t": elapsed, "kind": "synthetic"}),
                            );
                            let _ = log_tx.send(StreamEvent::Chunk("</think>\n".to_string()));
                        }
                    }
                    let elapsed = request_start.elapsed().as_millis();
                    assembled_content.push_str(&text);
                    append_token(
                        &prefix,
                        serde_json::json!({"c": text, "t": elapsed, "kind": "text"}),
                    );
                    let _ = log_tx.send(StreamEvent::Chunk(text));
                }
                transport::StreamEvent::ToolCallStart { name, .. } => {
                    let _ = log_tx.send(StreamEvent::PreparingToolCall(name));
                }
                transport::StreamEvent::ToolCalls {
                    calls,
                    provider_content,
                } => {
                    if in_thought_mode && emitted_think_open {
                        in_thought_mode = false;
                        let elapsed = request_start.elapsed().as_millis();
                        assembled_content.push_str("</think>\n");
                        append_token(
                            &prefix,
                            serde_json::json!({"c": "</think>\n", "t": elapsed, "kind": "synthetic"}),
                        );
                        let _ = log_tx.send(StreamEvent::Chunk("</think>\n".to_string()));
                    }
                    pending_tool_calls.extend(calls.into_iter().map(|call| {
                        let provider_id = (!call.id.is_empty()).then(|| call.id.clone());
                        let effective_id = effective_tool_call_id(
                            config.active_connection_kind(),
                            &call.id,
                            &call.arguments,
                        );
                        append_token(
                            &prefix,
                            serde_json::json!({
                                "c": "",
                                "t": request_start.elapsed().as_millis(),
                                "kind": "tool",
                                "name": call.name,
                                "id": effective_id
                            }),
                        );
                        ToolCall {
                            id: effective_id,
                            provider_id,
                            function: crate::context::FunctionCall {
                                name: call.name,
                                arguments: call.arguments,
                            },
                        }
                    }));
                    if provider_content.is_some() {
                        pending_provider_content = provider_content;
                    }
                }
                transport::StreamEvent::UsageUpdate(usage) => {
                    latest_usage = Some(usage);
                    append_token(
                        &prefix,
                        serde_json::json!({
                            "t": request_start.elapsed().as_millis(),
                            "kind": "usage",
                            "usage": usage
                        }),
                    );
                    let _ = log_tx.send(StreamEvent::UsageUpdate {
                        request_id: stream_request_id.clone(),
                        usage,
                    });
                }
                transport::StreamEvent::Done {
                    completion_tokens,
                    prompt_tokens,
                    usage,
                    tg_per_s,
                    pp_per_s,
                    stop_reason,
                    provider_content,
                } => {
                    if in_thought_mode && emitted_think_open {
                        let elapsed = request_start.elapsed().as_millis();
                        assembled_content.push_str("</think>\n");
                        append_token(
                            &prefix,
                            serde_json::json!({"c": "</think>\n", "t": elapsed, "kind": "synthetic"}),
                        );
                        let _ = log_tx.send(StreamEvent::Chunk("</think>\n".to_string()));
                    }
                    let usage = usage.or(latest_usage).or_else(|| {
                        crate::accounting::Usage::from_legacy_counts(
                            prompt_tokens.map(u64::from),
                            completion_tokens.map(u64::from),
                        )
                    });
                    let accounting = request_accounting(&config, &stream_request_id, usage, true);
                    let assistant_provider_content = pending_provider_content
                        .clone()
                        .or_else(|| provider_content.clone());
                    let mut transcript = transcript_prefix.clone();
                    transcript.push(crate::context::Message {
                        role: "assistant".to_string(),
                        content: assembled_content.clone(),
                        tool_calls: (!pending_tool_calls.is_empty())
                            .then(|| pending_tool_calls.clone()),
                        provider_content: assistant_provider_content.clone(),
                        tool_result_is_error: false,
                    });
                    if let Err(error) = emit_checkpointed_request_finish_with_fallback(
                        &log_tx,
                        settlement_fallback.as_ref(),
                        &request_started_hook,
                        accounting,
                        Some(transcript),
                    ) {
                        emit_request_settlement_failure_with_fallback(
                            &log_tx,
                            settlement_fallback.as_ref(),
                            &stream_request_id,
                            format!("terminal provider response checkpoint failed: {error}"),
                            false,
                        );
                        return;
                    }
                    if !pending_tool_calls.is_empty() {
                        let _ = log_tx.send(StreamEvent::ToolCalls {
                            calls: std::mem::take(&mut pending_tool_calls),
                            provider_content: assistant_provider_content,
                        });
                    }
                    let _ = log_tx.send(StreamEvent::Done {
                        request_id: stream_request_id,
                        completion_tokens,
                        prompt_tokens,
                        usage,
                        tg_per_s,
                        pp_per_s,
                        stop_reason,
                        provider_content,
                    });
                    return;
                }
                transport::StreamEvent::Error(error) => {
                    let accounting =
                        request_accounting(&config, &stream_request_id, latest_usage, false);
                    if let Err(checkpoint_error) = emit_checkpointed_request_finish_with_fallback(
                        &log_tx,
                        settlement_fallback.as_ref(),
                        &request_started_hook,
                        accounting,
                        None,
                    ) {
                        emit_request_settlement_failure_with_fallback(
                            &log_tx,
                            settlement_fallback.as_ref(),
                            &stream_request_id,
                            format!(
                                "provider error accounting checkpoint failed: {checkpoint_error}; original provider error: {error}"
                            ),
                            false,
                        );
                        return;
                    }
                    let _ = log_tx.send(StreamEvent::Error(error));
                    return;
                }
                transport::StreamEvent::ToolCallDelta { .. } => {}
            }
        }
    });
    Ok(request_id)
}

pub async fn summarize_llm(
    client: &Client,
    config: &Config,
    context: &str,
    prompt: &str,
) -> Result<String, String> {
    summarize_llm_with_usage(client, config, context, prompt)
        .await
        .map(|completion| completion.text)
}

pub async fn summarize_llm_with_usage(
    client: &Client,
    config: &Config,
    context: &str,
    prompt: &str,
) -> Result<transport::Completion, String> {
    summarize_llm_with_usage_or_error(client, config, context, prompt)
        .await
        .map_err(|error| error.message)
}

async fn summarize_llm_with_usage_or_error(
    client: &Client,
    config: &Config,
    context: &str,
    prompt: &str,
) -> Result<transport::Completion, transport::CompletionError> {
    let truncated_context = crate::context::truncate_to_tokens(context, 160_000);
    let user_text = format!("{prompt}\n\nContext to summarize:\n{truncated_context}");
    transport::complete_with_usage_or_error(
        client,
        config,
        &[transport::Message::user(user_text)],
        4096_u32.min(config.maximum_output_tokens()),
    )
    .await
}

pub async fn summarize_llm_accounted(
    client: &Client,
    config: &Config,
    context: &str,
    prompt: &str,
    tx: &mpsc::UnboundedSender<StreamEvent>,
    cancellation: CancellationToken,
) -> Result<String, String> {
    summarize_llm_accounted_with_hook(client, config, context, prompt, tx, cancellation, None).await
}

pub async fn summarize_llm_accounted_with_hook(
    client: &Client,
    config: &Config,
    context: &str,
    prompt: &str,
    tx: &mpsc::UnboundedSender<StreamEvent>,
    cancellation: CancellationToken,
    request_hook: Option<RequestStartedHook>,
) -> Result<String, String> {
    let request_id = next_request_id();
    let started = request_started_accounting(config, &request_id);
    invoke_request_checkpoint(&request_hook, &started, None)?;
    let _ = tx.send(StreamEvent::RequestStarted(started));
    let (result, cancellation_requested) = tokio::select! {
        biased;
        result = summarize_llm_with_usage_or_error(client, config, context, prompt) => (result, false),
        _ = cancellation.cancelled() => (Err(transport::CompletionError::without_usage(
            "Summarization was cancelled",
        )), true),
    };
    let accounting = match &result {
        Ok(completion) => request_accounting(config, &request_id, completion.usage, true),
        Err(error) => request_accounting(config, &request_id, error.usage, false),
    };
    emit_checkpointed_nested_finish(
        Some(tx),
        &request_hook,
        accounting,
        cancellation_requested,
        "summarization terminal accounting checkpoint failed",
    )?;
    result
        .map(|completion| completion.text)
        .map_err(|error| error.message)
}

pub async fn get_available_models(
    client: &Client,
    kind: ConnectionKind,
    server_url: &str,
    api_key: Option<&str>,
) -> Result<Vec<transport::ModelInfo>, String> {
    transport::discover_models(client, kind, server_url, api_key).await
}

pub async fn get_single_response(
    client: &Client,
    config: &Config,
    prompt: String,
    images: Option<Vec<String>>,
    tx: Option<&mpsc::UnboundedSender<StreamEvent>>,
    cancellation: CancellationToken,
) -> Result<String, String> {
    get_single_response_with_usage(client, config, prompt, images, tx, cancellation)
        .await
        .map(|completion| completion.text)
}

pub async fn get_single_response_with_hook(
    client: &Client,
    config: &Config,
    prompt: String,
    images: Option<Vec<String>>,
    tx: Option<&mpsc::UnboundedSender<StreamEvent>>,
    cancellation: CancellationToken,
    request_hook: Option<RequestStartedHook>,
) -> Result<String, String> {
    get_single_response_with_usage_and_hook(
        client,
        config,
        prompt,
        images,
        tx,
        cancellation,
        request_hook,
    )
    .await
    .map(|completion| completion.text)
}

pub async fn get_single_response_with_usage(
    client: &Client,
    config: &Config,
    prompt: String,
    images: Option<Vec<String>>,
    tx: Option<&mpsc::UnboundedSender<StreamEvent>>,
    cancellation: CancellationToken,
) -> Result<transport::Completion, String> {
    get_single_response_with_usage_and_hook(client, config, prompt, images, tx, cancellation, None)
        .await
}

pub async fn get_single_response_with_usage_and_hook(
    client: &Client,
    config: &Config,
    prompt: String,
    images: Option<Vec<String>>,
    tx: Option<&mpsc::UnboundedSender<StreamEvent>>,
    cancellation: CancellationToken,
    request_hook: Option<RequestStartedHook>,
) -> Result<transport::Completion, String> {
    if request_hook.is_some() && tx.is_none() {
        return Err(
            "accounted single-response requests require a settlement event sender".to_string(),
        );
    }
    if let Some(log_tx) = tx {
        let _ = log_tx.send(StreamEvent::DebugLog(format!(
            "SINGLE_CALL_START|{}",
            config.server_url
        )));
    }
    let request_id = next_request_id();
    let started = request_started_accounting(config, &request_id);
    invoke_request_checkpoint(&request_hook, &started, None)?;
    if let Some(log_tx) = tx {
        let _ = log_tx.send(StreamEvent::RequestStarted(started));
    }
    let message = match images {
        Some(images) if !images.is_empty() => transport::Message::user_with_pngs(prompt, &images),
        _ => transport::Message::user(prompt),
    };
    let messages = [message];
    let (result, cancellation_requested) = tokio::select! {
        biased;
        result = transport::complete_with_usage_or_error(
            client,
            config,
            &messages,
            4096_u32.min(config.maximum_output_tokens()),
        ) => (result, false),
        _ = cancellation.cancelled() => (Err(transport::CompletionError::without_usage(
            "Vision request was cancelled",
        )), true),
    };
    let accounting = match &result {
        Ok(completion) => request_accounting(config, &request_id, completion.usage, true),
        Err(error) => request_accounting(config, &request_id, error.usage, false),
    };
    emit_checkpointed_nested_finish(
        tx,
        &request_hook,
        accounting,
        cancellation_requested,
        "single-response terminal accounting checkpoint failed",
    )?;
    result.map_err(|error| error.message)
}

// ───────────────────────── Session compaction ─────────────────────────

/// Strip large file bodies out of a `ui_log.txt` before compaction:
/// - Large `content` / `new_string` / `old_string` / `patch` fields in
///   write/edit/replace/apply_patch tool calls are elided.
/// - The entire TOOL RESULT block that follows a `read_file` or
///   `read_file_lines` call, since those files will be re-read dynamically.
///
/// Everything else (user messages, shell output, build results, todo updates,
/// short tool results) is preserved so the model understands what happened.
pub fn strip_file_content_from_log(log: &str) -> String {
    fn is_file_read_tool(name: &str) -> bool {
        matches!(name, "read_file" | "read_file_lines")
    }

    fn is_file_write_tool(name: &str) -> bool {
        matches!(
            name,
            "write_file" | "edit_file" | "replace_text" | "apply_patch"
        )
    }

    const LARGE_FIELDS: &[&str] = &[
        "content",
        "new_string",
        "old_string",
        "new_content",
        "patch",
    ];

    fn elide_field(line: &str, field: &str) -> String {
        let needle = format!("\"{}\":\"", field);
        let Some(start) = line.find(&needle) else {
            return line.to_string();
        };
        let val_start = start + needle.len();
        let bytes = line.as_bytes();
        let mut pos = val_start;
        while pos < bytes.len() {
            if bytes[pos] == b'\\' {
                pos += 2;
            } else if bytes[pos] == b'"' {
                break;
            } else {
                pos += 1;
            }
        }
        let pos = pos.min(bytes.len());
        let original_len = pos - val_start;
        if original_len < 120 {
            return line.to_string();
        }
        format!(
            "{}\"[elided {} chars]\"{}",
            &line[..val_start],
            original_len,
            if pos < line.len() {
                &line[pos + 1..]
            } else {
                ""
            }
        )
    }

    let mut out = String::with_capacity(log.len() / 2);
    let mut last_tool: Option<String> = None;
    let mut skip_until_next_section = false;

    for line in log.lines() {
        if line.starts_with("=== ") {
            skip_until_next_section = false;
            if line.starts_with("=== TOOL RESULT ===") {
                let is_read = last_tool.as_deref().map(is_file_read_tool).unwrap_or(false);
                out.push_str(line);
                out.push('\n');
                if is_read {
                    out.push_str("[file content elided — will be re-read dynamically]\n");
                    skip_until_next_section = true;
                }
                continue;
            }
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if skip_until_next_section {
            continue;
        }
        if let Some(rest) = line.strip_prefix("call:") {
            let tool_name = rest
                .split(|c| c == '{' || c == ' ')
                .next()
                .unwrap_or("")
                .to_string();
            last_tool = Some(tool_name.clone());
            if is_file_write_tool(&tool_name) {
                let mut elided = line.to_string();
                for field in LARGE_FIELDS {
                    elided = elide_field(&elided, field);
                }
                out.push_str(&elided);
            } else {
                out.push_str(line);
            }
            out.push('\n');
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Window size for multipass compaction (chars after stripping).
const COMPACT_WINDOW: usize = 80_000;
const COMPACT_OVERLAP: usize = 8_000;

/// Split `text` into overlapping windows of at most `window` chars, stepping by
/// `window - overlap`. Splits only on newline boundaries.
fn sliding_windows(text: &str, window: usize, overlap: usize) -> Vec<&str> {
    let step = window.saturating_sub(overlap).max(1);
    let mut windows = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let end = (start + window).min(text.len());
        let end = if end < text.len() {
            text[..end].rfind('\n').map(|p| p + 1).unwrap_or(end)
        } else {
            end
        };
        if end <= start {
            // Degenerate single line longer than a window: take it whole.
            let end = text[start..]
                .find('\n')
                .map(|p| start + p + 1)
                .unwrap_or(text.len());
            windows.push(&text[start..end]);
            start = end;
            continue;
        }
        windows.push(&text[start..end]);
        if end >= text.len() {
            break;
        }
        start += step;
        if start < text.len()
            && let Some(nl) = text[start..].find('\n')
        {
            start += nl + 1;
        }
    }
    windows
}

/// Compaction always runs as a plain completion on the General tool profile,
/// with no tools, so the Python-only request policy never applies.
pub fn compaction_config(config: &Config) -> Config {
    let mut compact = config.clone();
    compact.tool_profile = crate::config::ToolProfile::General;
    compact
}

async fn run_pass(
    client: &Client,
    config: &Config,
    system: &str,
    user: &str,
    max_tokens: u32,
    cancel: &CancellationToken,
) -> Result<String, String> {
    let messages = vec![
        transport::Message::system(system),
        transport::Message::user(user),
    ];
    let max_tokens = max_tokens.min(config.maximum_output_tokens());
    let fut = transport::complete(client, config, &messages, max_tokens);
    let raw = tokio::select! {
        _ = cancel.cancelled() => return Err("Cancelled".to_string()),
        result = fut => result?,
    };
    Ok(truncate_at_repetition(&strip_think_tags(&raw)))
}

async fn run_pass_streaming(
    client: &Client,
    config: &Config,
    system: &str,
    user: &str,
    tx: &mpsc::UnboundedSender<StreamEvent>,
    cancel: &CancellationToken,
) -> Result<String, String> {
    let messages = vec![
        transport::Message::system(system),
        transport::Message::user(user),
    ];
    let mut stream = transport::stream(
        client,
        config,
        &messages,
        &[],
        config.request_output_tokens(),
    )
    .await?;

    let mut accumulated = String::new();
    let mut line_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut last_checked = 0usize;
    let mut loop_detected = false;

    loop {
        let event = tokio::select! {
            _ = cancel.cancelled() => return Err("Cancelled".to_string()),
            ev = stream.next() => ev,
        };
        match event {
            Some(transport::StreamEvent::TextDelta(text))
            | Some(transport::StreamEvent::ReasoningDelta { text, .. }) => {
                let _ = tx.send(StreamEvent::CompactionChunk(text.clone()));
                accumulated.push_str(&text);
                let slice = &accumulated[last_checked..];
                if let Some(nl) = slice.rfind('\n') {
                    for line in slice[..nl].lines() {
                        let t = line.trim();
                        if t.is_empty() {
                            continue;
                        }
                        let c = line_counts.entry(t.to_string()).or_insert(0);
                        *c += 1;
                        if *c >= 3 {
                            loop_detected = true;
                            break;
                        }
                    }
                    last_checked += nl + 1;
                }
                if loop_detected {
                    break;
                }
            }
            Some(transport::StreamEvent::Done { .. }) | None => break,
            Some(transport::StreamEvent::Error(e)) => return Err(e),
            Some(_) => {}
        }
    }
    Ok(truncate_at_repetition(&strip_think_tags(
        accumulated.trim(),
    )))
}

const WINDOW_SYSTEM: &str = "\
You are summarizing one section of a coding session log. \
Capture what happened: user requests, files created/modified (exact paths), \
key decisions, errors encountered, and state at the end of this section.\n\
Rules: output ONLY the summary, begin immediately, no preamble, no planning. \
Preserve exact file paths, function names, and command lines. Be terse.";

const MERGE_SYSTEM: &str = "\
You are merging partial summaries of a coding session into one complete summary.\n\
Produce a single concise summary covering: the user's original goal, all files \
created or modified (exact paths), key decisions and reasoning, current project \
state (done / pending / blocked), important errors and how they were resolved.\n\
Rules: output ONLY the merged summary, begin immediately, no preamble, no planning. \
Preserve exact paths, names, and command lines. The result must be shorter than \
the combined partials.";

/// Run all window passes in parallel, returning ordered partial summaries and
/// reporting each completion through `on_done`.
async fn run_windows_parallel(
    client: &Client,
    config: &Config,
    windows: &[&str],
    cancel: &CancellationToken,
    mut on_done: impl FnMut(usize, usize),
) -> Result<Vec<String>, String> {
    let n = windows.len();
    let mut set = tokio::task::JoinSet::new();
    for (i, window) in windows.iter().enumerate() {
        let client = client.clone();
        let config = config.clone();
        let cancel = cancel.clone();
        let user = format!("Section {}/{} of the session log:\n\n{}", i + 1, n, window);
        set.spawn(async move {
            run_pass(&client, &config, WINDOW_SYSTEM, &user, 1024, &cancel)
                .await
                .map(|r| (i, r))
        });
    }
    let mut partials = vec![String::new(); n];
    let mut done = 0usize;
    while let Some(join_result) = set.join_next().await {
        if cancel.is_cancelled() {
            set.abort_all();
            return Err("Cancelled".to_string());
        }
        let (i, text) = join_result.map_err(|e| e.to_string())??;
        partials[i] = text;
        done += 1;
        on_done(done, n);
    }
    Ok(partials)
}

fn combine_partials(partials: &[String]) -> String {
    partials
        .iter()
        .enumerate()
        .map(|(i, p)| format!("=== Section {} ===\n{}", i + 1, p))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// CLI variant: window passes run in parallel, merge pass sequential.
pub async fn compact_llm(client: &Client, config: &Config, log: &str) -> Result<String, String> {
    let config = compaction_config(config);
    let cancel = CancellationToken::new();
    let stripped = strip_file_content_from_log(log);
    let window_strs = sliding_windows(&stripped, COMPACT_WINDOW, COMPACT_OVERLAP);
    let n = window_strs.len();
    eprintln!("Compacting: {} windows in parallel", n);
    let mut partials = run_windows_parallel(client, &config, &window_strs, &cancel, |done, n| {
        eprintln!("  pass {done}/{n} complete");
    })
    .await?;
    if partials.len() == 1 {
        return Ok(partials.remove(0));
    }
    eprintln!("  All passes done, merging…");
    let combined = combine_partials(&partials);
    run_pass(
        client,
        &config,
        MERGE_SYSTEM,
        &format!("Partial summaries to merge:\n\n{}", combined),
        2048,
        &cancel,
    )
    .await
}

/// UI variant: window passes run in parallel (reporting completions), then the
/// merge pass streams live into the popup.
pub async fn compact_llm_streaming(
    client: &Client,
    config: &Config,
    log: &str,
    tx: &mpsc::UnboundedSender<StreamEvent>,
    cancel: &CancellationToken,
) -> Result<String, String> {
    let config = compaction_config(config);
    let stripped = strip_file_content_from_log(log);
    let window_strs = sliding_windows(&stripped, COMPACT_WINDOW, COMPACT_OVERLAP);
    let n = window_strs.len();
    let _ = tx.send(StreamEvent::CompactionChunk(format!(
        "Log: {} chars → {} chars after stripping file content\nRunning {} window pass{} in parallel…\n",
        log.len(),
        stripped.len(),
        n,
        if n == 1 { "" } else { "es" }
    )));
    let progress_tx = tx.clone();
    let mut partials = run_windows_parallel(client, &config, &window_strs, cancel, |done, n| {
        let _ = progress_tx.send(StreamEvent::CompactionChunk(format!(
            "  ✓ Pass {}/{} complete\n",
            done, n
        )));
    })
    .await?;
    if partials.len() == 1 {
        let summary = partials.remove(0);
        let _ = tx.send(StreamEvent::CompactionChunk(format!("\n{summary}\n")));
        return Ok(summary);
    }
    let _ = tx.send(StreamEvent::CompactionChunk(format!(
        "\n── Merging {} sections (streaming) ──\n",
        n
    )));
    let combined = combine_partials(&partials);
    run_pass_streaming(
        client,
        &config,
        MERGE_SYSTEM,
        &format!("Partial summaries to merge:\n\n{}", combined),
        tx,
        cancel,
    )
    .await
}

/// Detect repeated-line loops and truncate before the third occurrence of any line.
fn truncate_at_repetition(text: &str) -> String {
    let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let mut out = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            out.push(line);
            continue;
        }
        let count = seen.entry(trimmed).or_insert(0);
        *count += 1;
        if *count >= 3 {
            break;
        }
        out.push(line);
    }
    out.join("\n").trim_end().to_string()
}

/// Remove `<think>…</think>` blocks from model output.
fn strip_think_tags(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<think>") {
        result.push_str(&rest[..start]);
        match rest[start..].find("</think>") {
            Some(rel_end) => rest = &rest[start + rel_end + "</think>".len()..],
            None => return result.trim().to_string(),
        }
    }
    result.push_str(rest);
    result.trim().to_string()
}

#[cfg(test)]
mod compaction_tests {
    use super::{
        sliding_windows, strip_file_content_from_log, strip_think_tags, truncate_at_repetition,
    };

    #[test]
    fn strips_write_file_content() {
        let body = "using System;\\n".repeat(20);
        let log = format!(
            "=== TOOL CALL: Write CLI ===\ncall:write_file{{\"content\":\"{body}\",\"path\":\"/foo/Bar.cs\",\"tool_call_id\":\"w1\"}}\n=== TOOL RESULT ===\nSuccessfully wrote to /foo/Bar.cs\n"
        );
        let out = strip_file_content_from_log(&log);
        assert!(!out.contains("using System"), "file body should be elided");
        assert!(out.contains("elided"));
        assert!(out.contains("/foo/Bar.cs"));
    }

    #[test]
    fn strips_read_file_result() {
        let log = "=== TOOL CALL: Read config ===\ncall:read_file{\"path\":\"/foo/x.csproj\",\"tool_call_id\":\"r1\"}\n=== TOOL RESULT ===\n<Project Sdk=\"...\"><lots of xml/></Project>\n=== THOUGHT ===\n\n";
        let out = strip_file_content_from_log(log);
        assert!(!out.contains("<Project"));
        assert!(out.contains("elided"));
        assert!(out.contains("=== THOUGHT ==="));
    }

    #[test]
    fn keeps_short_tool_fields_and_shell_results() {
        let log = "=== TOOL CALL: Shell ===\ncall:run_shell_command{\"command\":\"dotnet build\",\"tool_call_id\":\"sh1\"}\n=== TOOL RESULT ===\nBuild succeeded.\n";
        let out = strip_file_content_from_log(log);
        assert!(out.contains("dotnet build"));
        assert!(out.contains("Build succeeded"));
    }

    #[test]
    fn windows_overlap_and_cover_everything() {
        let text = (0..2000).map(|i| format!("line {i}\n")).collect::<String>();
        let windows = sliding_windows(&text, 4000, 500);
        assert!(windows.len() > 1);
        assert!(windows[0].starts_with("line 0\n"));
        assert!(windows.last().unwrap().ends_with("line 1999\n"));
        for w in &windows {
            assert!(w.len() <= 4000);
            assert!(w.ends_with('\n'));
        }
    }

    #[test]
    fn think_tags_and_repetition_are_removed() {
        assert_eq!(strip_think_tags("<think>plan</think>result"), "result");
        assert_eq!(strip_think_tags("head<think>unclosed"), "head");
        let looped = "a\nb\na\nb\na\nc";
        assert_eq!(truncate_at_repetition(looped), "a\nb\na\nb");
    }
}

#[cfg(test)]
mod tests;
