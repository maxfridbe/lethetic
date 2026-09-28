use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static LOGICAL_TURN_COUNTER: AtomicU64 = AtomicU64::new(0);

fn next_logical_turn_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let counter = LOGICAL_TURN_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("turn-{:x}-{nanos:x}-{counter:x}", std::process::id())
}

fn next_cancellation_id() -> String {
    format!("cancel-{}", uuid::Uuid::new_v4().simple())
}

pub(crate) fn apply_accounting_totals_to_user_block(
    block: &mut RenderBlock,
    totals: &crate::accounting::AccountingTotals,
) -> bool {
    if block.block_type != BlockType::User || totals.request_count == 0 {
        return false;
    }
    let mut cost = totals.estimated_cost.clone();
    if (totals.unpriced_request_count > 0 || totals.incomplete_usage_request_count > 0)
        && let Some(cost) = &mut cost
    {
        cost.incomplete = true;
    }
    let prompt_tokens = u32::try_from(totals.usage.total_input()).ok();
    let completion_tokens = u32::try_from(totals.usage.output_tokens).ok();
    let changed = block.usage != Some(totals.usage)
        || block.estimated_cost != cost
        || block.prompt_tokens != prompt_tokens
        || block.completion_tokens != completion_tokens;
    if changed {
        block.usage = Some(totals.usage);
        block.estimated_cost = cost;
        block.prompt_tokens = prompt_tokens;
        block.completion_tokens = completion_tokens;
        block.invalidate();
    }
    changed
}

fn validate_provider_transcript_checkpoint(
    current: &[crate::context::Message],
    checkpoint: &[crate::context::Message],
) -> Result<(), String> {
    let (assistant, prefix) = checkpoint
        .split_last()
        .ok_or_else(|| "provider transcript checkpoint is empty".to_string())?;
    if assistant.role != "assistant" {
        return Err(
            "provider transcript checkpoint does not end in an assistant message".to_string(),
        );
    }
    if current != prefix && current != checkpoint {
        return Err(
            "provider transcript checkpoint does not extend the launched conversation".to_string(),
        );
    }
    Ok(())
}

impl App {
    pub fn add_logical_turn_user_segment(&mut self, content: String) -> String {
        let logical_turn_id = next_logical_turn_id();
        self.add_block(content, BlockType::User, None);
        let user_block = self
            .blocks
            .last_mut()
            .expect("adding a logical turn creates a User block");
        debug_assert_eq!(user_block.block_type, BlockType::User);
        debug_assert!(user_block.logical_turn_id.is_none());
        user_block.logical_turn_id = Some(logical_turn_id.clone());
        user_block.invalidate();
        self.activate_logical_turn_accounting(logical_turn_id.clone());
        logical_turn_id
    }

    pub fn begin_logical_turn_accounting(&mut self) -> Result<String, String> {
        let user_block = self
            .blocks
            .iter_mut()
            .rev()
            .find(|block| block.block_type == BlockType::User)
            .ok_or_else(|| "logical turn has no User block".to_string())?;
        if user_block.logical_turn_id.is_some() {
            return Err("refusing to overwrite an existing User logical-turn identity".to_string());
        }
        let logical_turn_id = next_logical_turn_id();
        user_block.logical_turn_id = Some(logical_turn_id.clone());
        user_block.invalidate();
        self.activate_logical_turn_accounting(logical_turn_id.clone());
        Ok(logical_turn_id)
    }

    fn activate_logical_turn_accounting(&mut self, logical_turn_id: String) {
        self.server_prompt_tokens = None;
        self.server_completion_tokens = None;
        self.server_usage = None;
        self.logical_turn_usage = None;
        self.active_logical_turn_id = Some(logical_turn_id);
        self.active_request_id = None;
        self.active_cancellation_id = Some(next_cancellation_id());
    }

    pub fn active_cancellation_id(&self) -> Option<&str> {
        self.active_cancellation_id.as_deref()
    }

    pub fn live_cancellation_id(&self) -> Option<&str> {
        (self.is_processing
            || self.is_executing_tool
            || self.show_approval_prompt
            || self.is_asking_user
            || self.lsp_install_in_progress)
            .then(|| self.active_cancellation_id())
            .flatten()
    }

    /// Creates an independently cancellable operation outside a provider turn.
    /// Callers must settle it before starting another cancellable operation.
    pub fn begin_standalone_cancellation(&mut self) -> Result<String, String> {
        if self.active_cancellation_id.is_some() {
            return Err("another cancellable operation is already active".to_string());
        }
        let cancel_id = next_cancellation_id();
        self.active_cancellation_id = Some(cancel_id.clone());
        Ok(cancel_id)
    }

    /// Ends the logical turn and invalidates its opaque cancellation target.
    pub fn settle_logical_turn(&mut self) {
        self.abandon_queued_tool_calls("the turn ended before it ran");
        self.active_logical_turn_id = None;
        self.active_cancellation_id = None;
    }

    /// Invalidates an independently cancellable operation without disturbing
    /// provider accounting that may belong to another operation.
    pub fn settle_standalone_cancellation(&mut self) {
        self.active_cancellation_id = None;
    }

    pub fn begin_provider_request(&mut self, request_id: String) {
        self.server_prompt_tokens = None;
        self.server_completion_tokens = None;
        self.server_usage = None;
        if let Some(logical_turn_id) = &self.active_logical_turn_id {
            self.provider_request_turns
                .insert(request_id.clone(), logical_turn_id.clone());
        }
        self.active_request_id = Some(request_id);
    }

    pub fn persist_provider_request_start(
        &mut self,
        provider: crate::accounting::ProviderRequestAccounting,
    ) -> Result<(), String> {
        if !provider.in_flight {
            return Err("provider request start must be marked in flight".to_string());
        }
        self.persist_provider_checkpoint(&crate::client::ProviderRequestCheckpoint {
            request: provider,
            transcript: None,
        })?;
        Ok(())
    }

    pub fn persist_provider_checkpoint(
        &mut self,
        checkpoint: &crate::client::ProviderRequestCheckpoint,
    ) -> Result<bool, String> {
        if self.current_session_dir.is_none() {
            return Err(
                "provider request cannot be journaled without a durable session directory"
                    .to_string(),
            );
        }
        #[cfg(target_os = "linux")]
        {
            let dir = self
                .current_session_dir
                .as_deref()
                .expect("a durable session directory was checked above");
            let binding = self
                .session_directory_binding
                .as_ref()
                .ok_or_else(|| "active session has no directory identity binding".to_string())?;
            self.session_lease
                .as_ref()
                .ok_or_else(|| "active session has no advisory lease".to_string())?
                .verify(std::path::Path::new(dir), &self.session_id, binding)?;
            if !self.session_creation_committed {
                return Err(
                    "provider request cannot start before session creation is durably committed"
                        .to_string(),
                );
            }
        }
        if self.active_logical_turn_id.is_none() {
            return Err("provider request checkpoint has no active logical turn".to_string());
        }
        if checkpoint.request.in_flight && checkpoint.transcript.is_some() {
            return Err("in-flight provider checkpoint cannot contain a transcript".to_string());
        }
        if !checkpoint.request.in_flight
            && !self
                .provider_request_turns
                .contains_key(&checkpoint.request.request_id)
        {
            return Err(
                "terminal provider checkpoint has no durably journaled request start".to_string(),
            );
        }

        let accounting_before = self.accounting.clone();
        let blocks_before = self.blocks.clone();
        let messages_before = self.context_manager.get_messages().to_vec();
        let partial_assistant_before = self.partial_assistant_checkpoint.clone();
        let request_turns_before = self.provider_request_turns.clone();
        let active_request_before = self.active_request_id.clone();
        let prompt_tokens_before = self.server_prompt_tokens;
        let completion_tokens_before = self.server_completion_tokens;
        let usage_before = self.server_usage;
        let logical_turn_usage_before = self.logical_turn_usage;
        let needs_save_before = self.needs_save;

        let result = (|| -> Result<bool, String> {
            if checkpoint.request.in_flight
                && !self
                    .provider_request_turns
                    .contains_key(&checkpoint.request.request_id)
            {
                self.begin_provider_request(checkpoint.request.request_id.clone());
            }
            let recorded = self.record_provider_request(checkpoint.request.clone())?;
            if !recorded {
                return Ok(false);
            }
            if let Some(transcript) = &checkpoint.transcript {
                validate_provider_transcript_checkpoint(
                    self.context_manager.get_messages(),
                    transcript,
                )?;
                self.context_manager.set_messages(transcript.clone());
                self.partial_assistant_checkpoint = None;
            }
            self.save_session_checked()?;
            Ok(true)
        })();
        if let Err(error) = result {
            self.accounting = accounting_before;
            self.blocks = blocks_before;
            self.context_manager.set_messages(messages_before);
            self.partial_assistant_checkpoint = partial_assistant_before;
            self.provider_request_turns = request_turns_before;
            self.active_request_id = active_request_before;
            self.server_prompt_tokens = prompt_tokens_before;
            self.server_completion_tokens = completion_tokens_before;
            self.server_usage = usage_before;
            self.logical_turn_usage = logical_turn_usage_before;
            self.needs_save = needs_save_before;
            return Err(error);
        }
        result
    }

    pub fn update_request_usage(
        &mut self,
        request_id: &str,
        usage: crate::accounting::Usage,
    ) -> bool {
        if self.active_request_id.as_deref() != Some(request_id) {
            return false;
        }
        self.server_usage = Some(usage);
        true
    }

    /// Record the streamed-so-far reply. The checkpoint is updated on every
    /// chunk, but written to disk at most every `PARTIAL_CHECKPOINT_INTERVAL`:
    /// a full session save per chunk stalled the UI during long replies. The
    /// periodic save and the end-of-reply commit persist the rest.
    pub fn persist_partial_assistant_checkpoint(&mut self, content: String) -> Result<(), String> {
        const PARTIAL_CHECKPOINT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);
        self.partial_assistant_checkpoint = Some(crate::context::Message {
            role: "assistant".to_string(),
            content,
            tool_calls: None,
            provider_content: None,
            tool_result_is_error: false,
        });
        self.needs_save = true;
        let due = self
            .last_partial_checkpoint_save
            .is_none_or(|last| last.elapsed() >= PARTIAL_CHECKPOINT_INTERVAL);
        if !due {
            return Ok(());
        }
        self.last_partial_checkpoint_save = Some(std::time::Instant::now());
        self.save_session_checked()
    }

    pub fn commit_partial_assistant_checkpoint(&mut self) -> Result<bool, String> {
        let Some(checkpoint) = self.partial_assistant_checkpoint.take() else {
            return Ok(false);
        };
        if self.context_manager.get_messages().last() != Some(&checkpoint) {
            self.context_manager.add_message_raw(checkpoint);
        }
        self.needs_save = true;
        self.save_session_checked()?;
        Ok(true)
    }

    pub fn record_provider_request(
        &mut self,
        provider: crate::accounting::ProviderRequestAccounting,
    ) -> Result<bool, String> {
        let request_id = provider.request_id.clone();
        let logical_turn_id = if provider.in_flight {
            self.provider_request_turns.get(&request_id).cloned()
        } else {
            self.provider_request_turns.remove(&request_id)
        };
        let Some(logical_turn_id) = logical_turn_id else {
            return Ok(false);
        };
        let is_current_turn =
            self.active_logical_turn_id.as_deref() == Some(logical_turn_id.as_str());
        let usage_reported = provider.usage_reported;
        let usage = provider.usage;
        let in_flight = provider.in_flight;
        let request = provider.into_logical_turn(logical_turn_id.clone());
        let recorded = if is_current_turn {
            self.accounting.record_request(request)?
        } else {
            self.accounting.record_request_preserving_latest(request)?
        };
        if recorded {
            let totals = self.accounting.totals_for_logical_turn(&logical_turn_id);
            if is_current_turn {
                self.logical_turn_usage = Some(totals.usage);
            }
            if let Some(user_block) = self.blocks.iter_mut().find(|block| {
                block.block_type == BlockType::User
                    && block.logical_turn_id.as_deref() == Some(logical_turn_id.as_str())
            }) {
                apply_accounting_totals_to_user_block(user_block, &totals);
            }
        }
        if !in_flight && self.active_request_id.as_deref() == Some(request_id.as_str()) {
            self.server_usage = usage_reported.then_some(usage);
            self.active_request_id = None;
        }
        Ok(recorded)
    }

    /// Closes only the in-memory liveness marker after a terminal durable
    /// checkpoint failed. The checkpoint error remains visible to the actor and
    /// final save; this prevents a failed persistence attempt from pinning
    /// `is_fully_idle` forever during shutdown.
    pub fn close_provider_request_marker(&mut self, request_id: &str) -> bool {
        let removed = self.provider_request_turns.remove(request_id).is_some();
        if self.active_request_id.as_deref() == Some(request_id) {
            self.active_request_id = None;
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn cancellation_identity_is_distinct_from_request_and_logical_turn_ids() {
        let mut app = App::new(&Config::default());
        app.add_logical_turn_user_segment("first".to_string());
        let logical_turn = app.active_logical_turn_id.clone().unwrap();
        let cancel_id = app.active_cancellation_id().unwrap().to_string();
        app.begin_provider_request("request-one".to_string());
        assert_eq!(app.active_cancellation_id(), Some(cancel_id.as_str()));
        assert_ne!(cancel_id, logical_turn);
        assert_ne!(app.active_request_id.as_deref(), Some(cancel_id.as_str()));

        app.active_request_id = None;
        assert_eq!(app.active_cancellation_id(), Some(cancel_id.as_str()));
        app.settle_logical_turn();
        assert!(app.active_logical_turn_id.is_none());
        assert!(app.active_cancellation_id().is_none());

        app.add_logical_turn_user_segment("second".to_string());
        assert_ne!(app.active_cancellation_id(), Some(cancel_id.as_str()));
    }

    #[test]
    fn standalone_cancellation_cannot_replace_a_live_target() {
        let mut app = App::new(&Config::default());
        let first = app.begin_standalone_cancellation().unwrap();
        assert!(app.begin_standalone_cancellation().is_err());
        assert_eq!(app.active_cancellation_id(), Some(first.as_str()));
        app.settle_standalone_cancellation();
        assert!(app.active_cancellation_id().is_none());
    }
}
