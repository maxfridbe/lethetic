use lethetic::app::App;
use lethetic::config::Config;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CommittedSessionTransition {
    pub(crate) session_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionTransitionStage {
    Precondition,
    SaveCurrent,
    DetachRuntime,
    Cancelled,
    CreateAndCommit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionTransitionFailure {
    pub(crate) stage: SessionTransitionStage,
    pub(crate) message: String,
}

impl SessionTransitionFailure {
    fn new(stage: SessionTransitionStage, message: impl Into<String>) -> Self {
        Self {
            stage,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for SessionTransitionFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

fn finish_committed_transition(
    app: &mut App,
    create_and_commit: impl FnOnce(&mut App) -> Result<(), String>,
) -> Result<CommittedSessionTransition, SessionTransitionFailure> {
    create_and_commit(app).map_err(|error| {
        SessionTransitionFailure::new(SessionTransitionStage::CreateAndCommit, error)
    })?;
    if app.current_session_dir.is_none() {
        return Err(SessionTransitionFailure::new(
            SessionTransitionStage::CreateAndCommit,
            "session creation returned without a durable session directory",
        ));
    }
    Ok(CommittedSessionTransition {
        session_id: app.session_id.clone(),
    })
}

/// Starts a replacement session and reports success only after App's checked
/// creation path has written the final state and committed the creation record.
pub(crate) async fn start_committed_session_transition(
    app: &mut App,
    config: &mut Config,
    cancellation: CancellationToken,
) -> Result<CommittedSessionTransition, SessionTransitionFailure> {
    if !app.is_fully_idle() {
        return Err(SessionTransitionFailure::new(
            SessionTransitionStage::Precondition,
            "wait for active work before creating a session",
        ));
    }
    if cancellation.is_cancelled() {
        return Err(SessionTransitionFailure::new(
            SessionTransitionStage::Cancelled,
            "session transition was cancelled",
        ));
    }
    app.save_session_checked().map_err(|error| {
        SessionTransitionFailure::new(SessionTransitionStage::SaveCurrent, error)
    })?;
    crate::app_events::prepare_python_policy_for_session_transition(app, config)
        .await
        .map_err(|error| {
            SessionTransitionFailure::new(SessionTransitionStage::DetachRuntime, error)
        })?;
    if cancellation.is_cancelled() {
        return Err(SessionTransitionFailure::new(
            SessionTransitionStage::Cancelled,
            "session transition was cancelled after runtime detach",
        ));
    }
    finish_committed_transition(app, App::start_new_session_checked)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_identity_does_not_turn_a_late_commit_failure_into_success() {
        let mut app = App::new(&Config::default());
        let replacement = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee".to_string();
        let result = finish_committed_transition(&mut app, |app| {
            app.session_id = replacement.clone();
            Err("simulated creation commit failure".to_string())
        });

        let failure = result.unwrap_err();
        assert_eq!(app.session_id, replacement);
        assert_eq!(failure.stage, SessionTransitionStage::CreateAndCommit);
        assert_eq!(failure.message, "simulated creation commit failure");
    }
}
