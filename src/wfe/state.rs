use super::contracts::{IStatePatch, IStateSnapshot, MAX_SAFE_JAVASCRIPT_INTEGER, WebAppSnapshot};
use std::sync::Arc;
use tokio::sync::{broadcast, watch};

pub const WFE_PATCH_BROADCAST_CAPACITY: usize = 128;

#[derive(Clone)]
pub struct MirrorHandle {
    snapshots: watch::Receiver<Arc<IStateSnapshot>>,
    patches: broadcast::Sender<Arc<IStatePatch>>,
}

impl MirrorHandle {
    pub fn latest_snapshot(&self) -> Arc<IStateSnapshot> {
        self.snapshots.borrow().clone()
    }

    pub fn subscribe_snapshots(&self) -> watch::Receiver<Arc<IStateSnapshot>> {
        self.snapshots.clone()
    }

    pub fn subscribe_patches(&self) -> broadcast::Receiver<Arc<IStatePatch>> {
        self.patches.subscribe()
    }
}

pub struct MirrorPublisher {
    state: WebAppSnapshot,
    sequence: u64,
    revision: u64,
    snapshots: watch::Sender<Arc<IStateSnapshot>>,
    patches: broadcast::Sender<Arc<IStatePatch>>,
}

impl MirrorPublisher {
    pub fn new(state: WebAppSnapshot) -> Result<Self, String> {
        Self::with_patch_capacity(state, WFE_PATCH_BROADCAST_CAPACITY)
    }

    pub fn with_patch_capacity(
        state: WebAppSnapshot,
        patch_capacity: usize,
    ) -> Result<Self, String> {
        if patch_capacity == 0 {
            return Err("patch broadcast capacity must be greater than zero".to_string());
        }
        let initial = Arc::new(IStateSnapshot::new(0, 0, state.clone())?);
        let (snapshots, _) = watch::channel(initial);
        let (patches, _) = broadcast::channel(patch_capacity);
        Ok(Self {
            state,
            sequence: 0,
            revision: 0,
            snapshots,
            patches,
        })
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn state(&self) -> &WebAppSnapshot {
        &self.state
    }

    pub fn latest_snapshot(&self) -> Arc<IStateSnapshot> {
        self.snapshots.borrow().clone()
    }

    pub fn subscribe_snapshots(&self) -> watch::Receiver<Arc<IStateSnapshot>> {
        self.snapshots.subscribe()
    }

    pub fn subscribe_patches(&self) -> broadcast::Receiver<Arc<IStatePatch>> {
        self.patches.subscribe()
    }

    pub fn handle(&self) -> MirrorHandle {
        MirrorHandle {
            snapshots: self.snapshots.subscribe(),
            patches: self.patches.clone(),
        }
    }

    pub fn publish(&mut self, state: WebAppSnapshot) -> Result<Option<Arc<IStatePatch>>, String> {
        if semantically_equal(&state, &self.state) {
            if state != self.state {
                self.state = state.clone();
                self.snapshots.send_replace(Arc::new(IStateSnapshot::new(
                    self.sequence,
                    self.revision,
                    state,
                )?));
            }
            return Ok(None);
        }
        if self.sequence == MAX_SAFE_JAVASCRIPT_INTEGER
            || self.revision == MAX_SAFE_JAVASCRIPT_INTEGER
        {
            return Err("WFE state counters exhausted their safe integer range".to_string());
        }
        let sequence = self.sequence + 1;
        let revision = self.revision + 1;
        let patch = Arc::new(IStatePatch::between(
            sequence,
            self.revision,
            revision,
            &self.state,
            &state,
        )?);
        let snapshot = Arc::new(IStateSnapshot::new(sequence, revision, state.clone())?);

        self.state = state;
        self.sequence = sequence;
        self.revision = revision;
        self.snapshots.send_replace(snapshot);
        let _ = self.patches.send(patch.clone());
        Ok(Some(patch))
    }
}

fn semantically_equal(left: &WebAppSnapshot, right: &WebAppSnapshot) -> bool {
    left.session == right.session
        && left.blocks == right.blocks
        && left.activity == right.activity
        && left.pending_approval == right.pending_approval
        && left.pending_question == right.pending_question
        && left.commands == right.commands
        && left.sessions == right.sessions
        && left.models == right.models
        && left.themes == right.themes
        && left.usage == right.usage
        && left.status.stop_reason == right.status.stop_reason
        && left.status.stop_reason_loss == right.status.stop_reason_loss
        && left.status.model_label == right.status.model_label
        && left.status.provider_label == right.status.provider_label
        && left.status.provider_transport == right.status.provider_transport
        && left.status.python == right.status.python
        && left.status.context_tokens == right.status.context_tokens
        && left.status.context_limit_tokens == right.status.context_limit_tokens
        && left.status.context_source == right.status.context_source
        && left.status.request_usage == right.status.request_usage
        && left.status.file_count == right.status.file_count
        && left.status.visible_block_count == right.status.visible_block_count
        && left.debugger.open == right.debugger.open
        && left.overlay == right.overlay
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::config::Config;
    use crate::wfe::presentation::{ProjectionContext, project_app};

    #[test]
    fn no_op_projection_does_not_advance_revision() {
        let app = App::new(&Config::default());
        let state = project_app(&app, ProjectionContext::default());
        let mut publisher = MirrorPublisher::new(state.clone()).unwrap();
        assert!(publisher.publish(state).unwrap().is_none());
        assert_eq!(publisher.revision(), 0);
        assert_eq!(publisher.sequence(), 0);
    }

    #[test]
    fn volatile_status_sampling_updates_snapshot_without_staling_commands() {
        let app = App::new(&Config::default());
        let state = project_app(&app, ProjectionContext::default());
        let mut publisher = MirrorPublisher::new(state.clone()).unwrap();
        let mut sampled = state;
        sampled.status.memory_mebibytes = "512".to_string();
        sampled.status.tokens_per_second = Some("42.00".to_string());
        sampled.status.git_state = crate::wfe::contracts::GitStateView::Dirty;
        assert!(publisher.publish(sampled.clone()).unwrap().is_none());
        assert_eq!(publisher.revision(), 0);
        assert_eq!(publisher.sequence(), 0);
        assert_eq!(publisher.latest_snapshot().state, sampled);
    }

    #[test]
    fn volatile_debugger_samples_update_without_staling_commands() {
        let app = App::new(&Config::default());
        let state = project_app(&app, ProjectionContext::default());
        let mut publisher = MirrorPublisher::new(state.clone()).unwrap();
        let mut sampled = state;
        sampled.debugger.summary = "Actor active · browser mirror ready".to_string();
        sampled
            .debugger
            .entries
            .push(crate::wfe::contracts::DiagnosticView {
                code: crate::wfe::contracts::DiagnosticCode::RemoteControlDegraded,
                severity: crate::wfe::contracts::DiagnosticSeverity::Warning,
                message: "Mirror recovered".to_string(),
            });
        assert!(publisher.publish(sampled.clone()).unwrap().is_none());
        assert_eq!(publisher.revision(), 0);
        assert_eq!(publisher.sequence(), 0);
        assert_eq!(publisher.latest_snapshot().state, sampled);
    }

    #[tokio::test]
    async fn semantic_change_publishes_patch_and_latest_snapshot() {
        let app = App::new(&Config::default());
        let state = project_app(&app, ProjectionContext::default());
        let mut publisher = MirrorPublisher::new(state.clone()).unwrap();
        let mut snapshots = publisher.subscribe_snapshots();
        let mut patches = publisher.subscribe_patches();
        let mut changed = state;
        changed.status.stop_reason = "changed".to_string();

        let patch = publisher.publish(changed.clone()).unwrap().unwrap();
        assert_eq!(patch.sequence, 1);
        assert_eq!(patch.base_revision, 0);
        assert_eq!(patch.revision, 1);
        assert_eq!(patches.recv().await.unwrap(), patch);
        snapshots.changed().await.unwrap();
        let snapshot = snapshots.borrow().clone();
        assert_eq!(snapshot.sequence, 1);
        assert_eq!(snapshot.revision, 1);
        assert_eq!(snapshot.state, changed);
    }

    #[tokio::test]
    async fn lagging_patch_receiver_does_not_block_publisher() {
        let app = App::new(&Config::default());
        let state = project_app(&app, ProjectionContext::default());
        let mut publisher = MirrorPublisher::with_patch_capacity(state.clone(), 1).unwrap();
        let mut patches = publisher.subscribe_patches();
        for index in 0..3 {
            let mut changed = publisher.state().clone();
            changed.status.stop_reason = format!("change-{index}");
            assert!(publisher.publish(changed).unwrap().is_some());
        }
        assert!(matches!(
            patches.try_recv(),
            Err(broadcast::error::TryRecvError::Lagged(_))
        ));
        assert_eq!(publisher.latest_snapshot().revision, 3);
    }
}
