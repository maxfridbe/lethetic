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
            let mut state = SessionState::default();
            state.session_id = Some(session_id);
            state.session_directory_binding = Some(lease.binding().clone());
            state.needs_migration_save = false;
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
            print_output,
            None,
            &self.tool_runtime,
            self.workspace.clone(),
            initial_messages,
            false,
            Some(session_dir),
            Some(request_hook),
            Some(transcript_hook),
            Some(cancellation.clone()),
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
mod tests {
    use super::*;

    #[tokio::test]
    async fn invalid_python_policy_is_rejected_before_session_creation() {
        let temp = tempfile::tempdir().unwrap();
        let config = Config {
            tool_profile: ToolProfile::PythonOnly,
            ..Default::default()
        };

        let error = DurableHeadlessSession::open(temp.path(), &config, true, None)
            .await
            .err()
            .expect("invalid policy must be rejected");

        assert!(error.contains("Invalid Python-only policy"), "{error}");
        assert!(!temp.path().join(".lethetic").exists());
    }

    #[test]
    fn retained_preflight_uses_notice_while_runtime_lock_is_held() {
        let temp = tempfile::tempdir().unwrap();
        let store = RuntimeStore::open_at(temp.path().join("state")).unwrap();
        let runtime_id = store.generate_runtime_id().unwrap();
        let lock = store.create_locked_runtime(&runtime_id).unwrap();
        assert!(store.try_lock_runtime(&runtime_id).unwrap().is_none());

        let workspace = temp.path().join("workspace");
        let container_id = "a".repeat(64);
        let notice = crate::python::PythonRuntimeNotice {
            container_id: container_id.clone(),
            container_name: format!("lethetic-python-{runtime_id}"),
            action: crate::python::RuntimeLaunchAction::Created,
            network: NetworkAccess::Nonlocal,
            mounted_cwd: workspace.clone(),
        };
        assert_eq!(
            retained_container_id_from_notice(Some(notice.clone()), &workspace).unwrap(),
            container_id
        );
        assert!(retained_container_id_from_notice(None, &workspace).is_err());

        let mut wrong_network = notice.clone();
        wrong_network.network = NetworkAccess::Full;
        assert!(retained_container_id_from_notice(Some(wrong_network), &workspace).is_err());
        assert!(retained_container_id_from_notice(Some(notice.clone()), temp.path()).is_err());
        let mut invalid_id = notice;
        invalid_id.container_id = "not-an-id".to_string();
        assert!(retained_container_id_from_notice(Some(invalid_id), &workspace).is_err());
        drop(lock);
    }

    fn message(role: &str, content: &str) -> crate::context::Message {
        crate::context::Message {
            role: role.to_string(),
            content: content.to_string(),
            tool_calls: None,
            provider_content: None,
            tool_result_is_error: false,
        }
    }

    fn tool_call_message(
        id: &str,
        provider_id: Option<&str>,
        name: &str,
        description: &str,
        content: &str,
    ) -> crate::context::Message {
        crate::context::Message {
            role: "assistant".to_string(),
            content: content.to_string(),
            tool_calls: Some(vec![crate::context::ToolCall {
                id: id.to_string(),
                provider_id: provider_id.map(str::to_string),
                function: crate::context::FunctionCall {
                    name: name.to_string(),
                    arguments: serde_json::json!({
                        "description": description,
                        "expression": "2 + 2"
                    }),
                },
            }]),
            provider_content: provider_id.map(|provider_id| {
                vec![serde_json::json!({
                    "type": "tool_use",
                    "id": provider_id,
                    "name": name,
                    "input": {"expression": "2 + 2"}
                })]
            }),
            tool_result_is_error: false,
        }
    }

    fn tool_result_message(
        id: &str,
        name: &str,
        payload: &str,
        is_error: bool,
    ) -> crate::context::Message {
        crate::context::Message {
            role: "tool".to_string(),
            content: format!(
                "<|tool_response>response:{name}{{result:<|'|>{payload}<|'|>,tool_call_id:<|'|>{id}<|'|>}}<tool_response|><turn|>"
            ),
            tool_calls: None,
            provider_content: None,
            tool_result_is_error: is_error,
        }
    }

    fn tool_event(
        projection_id: &str,
        call_message: &crate::context::Message,
        ui: &str,
        is_error: bool,
    ) -> ToolTranscriptEvent {
        ToolTranscriptEvent {
            projection_id: projection_id.to_string(),
            call: call_message.tool_calls.as_ref().unwrap()[0].clone(),
            ui: ui.to_string(),
            is_error,
        }
    }

    fn started_request(request_id: &str) -> crate::accounting::ProviderRequestAccounting {
        crate::accounting::ProviderRequestAccounting {
            request_id: request_id.to_string(),
            connection_id: "proxy".to_string(),
            model: "test-model".to_string(),
            usage: Default::default(),
            usage_reported: false,
            estimated_cost: None,
            completed: false,
            in_flight: true,
        }
    }

    #[test]
    fn streaming_checkpoint_upserts_one_partial_assistant_block() {
        let mut state = SessionState::default();
        state.blocks.push(RenderBlock::user("prompt"));
        state.messages = vec![message("user", "prompt")];
        let mut cursor = HeadlessCheckpointCursor::starting_after(1);
        let mut messages = vec![message("user", "prompt"), message("assistant", "par")];

        assert!(
            apply_transcript_checkpoint(
                &mut state,
                &mut cursor,
                &messages,
                TranscriptStage::Intermediate,
            )
            .unwrap()
        );
        let first_index = cursor.streaming_block_index.unwrap();
        assert_eq!(state.blocks[first_index].content, "par");
        let block_count = state.blocks.len();

        messages[1].content = "partial response".to_string();
        assert!(
            apply_transcript_checkpoint(
                &mut state,
                &mut cursor,
                &messages,
                TranscriptStage::Intermediate,
            )
            .unwrap()
        );
        assert_eq!(state.blocks.len(), block_count);
        assert_eq!(state.blocks[first_index].content, "partial response");
        assert_eq!(state.messages.last().unwrap().content, "partial response");

        assert!(
            apply_transcript_checkpoint(
                &mut state,
                &mut cursor,
                &messages,
                TranscriptStage::Final,
            )
            .unwrap()
        );
        assert!(cursor.streaming_block_index.is_none());
        assert_eq!(cursor.committed_messages, 2);
        assert_eq!(state.blocks.len(), block_count);
        assert!(
            !apply_transcript_checkpoint(
                &mut state,
                &mut cursor,
                &messages,
                TranscriptStage::Final,
            )
            .unwrap()
        );
    }

    #[test]
    fn terminal_checkpoint_applies_usage_and_tool_transcript_together() {
        let logical_turn_id = "turn-one";
        let mut state = SessionState::default();
        let mut user = RenderBlock::user("prompt");
        user.logical_turn_id = Some(logical_turn_id.to_string());
        state.blocks.push(user);
        state.messages = vec![message("user", "prompt")];
        let mut cursor = HeadlessCheckpointCursor::starting_after(1);
        let started = started_request("request-one");
        assert!(
            apply_request_checkpoint(
                &mut state,
                logical_turn_id,
                &mut cursor,
                &crate::client::ProviderRequestCheckpoint {
                    request: started.clone(),
                    transcript: None,
                },
            )
            .unwrap()
        );

        let partial = vec![message("user", "prompt"), message("assistant", "working")];
        assert!(
            apply_transcript_checkpoint(
                &mut state,
                &mut cursor,
                &partial,
                TranscriptStage::Intermediate,
            )
            .unwrap()
        );
        let usage = crate::accounting::Usage {
            uncached_input_tokens: 11,
            output_tokens: 4,
            total_input_tokens: Some(11),
            breakdown_complete: true,
            ..Default::default()
        };
        let mut finished = started;
        finished.in_flight = false;
        finished.completed = true;
        finished.usage_reported = true;
        finished.usage = usage;
        let assistant = tool_call_message(
            "effective-call",
            Some("toolu_native"),
            "python",
            "Run Python",
            "working",
        );
        let terminal = crate::client::ProviderRequestCheckpoint {
            request: finished,
            transcript: Some(vec![message("user", "prompt"), assistant.clone()]),
        };

        assert!(
            apply_request_checkpoint(&mut state, logical_turn_id, &mut cursor, &terminal).unwrap()
        );
        assert_eq!(state.messages.last(), Some(&assistant));
        assert_eq!(
            state.messages.last().unwrap().tool_calls.as_ref().unwrap()[0]
                .provider_id
                .as_deref(),
            Some("toolu_native")
        );
        assert_eq!(state.accounting.requests.len(), 1);
        assert!(!state.accounting.requests[0].in_flight);
        assert_eq!(state.accounting.requests[0].usage, usage);
        assert_eq!(state.blocks[0].usage, Some(usage));
        assert!(cursor.streaming_block_index.is_none());
        assert_eq!(state.blocks.last().unwrap().block_type, BlockType::ToolCall);
        assert_eq!(
            state.blocks.last().unwrap().title.as_deref(),
            Some("Run Python")
        );
    }

    #[test]
    fn legacy_terminal_text_is_enriched_and_uses_typed_ui_result_once() {
        let logical_turn_id = "legacy-turn";
        let request_id = "legacy-request";
        let legacy_content = "I will calculate.\n<|tool_call>legacy payload<tool_call|>";
        let mut state = SessionState::default();
        let mut user = RenderBlock::user("prompt");
        user.logical_turn_id = Some(logical_turn_id.to_string());
        state.blocks.push(user);
        state.messages = vec![message("user", "prompt")];
        let mut cursor = HeadlessCheckpointCursor::starting_after(1);
        let started = started_request(request_id);
        apply_request_checkpoint(
            &mut state,
            logical_turn_id,
            &mut cursor,
            &crate::client::ProviderRequestCheckpoint {
                request: started.clone(),
                transcript: None,
            },
        )
        .unwrap();
        let mut finished = started;
        finished.in_flight = false;
        finished.completed = true;
        apply_request_checkpoint(
            &mut state,
            logical_turn_id,
            &mut cursor,
            &crate::client::ProviderRequestCheckpoint {
                request: finished,
                transcript: Some(vec![
                    message("user", "prompt"),
                    message("assistant", legacy_content),
                ]),
            },
        )
        .unwrap();
        assert_eq!(state.blocks.len(), 2);

        let structured = tool_call_message(
            "legacy-effective",
            None,
            "calculate",
            "Legacy calculation",
            legacy_content,
        );
        let structured_checkpoint = vec![message("user", "prompt"), structured.clone()];
        assert!(
            apply_transcript_and_tool_checkpoint(
                &mut state,
                &mut cursor,
                &structured_checkpoint,
                TranscriptStage::Intermediate,
                None,
                &[],
            )
            .unwrap()
        );
        assert_eq!(state.blocks.len(), 3);
        assert_eq!(state.blocks[2].block_type, BlockType::ToolCall);

        let mut result_checkpoint = structured_checkpoint;
        result_checkpoint.push(tool_result_message(
            "legacy-effective",
            "calculate",
            "MODEL CONTEXT RESULT",
            false,
        ));
        let event = tool_event("legacy-projection", &structured, "VISIBLE UI RESULT", false);
        assert!(
            apply_transcript_and_tool_checkpoint(
                &mut state,
                &mut cursor,
                &result_checkpoint,
                TranscriptStage::Intermediate,
                None,
                std::slice::from_ref(&event),
            )
            .unwrap()
        );
        assert_eq!(state.blocks.len(), 4);
        assert_eq!(state.blocks[3].block_type, BlockType::ToolResult);
        assert_eq!(state.blocks[3].content, "\nVISIBLE UI RESULT\n");
        assert!(!state.blocks[3].content.contains("MODEL CONTEXT RESULT"));

        assert!(
            !apply_transcript_and_tool_checkpoint(
                &mut state,
                &mut cursor,
                &result_checkpoint,
                TranscriptStage::Intermediate,
                None,
                &[event],
            )
            .unwrap()
        );
        assert_eq!(state.blocks.len(), 4);
    }

    #[test]
    fn canonical_large_host_ui_survives_durable_projection() {
        let workspace = tempfile::tempdir().unwrap();
        let call_message = tool_call_message(
            "large-effective",
            Some("large-native"),
            "calculate",
            "Large result",
            "",
        );
        let presented = crate::tools::present_tool_execution_in(
            workspace.path(),
            "large-effective",
            crate::tools::ToolExecution {
                output: format!("HOST_HEAD{}HOST_TAIL", "x".repeat(30_000)),
                cwd: workspace.path().to_string_lossy().into_owned(),
                is_error: false,
                provenance: crate::tools::ToolOutputProvenance::OrdinaryHost,
            },
        );
        assert_ne!(presented.context, presented.ui);
        let expected_ui = presented.ui.clone();

        let mut state = SessionState::default();
        state.messages = vec![message("user", "prompt")];
        state.blocks.push(RenderBlock::user("prompt"));
        let mut cursor = HeadlessCheckpointCursor::starting_after(1);
        let call_checkpoint = vec![message("user", "prompt"), call_message.clone()];
        apply_transcript_checkpoint(
            &mut state,
            &mut cursor,
            &call_checkpoint,
            TranscriptStage::Final,
        )
        .unwrap();
        let mut result_checkpoint = call_checkpoint;
        result_checkpoint.push(tool_result_message(
            "large-effective",
            "calculate",
            &presented.context,
            false,
        ));
        let event = tool_event("large-projection", &call_message, &presented.ui, false);
        apply_transcript_and_tool_checkpoint(
            &mut state,
            &mut cursor,
            &result_checkpoint,
            TranscriptStage::Intermediate,
            None,
            &[event],
        )
        .unwrap();

        let resumed: SessionState =
            serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        assert_eq!(
            resumed.blocks.last().unwrap().content,
            format!("\n{expected_ui}\n")
        );
        assert!(
            !resumed
                .blocks
                .last()
                .unwrap()
                .content
                .contains("Use `summarize_content`")
        );
    }

    #[test]
    fn finalized_request_rejects_stale_partial_without_regressing_tool_metadata() {
        let logical_turn_id = "turn-one";
        let request_id = "request-one";
        let mut state = SessionState::default();
        let mut user = RenderBlock::user("prompt");
        user.logical_turn_id = Some(logical_turn_id.to_string());
        state.blocks.push(user);
        state.messages = vec![message("user", "prompt")];
        let mut cursor = HeadlessCheckpointCursor::starting_after(1);
        let started = started_request(request_id);
        apply_request_checkpoint(
            &mut state,
            logical_turn_id,
            &mut cursor,
            &crate::client::ProviderRequestCheckpoint {
                request: started.clone(),
                transcript: None,
            },
        )
        .unwrap();
        let terminal_assistant = tool_call_message(
            "effective-call",
            Some("toolu_native"),
            "python",
            "Run Python",
            "working",
        );
        let mut finished = started;
        finished.in_flight = false;
        finished.completed = true;
        apply_request_checkpoint(
            &mut state,
            logical_turn_id,
            &mut cursor,
            &crate::client::ProviderRequestCheckpoint {
                request: finished,
                transcript: Some(vec![message("user", "prompt"), terminal_assistant.clone()]),
            },
        )
        .unwrap();

        let terminal_messages = state.messages.clone();
        let terminal_blocks = state.blocks.len();
        let stale_partial = vec![message("user", "prompt"), message("assistant", "work")];
        assert!(
            !apply_scoped_transcript_checkpoint(
                &mut state,
                &mut cursor,
                &stale_partial,
                TranscriptStage::Intermediate,
                Some(request_id),
            )
            .unwrap()
        );
        assert_eq!(state.messages, terminal_messages);
        assert_eq!(state.messages.last(), Some(&terminal_assistant));
        assert_eq!(state.blocks.len(), terminal_blocks);
        assert!(cursor.streaming_block_index.is_none());

        let mut next_partial = terminal_messages;
        next_partial.push(tool_result_message(
            "effective-call",
            "python",
            "Out[1]:\n4",
            false,
        ));
        next_partial.push(message("assistant", "next partial"));
        assert!(
            apply_scoped_transcript_checkpoint(
                &mut state,
                &mut cursor,
                &next_partial,
                TranscriptStage::Intermediate,
                Some("request-two"),
            )
            .unwrap()
        );
        assert_eq!(state.messages.last().unwrap().content, "next partial");
        assert!(cursor.streaming_block_index.is_some());
        assert_eq!(
            state.blocks[state.blocks.len() - 2].block_type,
            BlockType::ToolResult
        );
    }

    #[test]
    fn projects_call_result_error_in_order_and_replay_is_idempotent() {
        let mut state = SessionState::default();
        state.messages = vec![message("user", "prompt")];
        state.blocks.push(RenderBlock::user("prompt"));
        let mut cursor = HeadlessCheckpointCursor::starting_after(1);

        let first_call = tool_call_message(
            "effective-one",
            Some("native-one"),
            "calculate",
            "First calculation",
            "",
        );
        let first_checkpoint = vec![message("user", "prompt"), first_call];
        assert!(
            apply_transcript_checkpoint(
                &mut state,
                &mut cursor,
                &first_checkpoint,
                TranscriptStage::Final,
            )
            .unwrap()
        );
        assert_eq!(state.blocks.last().unwrap().block_type, BlockType::ToolCall);

        let mut first_result = first_checkpoint;
        first_result.push(tool_result_message(
            "effective-one",
            "calculate",
            "4",
            false,
        ));
        assert!(
            apply_transcript_checkpoint(
                &mut state,
                &mut cursor,
                &first_result,
                TranscriptStage::Intermediate,
            )
            .unwrap()
        );
        let blocks_after_first = state.blocks.len();
        assert_eq!(
            state.blocks.last().unwrap().block_type,
            BlockType::ToolResult
        );
        assert_eq!(
            state.blocks.last().unwrap().title.as_deref(),
            Some("First calculation")
        );
        assert!(
            !apply_transcript_checkpoint(
                &mut state,
                &mut cursor,
                &first_result,
                TranscriptStage::Intermediate,
            )
            .unwrap()
        );
        assert_eq!(state.blocks.len(), blocks_after_first);

        let mut second_call = first_result;
        second_call.push(tool_call_message(
            "effective-two",
            Some("native-two"),
            "calculate",
            "Second calculation",
            "",
        ));
        apply_transcript_checkpoint(
            &mut state,
            &mut cursor,
            &second_call,
            TranscriptStage::Final,
        )
        .unwrap();
        second_call.push(tool_result_message(
            "effective-two",
            "calculate",
            "ERROR: division failed",
            true,
        ));
        apply_transcript_checkpoint(
            &mut state,
            &mut cursor,
            &second_call,
            TranscriptStage::Intermediate,
        )
        .unwrap();

        let tail = &state.blocks[state.blocks.len() - 4..];
        assert_eq!(tail[0].block_type, BlockType::ToolCall);
        assert_eq!(tail[1].block_type, BlockType::ToolResult);
        assert_eq!(tail[2].block_type, BlockType::ToolCall);
        assert_eq!(tail[3].block_type, BlockType::ToolError);
        assert_eq!(tail[3].success, Some(false));
        assert_eq!(tail[3].title.as_deref(), Some("Second calculation"));
    }

    #[test]
    fn failed_save_rolls_back_projection_cursor_and_retries_once() {
        let mut state = SessionState::default();
        state.messages = vec![message("user", "prompt")];
        state.blocks.push(RenderBlock::user("prompt"));
        let mut cursor = HeadlessCheckpointCursor::starting_after(1);
        let original_cursor = cursor.clone();
        let checkpoint = vec![
            message("user", "prompt"),
            tool_call_message(
                "effective-call",
                Some("native-call"),
                "calculate",
                "Calculate",
                "",
            ),
        ];

        let error = commit_checkpoint(
            &mut state,
            &mut cursor,
            |candidate, candidate_cursor| {
                apply_transcript_checkpoint(
                    candidate,
                    candidate_cursor,
                    &checkpoint,
                    TranscriptStage::Final,
                )
            },
            |_| Err("injected save failure".to_string()),
        )
        .unwrap_err();
        assert_eq!(error, "injected save failure");
        assert_eq!(cursor, original_cursor);
        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.blocks.len(), 1);

        assert!(
            commit_checkpoint(
                &mut state,
                &mut cursor,
                |candidate, candidate_cursor| {
                    apply_transcript_checkpoint(
                        candidate,
                        candidate_cursor,
                        &checkpoint,
                        TranscriptStage::Final,
                    )
                },
                |_| Ok(()),
            )
            .unwrap()
        );
        assert_eq!(state.blocks.len(), 2);
        assert!(
            !commit_checkpoint(
                &mut state,
                &mut cursor,
                |candidate, candidate_cursor| {
                    apply_transcript_checkpoint(
                        candidate,
                        candidate_cursor,
                        &checkpoint,
                        TranscriptStage::Final,
                    )
                },
                |_| panic!("idempotent replay must not save"),
            )
            .unwrap()
        );
        assert_eq!(state.blocks.len(), 2);

        let result_event = tool_event("result-projection", &checkpoint[1], "VISIBLE 4", false);
        let mut result_checkpoint = checkpoint.clone();
        result_checkpoint.push(tool_result_message(
            "effective-call",
            "calculate",
            "4",
            false,
        ));
        let committed_call_cursor = cursor.clone();
        let error = commit_checkpoint(
            &mut state,
            &mut cursor,
            |candidate, candidate_cursor| {
                apply_transcript_checkpoint(
                    candidate,
                    candidate_cursor,
                    &result_checkpoint,
                    TranscriptStage::Intermediate,
                )
            },
            |_| Err("injected result save failure".to_string()),
        )
        .unwrap_err();
        assert_eq!(error, "injected result save failure");
        assert_eq!(cursor, committed_call_cursor);
        assert_eq!(state.blocks.len(), 2);

        assert!(
            commit_checkpoint(
                &mut state,
                &mut cursor,
                |candidate, candidate_cursor| {
                    apply_transcript_and_tool_checkpoint(
                        candidate,
                        candidate_cursor,
                        &result_checkpoint,
                        TranscriptStage::Intermediate,
                        None,
                        std::slice::from_ref(&result_event),
                    )
                },
                |_| Ok(()),
            )
            .unwrap()
        );
        assert_eq!(state.blocks.len(), 3);
        assert_eq!(state.blocks[2].block_type, BlockType::ToolResult);
        assert_eq!(state.blocks[2].content, "\nVISIBLE 4\n");
        assert!(cursor.projected_tool_events.contains("result-projection"));
    }

    #[test]
    fn resume_preserves_missing_history_and_projects_only_new_provenance() {
        let historical_call = tool_call_message(
            "old-effective",
            Some("old-native"),
            "calculate",
            "Old calculation",
            "",
        );
        let historical_result =
            tool_result_message("old-effective", "calculate", "old result", false);
        let mut state = SessionState::default();
        state.messages = vec![
            message("user", "old prompt"),
            historical_call,
            historical_result,
        ];
        // A legacy headless state may have no tool blocks. Do not reconstruct them without a
        // persisted projection cursor that proves their association.
        state.blocks.push(RenderBlock::user("old prompt"));
        let encoded = serde_json::to_vec(&state).unwrap();
        let mut state: SessionState = serde_json::from_slice(&encoded).unwrap();
        let mut cursor = HeadlessCheckpointCursor::starting_after(state.messages.len());
        let unchanged = state.messages.clone();
        assert!(
            !apply_transcript_checkpoint(
                &mut state,
                &mut cursor,
                &unchanged,
                TranscriptStage::Final,
            )
            .unwrap()
        );
        assert_eq!(state.blocks.len(), 1);

        state.messages.push(message("user", "new prompt"));
        state.blocks.push(RenderBlock::user("new prompt"));
        cursor = HeadlessCheckpointCursor::starting_after(state.messages.len());
        let mut forward = state.messages.clone();
        forward.push(tool_call_message(
            "new-effective",
            Some("new-native"),
            "calculate",
            "New calculation",
            "",
        ));
        forward.push(tool_result_message(
            "new-effective",
            "calculate",
            "new result",
            false,
        ));
        assert!(
            apply_transcript_checkpoint(&mut state, &mut cursor, &forward, TranscriptStage::Final,)
                .unwrap()
        );
        assert_eq!(state.blocks.len(), 4);
        assert_eq!(state.blocks[2].block_type, BlockType::ToolCall);
        assert_eq!(state.blocks[3].block_type, BlockType::ToolResult);

        let resumed: SessionState =
            serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        assert_eq!(resumed.blocks[2].block_type, BlockType::ToolCall);
        assert_eq!(resumed.blocks[3].block_type, BlockType::ToolResult);
        assert_eq!(
            resumed.messages[4].tool_calls.as_ref().unwrap()[0]
                .provider_id
                .as_deref(),
            Some("new-native")
        );
    }

    #[test]
    fn typed_tool_event_survives_complete_transcript_omission_and_successor() {
        let call_message = tool_call_message(
            "trimmed-effective",
            Some("trimmed-native"),
            "calculate",
            "Trimmed calculation",
            "",
        );
        let call_checkpoint = vec![message("user", "prompt"), call_message.clone()];
        let mut state = SessionState::default();
        state.messages = vec![message("user", "prompt")];
        state.blocks.push(RenderBlock::user("prompt"));
        let mut cursor = HeadlessCheckpointCursor::starting_after(1);
        apply_transcript_checkpoint(
            &mut state,
            &mut cursor,
            &call_checkpoint,
            TranscriptStage::Final,
        )
        .unwrap();
        assert_eq!(state.blocks.last().unwrap().block_type, BlockType::ToolCall);

        // The execution event is deliberately independent from context retention. Simulate a
        // checkpoint in which no provider message survived, then continue from that snapshot.
        let omitted_messages = Vec::new();
        let event = tool_event(
            "trimmed-projection",
            &call_message,
            "VISIBLE TRIMMED RESULT",
            false,
        );
        assert!(
            apply_transcript_and_tool_checkpoint(
                &mut state,
                &mut cursor,
                &omitted_messages,
                TranscriptStage::Intermediate,
                None,
                std::slice::from_ref(&event),
            )
            .unwrap()
        );
        assert!(state.messages.is_empty());
        assert_eq!(state.blocks.len(), 3);
        assert_eq!(state.blocks[1].block_type, BlockType::ToolCall);
        assert_eq!(state.blocks[2].block_type, BlockType::ToolResult);
        assert_eq!(state.blocks[2].content, "\nVISIBLE TRIMMED RESULT\n");
        assert!(
            !apply_transcript_and_tool_checkpoint(
                &mut state,
                &mut cursor,
                &omitted_messages,
                TranscriptStage::Intermediate,
                None,
                &[event],
            )
            .unwrap()
        );

        let successor = vec![message("assistant", "successor response")];
        assert!(
            apply_transcript_and_tool_checkpoint(
                &mut state,
                &mut cursor,
                &successor,
                TranscriptStage::Final,
                None,
                &[],
            )
            .unwrap()
        );
        assert_eq!(state.blocks.len(), 4);
        assert_eq!(state.blocks[3].block_type, BlockType::Text);
        assert_eq!(state.blocks[3].content, "successor response");
    }

    #[test]
    fn context_front_trim_rebases_without_replaying_visible_history() {
        let mut state = SessionState::default();
        state.messages = vec![
            message("user", "old prompt"),
            message("assistant", "old answer"),
            message("user", "current prompt"),
        ];
        state.blocks = vec![
            RenderBlock::user("old prompt"),
            RenderBlock::text("old answer"),
            RenderBlock::user("current prompt"),
        ];
        let mut cursor = HeadlessCheckpointCursor::starting_after(state.messages.len());
        let checkpoint = vec![
            message("user", "current prompt"),
            tool_call_message(
                "effective-call",
                Some("native-call"),
                "calculate",
                "Calculate",
                "",
            ),
            tool_result_message("effective-call", "calculate", "4", false),
        ];

        assert!(
            apply_transcript_checkpoint(
                &mut state,
                &mut cursor,
                &checkpoint,
                TranscriptStage::Final,
            )
            .unwrap()
        );

        assert_eq!(state.messages, checkpoint);
        assert_eq!(state.blocks.len(), 5);
        assert_eq!(state.blocks[3].block_type, BlockType::ToolCall);
        assert_eq!(state.blocks[4].block_type, BlockType::ToolResult);
    }

    #[test]
    fn projected_tool_history_respects_shared_block_cap() {
        let mut state = SessionState::default();
        state.messages = vec![message("user", "prompt")];
        state.blocks = (0..200)
            .map(|index| RenderBlock::text(format!("old-{index}")))
            .collect();
        let mut cursor = HeadlessCheckpointCursor::starting_after(1);
        let checkpoint = vec![
            message("user", "prompt"),
            tool_call_message(
                "effective-call",
                Some("native-call"),
                "calculate",
                "Calculate",
                "",
            ),
            tool_result_message("effective-call", "calculate", "4", false),
        ];

        apply_transcript_checkpoint(&mut state, &mut cursor, &checkpoint, TranscriptStage::Final)
            .unwrap();

        assert_eq!(state.blocks.len(), 200);
        assert_eq!(state.blocks[198].block_type, BlockType::ToolCall);
        assert_eq!(state.blocks[199].block_type, BlockType::ToolResult);
    }
}
