use crate::app::{BlockType, RenderBlock, SessionState, SessionWorkspaceBinding};
use crate::config::{Config, NetworkAccess, ToolProfile};
use crate::headless::{
    AgentRun, RequestAccountingHook, ToolTranscriptEvent, TranscriptHook, TranscriptStage,
};
use crate::python::runtime_store::{ManagedWorkspaceStore, RuntimeStore};
use crate::session_store::{SessionLease, SessionStore};
use crate::tool_runtime::ToolRuntime;
use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub struct DurableHeadlessSession {
    pub session_id: String,
    pub session_path: PathBuf,
    pub workspace: PathBuf,
    pub runtime_id: Option<String>,
    pub container_id: Option<String>,
    pub audit_path: Option<PathBuf>,
    state: Arc<Mutex<SessionState>>,
    tool_runtime: ToolRuntime,
    lease: Arc<SessionLease>,
}

pub struct DurableHeadlessRun {
    pub agent: AgentRun,
    pub state: SessionState,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct HeadlessCheckpointCursor {
    /// Messages below this index have terminal, committed projections.
    committed_messages: usize,
    /// The last assistant message may still be replaced by streaming updates.
    streaming_message_index: Option<usize>,
    streaming_block_index: Option<usize>,
    enrichable_assistant_index: Option<usize>,
    pending_calls: VecDeque<ProjectedToolCall>,
    unclaimed_results: VecDeque<ProjectedToolResult>,
    projected_tool_events: HashSet<String>,
    finalized_requests: HashSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProjectedToolCall {
    id: String,
    function_name: String,
    title: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProjectedToolResult {
    id: String,
    function_name: String,
    title: String,
    block_index: usize,
}

impl HeadlessCheckpointCursor {
    fn starting_after(message_count: usize) -> Self {
        Self {
            committed_messages: message_count,
            ..Self::default()
        }
    }
}

impl DurableHeadlessSession {
    pub async fn open(
        project_root: &Path,
        config: &Config,
        create_new: bool,
        requested_session_id: Option<&str>,
    ) -> Result<Self, String> {
        if create_new == requested_session_id.is_some() {
            return Err(
                "durable headless execution requires exactly one of new session or session ID"
                    .to_string(),
            );
        }
        if let Some(error) = config.python_mode_validation_error() {
            return Err(format!("Invalid Python-only policy: {error}"));
        }
        let session_store = SessionStore::open(project_root)?;
        let (session_path, lease, mut state) = if create_new {
            let session_id = uuid::Uuid::new_v4().to_string();
            let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
            let directory_name = format!("session_{timestamp}_{}", &session_id[..8]);
            let (path, lease) =
                session_store.create_locked_session(&directory_name, &session_id)?;
            let state = SessionState {
                session_id: Some(session_id),
                session_directory_binding: Some(lease.binding().clone()),
                needs_migration_save: false,
                ..Default::default()
            };
            (path, Arc::new(lease), state)
        } else {
            let requested =
                requested_session_id.ok_or_else(|| "headless session ID is missing".to_string())?;
            let lease = Arc::new(session_store.lock_registered_session(requested)?);
            let path = lease.canonical_path().to_path_buf();
            let path_text = path
                .to_str()
                .ok_or_else(|| "session path must be UTF-8".to_string())?;
            let mut state = SessionState::load_checked(path_text)?;
            if state.session_id.as_deref() != Some(requested) {
                return Err("loaded session ID does not match --session-id".to_string());
            }
            match &state.session_directory_binding {
                Some(binding) if binding != lease.binding() => {
                    return Err(
                        "loaded session directory binding does not match its registry".to_string(),
                    );
                }
                Some(_) => {}
                None => {
                    state.session_directory_binding = Some(lease.binding().clone());
                    state.needs_migration_save = true;
                }
            }
            (path, lease, state)
        };
        state.connection_id = config.active_connection_id().map(str::to_string);
        state.model_name = config.model.clone();
        let session_id = state
            .session_id
            .as_deref()
            .ok_or_else(|| "durable session has no session ID".to_string())?
            .to_string();
        persist_state(&state, &session_path, &lease)?;
        session_store.commit_locked_session_creation(&lease)?;

        let retained = crate::config::is_exact_retained_nonlocal_python_policy(
            config.tool_profile,
            &config.python_runtime,
        );
        let (workspace, runtime_id, retained_container_id, tool_runtime) = if retained {
            use crate::config::PythonWorkspaceExposure;
            use crate::python::runtime_store::WorkspaceIdentity;

            let workspace_store = ManagedWorkspaceStore::open()?;
            let workspace = match &state.managed_python_workspace {
                Some(stored) => {
                    let expected = stored.to_runtime_identity()?;
                    let current = workspace_store.load(&session_id)?;
                    if current != expected {
                        return Err("managed workspace does not match the durable session state"
                            .to_string());
                    }
                    current
                }
                None => workspace_store.load_or_create(&session_id)?,
            };
            let had_runtime = state.python_runtime_id.is_some();
            let runtime_id = match &state.python_runtime_id {
                Some(runtime_id) => runtime_id.clone(),
                None => RuntimeStore::open()?.generate_runtime_id()?,
            };
            let project_workspace = project_root
                .canonicalize()
                .map_err(|error| format!("could not canonicalize launch workspace: {error}"))?;
            let shared_workspace = if config.python_invocation.workspace_exposure
                == PythonWorkspaceExposure::SharedLaunchCwd
            {
                let current = WorkspaceIdentity::capture(&project_workspace)?;
                match &state.shared_python_workspace {
                    Some(stored) => {
                        let expected = stored.to_runtime_identity()?;
                        if expected != current {
                            return Err(format!(
                                "Retained session workspace mismatch: session is bound to {}; Lethetic was launched from {}. Resume from the original directory or create a new session.",
                                expected.canonical_path.display(),
                                current.canonical_path.display(),
                            ));
                        }
                    }
                    None if had_runtime => {
                        return Err(
                            "existing managed Python runtime cannot be silently converted to shared launch-cwd mode; create a new session"
                                .to_string(),
                        );
                    }
                    None => {}
                }
                Some(current)
            } else {
                if let Some(stored) = &state.shared_python_workspace {
                    return Err(format!(
                        "this retained session is bound to shared cwd {}; resume it with --python-isolated-with-nonlocal-network from that directory",
                        stored.canonical_path.display(),
                    ));
                }
                None
            };
            state.managed_python_workspace =
                Some(SessionWorkspaceBinding::from_runtime_identity(&workspace));
            state.shared_python_workspace = shared_workspace
                .as_ref()
                .map(SessionWorkspaceBinding::from_runtime_identity);
            state.python_runtime_id = Some(runtime_id.clone());
            state.needs_migration_save = false;
            persist_state(&state, &session_path, &lease)?;

            let execution_workspace = shared_workspace
                .as_ref()
                .map(|identity| identity.canonical_path.clone())
                .unwrap_or_else(|| workspace.canonical_path.clone());
            let tool_runtime = ToolRuntime::headless(project_workspace);
            tool_runtime
                .bind_managed_session_with_shared(
                    session_id.clone(),
                    Some(runtime_id.clone()),
                    workspace,
                    shared_workspace,
                )
                .await?;
            tool_runtime
                .ensure_ready(config, &execution_workspace)
                .await?;
            let container_id = retained_container_id_from_notice(
                tool_runtime.take_python_runtime_notice(),
                &execution_workspace,
            )?;
            (
                execution_workspace,
                Some(runtime_id),
                Some(container_id),
                tool_runtime,
            )
        } else {
            let workspace = project_root
                .canonicalize()
                .map_err(|error| format!("could not canonicalize headless workspace: {error}"))?;
            let tool_runtime = ToolRuntime::headless(workspace.clone());
            if config.tool_profile == ToolProfile::PythonOnly {
                tool_runtime.ensure_ready(config, &workspace).await?;
            }
            (workspace, None, None, tool_runtime)
        };
        tool_runtime.bind_python_notebook_session_lease(&lease)?;

        let audit_path = if let Some(runtime_id) = &runtime_id {
            let store = RuntimeStore::open()?;
            Some(
                store
                    .root()
                    .join("python-runtimes")
                    .join(runtime_id)
                    .join("egress-audit.jsonl"),
            )
        } else {
            None
        };
        let container_id = retained_container_id;

        Ok(Self {
            session_id,
            session_path,
            workspace,
            runtime_id,
            container_id,
            audit_path,
            state: Arc::new(Mutex::new(state)),
            tool_runtime,
            lease,
        })
    }

    pub async fn run(
        self,
        prompt: String,
        client: &reqwest::Client,
        config: &Config,
        print_output: bool,
        timeout: std::time::Duration,
    ) -> Result<DurableHeadlessRun, String> {
        let logical_turn_id = format!("headless-turn-{}", uuid::Uuid::new_v4());
        {
            let mut state = lock_state(&self.state)?;
            state.history.retain(|entry| entry != prompt.trim());
            state.history.push(prompt.trim().to_string());
            if state.history.len() > 100 {
                state.history.remove(0);
            }
            let mut user_block = RenderBlock::user(prompt.clone());
            user_block.logical_turn_id = Some(logical_turn_id.clone());
            RenderBlock::push_capped(&mut state.blocks, user_block);
            state.messages.push(crate::context::Message {
                role: "user".to_string(),
                content: prompt.clone(),
                tool_calls: None,
                provider_content: None,
                tool_result_is_error: false,
            });
            persist_state(&state, &self.session_path, &self.lease)?;
        }

        let initial_messages = lock_state(&self.state)?.messages.clone();
        let checkpoint_cursor = Arc::new(Mutex::new(HeadlessCheckpointCursor::starting_after(
            initial_messages.len(),
        )));
        let checkpoints_open = Arc::new(AtomicBool::new(true));
        let state_for_hook = self.state.clone();
        let path_for_hook = self.session_path.clone();
        let lease_for_hook = self.lease.clone();
        let logical_turn_for_hook = logical_turn_id.clone();
        let cursor_for_request = checkpoint_cursor.clone();
        let open_for_request = checkpoints_open.clone();
        let request_hook: RequestAccountingHook = Arc::new(move |checkpoint| {
            if !open_for_request.load(Ordering::Acquire) {
                return Ok(());
            }
            let mut state = lock_state(&state_for_hook)?;
            if !open_for_request.load(Ordering::Acquire) {
                return Ok(());
            }
            let mut cursor = cursor_for_request
                .lock()
                .map_err(|_| "headless checkpoint cursor lock was poisoned".to_string())?;
            commit_checkpoint(
                &mut state,
                &mut cursor,
                |candidate, candidate_cursor| {
                    apply_request_checkpoint(
                        candidate,
                        &logical_turn_for_hook,
                        candidate_cursor,
                        checkpoint,
                    )
                },
                |candidate| persist_state(candidate, &path_for_hook, &lease_for_hook),
            )?;
            Ok(())
        });
        let state_for_transcript = self.state.clone();
        let path_for_transcript = self.session_path.clone();
        let lease_for_transcript = self.lease.clone();
        let cursor_for_transcript = checkpoint_cursor.clone();
        let open_for_transcript = checkpoints_open.clone();
        let transcript_hook: TranscriptHook =
            Arc::new(move |messages, stage, request_id, tool_events| {
                if !open_for_transcript.load(Ordering::Acquire) {
                    return Ok(());
                }
                let mut state = lock_state(&state_for_transcript)?;
                if !open_for_transcript.load(Ordering::Acquire) {
                    return Ok(());
                }
                let mut cursor = cursor_for_transcript
                    .lock()
                    .map_err(|_| "headless checkpoint cursor lock was poisoned".to_string())?;
                commit_checkpoint(
                    &mut state,
                    &mut cursor,
                    |candidate, candidate_cursor| {
                        apply_transcript_and_tool_checkpoint(
                            candidate,
                            candidate_cursor,
                            messages,
                            stage,
                            request_id,
                            tool_events,
                        )
                    },
                    |candidate| {
                        persist_state(candidate, &path_for_transcript, &lease_for_transcript)
                    },
                )?;
                Ok(())
            });
        let session_dir = self
            .session_path
            .to_str()
            .ok_or_else(|| "session path must be UTF-8".to_string())?
            .to_string();
        let cancellation = tokio_util::sync::CancellationToken::new();
        let mut agent_future = Box::pin(crate::headless::run_agent_accounted_with_runtime(
            prompt,
            client,
            config,
            crate::headless::AgentRunOptions {
                print_output,
                progress_tx: None,
                workspace: self.workspace.clone(),
                initial_messages,
                add_initial_user_message: false,
                session_dir: Some(session_dir),
                request_hook: Some(request_hook),
                transcript_hook: Some(transcript_hook),
                cancellation: Some(cancellation.clone()),
            },
            &self.tool_runtime,
        ));
        let agent_result = tokio::select! {
            result = &mut agent_future => result,
            _ = tokio::time::sleep(timeout) => {
                cancellation.cancel();
                let cancellation_settled = tokio::time::timeout(
                    std::time::Duration::from_secs(30),
                    &mut agent_future,
                )
                .await
                .is_ok();
                if cancellation_settled {
                    Err(format!(
                        "headless operation timed out after {}s",
                        timeout.as_secs()
                    ))
                } else {
                    Err(format!(
                        "headless operation timed out after {}s and request cancellation did not settle within 30s",
                        timeout.as_secs()
                    ))
                }
            }
        };
        drop(agent_future);
        checkpoints_open.store(false, Ordering::Release);
        let detach_result = self.tool_runtime.unbind_session_checked().await;

        let state = {
            let mut state = lock_state(&self.state)?;
            if let Ok(agent) = &agent_result {
                state.messages = agent.messages.clone();
                let final_text = RenderBlock::assistant_text_content(&agent.text);
                if !final_text.is_empty()
                    && state.blocks.last().is_none_or(|block| {
                        block.block_type != BlockType::Text || block.content != final_text
                    })
                {
                    RenderBlock::push_capped(&mut state.blocks, RenderBlock::text(final_text));
                }
            }
            let repairs =
                crate::context::repair_interrupted_tool_calls_with_details(&mut state.messages);
            crate::app::reconcile_interrupted_tool_error_blocks(&mut state.blocks, &repairs);
            apply_turn_to_user(&mut state, &logical_turn_id);
            state.needs_migration_save = false;
            persist_state(&state, &self.session_path, &self.lease)?;
            state.clone()
        };
        match (agent_result, detach_result) {
            (Ok(agent), Ok(())) => Ok(DurableHeadlessRun { agent, state }),
            (Err(error), Ok(())) => Err(error),
            (Ok(_), Err(error)) => Err(format!("Python runtime detach failed: {error}")),
            (Err(run), Err(detach)) => {
                Err(format!("{run}; Python runtime detach failed: {detach}"))
            }
        }
    }
}

fn retained_container_id_from_notice(
    notice: Option<crate::python::PythonRuntimeNotice>,
    expected_workspace: &Path,
) -> Result<String, String> {
    let notice = notice.ok_or_else(|| {
        "retained runtime preflight completed without a runtime identity notice".to_string()
    })?;
    if notice.network != NetworkAccess::Nonlocal {
        return Err("retained runtime notice has the wrong network posture".to_string());
    }
    if notice.mounted_cwd != expected_workspace {
        return Err("retained runtime notice has the wrong workspace binding".to_string());
    }
    crate::python::retained_podman::validate_container_id(&notice.container_id)?;
    Ok(notice.container_id)
}

fn persist_state(state: &SessionState, path: &Path, lease: &SessionLease) -> Result<(), String> {
    let session_id = state
        .session_id
        .as_deref()
        .ok_or_else(|| "session state has no session ID".to_string())?;
    let binding = state
        .session_directory_binding
        .as_ref()
        .ok_or_else(|| "session state has no directory binding".to_string())?;
    lease.verify(path, session_id, binding)?;
    let path = path
        .to_str()
        .ok_or_else(|| "session path must be UTF-8".to_string())?;
    state.save_to_directory_checked(path)
}

fn lock_state(
    state: &Arc<Mutex<SessionState>>,
) -> Result<std::sync::MutexGuard<'_, SessionState>, String> {
    state
        .lock()
        .map_err(|_| "durable headless session state lock was poisoned".to_string())
}

fn commit_checkpoint<Apply, Persist>(
    state: &mut SessionState,
    cursor: &mut HeadlessCheckpointCursor,
    apply: Apply,
    persist: Persist,
) -> Result<bool, String>
where
    Apply: FnOnce(&mut SessionState, &mut HeadlessCheckpointCursor) -> Result<bool, String>,
    Persist: FnOnce(&SessionState) -> Result<(), String>,
{
    let mut candidate_state = state.clone();
    let mut candidate_cursor = cursor.clone();
    if !apply(&mut candidate_state, &mut candidate_cursor)? {
        return Ok(false);
    }
    persist(&candidate_state)?;
    *state = candidate_state;
    *cursor = candidate_cursor;
    Ok(true)
}

fn apply_request_checkpoint(
    state: &mut SessionState,
    logical_turn_id: &str,
    cursor: &mut HeadlessCheckpointCursor,
    checkpoint: &crate::client::ProviderRequestCheckpoint,
) -> Result<bool, String> {
    let mut changed = state.accounting.record_request(
        checkpoint
            .request
            .clone()
            .into_logical_turn(logical_turn_id.to_string()),
    )?;
    if changed {
        apply_turn_to_user(state, logical_turn_id);
    }
    if let Some(messages) = &checkpoint.transcript {
        if messages
            .last()
            .is_none_or(|message| message.role != "assistant")
        {
            return Err(
                "terminal provider checkpoint does not end in an assistant message".to_string(),
            );
        }
        if !cursor
            .finalized_requests
            .contains(&checkpoint.request.request_id)
        {
            changed |=
                apply_transcript_checkpoint(state, cursor, messages, TranscriptStage::Final)?;
            changed |= cursor
                .finalized_requests
                .insert(checkpoint.request.request_id.clone());
        }
    }
    Ok(changed)
}

fn apply_transcript_and_tool_checkpoint(
    state: &mut SessionState,
    cursor: &mut HeadlessCheckpointCursor,
    messages: &[crate::context::Message],
    stage: TranscriptStage,
    request_id: Option<&str>,
    tool_events: &[ToolTranscriptEvent],
) -> Result<bool, String> {
    let mut changed =
        apply_scoped_transcript_checkpoint(state, cursor, messages, stage, request_id)?;
    changed |= apply_explicit_tool_events(state, cursor, tool_events)?;

    if !tool_events.is_empty() {
        // A context budget may evict the complete call/result unit before this checkpoint.
        // The typed execution event is authoritative for visible history, so synchronize the
        // provider transcript independently after projecting that event.
        if state.messages != messages {
            state.messages = messages.to_vec();
            changed = true;
        }
        cursor.committed_messages = messages.len();
        cursor.streaming_message_index = None;
        cursor.streaming_block_index = None;
        cursor.enrichable_assistant_index = None;
    }
    Ok(changed)
}

fn apply_scoped_transcript_checkpoint(
    state: &mut SessionState,
    cursor: &mut HeadlessCheckpointCursor,
    messages: &[crate::context::Message],
    stage: TranscriptStage,
    request_id: Option<&str>,
) -> Result<bool, String> {
    if request_id.is_some_and(|request_id| cursor.finalized_requests.contains(request_id)) {
        return Ok(false);
    }
    apply_transcript_checkpoint(state, cursor, messages, stage)
}

/// Project only messages committed by this run. Historical messages form an immutable anchor:
/// an exact ContextManager front-trim may rebase it, but any other rewrite is left untouched
/// rather than inventing associations for an arbitrary older session.
fn apply_transcript_checkpoint(
    state: &mut SessionState,
    cursor: &mut HeadlessCheckpointCursor,
    messages: &[crate::context::Message],
    stage: TranscriptStage,
) -> Result<bool, String> {
    let mut changed = enrich_finalized_assistant_tool_call(state, cursor, messages)?;
    let mut committed = cursor.committed_messages;
    if messages.len() < committed
        || state.messages.len() < committed
        || state.messages[..committed] != messages[..committed]
    {
        if rebase_after_context_trim(state, cursor, messages).is_none() {
            return Ok(false);
        }
        committed = cursor.committed_messages;
        changed = true;
    }

    changed |= state.messages != messages;
    let mut index = committed;
    while index < messages.len() {
        let message = &messages[index];
        if cursor
            .enrichable_assistant_index
            .is_some_and(|assistant_index| assistant_index < index)
        {
            cursor.enrichable_assistant_index = None;
        }
        let is_streaming_tail = index + 1 == messages.len()
            && stage == TranscriptStage::Intermediate
            && message.role == "assistant"
            && message.tool_calls.as_ref().is_none_or(Vec::is_empty);
        if is_streaming_tail {
            changed |= upsert_streaming_assistant(state, cursor, index, &message.content);
            break;
        }

        match message.role.as_str() {
            "assistant" => {
                changed |= project_terminal_assistant(state, cursor, index, message)?;
            }
            "tool" => {
                project_tool_result(state, cursor, message)?;
                changed = true;
            }
            // New user/system messages inside the agent loop are context-only. The explicit
            // headless user prompt is inserted before projection starts.
            _ => {}
        }
        cursor.committed_messages = index + 1;
        index += 1;
    }

    if changed {
        state.messages = messages.to_vec();
    }
    Ok(changed)
}

fn enrich_finalized_assistant_tool_call(
    state: &mut SessionState,
    cursor: &mut HeadlessCheckpointCursor,
    messages: &[crate::context::Message],
) -> Result<bool, String> {
    let Some(index) = cursor.enrichable_assistant_index else {
        return Ok(false);
    };
    if cursor.committed_messages != index + 1
        || state.messages.len() <= index
        || messages.len() <= index
        || state.messages[..index] != messages[..index]
    {
        return Ok(false);
    }
    let previous = &state.messages[index];
    let enriched = &messages[index];
    let Some(calls) = enriched
        .tool_calls
        .as_ref()
        .filter(|calls| !calls.is_empty())
    else {
        return Ok(false);
    };
    if previous.role != "assistant"
        || enriched.role != "assistant"
        || previous.content != enriched.content
        || previous
            .tool_calls
            .as_ref()
            .is_some_and(|calls| !calls.is_empty())
    {
        return Ok(false);
    }

    project_tool_calls(state, cursor, calls)?;
    state.messages[index] = enriched.clone();
    cursor.enrichable_assistant_index = None;
    Ok(true)
}

/// Accept only a front-trim of already committed context. This keeps visible history while
/// rebasing message indices onto the exact suffix retained by ContextManager.
fn rebase_after_context_trim(
    state: &SessionState,
    cursor: &mut HeadlessCheckpointCursor,
    messages: &[crate::context::Message],
) -> Option<usize> {
    let committed = cursor.committed_messages;
    if committed == 0 || state.messages.len() < committed {
        return None;
    }
    let dropped = (1..committed).find(|dropped| {
        let dropped = *dropped;
        let retained = committed - dropped;
        messages.len() >= retained && state.messages[dropped..committed] == messages[..retained]
    })?;

    cursor.committed_messages -= dropped;
    cursor.streaming_message_index = cursor
        .streaming_message_index
        .and_then(|index| index.checked_sub(dropped));
    cursor.enrichable_assistant_index = cursor
        .enrichable_assistant_index
        .and_then(|index| index.checked_sub(dropped));
    Some(dropped)
}

fn upsert_streaming_assistant(
    state: &mut SessionState,
    cursor: &mut HeadlessCheckpointCursor,
    message_index: usize,
    content: &str,
) -> bool {
    let content = RenderBlock::assistant_text_content(content);
    let mut changed = cursor.streaming_message_index != Some(message_index);
    cursor.streaming_message_index = Some(message_index);

    if let Some(block_index) = cursor.streaming_block_index
        && let Some(block) = state.blocks.get_mut(block_index)
        && block.block_type == BlockType::Text
    {
        if block.content != content {
            block.content = content;
            block.invalidate();
            changed = true;
        }
        return changed;
    }
    if content.is_empty() {
        cursor.streaming_block_index = None;
        return changed;
    }

    let block_index = push_projected_block(state, cursor, RenderBlock::text(content));
    cursor.streaming_block_index = Some(block_index);
    true
}

fn project_terminal_assistant(
    state: &mut SessionState,
    cursor: &mut HeadlessCheckpointCursor,
    message_index: usize,
    message: &crate::context::Message,
) -> Result<bool, String> {
    let content = RenderBlock::assistant_text_content(&message.content);
    let mut changed = cursor.streaming_message_index == Some(message_index);
    if cursor.streaming_message_index == Some(message_index) {
        if let Some(block_index) = cursor.streaming_block_index
            && let Some(block) = state.blocks.get_mut(block_index)
            && block.block_type == BlockType::Text
            && block.content != content
        {
            block.content = content.clone();
            block.invalidate();
            changed = true;
        } else if cursor.streaming_block_index.is_none() && !content.is_empty() {
            push_projected_block(state, cursor, RenderBlock::text(content.clone()));
            changed = true;
        }
    } else if !content.is_empty() {
        push_projected_block(state, cursor, RenderBlock::text(content));
        changed = true;
    }
    cursor.streaming_message_index = None;
    cursor.streaming_block_index = None;

    if let Some(calls) = message
        .tool_calls
        .as_ref()
        .filter(|calls| !calls.is_empty())
    {
        cursor.enrichable_assistant_index = None;
        changed |= project_tool_calls(state, cursor, calls)?;
    } else {
        cursor.enrichable_assistant_index = Some(message_index);
    }
    Ok(changed)
}

fn project_tool_calls(
    state: &mut SessionState,
    cursor: &mut HeadlessCheckpointCursor,
    calls: &[crate::context::ToolCall],
) -> Result<bool, String> {
    let mut ids = HashSet::with_capacity(calls.len());
    for call in calls {
        if !ids.insert(call.id.as_str())
            || cursor
                .pending_calls
                .iter()
                .any(|pending| pending.id == call.id && pending.function_name == call.function.name)
        {
            return Err(format!(
                "assistant checkpoint contains duplicate tool-call ID {:?}",
                call.id
            ));
        }
    }
    for call in calls {
        let block = RenderBlock::tool_call(call);
        let title = block.title.clone().unwrap_or_else(|| "Action".to_string());
        push_projected_block(state, cursor, block);
        cursor.pending_calls.push_back(ProjectedToolCall {
            id: call.id.clone(),
            function_name: call.function.name.clone(),
            title,
        });
    }
    Ok(!calls.is_empty())
}

fn project_tool_result(
    state: &mut SessionState,
    cursor: &mut HeadlessCheckpointCursor,
    message: &crate::context::Message,
) -> Result<(), String> {
    let (tool_call_id, payload) =
        crate::context::ContextManager::extract_tool_result_fields(&message.content)
            .ok_or_else(|| "tool checkpoint has no canonical tool-result envelope".to_string())?;
    let function_name = message
        .content
        .strip_prefix("<|tool_response>response:")
        .and_then(|rest| rest.split_once("{result:<|'|>"))
        .map(|(name, _)| name)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "tool checkpoint has no canonical function name".to_string())?;
    let matches = cursor
        .pending_calls
        .iter()
        .enumerate()
        .filter_map(|(index, call)| {
            (call.id == tool_call_id && call.function_name == function_name).then_some(index)
        })
        .collect::<Vec<_>>();
    let [pending_index] = matches.as_slice() else {
        return Err(format!(
            "tool checkpoint cannot be associated unambiguously with {function_name:?} call ID {tool_call_id:?}"
        ));
    };
    let pending = cursor
        .pending_calls
        .remove(*pending_index)
        .expect("matched pending tool call exists");
    let block = RenderBlock::tool_result(
        format!("\n{payload}\n"),
        pending.title.clone(),
        message.tool_result_is_error,
    );
    let block_index = push_projected_block(state, cursor, block);
    cursor.unclaimed_results.push_back(ProjectedToolResult {
        id: pending.id,
        function_name: pending.function_name,
        title: pending.title,
        block_index,
    });
    Ok(())
}

fn apply_explicit_tool_events(
    state: &mut SessionState,
    cursor: &mut HeadlessCheckpointCursor,
    events: &[ToolTranscriptEvent],
) -> Result<bool, String> {
    let mut changed = false;
    for event in events {
        if event.projection_id.is_empty() {
            return Err("headless tool projection ID must not be empty".to_string());
        }
        if cursor.projected_tool_events.contains(&event.projection_id) {
            continue;
        }

        let result_matches = cursor
            .unclaimed_results
            .iter()
            .enumerate()
            .filter_map(|(index, result)| {
                (result.id == event.call.id && result.function_name == event.call.function.name)
                    .then_some(index)
            })
            .collect::<Vec<_>>();
        match result_matches.as_slice() {
            [result_index] => {
                let result = cursor
                    .unclaimed_results
                    .remove(*result_index)
                    .expect("matched projected tool result exists");
                let block = state.blocks.get_mut(result.block_index).ok_or_else(|| {
                    "projected tool result block was evicted before its execution event".to_string()
                })?;
                *block = RenderBlock::tool_result(
                    format!("\n{}\n", event.ui),
                    result.title,
                    event.is_error,
                );
            }
            [] => {
                let pending_matches = cursor
                    .pending_calls
                    .iter()
                    .enumerate()
                    .filter_map(|(index, call)| {
                        (call.id == event.call.id && call.function_name == event.call.function.name)
                            .then_some(index)
                    })
                    .collect::<Vec<_>>();
                let title = match pending_matches.as_slice() {
                    [pending_index] => {
                        cursor
                            .pending_calls
                            .remove(*pending_index)
                            .expect("matched pending tool call exists")
                            .title
                    }
                    [] => {
                        let call_block = RenderBlock::tool_call(&event.call);
                        let title = call_block
                            .title
                            .clone()
                            .unwrap_or_else(|| "Action".to_string());
                        push_projected_block(state, cursor, call_block);
                        title
                    }
                    _ => {
                        return Err(format!(
                            "typed tool event cannot be associated unambiguously with {:?} call ID {:?}",
                            event.call.function.name, event.call.id
                        ));
                    }
                };
                let result_block =
                    RenderBlock::tool_result(format!("\n{}\n", event.ui), title, event.is_error);
                push_projected_block(state, cursor, result_block);
            }
            _ => {
                return Err(format!(
                    "typed tool event matched multiple {:?} results for call ID {:?}",
                    event.call.function.name, event.call.id
                ));
            }
        }
        cursor
            .projected_tool_events
            .insert(event.projection_id.clone());
        changed = true;
    }
    Ok(changed)
}

fn push_projected_block(
    state: &mut SessionState,
    cursor: &mut HeadlessCheckpointCursor,
    block: RenderBlock,
) -> usize {
    let removed = RenderBlock::push_capped(&mut state.blocks, block);
    if removed != 0 {
        cursor.streaming_block_index = cursor
            .streaming_block_index
            .and_then(|index| index.checked_sub(removed));
        cursor
            .unclaimed_results
            .retain(|result| result.block_index >= removed);
        for result in &mut cursor.unclaimed_results {
            result.block_index -= removed;
        }
    }
    state.blocks.len() - 1
}

fn apply_turn_to_user(state: &mut SessionState, logical_turn_id: &str) {
    let totals = state.accounting.totals_for_logical_turn(logical_turn_id);
    if let Some(user) = state.blocks.iter_mut().find(|block| {
        block.block_type == BlockType::User
            && block.logical_turn_id.as_deref() == Some(logical_turn_id)
    }) {
        crate::app::apply_accounting_totals_to_user_block(user, &totals);
    }
}

#[cfg(test)]
mod tests;
