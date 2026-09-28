use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::client::{
    StreamEvent, trigger_llm_request_with_surface_and_start_hook_and_settlement_fallback,
};
use crate::config::Config;
use crate::context::{ContextManager, ToolCall};
use crate::parser::{self, StreamParser};
use crate::system_prompt::SystemPromptManager;
use crate::tools;

struct CancelOnDrop(CancellationToken);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

fn trigger_headless_request(
    client: reqwest::Client,
    config: Config,
    context: &ContextManager,
    tx: mpsc::UnboundedSender<StreamEvent>,
    settlement_fallback: Option<mpsc::UnboundedSender<StreamEvent>>,
    cancellation_token: CancellationToken,
    session_dir: Option<String>,
    request_started_hook: Option<RequestAccountingHook>,
) -> Result<String, String> {
    trigger_llm_request_with_surface_and_start_hook_and_settlement_fallback(
        client,
        config,
        context,
        tx,
        settlement_fallback,
        cancellation_token,
        false,
        session_dir,
        tools::ToolSurface::Headless,
        request_started_hook,
    )
}

fn find_legacy_text_tool_call(
    config: &Config,
    text: &str,
    is_final: bool,
) -> Option<Result<(Vec<ToolCall>, usize), (String, usize)>> {
    if config.active_connection_kind().uses_native_tools() {
        None
    } else if crate::tool_call_mode::allows_batches(config) {
        parser::find_tool_calls(text, is_final).map(|found| {
            found.map(|(calls, end)| (crate::tool_call_mode::with_unique_ids(calls), end))
        })
    } else {
        parser::find_tool_call(text, is_final)
            .map(|found| found.map(|(call, end)| (vec![call], end)))
    }
}

async fn forward_nested_tool_events(
    mut tool_rx: mpsc::UnboundedReceiver<StreamEvent>,
    local_tx: mpsc::UnboundedSender<StreamEvent>,
    progress_tx: Option<mpsc::UnboundedSender<StreamEvent>>,
) {
    while let Some(event) = tool_rx.recv().await {
        match event {
            StreamEvent::RequestStarted(_) | StreamEvent::RequestFinished(_) => {
                if let Some(progress) = &progress_tx {
                    let _ = progress.send(event.clone());
                }
                let _ = local_tx.send(event);
            }
            _ => {
                if let Some(progress) = &progress_tx {
                    let _ = progress.send(event);
                } else {
                    let _ = local_tx.send(event);
                }
            }
        }
    }
}

fn mark_pending_python_audits(
    pending: &Option<(Vec<ToolCall>, Option<Vec<serde_json::Value>>)>,
    tool_runtime: &crate::tool_runtime::ToolRuntime,
    status: crate::python::notebook::NotebookAttemptStatus,
    message: &str,
) -> Result<(), String> {
    if let Some((calls, _)) = pending {
        for call in calls {
            if call.function.name == "python" {
                tool_runtime.mark_python_audit_status(
                    call.provider_id.as_deref().unwrap_or(&call.id),
                    status,
                    Some(message),
                )?;
            }
        }
    }
    Ok(())
}

async fn execute_tool_calls(
    calls: Vec<ToolCall>,
    assistant_content: &str,
    provider_content: Option<Vec<serde_json::Value>>,
    assistant_already_checkpointed: bool,
    context: &mut ContextManager,
    current_dir: &mut String,
    client: &reqwest::Client,
    config: &Config,
    print_output: bool,
    tx: &mpsc::UnboundedSender<StreamEvent>,
    progress_tx: &Option<mpsc::UnboundedSender<StreamEvent>>,
    cancellation_token: CancellationToken,
    tool_runtime: &crate::tool_runtime::ToolRuntime,
    result_root: &Path,
    session_directory: Option<&Path>,
    request_hook: &Option<RequestAccountingHook>,
    transcript_hook: &Option<TranscriptHook>,
) -> Result<(), String> {
    context.add_assistant_tool_call_with_provider(
        assistant_content,
        calls.clone(),
        provider_content,
    );
    if !assistant_already_checkpointed && let Some(hook) = transcript_hook {
        hook(
            context.get_messages(),
            TranscriptStage::Intermediate,
            None,
            &[],
        )?;
    }

    if calls.len() > 1 && !crate::tool_call_mode::allows_batches(config) {
        let reason = format!(
            "Provider returned {} tool calls in one turn; Headless mode requires exactly one and rejected the entire batch.",
            calls.len()
        );
        let mut tool_events = Vec::with_capacity(calls.len());
        for call in &calls {
            let mut result = format!("ERROR: {reason}");
            if call.function.name == "python" {
                let audit_id = call.provider_id.as_deref().unwrap_or(&call.id);
                let begin = tool_runtime.begin_python_audit_attempt(
                    session_directory,
                    config,
                    audit_id,
                    call.function.arguments["code"].as_str().unwrap_or(""),
                    call.function.arguments["description"]
                        .as_str()
                        .unwrap_or("Python cell"),
                    current_dir,
                );
                if let Some(path) = tool_runtime.take_python_notebook_notice() {
                    if let Some(progress) = progress_tx {
                        let _ = progress.send(StreamEvent::PythonNotebookNotice(path));
                    } else {
                        let _ = tx.send(StreamEvent::PythonNotebookNotice(path));
                    }
                }
                let audit = begin.and_then(|_| {
                    tool_runtime.mark_python_audit_status(
                        audit_id,
                        crate::python::notebook::NotebookAttemptStatus::Denied,
                        Some(
                            "Provider returned multiple tool calls; the entire batch was rejected.",
                        ),
                    )
                });
                if let Err(error) = audit {
                    result.push_str(&format!(
                        " The notebook rejection checkpoint also failed: {error}"
                    ));
                }
            }
            tool_events.push(ToolTranscriptEvent::new(call, result.clone(), true));
            context.add_tool_message_with_status(
                call.id.clone(),
                &call.function.name,
                &result,
                true,
            );
        }
        if let Some(hook) = transcript_hook {
            hook(
                context.get_messages(),
                TranscriptStage::Intermediate,
                None,
                &tool_events,
            )?;
        }
        return Ok(());
    }

    for call in calls {
        if call.function.name == "python"
            && print_output
            && let Some(display) =
                crate::python::display::PythonCallDisplay::from_arguments(&call.function.arguments)
        {
            print!("\r{:60}\r", "");
            println!("{}", display.plain_text(false));
        }
        let mut arguments = call.function.arguments.clone();
        let audit_call_id = if call.function.name == "python" {
            let source = call.function.arguments["code"].as_str().unwrap_or("");
            let description = call.function.arguments["description"]
                .as_str()
                .unwrap_or("Python cell");
            let audit_call_id = call.provider_id.clone().unwrap_or_else(|| call.id.clone());
            tool_runtime.begin_python_audit_attempt(
                session_directory,
                config,
                &audit_call_id,
                source,
                description,
                current_dir,
            )?;
            if let Some(path) = tool_runtime.take_python_notebook_notice() {
                if let Some(progress) = progress_tx {
                    let _ = progress.send(StreamEvent::PythonNotebookNotice(path));
                } else {
                    let _ = tx.send(StreamEvent::PythonNotebookNotice(path));
                }
            }
            Some(audit_call_id)
        } else {
            None
        };
        if let Some(admission_error) =
            tools::tool_admission_error(config, tools::ToolSurface::Headless, &call.function.name)
        {
            let mut message = format!("ERROR: {admission_error}");
            if let Some(audit_call_id) = audit_call_id.as_deref()
                && let Err(error) = tool_runtime.mark_python_audit_status(
                    audit_call_id,
                    crate::python::notebook::NotebookAttemptStatus::Denied,
                    Some("Python dispatch was rejected by the active headless tool policy."),
                )
            {
                message.push_str(&format!(
                    " The notebook rejection checkpoint also failed: {error}"
                ));
            }
            let tool_event = ToolTranscriptEvent::new(&call, message.clone(), true);
            context.add_tool_message_with_status(
                call.id.clone(),
                &call.function.name,
                &message,
                true,
            );
            if let Some(hook) = transcript_hook {
                hook(
                    context.get_messages(),
                    TranscriptStage::Intermediate,
                    None,
                    std::slice::from_ref(&tool_event),
                )?;
            }
            continue;
        }
        if call.function.name == "python" {
            let audit_call_id = audit_call_id.expect("Python audit ID was initialized");
            let validation = tools::python::validate_arguments(&call.function.arguments);
            if let Err(argument_error) = validation {
                let message = format!("Malformed Python tool call: {argument_error}.");
                tool_runtime.mark_python_audit_status(
                    &audit_call_id,
                    crate::python::notebook::NotebookAttemptStatus::Denied,
                    Some(&message),
                )?;
                let result = format!("ERROR: {message}");
                let tool_event = ToolTranscriptEvent::new(&call, result.clone(), true);
                context.add_tool_message_with_status(
                    call.id.clone(),
                    &call.function.name,
                    &result,
                    true,
                );
                if let Some(hook) = transcript_hook {
                    hook(
                        context.get_messages(),
                        TranscriptStage::Intermediate,
                        None,
                        std::slice::from_ref(&tool_event),
                    )?;
                }
                continue;
            }
            tool_runtime.mark_python_audit_status(
                &audit_call_id,
                crate::python::notebook::NotebookAttemptStatus::Approved,
                None,
            )?;
            arguments
                .as_object_mut()
                .expect("validated Python arguments are an object")
                .insert(
                    tools::INTERNAL_TOOL_CALL_ID_KEY.to_string(),
                    serde_json::Value::String(audit_call_id),
                );
        }
        let (tool_tx, tool_rx) = mpsc::unbounded_channel();
        let local_tx = tx.clone();
        let progress = progress_tx.clone();
        let forwarder = tokio::spawn(forward_nested_tool_events(tool_rx, local_tx, progress));
        let execution = tools::execute_with_runtime_and_request_hook(
            tool_runtime,
            &call.function.name,
            &arguments,
            current_dir,
            cancellation_token.clone(),
            tool_tx,
            client,
            config,
            request_hook.clone(),
        )
        .await;
        forwarder
            .await
            .map_err(|error| format!("nested tool accounting forwarder failed: {error}"))?;
        let presented = tools::present_tool_execution_in(result_root, &call.id, execution);
        let tool_event = ToolTranscriptEvent::new(&call, presented.ui, presented.is_error);
        *current_dir = presented.cwd;
        context.add_tool_message_with_status(
            call.id.clone(),
            &call.function.name,
            &presented.context,
            presented.is_error,
        );
        if let Some(hook) = transcript_hook {
            hook(
                context.get_messages(),
                TranscriptStage::Intermediate,
                None,
                std::slice::from_ref(&tool_event),
            )?;
        }
    }

    context.set_cwd(current_dir.clone());
    Ok(())
}

#[derive(Debug, Clone)]
pub struct AgentRun {
    pub text: String,
    pub requests: Vec<crate::accounting::ProviderRequestAccounting>,
    pub messages: Vec<crate::context::Message>,
}

impl AgentRun {
    pub fn total_usage(&self) -> crate::accounting::Usage {
        self.requests.iter().map(|request| request.usage).fold(
            crate::accounting::Usage {
                breakdown_complete: true,
                ..crate::accounting::Usage::default()
            },
            crate::accounting::Usage::saturating_add,
        )
    }
}

/// Run a self-contained agent loop with the given prompt.
///
/// If `progress_tx` is Some, ToolProgress events from tool execution are
/// forwarded so the parent TUI session can show sub-agent activity.
/// Returns the final assistant text response.
pub async fn run_agent(
    prompt: String,
    client: &reqwest::Client,
    config: &Config,
    print_output: bool,
    progress_tx: Option<mpsc::UnboundedSender<StreamEvent>>,
) -> Result<String, String> {
    run_agent_accounted(prompt, client, config, print_output, progress_tx)
        .await
        .map(|run| run.text)
}

pub type RequestAccountingHook = crate::client::RequestStartedHook;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TranscriptStage {
    Intermediate,
    Final,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolTranscriptEvent {
    pub projection_id: String,
    pub call: ToolCall,
    pub ui: String,
    pub is_error: bool,
}

impl ToolTranscriptEvent {
    fn new(call: &ToolCall, ui: String, is_error: bool) -> Self {
        Self {
            projection_id: format!("headless-tool-{}", uuid::Uuid::new_v4()),
            call: call.clone(),
            ui,
            is_error,
        }
    }
}

pub type TranscriptHook = Arc<
    dyn Fn(
            &[crate::context::Message],
            TranscriptStage,
            Option<&str>,
            &[ToolTranscriptEvent],
        ) -> Result<(), String>
        + Send
        + Sync,
>;

pub async fn run_agent_accounted(
    prompt: String,
    client: &reqwest::Client,
    config: &Config,
    print_output: bool,
    progress_tx: Option<mpsc::UnboundedSender<StreamEvent>>,
) -> Result<AgentRun, String> {
    run_agent_accounted_with_cancellation(prompt, client, config, print_output, progress_tx, None)
        .await
}

pub async fn run_agent_accounted_with_cancellation(
    prompt: String,
    client: &reqwest::Client,
    config: &Config,
    print_output: bool,
    progress_tx: Option<mpsc::UnboundedSender<StreamEvent>>,
    cancellation: Option<CancellationToken>,
) -> Result<AgentRun, String> {
    run_agent_accounted_with_cancellation_and_hook(
        prompt,
        client,
        config,
        print_output,
        progress_tx,
        cancellation,
        None,
    )
    .await
}

pub async fn run_agent_accounted_with_cancellation_and_hook(
    prompt: String,
    client: &reqwest::Client,
    config: &Config,
    print_output: bool,
    progress_tx: Option<mpsc::UnboundedSender<StreamEvent>>,
    cancellation: Option<CancellationToken>,
    request_hook: Option<RequestAccountingHook>,
) -> Result<AgentRun, String> {
    let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let tool_runtime = crate::tool_runtime::ToolRuntime::headless(workspace.clone());
    run_agent_accounted_with_runtime(
        prompt,
        client,
        config,
        print_output,
        progress_tx,
        &tool_runtime,
        workspace,
        Vec::new(),
        true,
        None,
        request_hook,
        None,
        cancellation,
    )
    .await
}

pub async fn run_agent_accounted_with_runtime(
    prompt: String,
    client: &reqwest::Client,
    config: &Config,
    print_output: bool,
    progress_tx: Option<mpsc::UnboundedSender<StreamEvent>>,
    tool_runtime: &crate::tool_runtime::ToolRuntime,
    workspace: PathBuf,
    initial_messages: Vec<crate::context::Message>,
    add_initial_user_message: bool,
    session_dir: Option<String>,
    request_hook: Option<RequestAccountingHook>,
    transcript_hook: Option<TranscriptHook>,
    cancellation: Option<CancellationToken>,
) -> Result<AgentRun, String> {
    if tool_runtime.surface() != tools::ToolSurface::Headless {
        return Err("Headless agent requires a Headless tool runtime".to_string());
    }
    if let Some(error) = config.python_mode_validation_error() {
        return Err(format!("Invalid Python-only policy: {error}"));
    }
    let workspace = workspace.canonicalize().unwrap_or(workspace);
    let cwd = workspace.to_string_lossy().into_owned();
    if config.tool_profile == crate::config::ToolProfile::PythonOnly {
        tool_runtime
            .ensure_ready(config, &workspace)
            .await
            .map_err(|error| format!("Python-only runtime is unavailable: {error}"))?;
        while let Some(notice) = tool_runtime.take_python_runtime_notice() {
            if print_output {
                println!("{}", notice.render());
            }
            if let Some(progress_tx) = &progress_tx {
                let _ = progress_tx.send(StreamEvent::PythonRuntimeNotice(notice));
            }
        }
    }

    // Build a system prompt without task/ask_the_user so a sub-agent cannot
    // recursively spawn another sub-agent or pause for interactive input.
    let spm = SystemPromptManager::new();
    let template = spm
        .load_prompt("software_engineer")
        .unwrap_or_else(|| crate::system_prompt::DEFAULT_PROMPT_TEMPLATE.to_string());
    let tool_declarations =
        tools::get_prompt_templates_excluding(config, &["task", "ask_the_user"]);
    let tool_call_format = format!(
        "{}\n{}",
        if config.active_connection_kind().uses_native_tools() {
            crate::system_prompt::TOOL_CALL_FORMAT_NATIVE
        } else {
            match config.active_parser() {
                "qwen3" | "default" | "generic" => crate::system_prompt::TOOL_CALL_FORMAT_QWEN3,
                _ => crate::system_prompt::TOOL_CALL_FORMAT_GEMMA4,
            }
        },
        crate::system_prompt::tool_call_count_guidance(config)
    );
    let mut resolved = template
        .replace("[TOOLS_DEFINITIONS]", &tool_declarations)
        .replace("[CWD]", &cwd)
        .replace("[TOOL_CALL_FORMAT]", &tool_call_format);
    if let Some(guidance) = crate::system_prompt::python_capability_guidance(config) {
        resolved.push_str("\n\n");
        resolved.push_str(&guidance);
    }

    let mut context = ContextManager::new(config.input_token_budget(), Some(resolved));
    if let Some(mode) = config.context_mode {
        context.mode = mode;
    }
    if !initial_messages.is_empty() {
        context.set_messages(initial_messages);
    }
    context.set_cwd(cwd.clone());
    if add_initial_user_message {
        context.add_message("user", &prompt);
        if let Some(hook) = &transcript_hook {
            hook(
                context.get_messages(),
                TranscriptStage::Intermediate,
                None,
                &[],
            )?;
        }
    }

    let mut parser =
        StreamParser::with_mode(crate::parser::ParserMode::from(config.active_parser()));
    let mut current_dir = cwd;
    let mut full_response = String::new();
    let mut retry_attempts = 0_u32;
    let mut pending_structured: Option<(Vec<ToolCall>, Option<Vec<serde_json::Value>>)> = None;
    let mut requests = Vec::new();

    let (tx, mut rx) = mpsc::unbounded_channel::<StreamEvent>();
    let run_cancellation = cancellation.unwrap_or_default();
    let _cancel_on_drop = CancelOnDrop(run_cancellation.clone());
    let mut cancellation_token = run_cancellation.child_token();

    let mut active_request_id = trigger_headless_request(
        client.clone(),
        config.clone(),
        &context,
        tx.clone(),
        progress_tx.clone(),
        cancellation_token.clone(),
        session_dir.clone(),
        request_hook.clone(),
    )?;

    loop {
        match rx.recv().await {
            Some(StreamEvent::Chunk(chunk)) => {
                full_response.push_str(&chunk);
                parser.parse_chunk(&chunk);
                if let Some(hook) = &transcript_hook {
                    let mut checkpoint = context.get_messages().to_vec();
                    checkpoint.push(crate::context::Message {
                        role: "assistant".to_string(),
                        content: full_response.clone(),
                        tool_calls: None,
                        provider_content: None,
                        tool_result_is_error: false,
                    });
                    hook(
                        &checkpoint,
                        TranscriptStage::Intermediate,
                        Some(&active_request_id),
                        &[],
                    )?;
                }
                if print_output {
                    print!("{}", chunk);
                    let _ = std::io::Write::flush(&mut std::io::stdout());
                }
            }
            Some(StreamEvent::ToolCalls {
                calls,
                provider_content,
            }) => {
                for call in &calls {
                    if call.function.name != "python" {
                        continue;
                    }
                    tool_runtime.begin_python_audit_attempt(
                        session_dir.as_deref().map(Path::new),
                        config,
                        call.provider_id.as_deref().unwrap_or(&call.id),
                        call.function.arguments["code"].as_str().unwrap_or(""),
                        call.function.arguments["description"]
                            .as_str()
                            .unwrap_or("Python cell"),
                        &current_dir,
                    )?;
                    if let Some(path) = tool_runtime.take_python_notebook_notice() {
                        if let Some(progress) = &progress_tx {
                            let _ = progress.send(StreamEvent::PythonNotebookNotice(path));
                        } else {
                            let _ = tx.send(StreamEvent::PythonNotebookNotice(path));
                        }
                    }
                }
                pending_structured = Some((calls, provider_content));
            }
            Some(StreamEvent::ToolProgress(message)) => {
                if print_output {
                    print!("\r[…] {}          ", message.replace('\n', " | "));
                    let _ = std::io::Write::flush(&mut std::io::stdout());
                }
                if let Some(progress_tx) = &progress_tx {
                    let _ = progress_tx.send(StreamEvent::ToolProgress(message));
                }
            }
            Some(StreamEvent::TodoUpdated(snapshot)) => {
                if print_output {
                    print!("\r{:60}\r", "");
                    println!(
                        "Todo list refreshed via lethetic_todo: {} task(s) (revision {})",
                        snapshot.todos.len(),
                        snapshot.revision
                    );
                }
                if let Some(progress_tx) = &progress_tx {
                    let _ = progress_tx.send(StreamEvent::TodoUpdated(snapshot));
                }
            }
            Some(StreamEvent::PythonRuntimeNotice(notice)) => {
                if print_output {
                    print!("\r{:60}\r", "");
                    println!("{}", notice.render());
                }
                if let Some(progress_tx) = &progress_tx {
                    let _ = progress_tx.send(StreamEvent::PythonRuntimeNotice(notice));
                }
            }
            Some(StreamEvent::PythonNotebookNotice(path)) => {
                if print_output {
                    print!("\r{:60}\r", "");
                    println!("Python notebook audit: {}", path.display());
                }
                if let Some(progress_tx) = &progress_tx {
                    let _ = progress_tx.send(StreamEvent::PythonNotebookNotice(path));
                }
            }
            Some(StreamEvent::UsageUpdate { .. }) => {}
            Some(StreamEvent::RequestStarted(_)) => {}
            Some(StreamEvent::RequestFinished(request)) => {
                requests.push(request);
            }
            Some(StreamEvent::Done {
                provider_content, ..
            }) => {
                retry_attempts = 0;
                if print_output {
                    print!("\r{:60}\r", "");
                }

                if let Some((calls, call_provider_content)) = pending_structured.take() {
                    execute_tool_calls(
                        calls,
                        &full_response,
                        call_provider_content.or(provider_content),
                        request_hook.is_some(),
                        &mut context,
                        &mut current_dir,
                        client,
                        config,
                        print_output,
                        &tx,
                        &progress_tx,
                        cancellation_token.clone(),
                        tool_runtime,
                        &workspace,
                        session_dir.as_deref().map(Path::new),
                        &request_hook,
                        &transcript_hook,
                    )
                    .await?;
                    full_response.clear();
                    parser.reset();
                    cancellation_token = run_cancellation.child_token();
                    active_request_id = trigger_headless_request(
                        client.clone(),
                        config.clone(),
                        &context,
                        tx.clone(),
                        progress_tx.clone(),
                        cancellation_token.clone(),
                        session_dir.clone(),
                        request_hook.clone(),
                    )?;
                    continue;
                }

                match find_legacy_text_tool_call(config, &full_response, true) {
                    Some(Ok((calls, _))) => {
                        execute_tool_calls(
                            calls,
                            &full_response,
                            None,
                            false,
                            &mut context,
                            &mut current_dir,
                            client,
                            config,
                            print_output,
                            &tx,
                            &progress_tx,
                            cancellation_token.clone(),
                            tool_runtime,
                            &workspace,
                            session_dir.as_deref().map(Path::new),
                            &request_hook,
                            &transcript_hook,
                        )
                        .await?;
                        full_response.clear();
                        parser.reset();
                        cancellation_token = run_cancellation.child_token();
                        active_request_id = trigger_headless_request(
                            client.clone(),
                            config.clone(),
                            &context,
                            tx.clone(),
                            progress_tx.clone(),
                            cancellation_token.clone(),
                            session_dir.clone(),
                            request_hook.clone(),
                        )?;
                        continue;
                    }
                    Some(Err((error, _))) => {
                        context.add_assistant_message(&full_response, provider_content);
                        context.add_message(
                            "user",
                            &format!(
                                "Your tool call could not be parsed: {error}. Correct it and call one tool again."
                            ),
                        );
                        if let Some(hook) = &transcript_hook {
                            hook(
                                context.get_messages(),
                                TranscriptStage::Intermediate,
                                None,
                                &[],
                            )?;
                        }
                        full_response.clear();
                        parser.reset();
                        cancellation_token = run_cancellation.child_token();
                        active_request_id = trigger_headless_request(
                            client.clone(),
                            config.clone(),
                            &context,
                            tx.clone(),
                            progress_tx.clone(),
                            cancellation_token.clone(),
                            session_dir.clone(),
                            request_hook.clone(),
                        )?;
                        continue;
                    }
                    None => {}
                }

                context.add_assistant_message(&full_response, provider_content);
                if request_hook.is_none()
                    && let Some(hook) = &transcript_hook
                {
                    hook(context.get_messages(), TranscriptStage::Final, None, &[])?;
                }
                return Ok(AgentRun {
                    text: full_response,
                    requests,
                    messages: context.get_messages().to_vec(),
                });
            }
            Some(StreamEvent::RequestCancelled { request_id }) => {
                mark_pending_python_audits(
                    &pending_structured,
                    tool_runtime,
                    crate::python::notebook::NotebookAttemptStatus::Cancelled,
                    "Provider request was cancelled before Python dispatch.",
                )?;
                return Err(format!("provider request {request_id} was cancelled"));
            }
            Some(StreamEvent::RequestSettlementFailed {
                request_id,
                error,
                cancellation_requested,
            }) => {
                if request_id != active_request_id
                    && let Some(progress_tx) = &progress_tx
                {
                    let _ = progress_tx.send(StreamEvent::RequestSettlementFailed {
                        request_id,
                        error: error.clone(),
                        cancellation_requested,
                    });
                }
                mark_pending_python_audits(
                    &pending_structured,
                    tool_runtime,
                    crate::python::notebook::NotebookAttemptStatus::Interrupted,
                    "Provider failed after recognizing Python but before dispatch.",
                )?;
                return Err(error);
            }
            Some(StreamEvent::Error(error)) => {
                let allowed = crate::provider_retry::retries_for(config);
                if pending_structured.is_none()
                    && retry_attempts < allowed
                    && crate::provider_retry::is_retryable(&error)
                    && !run_cancellation.is_cancelled()
                {
                    retry_attempts += 1;
                    let wait = crate::provider_retry::delay(retry_attempts);
                    if print_output {
                        println!(
                            "\n⚠ Model request failed: {error}\n↻ Retrying in {}s (attempt {retry_attempts} of {allowed})",
                            wait.as_secs()
                        );
                    }
                    tokio::select! {
                        () = tokio::time::sleep(wait) => {}
                        () = run_cancellation.cancelled() => return Err(error),
                    }
                    full_response.clear();
                    parser.reset();
                    cancellation_token = run_cancellation.child_token();
                    active_request_id = trigger_headless_request(
                        client.clone(),
                        config.clone(),
                        &context,
                        tx.clone(),
                        progress_tx.clone(),
                        cancellation_token.clone(),
                        session_dir.clone(),
                        request_hook.clone(),
                    )?;
                    continue;
                }
                mark_pending_python_audits(
                    &pending_structured,
                    tool_runtime,
                    crate::python::notebook::NotebookAttemptStatus::Interrupted,
                    "Provider failed after recognizing Python but before dispatch.",
                )?;
                return Err(error);
            }
            None => {
                mark_pending_python_audits(
                    &pending_structured,
                    tool_runtime,
                    crate::python::notebook::NotebookAttemptStatus::Interrupted,
                    "Provider stream ended after recognizing Python but before dispatch.",
                )?;
                break;
            }
            _ => {}
        }
    }

    Ok(AgentRun {
        text: full_response,
        requests,
        messages: context.get_messages().to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConnectionKind;

    #[tokio::test]
    async fn nested_settlement_failure_is_forwarded_to_parent_progress() {
        let (tool_tx, tool_rx) = mpsc::unbounded_channel();
        let (local_tx, mut local_rx) = mpsc::unbounded_channel();
        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
        let forwarder = tokio::spawn(forward_nested_tool_events(
            tool_rx,
            local_tx,
            Some(progress_tx),
        ));
        tool_tx
            .send(StreamEvent::RequestSettlementFailed {
                request_id: "nested-one".to_string(),
                error: "durability failed".to_string(),
                cancellation_requested: true,
            })
            .unwrap();
        drop(tool_tx);
        forwarder.await.unwrap();

        let event = progress_rx.try_recv().unwrap();
        assert!(matches!(
            event,
            StreamEvent::RequestSettlementFailed {
                request_id,
                error,
                cancellation_requested: true,
            } if request_id == "nested-one" && error == "durability failed"
        ));
        assert!(local_rx.try_recv().is_err());
    }

    #[test]
    fn accounted_run_sums_each_provider_request() {
        let request = |id: &str, input: u64| crate::accounting::ProviderRequestAccounting {
            request_id: id.to_string(),
            connection_id: "proxy".to_string(),
            model: "gpt-5.6-sol".to_string(),
            usage: crate::accounting::Usage {
                uncached_input_tokens: input,
                output_tokens: 1,
                total_input_tokens: Some(input),
                breakdown_complete: true,
                ..Default::default()
            },
            usage_reported: true,
            estimated_cost: None,
            completed: true,
            in_flight: false,
        };
        let run = AgentRun {
            text: "done".to_string(),
            requests: vec![request("one", 10), request("two", 20)],
            messages: Vec::new(),
        };

        let usage = run.total_usage();
        assert_eq!(usage.total_input(), 30);
        assert_eq!(usage.output_tokens, 2);
        assert!(usage.breakdown_complete);
    }

    #[tokio::test]
    async fn nested_accounting_reaches_parent_before_successor_and_stays_local() {
        let request = |request_id: &str| crate::accounting::ProviderRequestAccounting {
            request_id: request_id.to_string(),
            connection_id: "proxy".to_string(),
            model: "gpt-5.6-sol".to_string(),
            usage: crate::accounting::Usage::default(),
            usage_reported: false,
            estimated_cost: None,
            completed: false,
            in_flight: true,
        };
        let nested = request("nested");
        let mut nested_finished = nested.clone();
        nested_finished.in_flight = false;
        nested_finished.completed = true;
        let successor = request("successor");

        let (tool_tx, tool_rx) = mpsc::unbounded_channel();
        let (local_tx, mut local_rx) = mpsc::unbounded_channel();
        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
        let forwarder = tokio::spawn(forward_nested_tool_events(
            tool_rx,
            local_tx,
            Some(progress_tx.clone()),
        ));
        tool_tx
            .send(StreamEvent::RequestStarted(nested.clone()))
            .unwrap();
        tool_tx
            .send(StreamEvent::ToolProgress("working".to_string()))
            .unwrap();
        tool_tx
            .send(StreamEvent::RequestFinished(nested_finished.clone()))
            .unwrap();
        drop(tool_tx);
        forwarder.await.unwrap();

        // This is the next primary request's independent parent route. Nested
        // accounting must already be ahead of it even though its local copies
        // have not been drained by the headless agent loop yet.
        progress_tx
            .send(StreamEvent::RequestStarted(successor.clone()))
            .unwrap();

        let parent_events = [
            progress_rx.try_recv().unwrap(),
            progress_rx.try_recv().unwrap(),
            progress_rx.try_recv().unwrap(),
            progress_rx.try_recv().unwrap(),
        ];
        assert!(matches!(
            &parent_events[0],
            StreamEvent::RequestStarted(request) if request.request_id == "nested"
        ));
        assert!(matches!(
            &parent_events[1],
            StreamEvent::ToolProgress(message) if message == "working"
        ));
        assert!(matches!(
            &parent_events[2],
            StreamEvent::RequestFinished(request) if request.request_id == "nested"
        ));
        assert!(matches!(
            &parent_events[3],
            StreamEvent::RequestStarted(request) if request.request_id == "successor"
        ));
        assert!(progress_rx.try_recv().is_err());

        let mut app = crate::app::App::new(&Config::default());
        app.add_logical_turn_user_segment("request".to_string());
        for event in parent_events {
            match event {
                StreamEvent::RequestStarted(request) => {
                    app.begin_provider_request(request.request_id.clone());
                    app.record_provider_request(request).unwrap();
                }
                StreamEvent::RequestFinished(request) => {
                    app.record_provider_request(request).unwrap();
                }
                _ => {}
            }
        }
        assert_eq!(app.active_request_id.as_deref(), Some("successor"));
        assert!(
            app.close_provider_request_marker("successor"),
            "successor provider_request_turns marker was lost"
        );
        assert!(
            !app.close_provider_request_marker("nested"),
            "finished nested provider_request_turns marker remained open"
        );

        assert!(matches!(
            local_rx.try_recv().unwrap(),
            StreamEvent::RequestStarted(request) if request.request_id == "nested"
        ));
        assert!(matches!(
            local_rx.try_recv().unwrap(),
            StreamEvent::RequestFinished(request) if request.request_id == "nested"
        ));
        assert!(local_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn injected_python_call_in_general_mode_is_denied_before_execution() {
        let workspace = tempfile::tempdir().unwrap();
        let workspace = workspace.path().canonicalize().unwrap();
        let runtime = crate::tool_runtime::ToolRuntime::headless(workspace.clone());
        let config = Config {
            context_size: 100_000,
            ..Default::default()
        };
        let mut context = ContextManager::new(100_000, None);
        let mut current_dir = workspace.to_string_lossy().into_owned();
        let call = ToolCall {
            id: "effective-injected".to_string(),
            provider_id: Some("provider-injected".to_string()),
            function: crate::context::FunctionCall {
                name: "python".to_string(),
                arguments: serde_json::json!({
                    "code": "raise RuntimeError('must not execute')",
                    "description": "injected unavailable call"
                }),
            },
        };
        let (tx, _rx) = mpsc::unbounded_channel();
        let checkpoints = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let checkpoint_count = checkpoints.clone();
        let transcript_hook: TranscriptHook = Arc::new(move |_, _, _, _| {
            checkpoint_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(())
        });

        execute_tool_calls(
            vec![call],
            "",
            None,
            false,
            &mut context,
            &mut current_dir,
            &reqwest::Client::new(),
            &config,
            false,
            &tx,
            &None,
            CancellationToken::new(),
            &runtime,
            &workspace,
            None,
            &None,
            &Some(transcript_hook),
        )
        .await
        .unwrap();

        let messages = context.get_messages();
        assert!(messages.iter().any(|message| message.role == "assistant"));
        let result = messages
            .iter()
            .find(|message| message.role == "tool")
            .unwrap();
        assert!(result.tool_result_is_error);
        assert!(result.content.contains("not allowed"));
        assert!(checkpoints.load(std::sync::atomic::Ordering::Relaxed) >= 2);
        assert!(!runtime.is_running().await);
        let notebook: serde_json::Value = serde_json::from_slice(
            &std::fs::read(runtime.python_notebook_path().unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            notebook["cells"][0]["metadata"]["lethetic"]["status"],
            "denied"
        );
    }

    #[tokio::test]
    async fn parallel_headless_batch_is_rejected_before_any_side_effect() {
        let workspace = tempfile::tempdir().unwrap();
        let workspace = workspace.path().canonicalize().unwrap();
        let sentinel = workspace.join("must-not-exist");
        let runtime = crate::tool_runtime::ToolRuntime::headless(workspace.clone());
        let config = Config {
            context_size: 100_000,
            ..Default::default()
        };
        let calls = vec![
            ToolCall {
                id: "shell-call".to_string(),
                provider_id: None,
                function: crate::context::FunctionCall {
                    name: "run_shell_command".to_string(),
                    arguments: serde_json::json!({
                        "command": format!("touch {}", sentinel.display()),
                        "description": "must be rejected"
                    }),
                },
            },
            ToolCall {
                id: "calculation-call".to_string(),
                provider_id: None,
                function: crate::context::FunctionCall {
                    name: "calculate".to_string(),
                    arguments: serde_json::json!({"expression": "2 + 2"}),
                },
            },
        ];
        let mut context = ContextManager::new(100_000, None);
        let mut current_dir = workspace.to_string_lossy().into_owned();
        let (tx, _rx) = mpsc::unbounded_channel();

        execute_tool_calls(
            calls,
            "",
            None,
            false,
            &mut context,
            &mut current_dir,
            &reqwest::Client::new(),
            &config,
            false,
            &tx,
            &None,
            CancellationToken::new(),
            &runtime,
            &workspace,
            None,
            &None,
            &None,
        )
        .await
        .unwrap();

        assert!(!sentinel.exists());
        let results = context
            .get_messages()
            .iter()
            .filter(|message| message.role == "tool")
            .collect::<Vec<_>>();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|result| result.tool_result_is_error));
        assert!(
            results
                .iter()
                .all(|result| result.content.contains("entire batch"))
        );
    }

    #[test]
    fn native_transport_never_executes_quoted_text_tool_markers() {
        let marker = r#"<|tool_call>{"name":"todowrite","args":{"todos":[],"tool_call_id":"quoted"}}<tool_call|>"#;
        assert!(parser::find_tool_call(marker, true).is_some());

        let mut native = Config::default();
        native.connection_kind = ConnectionKind::ClaudeCodeProxy;
        assert!(find_legacy_text_tool_call(&native, marker, true).is_none());

        let textual = Config::default();
        assert!(find_legacy_text_tool_call(&textual, marker, true).is_some());
    }
}
