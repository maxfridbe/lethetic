use super::session_state::validate_session_uuid;
use super::*;
use std::env;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSummary {
    pub session_id: String,
    pub display_name: Option<String>,
    pub fallback_label: String,
    path: String,
    /// Estimated tokens in the context window at last save (0 when unknown).
    pub context_tokens: usize,
    /// Cumulative prompt/completion tokens reported across the session.
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Cumulative API-equivalent estimate in nano-units of currency, when priced.
    pub estimated_cost_nanos: Option<u64>,
    /// Model, Agent Mode and remote control recorded with the session.
    pub details: String,
}

impl SessionSummary {
    pub(super) fn stats_from_state(state: &SessionState) -> (usize, u64, u64, Option<u64>) {
        let context_tokens = state
            .messages
            .iter()
            .map(|message| message.content.len() / 4)
            .sum();
        let totals = &state.accounting.session;
        (
            context_tokens,
            totals.usage.total_input(),
            totals.usage.output_tokens,
            totals.estimated_cost.as_ref().map(|cost| cost.nanos),
        )
    }

    pub(super) fn details_from_state(state: &SessionState) -> String {
        let mut parts = Vec::new();
        if !state.model_name.is_empty() {
            parts.push(format!("model {}", state.model_name));
        }
        if let Some(policy) = &state.python_policy {
            parts.push(describe_python_policy(policy));
        }
        if let Some(target) = &state.remote_control {
            parts.push(format!("rc {target}"));
        }
        parts.join(" · ")
    }

    fn stats_suffix(&self) -> String {
        let mut suffix = String::new();
        if self.context_tokens > 0 {
            suffix.push_str(&format!(
                "  ctx:{}",
                crate::status_summary::format_tokens(self.context_tokens as u64)
            ));
        }
        if self.input_tokens > 0 || self.output_tokens > 0 {
            suffix.push_str(&format!(
                "  [in:{} out:{}",
                crate::status_summary::format_tokens(self.input_tokens),
                crate::status_summary::format_tokens(self.output_tokens)
            ));
            if let Some(nanos) = self.estimated_cost_nanos {
                suffix.push_str(&format!("  ${:.4}", nanos as f64 / 1e9));
            }
            suffix.push(']');
        }
        suffix
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SessionCleanupTarget {
    session_id: String,
    fallback_label: String,
    path: String,
}

#[cfg(target_os = "linux")]
#[derive(Clone, Debug, PartialEq, Eq)]
enum SessionListCandidate {
    Resumable(SessionSummary),
    CleanupOnly(SessionCleanupTarget),
}

#[cfg(target_os = "linux")]
impl SessionListCandidate {
    fn session_id(&self) -> &str {
        match self {
            Self::Resumable(summary) => &summary.session_id,
            Self::CleanupOnly(target) => &target.session_id,
        }
    }
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionDeletionKind {
    Resumable,
    CleanupOnly,
}

#[cfg(target_os = "linux")]
#[derive(Clone, Debug, PartialEq, Eq)]
struct SessionDeletionTarget {
    session_id: String,
    path: String,
    kind: SessionDeletionKind,
}

#[cfg(target_os = "linux")]
struct SessionDeletionPlan {
    lease: std::sync::Arc<crate::session_store::SessionLease>,
    session_id: String,
    runtime_id: Option<String>,
    workspace: Option<crate::python::runtime_store::WorkspaceIdentity>,
    deleting_active: bool,
}

fn abbreviated_session_id(session_id: &str) -> &str {
    session_id.get(..8).unwrap_or(session_id)
}

impl SessionSummary {
    pub fn label(&self, current_session_id: &str) -> String {
        let current = if self.session_id == current_session_id {
            "▶ "
        } else {
            "  "
        };
        let base = match &self.display_name {
            Some(name) => format!(
                "{current}{name}  [{}]",
                abbreviated_session_id(&self.session_id)
            ),
            None => format!("{current}{}", self.fallback_label),
        };
        format!("{base}{}", self.stats_suffix())
    }
}

pub(super) fn new_session_id() -> String {
    uuid::Uuid::new_v4().hyphenated().to_string()
}

#[cfg(target_os = "linux")]
fn valid_session_fallback_label(path: &std::path::Path) -> Option<String> {
    let label = path.file_name()?.to_str()?;
    if !label.starts_with("session_")
        || label.len() > 128
        || !label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return None;
    }
    Some(label.to_string())
}

#[cfg(target_os = "linux")]
fn split_unique_session_candidates(
    candidates: Vec<SessionListCandidate>,
) -> (Vec<SessionSummary>, Vec<SessionCleanupTarget>) {
    let mut id_counts = std::collections::HashMap::new();
    for candidate in &candidates {
        *id_counts
            .entry(candidate.session_id().to_string())
            .or_insert(0usize) += 1;
    }
    let mut summaries = Vec::new();
    let mut cleanup_targets = Vec::new();
    for candidate in candidates {
        if id_counts.get(candidate.session_id()).copied() != Some(1) {
            continue;
        }
        match candidate {
            SessionListCandidate::Resumable(summary) => summaries.push(summary),
            SessionListCandidate::CleanupOnly(target) => cleanup_targets.push(target),
        }
    }
    summaries.sort_by(|left, right| right.fallback_label.cmp(&left.fallback_label));
    cleanup_targets.sort_by(|left, right| right.fallback_label.cmp(&left.fallback_label));
    (summaries, cleanup_targets)
}

#[cfg(target_os = "linux")]
fn complete_loaded_session_creation_transaction(
    committed: &mut bool,
    needs_migration_save: bool,
    save_migrated_state: impl FnOnce() -> Result<(), String>,
    commit_creation: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    *committed = false;
    if needs_migration_save {
        save_migrated_state()?;
    }
    commit_creation()?;
    *committed = true;
    Ok(())
}

#[cfg(target_os = "linux")]
fn commit_loaded_session_creation(
    store: &crate::session_store::SessionStore,
    path: &str,
    state: &SessionState,
    lease: &crate::session_store::SessionLease,
) -> Result<bool, String> {
    let mut committed = false;
    complete_loaded_session_creation_transaction(
        &mut committed,
        state.needs_migration_save,
        || state.save_to_directory_checked(path),
        || store.commit_locked_session_creation(lease),
    )?;
    Ok(committed)
}

impl App {
    #[cfg(target_os = "linux")]
    pub fn acquire_session_path_lock(
        &self,
        path: &str,
    ) -> Result<crate::session_store::SessionPathLock, String> {
        self.session_store
            .as_ref()
            .ok_or_else(|| "secure session storage is unavailable".to_string())?
            .try_lock_session_path(std::path::Path::new(path))?
            .ok_or_else(|| "session is already active in another Lethetic process".to_string())
    }

    #[cfg(target_os = "linux")]
    pub fn install_loaded_session_identity(
        &mut self,
        path: &str,
        state: &SessionState,
        lease: std::sync::Arc<crate::session_store::SessionLease>,
    ) -> Result<(), String> {
        let session_id = state
            .session_id
            .as_deref()
            .ok_or_else(|| "checked session state has no session ID".to_string())?;
        let binding = state
            .session_directory_binding
            .as_ref()
            .ok_or_else(|| "checked session state has no directory binding".to_string())?;
        lease.verify(std::path::Path::new(path), session_id, binding)?;
        let session_creation_committed = {
            let store = self.session_store_ref()?;
            commit_loaded_session_creation(store, path, state, &lease)?
        };
        self.tool_runtime
            .bind_python_notebook_session_lease(&lease)?;
        self.current_session_dir = Some(
            lease
                .canonical_path()
                .to_str()
                .ok_or_else(|| "session directory must be UTF-8".to_string())?
                .to_string(),
        );
        self.session_id = session_id.to_string();
        self.session_directory_binding = Some(binding.clone());
        self.session_lease = Some(lease);
        self.session_creation_committed = session_creation_committed;
        self.active_logical_turn_id = None;
        self.active_request_id = None;
        self.active_cancellation_id = None;
        self.provider_request_turns.clear();
        self.partial_assistant_checkpoint = None;
        Ok(())
    }

    /// Clears request-local display state and positions the current transcript at
    /// its rendered tail after any session transition that replaces the blocks.
    pub fn reset_session_view_state(&mut self) {
        self.tokens_per_s = 0.0;
        self.pp_tokens_per_s = 0.0;
        self.server_prompt_tokens = None;
        self.server_completion_tokens = None;
        self.server_usage = None;
        self.request_start_time = None;
        self.active_logical_turn_id = None;
        self.active_request_id = None;
        self.active_cancellation_id = None;
        self.provider_request_turns.clear();
        self.partial_assistant_checkpoint = None;
        self.reset_transcript_view_to_tail();
    }

    #[cfg(target_os = "linux")]
    pub async fn resume_registered_session(&mut self, session_id: &str) -> Result<(), String> {
        self.resume_registered_session_with_cancellation(
            session_id,
            tokio_util::sync::CancellationToken::new(),
        )
        .await
    }

    #[cfg(target_os = "linux")]
    pub async fn resume_registered_session_with_cancellation(
        &mut self,
        session_id: &str,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> Result<(), String> {
        if cancellation.is_cancelled() {
            return Err("Session restoration was cancelled".to_string());
        }
        validate_session_uuid(session_id, "session ID")?;
        if self.current_session_dir.is_some() && self.session_id == session_id {
            self.show_session_manager = false;
            self.stop_reason = format!("Session {session_id} is already active");
            self.should_redraw = true;
            return Ok(());
        }
        self.save_session_checked()?;
        let lease = std::sync::Arc::new(
            self.session_store
                .as_ref()
                .ok_or_else(|| "secure session storage is unavailable".to_string())?
                .lock_registered_session(session_id)?,
        );
        let path = lease
            .canonical_path()
            .to_str()
            .ok_or_else(|| "session directory must be UTF-8".to_string())?
            .to_string();
        let mut state = SessionState::load_checked(&path)?;
        if state.session_id.as_deref() != Some(session_id) {
            return Err("registered session state contains a different session ID".to_string());
        }
        match &state.session_directory_binding {
            Some(binding) if binding != lease.binding() => {
                return Err(
                    "registered session state does not match its directory registry".to_string(),
                );
            }
            Some(_) => {}
            None => {
                state.session_directory_binding = Some(lease.binding().clone());
                state.needs_migration_save = true;
            }
        }
        if cancellation.is_cancelled() {
            return Err("Session restoration was cancelled".to_string());
        }
        self.tool_runtime.unbind_session_checked().await?;
        if cancellation.is_cancelled() {
            return Err("Session restoration was cancelled".to_string());
        }
        self.install_loaded_session_identity(&path, &state, lease)?;

        let SessionState {
            display_name,
            python_runtime_id,
            managed_python_workspace,
            shared_python_workspace,
            messages,
            blocks,
            history,
            theme_name,
            accounting,
            needs_migration_save,
            hide_thinking,
            connection_id,
            model_name,
            python_policy,
            loop_mode,
            system_prompt,
            ..
        } = state;
        self.pending_session_settings = Some(SessionSettings {
            system_prompt,
            connection_id,
            model_name,
            python_policy,
            loop_mode,
        });
        self.display_name = display_name;
        self.python_runtime_id = python_runtime_id;
        self.managed_python_workspace = managed_python_workspace;
        self.shared_python_workspace = shared_python_workspace;
        self.accounting = accounting;
        self.blocks = blocks;
        self.logical_turn_usage = self
            .blocks
            .iter()
            .rev()
            .find(|block| block.block_type == BlockType::User)
            .and_then(|block| block.usage);
        self.adopt_global_history(history);
        self.hide_thinking = hide_thinking;
        if !theme_name.is_empty()
            && theme_name != self.theme.name
            && let Some(index) = self
                .themes
                .iter()
                .position(|theme| theme.name == theme_name)
        {
            self.theme = self.themes[index].clone();
            self.theme_state.select(Some(index));
            for block in &mut self.blocks {
                block.invalidate();
            }
        }
        self.context_manager.clear();
        self.context_manager.set_messages(messages);
        self.reset_session_view_state();
        self.needs_save = needs_migration_save;
        if needs_migration_save {
            self.save_session_checked()?;
        }
        if self.managed_python_workspace.is_some() {
            self.restore_managed_python_session_with_cancellation(cancellation.clone())
                .await?;
        }
        self.ensure_nonlocal_python_session_with_cancellation(cancellation.clone())
            .await?;
        if cancellation.is_cancelled() {
            let cleanup = self.tool_runtime.reset_checked().await;
            return match cleanup {
                Ok(()) => Err("Session restoration was cancelled".to_string()),
                Err(cleanup) => Err(format!(
                    "Session restoration was cancelled; runtime cleanup failed: {cleanup}"
                )),
            };
        }
        self.show_session_manager = false;
        self.stop_reason = format!("Resumed session {session_id}");
        self.should_redraw = true;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    pub async fn delete_session_transaction(&mut self, session_id: &str) -> Result<bool, String> {
        self.delete_session_transaction_with_cancellation(
            session_id,
            tokio_util::sync::CancellationToken::new(),
        )
        .await
    }

    #[cfg(target_os = "linux")]
    pub async fn delete_session_transaction_with_cancellation(
        &mut self,
        session_id: &str,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> Result<bool, String> {
        if cancellation.is_cancelled() {
            return Err("session deletion was cancelled".to_string());
        }
        self.refresh_session_list();
        let target = self.session_deletion_target_for_id(session_id)?;
        self.delete_session_path_transaction(target, cancellation)
            .await
    }

    #[cfg(target_os = "linux")]
    async fn delete_session_path_transaction(
        &mut self,
        selection: SessionDeletionTarget,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> Result<bool, String> {
        if cancellation.is_cancelled() {
            return Err("session deletion was cancelled".to_string());
        }
        if !self.is_fully_idle() {
            return Err("wait for the active turn before deleting a session".to_string());
        }
        let target = std::path::Path::new(&selection.path)
            .canonicalize()
            .map_err(|error| format!("could not canonicalize session deletion target: {error}"))?;
        let plan = self.prepare_session_deletion(&selection, &target)?;
        self.execute_session_deletion(plan, cancellation).await
    }

    #[cfg(target_os = "linux")]
    fn prepare_session_deletion(
        &mut self,
        selection: &SessionDeletionTarget,
        target: &std::path::Path,
    ) -> Result<SessionDeletionPlan, String> {
        let deleting_active = self
            .session_lease
            .as_ref()
            .is_some_and(|lease| lease.canonical_path() == target);
        if deleting_active {
            return self.prepare_active_session_deletion(selection, target);
        }
        match selection.kind {
            SessionDeletionKind::CleanupOnly => {
                self.prepare_cleanup_session_deletion(selection, target)
            }
            SessionDeletionKind::Resumable => {
                self.prepare_resumable_session_deletion(selection, target)
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn prepare_active_session_deletion(
        &mut self,
        selection: &SessionDeletionTarget,
        target: &std::path::Path,
    ) -> Result<SessionDeletionPlan, String> {
        if selection.kind == SessionDeletionKind::CleanupOnly
            || self.session_id != selection.session_id
        {
            return Err("active session deletion target changed after it was selected".to_string());
        }
        self.save_session_checked()?;
        let lease = self
            .session_lease
            .as_ref()
            .ok_or_else(|| "active session lease disappeared".to_string())?
            .clone();
        let binding = self
            .session_directory_binding
            .as_ref()
            .ok_or_else(|| "active session directory binding disappeared".to_string())?;
        lease.verify(target, &self.session_id, binding)?;
        let workspace = self
            .managed_python_workspace
            .as_ref()
            .map(SessionWorkspaceBinding::to_runtime_identity)
            .transpose()?;
        Ok(SessionDeletionPlan {
            lease,
            session_id: self.session_id.clone(),
            runtime_id: self.python_runtime_id.clone(),
            workspace,
            deleting_active: true,
        })
    }

    #[cfg(target_os = "linux")]
    fn prepare_cleanup_session_deletion(
        &self,
        selection: &SessionDeletionTarget,
        target: &std::path::Path,
    ) -> Result<SessionDeletionPlan, String> {
        let store = self.session_store_ref()?;
        let lease = std::sync::Arc::new(store.lock_recoverable_session_path(target)?);
        if lease.session_id() != selection.session_id {
            return Err("cleanup session identity changed after it was selected".to_string());
        }
        self.deletion_plan_from_registered_resources(store, lease, false)
    }

    #[cfg(target_os = "linux")]
    fn prepare_resumable_session_deletion(
        &self,
        selection: &SessionDeletionTarget,
        target: &std::path::Path,
    ) -> Result<SessionDeletionPlan, String> {
        let store = self.session_store_ref()?;
        match SessionState::load_checked(&selection.path) {
            Ok(state) => {
                let session_id = state
                    .session_id
                    .as_deref()
                    .ok_or_else(|| "checked session state has no session ID".to_string())?
                    .to_string();
                if session_id != selection.session_id {
                    return Err("session identity changed after it was selected".to_string());
                }
                let lease =
                    std::sync::Arc::new(store.lock_session_for_deletion(target, &session_id)?);
                if let Some(binding) = &state.session_directory_binding
                    && binding != lease.binding()
                {
                    return Err(
                        "session deletion target does not match its identity registry".to_string(),
                    );
                }
                lease.verify(target, &session_id, lease.binding())?;
                let workspace = state
                    .managed_python_workspace
                    .as_ref()
                    .map(SessionWorkspaceBinding::to_runtime_identity)
                    .transpose()?;
                Ok(SessionDeletionPlan {
                    lease,
                    session_id,
                    runtime_id: state.python_runtime_id,
                    workspace,
                    deleting_active: false,
                })
            }
            Err(load_error) => {
                let lease = std::sync::Arc::new(
                    store
                        .lock_recoverable_session_path(target)
                        .map_err(|recovery_error| {
                            format!(
                                "could not load session state ({load_error}); no retryable deletion tombstone was available: {recovery_error}"
                            )
                        })?,
                );
                if lease.session_id() != selection.session_id {
                    return Err(
                        "recoverable session identity changed after it was selected".to_string()
                    );
                }
                self.deletion_plan_from_registered_resources(store, lease, false)
            }
        }
    }

    #[cfg(target_os = "linux")]
    pub(super) fn session_store_ref(&self) -> Result<&crate::session_store::SessionStore, String> {
        self.session_store
            .as_ref()
            .ok_or_else(|| "secure session storage is unavailable".to_string())
    }

    #[cfg(target_os = "linux")]
    fn deletion_plan_from_registered_resources(
        &self,
        store: &crate::session_store::SessionStore,
        lease: std::sync::Arc<crate::session_store::SessionLease>,
        deleting_active: bool,
    ) -> Result<SessionDeletionPlan, String> {
        let session_id = lease.session_id().to_string();
        let resources = store.deletion_resources(&lease)?;
        let workspace = resources
            .managed_python_workspace
            .as_ref()
            .map(SessionWorkspaceBinding::to_runtime_identity)
            .transpose()?;
        Ok(SessionDeletionPlan {
            lease,
            session_id,
            runtime_id: resources.python_runtime_id,
            workspace,
            deleting_active,
        })
    }

    #[cfg(target_os = "linux")]
    async fn execute_session_deletion(
        &mut self,
        plan: SessionDeletionPlan,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> Result<bool, String> {
        let SessionDeletionPlan {
            lease,
            session_id,
            runtime_id,
            workspace,
            deleting_active,
        } = plan;
        let workspace_tombstone = workspace
            .as_ref()
            .map(SessionWorkspaceBinding::from_runtime_identity);
        self.session_store_ref()?.begin_locked_session_deletion(
            &lease,
            runtime_id.as_deref(),
            workspace_tombstone.as_ref(),
        )?;
        if cancellation.is_cancelled() {
            return Err("session deletion was cancelled after its retryable tombstone".to_string());
        }

        if deleting_active {
            self.tool_runtime.unbind_session_checked().await?;
            if cancellation.is_cancelled() {
                return Err("session deletion was cancelled after safe runtime detach".to_string());
            }
        }
        if let Some(runtime_id) = &runtime_id {
            crate::python::retained_runtime::delete_retained_runtime_with_cancellation(
                runtime_id,
                &session_id,
                cancellation.child_token(),
            )
            .await?;
            if cancellation.is_cancelled() {
                return Err("session deletion was cancelled".to_string());
            }
        }
        let workspace_store = crate::python::runtime_store::ManagedWorkspaceStore::open()?;
        if let Some(workspace) = workspace {
            workspace_store.delete(&session_id, &workspace)?;
        } else if let Some(workspace) = workspace_store.load_optional(&session_id)? {
            workspace_store.delete(&session_id, &workspace)?;
        }
        if cancellation.is_cancelled() {
            return Err("session deletion was cancelled after workspace cleanup".to_string());
        }
        let removal = self.session_store_ref()?.remove_locked_session(&lease)?;

        if deleting_active {
            self.reset_deleted_active_session();
        }
        self.refresh_session_list();
        if !removal.durability_warnings.is_empty() {
            return Err(format!(
                "session data was removed, but deletion durability could not be fully confirmed: {}",
                removal.durability_warnings.join("; ")
            ));
        }
        Ok(deleting_active)
    }

    #[cfg(target_os = "linux")]
    fn reset_deleted_active_session(&mut self) {
        self.session_lease = None;
        self.session_creation_committed = false;
        self.current_session_dir = None;
        self.display_name = None;
        self.session_directory_binding = None;
        self.python_runtime_id = None;
        self.managed_python_workspace = None;
        self.shared_python_workspace = None;
        self.context_manager.clear();
        self.blocks.clear();
        self.logical_turn_usage = None;
        self.accounting = crate::accounting::SessionAccounting::default();
        self.reset_session_view_state();
        self.needs_save = false;
    }

    pub fn start_new_session(&mut self) {
        if let Err(error) = self.start_new_session_checked() {
            self.stop_reason = format!("Session creation failed: {error}");
            self.should_redraw = true;
        }
    }

    #[cfg(not(target_os = "linux"))]
    pub fn start_new_session_checked(&mut self) -> Result<(), String> {
        Err(
            "persistent session creation is disabled because secure session locking is available only on Linux"
                .to_string(),
        )
    }

    #[cfg(target_os = "linux")]
    pub fn start_new_session_checked(&mut self) -> Result<(), String> {
        if self.current_session_dir.is_some() {
            self.save_session_checked()?;
        }
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
        let session_id = new_session_id();
        let directory_name = format!("session_{}_{}", timestamp, &session_id[..8]);

        #[cfg(target_os = "linux")]
        let (session_dir, session_directory_binding, session_lease) = {
            let store = self
                .session_store
                .as_ref()
                .ok_or_else(|| "secure session storage is unavailable".to_string())?;
            let (path, lease) = store.create_locked_session(&directory_name, &session_id)?;
            let path = path
                .to_str()
                .ok_or_else(|| "session directory must be UTF-8".to_string())?
                .to_string();
            let binding = lease.binding().clone();
            (path, binding, lease)
        };

        self.current_session_dir = Some(session_dir);
        self.session_id = session_id;
        self.display_name = None;
        self.session_directory_binding = Some(session_directory_binding);
        #[cfg(target_os = "linux")]
        {
            let session_lease = std::sync::Arc::new(session_lease);
            self.tool_runtime
                .bind_python_notebook_session_lease(&session_lease)?;
            self.session_lease = Some(session_lease);
            self.session_creation_committed = false;
        }
        self.python_runtime_id = None;
        self.managed_python_workspace = None;
        self.shared_python_workspace = None;
        self.blocks.clear();
        self.blocks.push(RenderBlock {
            block_type: BlockType::Text,
            content: "New session started. Type a prompt to begin.".to_string(),
            title: None,
            success: Some(true),
            prompt_tokens: None,
            completion_tokens: None,
            usage: None,
            estimated_cost: None,
            logical_turn_id: None,
            cached_lines: None,
            cached_line_count: None,
        });
        self.context_manager.clear();
        self.logical_turn_usage = None;
        self.accounting = crate::accounting::SessionAccounting::default();
        self.reset_session_view_state();
        self.needs_save = true;
        self.save_session_checked()?;
        self.refresh_session_list();
        Ok(())
    }

    pub fn add_to_history(&mut self, text: String) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }

        // Remove if already exists to move it to the end (most recent)
        if let Some(pos) = self.history.iter().position(|x| x == trimmed) {
            self.history.remove(pos);
        }
        self.history.push(trimmed.to_string());

        // Cap across all sessions of this project and persist globally.
        let cap = Self::global_history_cap();
        if self.history.len() > cap {
            let excess = self.history.len() - cap;
            self.history.drain(..excess);
        }
        Self::persist_global_history(&self.history);
    }

    #[cfg(not(target_os = "linux"))]
    pub fn refresh_session_list(&mut self) {
        self.clear_session_list();
    }

    #[cfg(target_os = "linux")]
    pub fn refresh_session_list(&mut self) {
        let Some(store) = self.session_store.as_ref() else {
            self.clear_session_list();
            return;
        };
        let candidates = std::fs::read_dir(store.sessions_root())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| self.classify_session_candidate(store, entry.path()))
            .collect();
        let (summaries, cleanup_targets) = split_unique_session_candidates(candidates);
        self.session_summaries = summaries;
        self.session_cleanup_targets = cleanup_targets;
        self.clamp_session_selection();
    }

    fn clear_session_list(&mut self) {
        self.session_summaries.clear();
        self.session_cleanup_targets.clear();
        self.session_list_state.select(None);
    }

    #[cfg(target_os = "linux")]
    fn classify_session_candidate(
        &self,
        store: &crate::session_store::SessionStore,
        path: std::path::PathBuf,
    ) -> Option<SessionListCandidate> {
        use crate::session_store::SessionPathDisposition;

        let fallback_label = valid_session_fallback_label(&path)?;
        let classification = store.classify_session_path(&path).ok()?;
        let path_string = classification.canonical_path.to_str()?.to_string();
        let registration = classification.registration.as_ref();
        if let Some(registration) = registration
            && registration.disposition == SessionPathDisposition::CleanupOnly
        {
            return Some(SessionListCandidate::CleanupOnly(SessionCleanupTarget {
                session_id: registration.session_id.clone(),
                fallback_label,
                path: path_string,
            }));
        }
        match SessionState::load_checked(&path_string) {
            Ok(state) => {
                self.classify_loaded_session(state, &classification, fallback_label, path_string)
            }
            Err(_) => {
                self.classify_unreadable_session(&classification, fallback_label, path_string)
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn classify_loaded_session(
        &self,
        state: SessionState,
        classification: &crate::session_store::SessionPathClassification,
        fallback_label: String,
        path: String,
    ) -> Option<SessionListCandidate> {
        let (context_tokens, input_tokens, output_tokens, estimated_cost_nanos) =
            SessionSummary::stats_from_state(&state);
        let details = SessionSummary::details_from_state(&state);
        let session_id = state.session_id?;
        let registration = classification.registration.as_ref();
        if validate_session_uuid(&session_id, "session ID").is_err()
            || registration.is_some_and(|registered| registered.session_id != session_id)
        {
            return None;
        }
        if let Some(binding) = &state.session_directory_binding
            && (binding.canonical_path != classification.canonical_path
                || binding.verify_current(&session_id).is_err()
                || registration
                    .and_then(|registered| registered.binding.as_ref())
                    .is_some_and(|registered| registered != binding))
        {
            return None;
        }
        Some(SessionListCandidate::Resumable(SessionSummary {
            session_id,
            display_name: state.display_name,
            fallback_label,
            path,
            context_tokens,
            input_tokens,
            output_tokens,
            estimated_cost_nanos,
            details,
        }))
    }

    #[cfg(target_os = "linux")]
    fn classify_unreadable_session(
        &self,
        classification: &crate::session_store::SessionPathClassification,
        fallback_label: String,
        path: String,
    ) -> Option<SessionListCandidate> {
        use crate::session_store::SessionPathDisposition;

        let registration = classification.registration.as_ref();
        let active_matches = self
            .session_lease
            .as_ref()
            .zip(self.session_directory_binding.as_ref())
            .is_some_and(|(lease, binding)| {
                lease.canonical_path() == classification.canonical_path
                    && lease
                        .verify(&classification.canonical_path, &self.session_id, binding)
                        .is_ok()
            })
            && registration.is_none_or(|registered| {
                registered.session_id == self.session_id
                    && registered.disposition != SessionPathDisposition::CleanupOnly
            });
        if active_matches {
            return Some(SessionListCandidate::Resumable(SessionSummary {
                session_id: self.session_id.clone(),
                display_name: self.display_name.clone(),
                fallback_label,
                path,
                context_tokens: self.context_manager.get_token_count(),
                input_tokens: self.accounting.session.usage.total_input(),
                output_tokens: self.accounting.session.usage.output_tokens,
                estimated_cost_nanos: self
                    .accounting
                    .session
                    .estimated_cost
                    .as_ref()
                    .map(|cost| cost.nanos),
                details: String::new(),
            }));
        }
        let registration = registration
            .filter(|registered| registered.disposition == SessionPathDisposition::Creating)?;
        Some(SessionListCandidate::CleanupOnly(SessionCleanupTarget {
            session_id: registration.session_id.clone(),
            fallback_label,
            path,
        }))
    }

    #[cfg(target_os = "linux")]
    fn clamp_session_selection(&mut self) {
        if self.session_summaries.is_empty() {
            self.session_list_state.select(None);
            return;
        }
        let selected = self
            .session_list_state
            .selected()
            .unwrap_or(0)
            .min(self.session_summaries.len() - 1);
        self.session_list_state.select(Some(selected));
    }

    pub fn python_setup_is_busy(&self) -> bool {
        self.python_setup
            .as_ref()
            .is_some_and(crate::python_setup::PythonSetupDialog::is_busy)
    }

    pub fn is_idle_except_python_setup(&self) -> bool {
        !self.is_processing
            && !self.is_executing_tool
            && !self.show_approval_prompt
            && !self.is_asking_user
            && self.pending_tool_call.is_none()
            && !self.is_loading_session
            && !self.lsp_install_in_progress
            && self.provider_request_turns.is_empty()
    }

    pub fn is_fully_idle(&self) -> bool {
        !self.python_setup_is_busy() && self.is_idle_except_python_setup()
    }
    pub fn refresh_system_stats(&mut self) {
        self.cwd = env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| String::from("N/A"));
    }
}

impl App {
    pub fn current_session_label(&self) -> String {
        self.display_name.clone().unwrap_or_else(|| {
            self.current_session_dir
                .as_deref()
                .and_then(|path| std::path::Path::new(path).file_name())
                .and_then(|name| name.to_str())
                .map(str::to_string)
                .unwrap_or_else(|| format!("session {}", abbreviated_session_id(&self.session_id)))
        })
    }

    pub fn open_session_name_dialog_checked(&mut self) -> Result<(), String> {
        if !self.is_fully_idle() {
            return Err("wait for the active turn before naming the session".to_string());
        }
        if self.current_session_dir.is_none() {
            self.start_new_session_checked()?;
        }
        self.session_name_input = self.display_name.clone().unwrap_or_default();
        self.session_name_error = None;
        self.show_session_name_dialog = true;
        self.show_palette = false;
        self.should_redraw = true;
        Ok(())
    }

    pub fn set_session_display_name(&mut self, value: &str) -> Result<(), String> {
        if !self.is_fully_idle() {
            return Err("wait for the active turn before naming the session".to_string());
        }
        let display_name = normalize_session_display_name(value)?;
        if self.current_session_dir.is_none() {
            self.start_new_session_checked()?;
        }
        let previous_name = self.display_name.clone();
        let previous_needs_save = self.needs_save;
        self.display_name = display_name;
        self.needs_save = true;
        if let Err(error) = self.save_session_checked() {
            self.display_name = previous_name;
            self.needs_save = previous_needs_save;
            return Err(format!("could not save session name: {error}"));
        }
        self.refresh_session_list();
        Ok(())
    }

    pub fn session_path_for_id(&self, session_id: &str) -> Result<String, String> {
        validate_session_uuid(session_id, "session ID")?;
        let mut matches = self
            .session_summaries
            .iter()
            .filter(|summary| summary.session_id == session_id);
        let path = matches
            .next()
            .ok_or_else(|| "session ID is not present in the trusted session list".to_string())?;
        if matches.next().is_some() {
            return Err(
                "session ID appears more than once in the trusted session list".to_string(),
            );
        }
        Ok(path.path.clone())
    }

    #[cfg(target_os = "linux")]
    fn session_deletion_target_for_id(
        &self,
        session_id: &str,
    ) -> Result<SessionDeletionTarget, String> {
        validate_session_uuid(session_id, "session ID")?;
        let resumable = self
            .session_summaries
            .iter()
            .filter(|summary| summary.session_id == session_id)
            .map(|summary| SessionDeletionTarget {
                session_id: summary.session_id.clone(),
                path: summary.path.clone(),
                kind: SessionDeletionKind::Resumable,
            });
        let cleanup_only = self
            .session_cleanup_targets
            .iter()
            .filter(|target| target.session_id == session_id)
            .map(|target| SessionDeletionTarget {
                session_id: target.session_id.clone(),
                path: target.path.clone(),
                kind: SessionDeletionKind::CleanupOnly,
            });
        let mut matches = resumable.chain(cleanup_only);
        let target = matches
            .next()
            .ok_or_else(|| "session ID is not present in the trusted deletion list".to_string())?;
        if matches.next().is_some() {
            return Err(
                "session ID appears more than once in the trusted deletion list".to_string(),
            );
        }
        Ok(target)
    }

    pub fn session_ids_for_cleanup(&self) -> Vec<String> {
        self.session_summaries
            .iter()
            .map(|summary| summary.session_id.clone())
            .chain(
                self.session_cleanup_targets
                    .iter()
                    .map(|target| target.session_id.clone()),
            )
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn cached_block(line_count: usize) -> RenderBlock {
        RenderBlock {
            block_type: BlockType::Text,
            content: "loaded".to_string(),
            title: None,
            success: Some(true),
            prompt_tokens: None,
            completion_tokens: None,
            usage: None,
            estimated_cost: None,
            logical_turn_id: None,
            cached_lines: None,
            cached_line_count: Some(line_count),
        }
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn cancelled_session_deletion_stops_before_target_resolution() {
        let mut app = App::new(&Config::default());
        let original_session = app.session_id.clone();
        let cancellation = tokio_util::sync::CancellationToken::new();
        cancellation.cancel();

        let error = app
            .delete_session_transaction_with_cancellation("not-a-session-id", cancellation)
            .await
            .unwrap_err();

        assert_eq!(error, "session deletion was cancelled");
        assert_eq!(app.session_id, original_session);
    }

    #[test]
    fn loaded_view_resets_request_metrics_and_uses_rendered_tail() {
        let mut app = App::new(&Config::default());
        app.blocks = vec![cached_block(3), cached_block(5)];
        app.tokens_per_s = 12.0;
        app.pp_tokens_per_s = 7.0;
        app.server_prompt_tokens = Some(111);
        app.server_completion_tokens = Some(222);
        app.server_usage = Some(crate::accounting::Usage::default());
        app.request_start_time = Some(tokio::time::Instant::now());
        app.scroll = 42;
        app.auto_scroll = false;
        app.total_line_count = 999;
        app.output_state.select(Some(1));

        app.reset_session_view_state();

        assert_eq!(app.tokens_per_s, 0.0);
        assert_eq!(app.pp_tokens_per_s, 0.0);
        assert_eq!(app.server_prompt_tokens, None);
        assert_eq!(app.server_completion_tokens, None);
        assert_eq!(app.server_usage, None);
        assert_eq!(app.request_start_time, None);
        assert_eq!(app.scroll, 0);
        assert!(app.auto_scroll);
        assert_eq!(app.total_line_count, 8);
        assert_eq!(app.output_state.selected(), Some(7));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn new_session_after_scroll_resets_view_and_request_metrics() {
        let mut app = App::new(&Config::default());
        let previous_session_id = app.session_id.clone();
        app.tokens_per_s = 19.0;
        app.pp_tokens_per_s = 11.0;
        app.server_prompt_tokens = Some(300);
        app.server_completion_tokens = Some(400);
        app.server_usage = Some(crate::accounting::Usage::default());
        app.request_start_time = Some(tokio::time::Instant::now());
        app.scroll = 55;
        app.auto_scroll = false;
        app.total_line_count = 700;
        app.output_state.select(Some(99));

        app.start_new_session_checked().unwrap();

        assert_ne!(app.session_id, previous_session_id);
        assert_eq!(app.blocks.len(), 1);
        assert_eq!(app.tokens_per_s, 0.0);
        assert_eq!(app.pp_tokens_per_s, 0.0);
        assert_eq!(app.server_prompt_tokens, None);
        assert_eq!(app.server_completion_tokens, None);
        assert_eq!(app.server_usage, None);
        assert_eq!(app.request_start_time, None);
        assert_eq!(app.scroll, 0);
        assert!(app.auto_scroll);
        assert_eq!(app.total_line_count, 0);
        assert_eq!(app.output_state.selected(), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn deleted_active_session_keeps_reset_state_when_replacement_fails() {
        let mut app = App::new(&Config::default());
        let deleted_session_id = app.session_id.clone();
        app.tokens_per_s = 23.0;
        app.pp_tokens_per_s = 17.0;
        app.server_prompt_tokens = Some(500);
        app.server_completion_tokens = Some(600);
        app.server_usage = Some(crate::accounting::Usage::default());
        app.request_start_time = Some(tokio::time::Instant::now());
        app.scroll = 66;
        app.auto_scroll = false;
        app.total_line_count = 800;
        app.output_state.select(Some(101));

        app.reset_deleted_active_session();
        app.session_store = None;
        app.start_new_session();

        assert_eq!(app.session_id, deleted_session_id);
        assert!(app.current_session_dir.is_none());
        assert!(app.session_directory_binding.is_none());
        assert!(!app.session_creation_committed);
        assert!(app.blocks.is_empty());
        assert_eq!(app.tokens_per_s, 0.0);
        assert_eq!(app.pp_tokens_per_s, 0.0);
        assert_eq!(app.server_prompt_tokens, None);
        assert_eq!(app.server_completion_tokens, None);
        assert_eq!(app.server_usage, None);
        assert_eq!(app.request_start_time, None);
        assert_eq!(app.scroll, 0);
        assert!(app.auto_scroll);
        assert_eq!(app.total_line_count, 0);
        assert_eq!(app.output_state.selected(), None);
        assert!(app.stop_reason.starts_with("Session creation failed:"));
        assert!(app.should_redraw);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn recovered_creation_is_not_marked_committed_after_commit_failure() {
        use std::cell::RefCell;

        let events = RefCell::new(Vec::new());
        let mut committed = true;
        let failed = complete_loaded_session_creation_transaction(
            &mut committed,
            true,
            || {
                events.borrow_mut().push("save");
                Ok(())
            },
            || {
                events.borrow_mut().push("commit");
                Err("simulated lifecycle commit failure".to_string())
            },
        );
        assert!(failed.is_err());
        assert!(!committed);
        assert_eq!(*events.borrow(), vec!["save", "commit"]);

        let retried = complete_loaded_session_creation_transaction(
            &mut committed,
            false,
            || panic!("a durable retry must not rewrite an already migrated state"),
            || {
                events.borrow_mut().push("retry-commit");
                Ok(())
            },
        );
        assert!(retried.is_ok());
        assert!(committed);
        assert_eq!(*events.borrow(), vec!["save", "commit", "retry-commit"]);
    }
}
