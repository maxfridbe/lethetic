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

#[cfg(test)]
mod tests;
