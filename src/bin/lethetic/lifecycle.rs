use std::io;

use futures_util::FutureExt;
use lethetic::app::{App, BlockType};
use lethetic::icons;
use tokio_util::sync::CancellationToken;

use crate::provider::{cancel_pending_interaction_for_shutdown, request_lsp_install_cancellation};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProcessSignal {
    Interrupt,
    #[cfg(unix)]
    Terminate,
    #[cfg(unix)]
    Hangup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeMode {
    Interactive,
    Service,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SignalAction {
    BeginShutdown(ShutdownReason),
    #[cfg(unix)]
    IgnoreHangup,
}

pub(crate) fn signal_action(mode: RuntimeMode, signal: ProcessSignal) -> SignalAction {
    match (mode, signal) {
        #[cfg(unix)]
        (RuntimeMode::Service, ProcessSignal::Hangup) => SignalAction::IgnoreHangup,
        #[cfg(unix)]
        (_, ProcessSignal::Hangup) => SignalAction::BeginShutdown(ShutdownReason::Hangup),
        (_, ProcessSignal::Interrupt) => SignalAction::BeginShutdown(ShutdownReason::Interrupt),
        #[cfg(unix)]
        (_, ProcessSignal::Terminate) => SignalAction::BeginShutdown(ShutdownReason::Terminate),
    }
}

fn propagate_signal_cancellation(
    mode: RuntimeMode,
    signal: ProcessSignal,
    shutdown: &CancellationToken,
) {
    if matches!(signal_action(mode, signal), SignalAction::BeginShutdown(_)) {
        shutdown.cancel();
    }
}

#[cfg(unix)]
pub(crate) struct SignalListener {
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
    hangup: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl SignalListener {
    pub(crate) fn new() -> io::Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
            hangup: signal(SignalKind::hangup())?,
        })
    }

    pub(crate) fn try_recv(&mut self) -> Option<ProcessSignal> {
        if self.interrupt.recv().now_or_never().flatten().is_some() {
            Some(ProcessSignal::Interrupt)
        } else if self.terminate.recv().now_or_never().flatten().is_some() {
            Some(ProcessSignal::Terminate)
        } else if self.hangup.recv().now_or_never().flatten().is_some() {
            Some(ProcessSignal::Hangup)
        } else {
            None
        }
    }

    pub(crate) async fn recv(&mut self) -> io::Result<ProcessSignal> {
        tokio::select! {
            value = self.interrupt.recv() => value
                .map(|_| ProcessSignal::Interrupt)
                .ok_or_else(signal_stream_closed),
            value = self.terminate.recv() => value
                .map(|_| ProcessSignal::Terminate)
                .ok_or_else(signal_stream_closed),
            value = self.hangup.recv() => value
                .map(|_| ProcessSignal::Hangup)
                .ok_or_else(signal_stream_closed),
        }
    }
}

#[cfg(windows)]
pub(crate) struct SignalListener {
    interrupt: tokio::signal::windows::CtrlC,
}

#[cfg(windows)]
impl SignalListener {
    pub(crate) fn new() -> io::Result<Self> {
        Ok(Self {
            interrupt: tokio::signal::windows::ctrl_c()?,
        })
    }

    pub(crate) fn try_recv(&mut self) -> Option<ProcessSignal> {
        self.interrupt
            .recv()
            .now_or_never()
            .flatten()
            .map(|_| ProcessSignal::Interrupt)
    }

    pub(crate) async fn recv(&mut self) -> io::Result<ProcessSignal> {
        self.interrupt
            .recv()
            .await
            .map(|_| ProcessSignal::Interrupt)
            .ok_or_else(signal_stream_closed)
    }
}

#[cfg(not(any(unix, windows)))]
pub(crate) struct SignalListener;

#[cfg(not(any(unix, windows)))]
impl SignalListener {
    pub(crate) fn new() -> io::Result<Self> {
        Ok(Self)
    }

    pub(crate) fn try_recv(&mut self) -> Option<ProcessSignal> {
        None
    }

    pub(crate) async fn recv(&mut self) -> io::Result<ProcessSignal> {
        std::future::pending().await
    }
}

fn signal_stream_closed() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "process signal stream closed")
}

#[derive(Debug)]
pub(crate) enum SupervisedSignalEvent {
    Signal(ProcessSignal),
    ListenerFailed(String),
}

pub(crate) struct SignalSupervisor {
    receiver: tokio::sync::mpsc::UnboundedReceiver<SupervisedSignalEvent>,
    stop: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl SignalSupervisor {
    pub(crate) fn spawn(
        mut listener: SignalListener,
        mode: RuntimeMode,
        shutdown: CancellationToken,
    ) -> Self {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let stop = CancellationToken::new();
        let task_stop = stop.clone();
        let task = tokio::spawn(async move {
            loop {
                let event = tokio::select! {
                    biased;
                    _ = task_stop.cancelled() => break,
                    event = listener.recv() => event,
                };
                match event {
                    Ok(signal) => {
                        propagate_signal_cancellation(mode, signal, &shutdown);
                        if sender.send(SupervisedSignalEvent::Signal(signal)).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        shutdown.cancel();
                        let _ =
                            sender.send(SupervisedSignalEvent::ListenerFailed(error.to_string()));
                        break;
                    }
                }
            }
        });
        Self {
            receiver,
            stop,
            task,
        }
    }

    pub(crate) async fn recv(&mut self) -> Option<SupervisedSignalEvent> {
        self.receiver.recv().await
    }

    pub(crate) async fn shutdown(self) -> Result<(), String> {
        self.stop.cancel();
        self.task
            .await
            .map_err(|error| format!("process signal supervisor failed: {error}"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShutdownReason {
    UserExit,
    Interrupt,
    #[cfg(unix)]
    Terminate,
    #[cfg(unix)]
    Hangup,
    TerminalClosed,
    TerminalError,
    WfeFailure,
    SignalListenerFailure,
}

impl ShutdownReason {
    pub(crate) fn status(self) -> &'static str {
        match self {
            Self::UserExit => "Shutting down safely…",
            Self::Interrupt => "Interrupt received; shutting down safely…",
            #[cfg(unix)]
            Self::Terminate => "Termination requested; shutting down safely…",
            #[cfg(unix)]
            Self::Hangup => "Terminal hangup received; shutting down safely…",
            Self::TerminalClosed => "Terminal input closed; shutting down safely…",
            Self::TerminalError => "Terminal input failed; shutting down safely…",
            Self::WfeFailure => "WFE failed; containing work before shutdown…",
            Self::SignalListenerFailure => {
                "Signal listener failed; containing work before shutdown…"
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShutdownState {
    Running,
    Draining,
    Contained,
}

pub(crate) struct ShutdownCoordinator {
    state: ShutdownState,
    reason: Option<ShutdownReason>,
    cancellation_issued: bool,
    errors: Vec<String>,
}

impl Default for ShutdownCoordinator {
    fn default() -> Self {
        Self {
            state: ShutdownState::Running,
            reason: None,
            cancellation_issued: false,
            errors: Vec::new(),
        }
    }
}

impl ShutdownCoordinator {
    pub(crate) fn is_shutting_down(&self) -> bool {
        self.state != ShutdownState::Running
    }

    pub(crate) fn reason(&self) -> Option<ShutdownReason> {
        self.reason
    }

    #[cfg(test)]
    pub(crate) fn cancellation_issued(&self) -> bool {
        self.cancellation_issued
    }

    pub(crate) fn record_error(&mut self, error: impl Into<String>) {
        self.errors.push(error.into());
    }

    /// Starts shutdown exactly once. This is the only transition that cancels
    /// actor-owned work, so duplicate signals and UI/browser exit requests are
    /// harmless.
    pub(crate) fn begin(
        &mut self,
        reason: ShutdownReason,
        app: &mut App,
        cancellation_token: &CancellationToken,
        cancellation_pending: &mut bool,
        tracked_python_setup_operation: bool,
    ) -> bool {
        if self.is_shutting_down() {
            return false;
        }

        self.state = ShutdownState::Draining;
        self.reason = Some(reason);
        app.stop_reason = reason.status().to_string();
        app.should_redraw = true;

        if let Err(error) = cancel_pending_interaction_for_shutdown(app) {
            self.errors.push(format!(
                "pending interaction shutdown checkpoint failed: {error}"
            ));
        }

        let setup_active = app.python_setup_is_busy() || tracked_python_setup_operation;
        let lsp_active = app.lsp_install_in_progress;
        let provider_or_tool_active = app.active_request_id.is_some()
            || app.is_processing
            || (app.is_executing_tool && !lsp_active);

        if setup_active || lsp_active || provider_or_tool_active {
            self.cancellation_issued = true;
        }
        // Tracked setup work owns a dedicated cancellation token. Preserve the
        // shared token for provider/tool/LSP work and as a fallback for any
        // legacy busy setup state that has no tracked operation.
        if lsp_active
            || provider_or_tool_active
            || (setup_active && !tracked_python_setup_operation)
        {
            cancellation_token.cancel();
        }

        if lsp_active {
            request_lsp_install_cancellation(
                app,
                cancellation_token,
                "Cancelling LSP server installation before shutdown…",
            );
        }

        if provider_or_tool_active {
            *cancellation_pending = true;
            app.is_processing = true;
            app.add_segment(
                format!("\n{} [STOPPING]\n", icons::WARNING),
                BlockType::Text,
            );
        }

        true
    }

    pub(crate) fn update_containment(
        &mut self,
        app: &App,
        cancellation_pending: bool,
        tracked_python_setup_operation: bool,
    ) -> bool {
        if self.state == ShutdownState::Running {
            return false;
        }
        if self.state == ShutdownState::Contained {
            return true;
        }

        if !tracked_python_setup_operation
            && !app.python_setup_is_busy()
            && app.is_fully_idle()
            && !cancellation_pending
        {
            self.state = ShutdownState::Contained;
            return true;
        }
        false
    }

    pub(crate) fn finish_result(&mut self) -> Result<(), String> {
        if self.errors.is_empty() {
            Ok(())
        } else {
            Err(std::mem::take(&mut self.errors).join("; "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lethetic::config::Config;

    #[test]
    fn signal_policy_distinguishes_service_hangup() {
        for mode in [RuntimeMode::Interactive, RuntimeMode::Service] {
            assert_eq!(
                signal_action(mode, ProcessSignal::Interrupt),
                SignalAction::BeginShutdown(ShutdownReason::Interrupt)
            );
        }
        #[cfg(unix)]
        {
            assert_eq!(
                signal_action(RuntimeMode::Service, ProcessSignal::Hangup),
                SignalAction::IgnoreHangup
            );
            assert_eq!(
                signal_action(RuntimeMode::Interactive, ProcessSignal::Hangup),
                SignalAction::BeginShutdown(ShutdownReason::Hangup)
            );
            for mode in [RuntimeMode::Interactive, RuntimeMode::Service] {
                assert_eq!(
                    signal_action(mode, ProcessSignal::Terminate),
                    SignalAction::BeginShutdown(ShutdownReason::Terminate)
                );
            }
        }
    }

    #[test]
    fn signal_supervision_cancels_work_before_actor_dispatch() {
        for mode in [RuntimeMode::Interactive, RuntimeMode::Service] {
            let shutdown = CancellationToken::new();
            propagate_signal_cancellation(mode, ProcessSignal::Interrupt, &shutdown);
            assert!(shutdown.is_cancelled());
        }
        #[cfg(unix)]
        {
            let service = CancellationToken::new();
            propagate_signal_cancellation(RuntimeMode::Service, ProcessSignal::Hangup, &service);
            assert!(!service.is_cancelled());

            let interactive = CancellationToken::new();
            propagate_signal_cancellation(
                RuntimeMode::Interactive,
                ProcessSignal::Hangup,
                &interactive,
            );
            assert!(interactive.is_cancelled());
        }
    }

    #[test]
    fn shutdown_is_idempotent_and_gates_work_immediately() {
        let mut app = App::new(&Config::default());
        app.is_processing = true;
        let token = CancellationToken::new();
        let mut pending = false;
        let mut shutdown = ShutdownCoordinator::default();

        assert!(shutdown.begin(
            ShutdownReason::Interrupt,
            &mut app,
            &token,
            &mut pending,
            false,
        ));
        assert!(shutdown.is_shutting_down());
        assert!(shutdown.cancellation_issued());
        assert!(token.is_cancelled());
        assert!(pending);

        let block_count = app.blocks.len();
        assert!(!shutdown.begin(
            ShutdownReason::UserExit,
            &mut app,
            &token,
            &mut pending,
            false,
        ));
        assert_eq!(shutdown.reason(), Some(ShutdownReason::Interrupt));
        assert_eq!(app.blocks.len(), block_count);
        assert!(!shutdown.update_containment(&app, pending, false));

        app.is_processing = false;
        app.is_executing_tool = false;
        pending = false;
        assert!(shutdown.update_containment(&app, pending, false));
    }

    #[test]
    fn idle_shutdown_is_immediately_contained_without_cancelling() {
        let mut app = App::new(&Config::default());
        app.show_session_manager = false;
        let token = CancellationToken::new();
        let mut pending = false;
        let mut shutdown = ShutdownCoordinator::default();

        shutdown.begin(
            ShutdownReason::UserExit,
            &mut app,
            &token,
            &mut pending,
            false,
        );
        assert!(!shutdown.cancellation_issued());
        assert!(shutdown.update_containment(&app, pending, false));
        assert!(shutdown.update_containment(&app, pending, false));
    }
}
