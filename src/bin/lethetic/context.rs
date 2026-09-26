use std::future::Future;

use reqwest::Client;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use lethetic::app::{App, AppEventOutcome};
use lethetic::client::StreamEvent;
use lethetic::config::Config;

use crate::app_events::{self, AppEventControl};
use crate::lifecycle::{RuntimeMode, ShutdownCoordinator, ShutdownReason};

pub(crate) enum PythonSetupCompletion {
    Capabilities(Result<Vec<lethetic::python::backend::BackendCapability>, String>),
    PolicyPrepared {
        snapshot: lethetic::python_policy::PythonPolicySnapshot,
        effective_snapshot: Box<lethetic::python_policy::PythonPolicySnapshot>,
        effective_source: lethetic::python_policy::PythonPolicySource,
        persistence: lethetic::python_setup::PolicyPersistence,
        expected_revision: Option<lethetic::python_policy::PolicyRevision>,
        validation: Result<(), String>,
    },
    PullFinished(Result<Vec<lethetic::python::backend::BackendCapability>, String>),
}

pub(crate) struct PythonSetupSettlement {
    pub(crate) completion: Result<PythonSetupCompletion, String>,
    pub(crate) dismiss_when_settled: bool,
    pub(crate) cancellation_requested: bool,
}

pub(crate) struct PythonSetupOperation {
    cancellation: CancellationToken,
    task: JoinHandle<PythonSetupCompletion>,
    dismiss_when_settled: bool,
}

impl PythonSetupOperation {
    #[cfg(test)]
    pub(crate) fn start<F, Fut>(slot: &mut Option<Self>, build: F) -> Result<(), String>
    where
        F: FnOnce(CancellationToken) -> Fut,
        Fut: Future<Output = PythonSetupCompletion> + Send + 'static,
    {
        Self::start_with_token(slot, CancellationToken::new(), build)
    }

    pub(crate) fn start_child<F, Fut>(
        slot: &mut Option<Self>,
        parent: &CancellationToken,
        build: F,
    ) -> Result<(), String>
    where
        F: FnOnce(CancellationToken) -> Fut,
        Fut: Future<Output = PythonSetupCompletion> + Send + 'static,
    {
        Self::start_with_token(slot, parent.child_token(), build)
    }

    fn start_with_token<F, Fut>(
        slot: &mut Option<Self>,
        cancellation: CancellationToken,
        build: F,
    ) -> Result<(), String>
    where
        F: FnOnce(CancellationToken) -> Fut,
        Fut: Future<Output = PythonSetupCompletion> + Send + 'static,
    {
        if slot.is_some() {
            return Err("A Python setup operation is already active".to_string());
        }
        let task = tokio::spawn(build(cancellation.clone()));
        *slot = Some(Self {
            cancellation,
            task,
            dismiss_when_settled: false,
        });
        Ok(())
    }

    pub(crate) fn cancel(slot: &mut Option<Self>, dismiss_when_settled: bool) -> bool {
        let Some(operation) = slot.as_mut() else {
            return false;
        };
        operation.dismiss_when_settled |= dismiss_when_settled;
        operation.cancellation.cancel();
        true
    }
}

pub(crate) struct RuntimeContext<'a> {
    pub(crate) app: &'a mut App,
    pub(crate) config: &'a mut Config,
    pub(crate) client: Client,
    pub(crate) tx: mpsc::UnboundedSender<StreamEvent>,
    pub(crate) cancellation_token: CancellationToken,
    pub(crate) shutdown_cancellation: CancellationToken,
    pub(crate) background_cancellation: CancellationToken,
    pub(crate) full_response_content: String,
    pub(crate) cancellation_pending: bool,
    pub(crate) python_setup_operation: Option<PythonSetupOperation>,
    pub(crate) auxiliary_tasks: Vec<JoinHandle<()>>,
    pub(crate) lifecycle: ShutdownCoordinator,
    /// Palette start/stop of remote control, performed by the run loop,
    /// which owns the web runtime and listener.
    pub(crate) remote_control_request: Option<RemoteControlRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RemoteControlRequest {
    Start {
        target: String,
        open: bool,
        files: bool,
    },
    Stop,
}

impl<'a> RuntimeContext<'a> {
    pub(crate) fn new(
        app: &'a mut App,
        config: &'a mut Config,
        tx: mpsc::UnboundedSender<StreamEvent>,
        _mode: RuntimeMode,
    ) -> Self {
        let shutdown_cancellation = CancellationToken::new();
        Self {
            app,
            config,
            client: Client::new(),
            tx,
            cancellation_token: shutdown_cancellation.child_token(),
            background_cancellation: shutdown_cancellation.child_token(),
            shutdown_cancellation,
            full_response_content: String::new(),
            cancellation_pending: false,
            python_setup_operation: None,
            auxiliary_tasks: Vec::new(),
            lifecycle: ShutdownCoordinator::default(),
            remote_control_request: None,
        }
    }

    pub(crate) fn dispatch_app_event(
        &mut self,
        outcome: AppEventOutcome,
    ) -> std::pin::Pin<Box<dyn Future<Output = AppEventControl> + '_>> {
        Box::pin(app_events::handle_app_event_outcome(outcome, self))
    }

    pub(crate) fn has_python_setup_operation(&self) -> bool {
        self.python_setup_operation.is_some()
    }

    pub(crate) fn cancel_python_setup_operation(&mut self, dismiss_when_settled: bool) -> bool {
        PythonSetupOperation::cancel(&mut self.python_setup_operation, dismiss_when_settled)
    }

    pub(crate) async fn settle_python_setup_operation(&mut self) -> Option<PythonSetupSettlement> {
        let operation = self.python_setup_operation.as_mut()?;
        let completion = (&mut operation.task)
            .await
            .map_err(|error| format!("Python setup operation task failed: {error}"));
        let operation = self
            .python_setup_operation
            .take()
            .expect("settled Python setup operation remained present");
        Some(PythonSetupSettlement {
            completion,
            dismiss_when_settled: operation.dismiss_when_settled,
            cancellation_requested: operation.cancellation.is_cancelled(),
        })
    }

    pub(crate) fn begin_shutdown(&mut self, reason: ShutdownReason) -> bool {
        self.shutdown_cancellation.cancel();
        let python_setup_operation_active = self.cancel_python_setup_operation(false);
        let began = self.lifecycle.begin(
            reason,
            self.app,
            &self.cancellation_token,
            &mut self.cancellation_pending,
            python_setup_operation_active,
        );
        if began {
            self.background_cancellation.cancel();
        }
        began
    }

    pub(crate) fn begin_fatal_shutdown(
        &mut self,
        reason: ShutdownReason,
        error: impl Into<String>,
    ) -> bool {
        let began = self.begin_shutdown(reason);
        self.lifecycle.record_error(error);
        began
    }

    pub(crate) fn accepts_new_work(&self) -> bool {
        !self.shutdown_cancellation.is_cancelled() && !self.lifecycle.is_shutting_down()
    }

    pub(crate) fn shutdown_contained(&mut self) -> bool {
        self.lifecycle.update_containment(
            self.app,
            self.cancellation_pending,
            self.python_setup_operation.is_some(),
        )
    }

    pub(crate) fn finish_shutdown(&mut self) -> Result<(), String> {
        self.lifecycle.finish_result()
    }
}
