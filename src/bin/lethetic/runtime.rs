use std::{error::Error, io, time::Duration};

use tokio::sync::mpsc;

use lethetic::app::App;
use lethetic::client::StreamEvent;
use lethetic::config::Config;
use lethetic::tools::get_git_info;
use lethetic::wfe::contracts::{CommandErrorCode, CommandOutcome};
#[cfg(target_os = "linux")]
use lethetic::wfe::diagnostics::MaintenanceResult;
use lethetic::wfe::diagnostics::{
    OperationResult, OperationalEvent, ProviderPhase, ShutdownKind, ToolPhase,
};
use lethetic::wfe::runtime::{WfeRuntime, WfeRuntimeEvent};

use crate::context::{PythonSetupCompletion, PythonSetupSettlement, RuntimeContext};
use crate::formatting::{format_wfe_connection_event, validate_wfe_bootstrap_acknowledgement};
use crate::input::{InputControl, InteractiveSurface, TerminalInput, handle_terminal_input};
use crate::lifecycle::{
    RuntimeMode, ShutdownReason, SignalAction, SignalListener, SignalSupervisor,
    SupervisedSignalEvent, signal_action,
};
use crate::line_reader::BootstrapLineReader;
use crate::service_console::{ServiceConsole, ServiceConsoleEvent};
use crate::stream_events::{StreamControl, handle_stream_event};
use crate::terminal::InteractiveTerminal;
use crate::wfe_commands::{
    PendingWfeSessionLoad, WfeCommandDisposition, execute_wfe_command, wfe_failure,
};

const MAX_STREAM_EVENTS_PER_TICK: usize = 256;

type WfeStatusReceiver = tokio::sync::watch::Receiver<lethetic::wfe::server::WfeServerStatus>;

pub(crate) enum ActorSurface {
    AwaitingTui(BootstrapLineReader),
    Interactive(InteractiveSurface),
    Service,
}

enum ActorInput {
    EnterTui(io::Result<String>),
    Terminal(TerminalInput),
}

impl ActorSurface {
    pub(crate) fn awaiting_tui(reader: BootstrapLineReader) -> Self {
        Self::AwaitingTui(reader)
    }

    pub(crate) fn interactive() -> io::Result<Self> {
        InteractiveSurface::enter().map(Self::Interactive)
    }

    pub(crate) fn service() -> Self {
        Self::Service
    }

    pub(crate) fn into_terminal(self) -> Option<InteractiveTerminal> {
        match self {
            Self::Interactive(surface) => Some(surface.into_terminal()),
            Self::AwaitingTui(_) | Self::Service => None,
        }
    }

    fn mode(&self) -> RuntimeMode {
        match self {
            Self::AwaitingTui(_) | Self::Interactive(_) => RuntimeMode::Interactive,
            Self::Service => RuntimeMode::Service,
        }
    }

    fn draw(&mut self, app: &mut App) -> io::Result<()> {
        match self {
            Self::Interactive(surface) => surface.draw(app),
            Self::AwaitingTui(_) | Self::Service => {
                // `should_redraw` is a terminal scheduling hint. Browser state is
                // published independently above.
                app.should_redraw = false;
                Ok(())
            }
        }
    }

    fn terminal_loss(&self) -> Option<TerminalInput> {
        match self {
            Self::Interactive(surface) => surface.terminal_loss(),
            Self::AwaitingTui(_) | Self::Service => None,
        }
    }

    async fn next_input(&mut self) -> ActorInput {
        match self {
            Self::AwaitingTui(reader) => ActorInput::EnterTui(reader.receive().await),
            Self::Interactive(surface) => ActorInput::Terminal(surface.next().await),
            Self::Service => std::future::pending().await,
        }
    }

    async fn handle_input(
        &mut self,
        context: &mut RuntimeContext<'_>,
        input: ActorInput,
    ) -> InputControl {
        match input {
            ActorInput::Terminal(input) => match self {
                Self::Interactive(surface) => handle_terminal_input(context, surface, input).await,
                Self::AwaitingTui(_) | Self::Service => {
                    unreachable!("only an interactive surface can yield terminal events")
                }
            },
            ActorInput::EnterTui(result) => {
                if !matches!(self, Self::AwaitingTui(_)) {
                    unreachable!("only the browser-first phase can yield its Enter gate");
                }
                let acknowledgement = match result {
                    Ok(acknowledgement) => acknowledgement,
                    Err(error) => {
                        context.begin_shutdown(
                            crate::wfe_startup::bootstrap_input_shutdown_reason(&error),
                        );
                        return InputControl::ContinueRunLoop;
                    }
                };
                if let Err(error) = validate_wfe_bootstrap_acknowledgement(&acknowledgement) {
                    context.begin_shutdown(crate::wfe_startup::bootstrap_input_shutdown_reason(
                        &error,
                    ));
                    return InputControl::ContinueRunLoop;
                }
                match InteractiveSurface::enter() {
                    Ok(surface) => {
                        *self = Self::Interactive(surface);
                        context.app.should_redraw = true;
                    }
                    Err(_) => {
                        context.begin_shutdown(ShutdownReason::TerminalError);
                    }
                }
                InputControl::ContinueRunLoop
            }
        }
    }

    fn settle_input(&mut self) -> io::Result<()> {
        match self {
            Self::AwaitingTui(reader) => reader.cancel_and_join(),
            Self::Interactive(_) | Self::Service => Ok(()),
        }
    }
}

/// Minimum spacing between browser state publications from the run loop.
const WFE_PUBLISH_INTERVAL: Duration = Duration::from_millis(100);

pub(crate) async fn run_actor(
    app: &mut App,
    config: &mut Config,
    mut wfe_runtime: Option<WfeRuntime>,
    mut wfe_status: Option<WfeStatusReceiver>,
    mut surface: ActorSurface,
    signals: SignalListener,
    mut console: Option<&mut ServiceConsole>,
) -> (ActorSurface, Result<(), Box<dyn Error>>) {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mode = surface.mode();
    lethetic::background::set_mode(config.background_tasks);
    let mut context = RuntimeContext::new(app, config, tx, mode);
    let mut signal_supervisor =
        SignalSupervisor::spawn(signals, mode, context.shutdown_cancellation.clone());
    let mut signal_events_open = true;
    let mut pending_wfe_session_load: Option<PendingWfeSessionLoad> = None;
    // Listener started from the command palette; the launch-time one is
    // owned by the caller.
    let mut in_session_server: Option<lethetic::wfe::server::WfeServerHandle> = None;
    // Projecting the whole app for the browser is expensive (it walks every
    // transcript block), so publish only after something happened and at
    // most every WFE_PUBLISH_INTERVAL. Browser commands publish on their own
    // before replying, so they never wait on this throttle.
    let mut wfe_dirty = true;
    let mut last_wfe_publish: Option<std::time::Instant> = None;
    let mut last_tick = std::time::Instant::now();
    let mut last_save = std::time::Instant::now();
    let mut last_background_revision = lethetic::background::revision();
    let mut last_background_tick = std::time::Instant::now();
    let mut last_times_label = String::new();
    let mut last_times_check = std::time::Instant::now();
    let mut shutdown_announced = false;
    let mut shutdown_diagnostic_recorded = false;
    let background = spawn_background_tasks(&context);

    context.app.refresh_system_stats();
    if let Some(runtime) = wfe_runtime.as_mut() {
        runtime.record_operational(OperationalEvent::ActorReady);
    }
    if mode == RuntimeMode::Service
        && !context.lifecycle.is_shutting_down()
        && emit_console(&mut console, ServiceConsoleEvent::Ready).is_err()
    {
        context.begin_fatal_shutdown(
            ShutdownReason::TerminalError,
            "service console output failed",
        );
    }

    loop {
        if let Some(request) = context.remote_control_request.take() {
            wfe_dirty = true;
            handle_remote_control_request(
                request,
                &mut context,
                &mut wfe_runtime,
                &mut wfe_status,
                &mut in_session_server,
            )
            .await;
        }
        if !shutdown_diagnostic_recorded && let Some(reason) = context.lifecycle.reason() {
            shutdown_diagnostic_recorded = true;
            if let Some(runtime) = wfe_runtime.as_mut() {
                runtime.record_operational(OperationalEvent::Shutdown(shutdown_kind(reason)));
            }
        }
        let publish_due =
            wfe_dirty && last_wfe_publish.is_none_or(|last| last.elapsed() >= WFE_PUBLISH_INTERVAL);
        let publish_failed = publish_due
            && wfe_runtime.as_mut().is_some_and(|runtime| {
                wfe_dirty = false;
                last_wfe_publish = Some(std::time::Instant::now());
                runtime.publish(context.app).is_err()
            });
        if publish_failed {
            context
                .begin_fatal_shutdown(ShutdownReason::WfeFailure, "WFE state publication failed");
            wfe_runtime = None;
        }

        if context.accepts_new_work()
            && let Some(input) = surface.terminal_loss()
        {
            let _ = surface
                .handle_input(&mut context, ActorInput::Terminal(input))
                .await;
        }

        if context.app.should_redraw && surface.draw(context.app).is_err() {
            context.begin_shutdown(ShutdownReason::TerminalError);
        }

        if context.lifecycle.is_shutting_down() && !shutdown_announced {
            shutdown_announced = true;
            let reason = context.lifecycle.reason().expect("shutdown has a reason");
            let event = if reason == ShutdownReason::WfeFailure {
                ServiceConsoleEvent::WfeFailed
            } else {
                ServiceConsoleEvent::ShutdownRequested(reason)
            };
            if emit_console(&mut console, event).is_err() {
                context
                    .lifecycle
                    .record_error("service console output failed");
            }
        }

        if context.shutdown_contained() {
            break;
        }

        if context.app.needs_save && last_save.elapsed() >= Duration::from_secs(2) {
            context.app.save_session();
            if let Some(runtime) = wfe_runtime.as_mut() {
                runtime.record_operational(OperationalEvent::Save(if context.app.needs_save {
                    OperationResult::Failed
                } else {
                    OperationResult::Completed
                }));
            }
            last_save = std::time::Instant::now();
        }

        // Background tasks: redraw when one changes (and each second while any
        // run, for elapsed times), announce notify=user finishes, and push
        // notify=model finishes to the model once the agent is idle.
        let background_revision = lethetic::background::revision();
        if background_revision != last_background_revision
            || (lethetic::background::running_count() > 0
                && last_background_tick.elapsed() >= Duration::from_secs(1))
        {
            last_background_revision = background_revision;
            last_background_tick = std::time::Instant::now();
            context
                .app
                .context_manager
                .set_background_summary(lethetic::background::context_summary());
            context.app.should_redraw = true;
            wfe_dirty = true;
        }
        if context.accepts_new_work() && context.app.is_fully_idle() {
            for task in lethetic::background::take_user_announcements() {
                context.app.add_segment(
                    format!("\n⏺ {}\n", lethetic::background::status_line(&task)),
                    lethetic::app::BlockType::Text,
                );
                context.app.should_redraw = true;
            }
            let finished = lethetic::background::take_model_notifications();
            if !finished.is_empty() {
                let notice = lethetic::background::finish_notice(&finished);
                let _ = context
                    .dispatch_app_event(lethetic::app::AppEventOutcome::SendPrompt(notice))
                    .await;
                context.app.should_redraw = true;
            }
        }

        context.app.accrue_session_time();
        if last_times_check.elapsed() >= Duration::from_secs(1) {
            last_times_check = std::time::Instant::now();
            let times = context.app.session_times;
            let label = [times.engine_ms, times.tool_ms, times.idle_ms]
                .map(lethetic::status_summary::format_compact_duration)
                .join("/");
            if label != last_times_label {
                last_times_label = label;
                context.app.should_redraw = true;
                wfe_dirty = true;
            }
        }
        if context.lifecycle.is_shutting_down() {
            crate::stream_events::abandon_provider_retry(context.app, "Retry dropped: shutting down");
        } else {
            crate::stream_events::run_due_provider_retry(&mut context);
        }

        let mut idle_tick = false;
        tokio::select! {
            biased;
            signal = async {
                if signal_events_open {
                    signal_supervisor.recv().await
                } else {
                    std::future::pending().await
                }
            } => {
                match signal {
                    Some(SupervisedSignalEvent::Signal(signal)) => match signal_action(mode, signal) {
                        #[cfg(unix)]
                        SignalAction::IgnoreHangup => {
                            context.app.log_runtime_debug("SERVICE_SIGHUP_IGNORED");
                            if let Some(runtime) = wfe_runtime.as_mut() {
                                runtime.record_operational(OperationalEvent::HangupIgnored);
                            }
                            if emit_console(
                                &mut console,
                                ServiceConsoleEvent::HangupIgnored,
                            ).is_err() {
                                context
                                    .app
                                    .log_runtime_debug("SERVICE_CONSOLE_WRITE_FAILED");
                            }
                        }
                        SignalAction::BeginShutdown(reason) => {
                            context.begin_shutdown(reason);
                        }
                    },
                    Some(SupervisedSignalEvent::ListenerFailed(error)) => {
                        signal_events_open = false;
                        context.begin_fatal_shutdown(
                            ShutdownReason::SignalListenerFailure,
                            format!("process signal listener failed: {error}"),
                        );
                    }
                    None => {
                        signal_events_open = false;
                        context.begin_fatal_shutdown(
                            ShutdownReason::SignalListenerFailure,
                            "process signal supervisor stopped unexpectedly",
                        );
                    }
                }
            }

            input = surface.next_input(), if context.accepts_new_work() => {
                let _ = surface.handle_input(&mut context, input).await;
            }

            status = receive_wfe_status(&mut wfe_status) => {
                match status {
                    lethetic::wfe::server::WfeServerStatus::Running => {}
                    lethetic::wfe::server::WfeServerStatus::Stopped { error } if in_session_server.is_some() => {
                        // A palette-started listener failing ends remote
                        // control, not the session.
                        wfe_status = None;
                        wfe_runtime = None;
                        in_session_server = None;
                        context.app.remote_control_target = None;
                        context.app.remote_control_clients = 0;
                        context.app.stop_reason = format!(
                            "⚠ Remote control stopped: {}",
                            error.unwrap_or_else(|| "listener closed".to_string())
                        );
                        context.app.should_redraw = true;
                    }
                    lethetic::wfe::server::WfeServerStatus::Stopped { error } => {
                        wfe_status = None;
                        context.begin_fatal_shutdown(
                            ShutdownReason::WfeFailure,
                            error.unwrap_or_else(|| {
                                "WFE HTTPS server stopped unexpectedly".to_string()
                            }),
                        );
                    }
                }
            }

            event = receive_wfe_event(&mut wfe_runtime) => {
                match event {
                    Some(WfeRuntimeEvent::Connection(event)) => {
                        if let Some(runtime) = wfe_runtime.as_mut() {
                            runtime.record_operational(connection_operational_event(&event));
                        }
                        record_wfe_connection(&mut context, &event);
                        if emit_console(
                            &mut console,
                            ServiceConsoleEvent::WfeConnection(event),
                        )
                        .is_err()
                        {
                            context
                                .app
                                .log_runtime_debug("SERVICE_CONSOLE_WRITE_FAILED");
                        }
                    }
                    Some(WfeRuntimeEvent::Command(envelope)) => {
                        if let Err(error) = process_wfe_command(
                            envelope,
                            &mut wfe_runtime,
                            &mut context,
                            &mut pending_wfe_session_load,
                        )
                        .await
                        {
                            wfe_runtime = None;
                            context.begin_fatal_shutdown(
                                ShutdownReason::WfeFailure,
                                format!("WFE command actor failed: {error}"),
                            );
                        }
                    }
                    None => {
                        wfe_runtime = None;
                        context.begin_fatal_shutdown(
                            ShutdownReason::WfeFailure,
                            "WFE runtime channels closed unexpectedly",
                        );
                    }
                }
            }

            settlement = context.settle_python_setup_operation(), if context.has_python_setup_operation() => {
                if let Some(settlement) = settlement {
                    handle_python_setup_settlement(&mut context, settlement).await;
                }
            }

            Some(mut event) = rx.recv() => {
                let mut processed = 0_usize;
                loop {
                    processed = processed.saturating_add(1);
                    if let Some(operational) = stream_operational_event(&event)
                        && let Some(runtime) = wfe_runtime.as_mut()
                    {
                        runtime.record_operational(operational);
                    }
                    let control = handle_stream_event(&mut context, event).await;
                    if let Err(error) = finish_pending_wfe_session_load(
                        &mut pending_wfe_session_load,
                        &mut wfe_runtime,
                        &mut context,
                    ) {
                        wfe_runtime = None;
                        context.begin_fatal_shutdown(
                            ShutdownReason::WfeFailure,
                            format!("WFE session-load completion failed: {error}"),
                        );
                    }
                    if control == StreamControl::BreakBatch
                        || processed >= MAX_STREAM_EVENTS_PER_TICK
                    {
                        break;
                    }
                    match rx.try_recv() {
                        Ok(next) => event = next,
                        Err(_) => break,
                    }
                }
            }

            _ = tokio::time::sleep(Duration::from_millis(16)) => {
                idle_tick = true;
                if context.app.is_processing
                    && last_tick.elapsed() >= Duration::from_millis(100)
                {
                    context.app.tick_spinner();
                    last_tick = std::time::Instant::now();
                }
            }
        }
        if !idle_tick {
            wfe_dirty = true;
        }
    }

    if let Some(server) = in_session_server.take()
        && let Err(error) = server.shutdown().await
    {
        context
            .lifecycle
            .record_error(format!("remote control shutdown failed: {error}"));
    }
    if let Err(error) = surface.settle_input() {
        context
            .lifecycle
            .record_error(format!("terminal input settlement failed: {error}"));
    }
    context.background_cancellation.cancel();
    lethetic::background::stop_all();
    if let Err(error) = signal_supervisor.shutdown().await {
        context.lifecycle.record_error(error);
    }
    let mut background = background;
    background.handles.append(&mut context.auxiliary_tasks);
    for error in settle_background_tasks(background).await {
        context.lifecycle.record_error(error);
    }
    let result = context
        .finish_shutdown()
        .map_err(|error| io::Error::other(error).into());
    drop(context);
    (surface, result)
}

fn connection_operational_event(
    event: &lethetic::wfe::runtime::WfeConnectionEvent,
) -> OperationalEvent {
    match event {
        lethetic::wfe::runtime::WfeConnectionEvent::Connected(event) => {
            OperationalEvent::ConnectionEstablished {
                active_clients: u32::try_from(event.active_clients).unwrap_or(u32::MAX),
            }
        }
        lethetic::wfe::runtime::WfeConnectionEvent::Disconnected(event) => {
            OperationalEvent::ConnectionEnded {
                category: event.category,
                active_clients: u32::try_from(event.active_clients).unwrap_or(u32::MAX),
            }
        }
    }
}

fn stream_operational_event(event: &StreamEvent) -> Option<OperationalEvent> {
    match event {
        StreamEvent::ToolCalls { .. } => Some(OperationalEvent::Tool(ToolPhase::Requested)),
        StreamEvent::ToolResult { is_error, .. } => Some(OperationalEvent::Tool(if *is_error {
            ToolPhase::Failed
        } else {
            ToolPhase::Completed
        })),
        StreamEvent::RequestStarted(_) => Some(OperationalEvent::Provider(ProviderPhase::Started)),
        StreamEvent::RequestFinished(_) => {
            Some(OperationalEvent::Provider(ProviderPhase::Completed))
        }
        StreamEvent::RequestSettlementFailed { .. } | StreamEvent::Error(_) => {
            Some(OperationalEvent::Provider(ProviderPhase::Failed))
        }
        StreamEvent::RequestCancelled { .. } => {
            Some(OperationalEvent::Provider(ProviderPhase::Cancelled))
        }
        StreamEvent::PersistRequestCheckpoint { .. } => Some(OperationalEvent::CheckpointRequested),
        #[cfg(target_os = "linux")]
        StreamEvent::RuntimeMaintenanceFinished(result) => {
            Some(OperationalEvent::Maintenance(match result {
                Ok(report) if report.errors.is_empty() => MaintenanceResult::Completed,
                Ok(_) => MaintenanceResult::Degraded,
                Err(_) => MaintenanceResult::Failed,
            }))
        }
        _ => None,
    }
}

fn shutdown_kind(reason: ShutdownReason) -> ShutdownKind {
    match reason {
        ShutdownReason::UserExit => ShutdownKind::UserExit,
        ShutdownReason::Interrupt => ShutdownKind::Interrupt,
        #[cfg(unix)]
        ShutdownReason::Terminate => ShutdownKind::Terminate,
        #[cfg(unix)]
        ShutdownReason::Hangup => ShutdownKind::Hangup,
        ShutdownReason::TerminalClosed => ShutdownKind::TerminalClosed,
        ShutdownReason::TerminalError => ShutdownKind::TerminalError,
        ShutdownReason::WfeFailure => ShutdownKind::WfeFailure,
        ShutdownReason::SignalListenerFailure => ShutdownKind::SignalListenerFailure,
    }
}

fn restore_cancelled_python_setup(app: &mut App, completion: &PythonSetupCompletion) {
    if let Some(setup) = app.python_setup.as_mut() {
        match completion {
            PythonSetupCompletion::Capabilities(_)
                if setup.stage == lethetic::python_setup::PythonSetupStage::Probing =>
            {
                setup.stage = setup
                    .previous_stage
                    .take()
                    .unwrap_or(lethetic::python_setup::PythonSetupStage::Profile);
            }
            PythonSetupCompletion::PolicyPrepared { .. } => {
                setup.stage = lethetic::python_setup::PythonSetupStage::Confirm;
            }
            PythonSetupCompletion::PullFinished(_)
                if setup.stage == lethetic::python_setup::PythonSetupStage::Pulling =>
            {
                setup.stage = setup
                    .previous_stage
                    .take()
                    .unwrap_or(lethetic::python_setup::PythonSetupStage::PodmanImage);
            }
            _ => {}
        }
        setup.error = None;
    }
    app.stop_reason = "Python setup cancelled".to_string();
    app.should_redraw = true;
}

async fn handle_python_setup_settlement(
    context: &mut RuntimeContext<'_>,
    settlement: PythonSetupSettlement,
) {
    let PythonSetupSettlement {
        completion,
        dismiss_when_settled,
        cancellation_requested,
    } = settlement;

    if context.lifecycle.is_shutting_down() || context.shutdown_cancellation.is_cancelled() {
        // The operation has joined, so its modal state must no longer keep the
        // shared idle predicate busy while shutdown containment is evaluated.
        context.app.python_setup = None;
        context.app.should_redraw = true;
        if let Err(error) = completion {
            context
                .lifecycle
                .record_error(format!("Python setup containment failed: {error}"));
        }
        return;
    }

    if dismiss_when_settled {
        context.app.python_setup = None;
        context.app.stop_reason = "Python setup cancelled".to_string();
        context.app.should_redraw = true;
        return;
    }

    if cancellation_requested {
        match completion {
            Ok(completion) => restore_cancelled_python_setup(context.app, &completion),
            Err(error) => {
                if let Some(setup) = context.app.python_setup.as_mut() {
                    setup.stage = setup
                        .previous_stage
                        .take()
                        .unwrap_or(lethetic::python_setup::PythonSetupStage::Confirm);
                    setup.error = Some(error.clone());
                }
                context.app.stop_reason = format!("⚠ {error}");
                context.app.should_redraw = true;
            }
        }
        return;
    }

    match completion {
        Ok(PythonSetupCompletion::Capabilities(Ok(capabilities))) => {
            let _ =
                handle_stream_event(context, StreamEvent::PythonCapabilities(capabilities)).await;
        }
        Ok(PythonSetupCompletion::Capabilities(Err(error))) => {
            if let Some(setup) = context.app.python_setup.as_mut()
                && setup.stage == lethetic::python_setup::PythonSetupStage::Probing
            {
                setup.stage = setup
                    .previous_stage
                    .take()
                    .unwrap_or(lethetic::python_setup::PythonSetupStage::Profile);
                setup.error = Some(error.clone());
            }
            context.app.stop_reason = format!("⚠ {error}");
            context.app.should_redraw = true;
        }
        Ok(PythonSetupCompletion::PolicyPrepared {
            snapshot,
            effective_snapshot,
            effective_source,
            persistence,
            expected_revision,
            validation,
        }) => {
            let _ = handle_stream_event(
                context,
                StreamEvent::PythonPolicyPrepared {
                    snapshot,
                    effective_snapshot: *effective_snapshot,
                    effective_source,
                    persistence,
                    expected_revision,
                    validation,
                },
            )
            .await;
        }
        Ok(PythonSetupCompletion::PullFinished(result)) => {
            let _ = handle_stream_event(context, StreamEvent::PythonPullFinished(result)).await;
        }
        Err(error) => {
            if let Some(setup) = context.app.python_setup.as_mut() {
                setup.stage = setup
                    .previous_stage
                    .take()
                    .unwrap_or(lethetic::python_setup::PythonSetupStage::Confirm);
                setup.error = Some(error.clone());
            }
            context.app.stop_reason = format!("⚠ {error}");
            context.app.should_redraw = true;
        }
    }
}

async fn receive_wfe_status(
    status: &mut Option<WfeStatusReceiver>,
) -> lethetic::wfe::server::WfeServerStatus {
    match status.as_mut() {
        Some(status) => match status.changed().await {
            Ok(()) => status.borrow().clone(),
            Err(_) => lethetic::wfe::server::WfeServerStatus::Stopped {
                error: Some("WFE server status channel closed".to_string()),
            },
        },
        None => std::future::pending().await,
    }
}

async fn receive_wfe_event(runtime: &mut Option<WfeRuntime>) -> Option<WfeRuntimeEvent> {
    match runtime.as_mut() {
        Some(runtime) => runtime.recv_event().await,
        None => std::future::pending().await,
    }
}

fn record_wfe_connection(
    context: &mut RuntimeContext<'_>,
    event: &lethetic::wfe::runtime::WfeConnectionEvent,
) {
    // Connection loss is telemetry only. It never controls the actor,
    // listener, authentication, in-flight work, or browser lifecycle.
    use lethetic::wfe::runtime::WfeConnectionEvent;
    match event {
        WfeConnectionEvent::Connected(connected) => {
            context.app.remote_control_clients = connected.active_clients;
            context.app.remote_control_last_peer = Some(connected.peer_ip.to_string());
        }
        WfeConnectionEvent::Disconnected(disconnected) => {
            context.app.remote_control_clients = disconnected.active_clients;
        }
    }
    context.app.should_redraw = true;
    context
        .app
        .log_runtime_debug(&format_wfe_connection_event(event));
}

async fn process_wfe_command(
    envelope: lethetic::wfe::actor::WfeCommandEnvelope,
    runtime: &mut Option<WfeRuntime>,
    context: &mut RuntimeContext<'_>,
    pending_load: &mut Option<PendingWfeSessionLoad>,
) -> Result<(), io::Error> {
    let runtime = runtime
        .as_mut()
        .expect("a WFE command requires an active runtime");
    let Some(admitted) = runtime.admit(context.app, envelope) else {
        return Ok(());
    };

    if !context.accepts_new_work() {
        runtime.publish(context.app).map_err(io::Error::other)?;
        runtime
            .complete(
                admitted,
                Err(wfe_failure(
                    CommandErrorCode::Busy,
                    "Lethetic is shutting down",
                    false,
                )),
            )
            .map_err(io::Error::other)?;
        return Ok(());
    }

    let disposition = execute_wfe_command(admitted, runtime, context).await;
    runtime.publish(context.app).map_err(io::Error::other)?;
    match disposition {
        WfeCommandDisposition::Complete { command, result } => {
            runtime
                .complete(command, result)
                .map_err(io::Error::other)?;
        }
        WfeCommandDisposition::AwaitSessionLoad(pending) => {
            if pending_load.is_some() {
                runtime
                    .complete(
                        pending.command,
                        Err(wfe_failure(
                            CommandErrorCode::Busy,
                            "Another session load is already pending",
                            true,
                        )),
                    )
                    .map_err(io::Error::other)?;
            } else {
                *pending_load = Some(pending);
            }
        }
    }
    Ok(())
}

fn finish_pending_wfe_session_load(
    pending: &mut Option<PendingWfeSessionLoad>,
    runtime: &mut Option<WfeRuntime>,
    context: &mut RuntimeContext<'_>,
) -> Result<(), io::Error> {
    if pending.is_none() || context.app.is_loading_session {
        return Ok(());
    }
    let pending = pending.take().expect("pending load was checked above");
    let result = if context.app.current_session_dir.is_some()
        && context.app.session_id == pending.session_id
    {
        Ok(CommandOutcome::SessionLoaded {
            session_id: pending.session_id,
        })
    } else {
        Err(wfe_failure(
            CommandErrorCode::BackendUnavailable,
            "Session loading did not complete safely",
            true,
        ))
    };
    let runtime = runtime.as_mut().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::BrokenPipe,
            "WFE runtime disappeared during session loading",
        )
    })?;
    runtime.clear_requested_panel();
    runtime.publish(context.app).map_err(io::Error::other)?;
    runtime
        .complete(pending.command, result)
        .map_err(io::Error::other)
}

struct BackgroundTasks {
    handles: Vec<tokio::task::JoinHandle<()>>,
}

fn spawn_background_tasks(context: &RuntimeContext<'_>) -> BackgroundTasks {
    let mut handles = Vec::new();
    let stats_tx = context.tx.clone();
    let stats_cancel = context.background_cancellation.clone();
    handles.push(tokio::spawn(async move {
        // The .lethetic walk is cheap but not free: refresh it every ~20 s.
        let mut lethetic_bytes: Option<u64> = None;
        let mut sample_index: u64 = 0;
        loop {
            if sample_index % 10 == 0 {
                lethetic_bytes = tokio::task::spawn_blocking(|| {
                    lethetic::platform::directory_size_bytes(std::path::Path::new(".lethetic"))
                })
                .await
                .ok()
                .flatten();
            }
            sample_index = sample_index.wrapping_add(1);
            let sample = async {
                let memory = lethetic::platform::process_rss_mb();
                let git = get_git_info().await;
                (memory, git)
            };
            let (memory, git) = tokio::select! {
                _ = stats_cancel.cancelled() => break,
                sample = sample => sample,
            };
            let size = lethetic_bytes
                .map(|bytes| bytes.to_string())
                .unwrap_or_default();
            if stats_tx
                .send(StreamEvent::DebugLog(format!(
                    "STATS|{memory}|{git}|{size}"
                )))
                .is_err()
            {
                break;
            }
            tokio::select! {
                _ = stats_cancel.cancelled() => break,
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            }
        }
    }));

    #[cfg(target_os = "linux")]
    {
        let maintenance_tx = context.tx.clone();
        let maintenance_cancel = context.background_cancellation.clone();
        handles.push(tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = maintenance_cancel.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_secs(60 * 60)) => {}
                }
                let result =
                    lethetic::python::retained_runtime::sweep_retained_runtimes_with_cancellation(
                        maintenance_cancel.clone(),
                    )
                    .await;
                if maintenance_cancel.is_cancelled() {
                    break;
                }
                if maintenance_tx
                    .send(StreamEvent::RuntimeMaintenanceFinished(result))
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    BackgroundTasks { handles }
}

async fn settle_background_tasks(tasks: BackgroundTasks) -> Vec<String> {
    let mut errors = Vec::new();
    for (index, mut handle) in tasks.handles.into_iter().enumerate() {
        match tokio::time::timeout(Duration::from_secs(3), &mut handle).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => errors.push(format!(
                "background task {} failed during shutdown: {error}",
                index + 1
            )),
            Err(_) => {
                handle.abort();
                let _ = handle.await;
                errors.push(format!(
                    "background task {} did not settle within the shutdown deadline",
                    index + 1
                ));
            }
        }
    }
    errors
}

fn emit_console(
    console: &mut Option<&mut ServiceConsole>,
    event: ServiceConsoleEvent,
) -> io::Result<()> {
    match console.as_deref_mut() {
        Some(console) => console.emit(event),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::PythonSetupOperation;

    #[test]
    fn raw_local_result_is_classified_from_execution_status_without_preprocessing() {
        let event = StreamEvent::ToolResult {
            id: None,
            func_name: "lsp_install".to_string(),
            result: "raw local output".to_string(),
            cwd: ".".to_string(),
            is_error: false,
            provenance: lethetic::tools::ToolOutputProvenance::OrdinaryHost,
        };

        assert!(matches!(
            stream_operational_event(&event),
            Some(OperationalEvent::Tool(ToolPhase::Completed))
        ));
        let StreamEvent::ToolResult { result, .. } = event else {
            unreachable!();
        };
        assert_eq!(result, "raw local output");
    }

    #[test]
    fn shutdown_parent_gates_work_and_cancels_child_operations_before_actor_dispatch() {
        let mut config = Config::default();
        let mut app = App::new(&config);
        let (tx, _rx) = mpsc::unbounded_channel();
        let context = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Service);
        let provider = context.cancellation_token.clone();
        let background = context.background_cancellation.clone();

        assert!(context.accepts_new_work());
        context.shutdown_cancellation.cancel();

        assert!(!context.accepts_new_work());
        assert!(provider.is_cancelled());
        assert!(background.is_cancelled());
    }

    #[tokio::test]
    async fn shutdown_cancels_and_joins_hanging_python_probe_before_containment() {
        struct DropWitness(std::sync::Arc<std::sync::atomic::AtomicBool>);
        impl Drop for DropWitness {
            fn drop(&mut self) {
                self.0.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }

        let workspace = tempfile::tempdir().unwrap();
        let workspace = workspace.path().canonicalize().unwrap();
        let mut config = Config::default();
        let mut app = App::new(&config);
        app.python_setup = Some(lethetic::python_setup::PythonSetupDialog::new(
            &config, workspace,
        ));
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut context = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Interactive);
        let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let dropped_by_probe = dropped.clone();
        PythonSetupOperation::start(
            &mut context.python_setup_operation,
            move |cancellation| async move {
                let _witness = DropWitness(dropped_by_probe);
                cancellation.cancelled().await;
                PythonSetupCompletion::Capabilities(Err(
                    "fake hanging probe was cancelled".to_string()
                ))
            },
        )
        .unwrap();

        context.begin_shutdown(ShutdownReason::UserExit);
        assert!(!context.shutdown_contained());
        let settlement = tokio::time::timeout(
            Duration::from_secs(1),
            context.settle_python_setup_operation(),
        )
        .await
        .expect("hanging probe did not settle after cancellation")
        .expect("tracked probe disappeared before join");
        handle_python_setup_settlement(&mut context, settlement).await;

        assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
        assert!(!context.has_python_setup_operation());
        assert!(!context.app.python_setup_is_busy());
        assert!(context.shutdown_contained());
        context.finish_shutdown().unwrap();
    }

    fn prepared_project_policy(
        config: &Config,
        workspace: &std::path::Path,
    ) -> (
        lethetic::python_setup::PythonSetupDialog,
        PythonSetupSettlement,
    ) {
        let snapshot = lethetic::python_policy::PythonPolicySnapshot::from_config(config);
        let (effective_snapshot, effective_source) =
            lethetic::python_policy::effective_policy_after_write(
                lethetic::python_policy::PythonPolicyScope::Project,
                workspace,
                &snapshot,
            )
            .unwrap();
        let mut setup =
            lethetic::python_setup::PythonSetupDialog::new(config, workspace.to_path_buf());
        setup.persistence = lethetic::python_setup::PolicyPersistence::Project;
        setup.project_revision = lethetic::python_policy::PolicyRevision::Missing;
        setup.stage = lethetic::python_setup::PythonSetupStage::Applying;
        let settlement = PythonSetupSettlement {
            completion: Ok(PythonSetupCompletion::PolicyPrepared {
                snapshot,
                effective_snapshot: Box::new(effective_snapshot),
                effective_source,
                persistence: lethetic::python_setup::PolicyPersistence::Project,
                expected_revision: Some(lethetic::python_policy::PolicyRevision::Missing),
                validation: Ok(()),
            }),
            dismiss_when_settled: false,
            cancellation_requested: true,
        };
        (setup, settlement)
    }

    #[tokio::test]
    async fn cancelled_policy_preparation_cannot_apply_after_success_race() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().canonicalize().unwrap();
        let mut config = Config::default();
        let mut app = App::new(&config);
        app.current_dir = workspace.to_string_lossy().into_owned();
        app.tool_runtime = lethetic::tool_runtime::ToolRuntime::interactive(workspace.clone());
        let (setup, settlement) = prepared_project_policy(&config, &workspace);
        app.python_setup = Some(setup);
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut context = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Interactive);

        handle_python_setup_settlement(&mut context, settlement).await;

        assert!(!lethetic::python_policy::project_policy_path(&workspace).exists());
        assert_eq!(
            context.app.python_setup.as_ref().map(|setup| setup.stage),
            Some(lethetic::python_setup::PythonSetupStage::Confirm)
        );
        assert_eq!(context.app.stop_reason, "Python setup cancelled");
    }

    #[tokio::test]
    async fn shutdown_discards_completed_policy_preparation_without_persisting() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().canonicalize().unwrap();
        let mut config = Config::default();
        let mut app = App::new(&config);
        app.current_dir = workspace.to_string_lossy().into_owned();
        app.tool_runtime = lethetic::tool_runtime::ToolRuntime::interactive(workspace.clone());
        let (setup, settlement) = prepared_project_policy(&config, &workspace);
        app.python_setup = Some(setup);
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut context = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Service);
        context.begin_shutdown(ShutdownReason::Interrupt);

        handle_python_setup_settlement(&mut context, settlement).await;

        assert!(!lethetic::python_policy::project_policy_path(&workspace).exists());
        assert!(context.lifecycle.is_shutting_down());
        assert_eq!(
            lethetic::python_policy::PythonPolicySnapshot::from_config(context.config),
            lethetic::python_policy::PythonPolicySnapshot::from_config(&Config::default())
        );
    }

    #[test]
    fn service_surface_has_no_terminal_state() {
        let surface = ActorSurface::service();
        assert_eq!(surface.mode(), RuntimeMode::Service);
        assert!(!matches!(surface, ActorSurface::Interactive(_)));
    }

    #[test]
    fn browser_disconnect_is_telemetry_only_and_raw_details_never_reach_wfe() {
        let mut config = Config::default();
        let mut app = App::new(&config);
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut context = RuntimeContext::new(&mut app, &mut config, tx, RuntimeMode::Service);
        let cancellation_was = context.cancellation_token.clone();
        let event = lethetic::wfe::runtime::WfeConnectionEvent::Disconnected(
            lethetic::wfe::runtime::WfeDisconnectedEvent {
                peer_ip: "2001:db8:face:feed::7".parse().unwrap(),
                connection_ordinal: 12,
                active_clients: 0,
                uptime: Duration::from_millis(2500),
                category: lethetic::wfe::runtime::WfeDisconnectCategory::PeerClosed,
            },
        );

        record_wfe_connection(&mut context, &event);
        context
            .app
            .log_debug("RAW_PROVIDER_ERROR raw-provider-request-private-73");

        assert!(!context.lifecycle.is_shutting_down());
        assert!(!cancellation_was.is_cancelled());
        assert_eq!(context.app.debug_log.len(), 2);
        assert!(context.app.debug_log[0].contains("peer-closed"));
        assert!(context.app.debug_log[0].contains("2001:db8:face:feed::7"));

        let (frontend, mut runtime) = WfeRuntime::new(context.app, Vec::new()).unwrap();
        runtime.record_operational(connection_operational_event(&event));
        runtime.record_operational(OperationalEvent::Provider(ProviderPhase::Failed));
        assert!(!runtime.publish(context.app).unwrap());
        let snapshot = frontend.mirror.latest_snapshot();
        assert_eq!(snapshot.sequence, 0);
        assert_eq!(snapshot.revision, 0);
        assert_eq!(
            snapshot.state.debugger.entries[0].message,
            "Provider request failed."
        );
        assert_eq!(
            snapshot.state.debugger.entries[1].message,
            "Browser controller disconnected normally. Active controllers: 0."
        );

        let encoded = serde_json::to_string(&*snapshot).unwrap();
        for private in [
            "2001:db8:face:feed::7",
            "raw-provider-request-private-73",
            "RAW_PROVIDER_ERROR",
        ] {
            assert!(
                !encoded.contains(private),
                "serialized private value {private}"
            );
        }
    }
}

async fn handle_remote_control_request(
    request: crate::context::RemoteControlRequest,
    context: &mut RuntimeContext<'_>,
    wfe_runtime: &mut Option<WfeRuntime>,
    wfe_status: &mut Option<WfeStatusReceiver>,
    in_session_server: &mut Option<lethetic::wfe::server::WfeServerHandle>,
) {
    use crate::context::RemoteControlRequest;
    context.app.should_redraw = true;
    match request {
        RemoteControlRequest::Start { .. } if wfe_runtime.is_some() => {
            context.app.stop_reason = "Remote control is already running".to_string();
        }
        RemoteControlRequest::Start {
            target,
            open,
            files,
        } => {
            match crate::wfe_startup::start_in_session(
                context.app,
                context.config,
                &target,
                open,
                files,
            )
            .await
            {
                Ok(started) => {
                    *wfe_runtime = Some(started.runtime);
                    *wfe_status = Some(started.status);
                    *in_session_server = Some(started.server);
                    context.app.remote_control_target = Some(target.clone());
                    context.app.remote_control_open = open;
                    context.app.remote_control_files = files;
                    context.app.remote_control_clients = 0;
                    context.app.remote_control_last_peer = None;
                    context.app.rc_info = Some(lethetic::app::RcInfoState {
                        lines: started.lines,
                        url: started.url,
                    });
                    context.app.stop_reason = format!("Remote control running at {target}");
                }
                Err(error) => {
                    context.app.stop_reason = format!("✗ Remote control failed: {error}");
                }
            }
        }
        RemoteControlRequest::Stop => {
            let Some(server) = in_session_server.take() else {
                context.app.stop_reason =
                    "Remote control was not started from the palette".to_string();
                return;
            };
            *wfe_runtime = None;
            *wfe_status = None;
            context.app.remote_control_target = None;
            context.app.remote_control_clients = 0;
            context.app.stop_reason = match server.shutdown().await {
                Ok(()) => "Remote control stopped".to_string(),
                Err(error) => format!("⚠ Remote control stopped with an error: {error}"),
            };
        }
    }
}
