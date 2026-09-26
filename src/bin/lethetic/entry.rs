use std::{
    env,
    error::Error,
    future::Future,
    io,
    path::{Path, PathBuf},
    pin::Pin,
};

#[cfg(target_os = "linux")]
use std::time::Duration;

use lethetic::app::App;
use lethetic::config::Config;

use crate::cli::{Cli, LiteralPythonMode, apply_literal_python_mode};
use crate::headless;
use crate::internal;
use crate::lifecycle::{RuntimeMode, ShutdownReason, SignalAction, SignalListener, signal_action};
use crate::line_reader::BootstrapLineReader;
use crate::runtime::{ActorSurface, run_actor};
use crate::service_console::{ServiceConsole, ServiceConsoleEvent};
use crate::terminal::{InteractiveTerminal, setup_panic_hook};
use crate::wfe_startup::{self, PreparedWfe, StartedWfe, WfeStartOutcome};

pub(crate) async fn run() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<String> = env::args().collect();
    if internal::run_if_requested(&arguments).await? {
        return Ok(());
    }

    let cli = Cli::parse(&arguments[1..])
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    if cli.help {
        Cli::print_help();
        return Ok(());
    }

    let signals = if cli.command.is_none() {
        Some(SignalListener::new()?)
    } else {
        None
    };
    run_cli(cli, signals).await
}

async fn run_cli(
    mut cli: Cli,
    mut startup_signals: Option<SignalListener>,
) -> Result<(), Box<dyn Error>> {
    if cli.rc_wizard {
        let signals = startup_signals
            .as_mut()
            .expect("remote-control chooser runs only in foreground mode");
        match crate::rc_wizard::run(&mut cli, signals).await? {
            crate::rc_wizard::WizardOutcome::Configured => {}
            crate::rc_wizard::WizardOutcome::Shutdown(reason) => {
                return finish_startup_without_app(None, reason, false);
            }
        }
        cli.rc_wizard = false;
    }
    let interactive = cli.command.is_none() && !cli.service;
    setup_panic_hook(interactive);
    let frontend_mode = cli.command.is_none().then_some(if cli.service {
        RuntimeMode::Service
    } else {
        RuntimeMode::Interactive
    });
    let mut startup_console = cli.service.then(ServiceConsole::stdout);

    let config_path = if Path::new("config.yml").exists() {
        PathBuf::from("config.yml")
    } else {
        lethetic::platform::lethetic_config_dir().join("config.yml")
    };
    let workspace_root = env::current_dir()?;
    let mut wfe_files = wfe_startup::pin_files(&cli, &workspace_root)?;
    let mut config = Config::load(&config_path)?;
    wfe_startup::protect_files(&mut wfe_files, &cli, &workspace_root, &config_path, &config)?;
    let python_policy_source =
        lethetic::python_policy::load_resolved_policy(&mut config, &workspace_root)?;
    config.merge_matching_server_settings();
    // Models and prices saved from the model picker's catalog scan.
    lethetic::saved_models::SavedModels::load().apply(&mut config);
    // Reopen with the model last selected in this directory.
    config.restore_last_model(&workspace_root);
    let mut python_policy =
        lethetic::python_policy::PythonPolicyState::from_config(&config, python_policy_source);
    if let Some(mode) = cli.python_mode {
        apply_literal_python_mode(&mut config, mode)?;
        python_policy = python_policy.with_process_literal(&config)?;
    }

    if let Some(mode) = frontend_mode {
        let signals = startup_signals
            .as_mut()
            .expect("foreground mode installed startup signals");
        if let Some(reason) = drain_startup_signals(signals, mode, startup_console.as_mut()) {
            return finish_startup_without_app(startup_console.as_mut(), reason, cli.service);
        }
        let maintenance_cancellation = tokio_util::sync::CancellationToken::new();
        match await_cancellable_startup_stage(
            perform_startup_runtime_maintenance(cli.service, maintenance_cancellation.clone()),
            maintenance_cancellation,
            signals,
            mode,
            startup_console.as_mut(),
        )
        .await?
        {
            CancellableStartupStage::Completed(()) => {}
            CancellableStartupStage::Shutdown(reason) => {
                return finish_startup_without_app(startup_console.as_mut(), reason, cli.service);
            }
        }
    } else {
        perform_startup_runtime_maintenance(
            cli.service,
            tokio_util::sync::CancellationToken::new(),
        )
        .await;
    }

    if let Some(prompt) = cli.command.clone() {
        return run_headless_with_maintenance(&config, prompt, &cli).await;
    }

    #[cfg(not(target_os = "linux"))]
    if cli.new_session || cli.session_id.is_some() {
        return Err(
            "persistent sessions are disabled on this platform because secure session locking is available only on Linux"
                .into(),
        );
    }

    let mode = frontend_mode.expect("headless mode returned before foreground startup");
    let signals = startup_signals
        .as_mut()
        .expect("foreground mode installed startup signals");
    let prepared_wfe = match await_startup_stage(
        wfe_startup::prepare(&cli, wfe_files),
        signals,
        mode,
        startup_console.as_mut(),
    )
    .await?
    {
        StartupStage::Completed(prepared) => prepared?,
        StartupStage::Shutdown(reason) => {
            return finish_startup_without_app(startup_console.as_mut(), reason, cli.service);
        }
    };
    let mut app = build_app(&config, python_policy, &cli)?;

    #[cfg(target_os = "linux")]
    if let Some(session_id) = cli.session_id.as_deref() {
        let resume_cancellation = tokio_util::sync::CancellationToken::new();
        let resume = await_cancellable_startup_stage(
            app.resume_registered_session_with_cancellation(
                session_id,
                resume_cancellation.clone(),
            ),
            resume_cancellation,
            signals,
            mode,
            startup_console.as_mut(),
        )
        .await;
        match resume {
            Ok(CancellableStartupStage::Completed(Ok(()))) => {}
            Ok(CancellableStartupStage::Completed(Err(error))) => {
                return finalize(
                    &mut app,
                    Err(io::Error::other(error).into()),
                    None,
                    None,
                    startup_console.as_mut(),
                    cli.service,
                )
                .await;
            }
            Ok(CancellableStartupStage::Shutdown(reason)) => {
                announce_startup_shutdown(startup_console.as_mut(), reason);
                return finalize(
                    &mut app,
                    Ok(()),
                    None,
                    None,
                    startup_console.as_mut(),
                    cli.service,
                )
                .await;
            }
            Err(error) => {
                announce_startup_shutdown(
                    startup_console.as_mut(),
                    ShutdownReason::SignalListenerFailure,
                );
                return finalize(
                    &mut app,
                    Err(error.into()),
                    None,
                    None,
                    startup_console.as_mut(),
                    cli.service,
                )
                .await;
            }
        }
    }

    if let Some(reason) = drain_startup_signals(signals, mode, startup_console.as_mut()) {
        announce_startup_shutdown(startup_console.as_mut(), reason);
        return finalize(
            &mut app,
            Ok(()),
            None,
            None,
            startup_console.as_mut(),
            cli.service,
        )
        .await;
    }

    let signals = startup_signals
        .take()
        .expect("foreground startup retained its signal listener");
    run_frontend(
        &mut app,
        &mut config,
        prepared_wfe,
        mode,
        signals,
        startup_console,
    )
    .await
}

enum StartupStage<T> {
    Completed(T),
    Shutdown(ShutdownReason),
}

enum CancellableStartupStage<T> {
    Completed(T),
    Shutdown(ShutdownReason),
}

trait StartupSignalSource {
    fn recv_startup(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = io::Result<crate::lifecycle::ProcessSignal>> + '_>>;
}

impl StartupSignalSource for SignalListener {
    fn recv_startup(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = io::Result<crate::lifecycle::ProcessSignal>> + '_>> {
        Box::pin(self.recv())
    }
}

async fn await_startup_stage<F>(
    future: F,
    signals: &mut impl StartupSignalSource,
    mode: RuntimeMode,
    mut console: Option<&mut ServiceConsole>,
) -> io::Result<StartupStage<F::Output>>
where
    F: Future,
{
    tokio::pin!(future);
    loop {
        tokio::select! {
            biased;
            signal = signals.recv_startup() => {
                if let Some(reason) = handle_startup_signal(
                    mode,
                    signal?,
                    console.as_deref_mut(),
                ) {
                    return Ok(StartupStage::Shutdown(reason));
                }
            }
            result = &mut future => return Ok(StartupStage::Completed(result)),
        }
    }
}

async fn await_cancellable_startup_stage<F>(
    future: F,
    cancellation: tokio_util::sync::CancellationToken,
    signals: &mut impl StartupSignalSource,
    mode: RuntimeMode,
    mut console: Option<&mut ServiceConsole>,
) -> io::Result<CancellableStartupStage<F::Output>>
where
    F: Future,
{
    tokio::pin!(future);
    loop {
        tokio::select! {
            biased;
            signal = signals.recv_startup() => {
                let signal = match signal {
                    Ok(signal) => signal,
                    Err(error) => {
                        cancellation.cancel();
                        let _ = (&mut future).await;
                        return Err(error);
                    }
                };
                if let Some(reason) = handle_startup_signal(
                    mode,
                    signal,
                    console.as_deref_mut(),
                ) {
                    cancellation.cancel();
                    let _ = (&mut future).await;
                    return Ok(CancellableStartupStage::Shutdown(reason));
                }
            }
            result = &mut future => return Ok(CancellableStartupStage::Completed(result)),
        }
    }
}

fn drain_startup_signals(
    signals: &mut SignalListener,
    mode: RuntimeMode,
    mut console: Option<&mut ServiceConsole>,
) -> Option<ShutdownReason> {
    while let Some(signal) = signals.try_recv() {
        if let Some(reason) = handle_startup_signal(mode, signal, console.as_deref_mut()) {
            return Some(reason);
        }
    }
    None
}

fn handle_startup_signal(
    mode: RuntimeMode,
    signal: crate::lifecycle::ProcessSignal,
    console: Option<&mut ServiceConsole>,
) -> Option<ShutdownReason> {
    #[cfg(not(unix))]
    let _ = console;
    match signal_action(mode, signal) {
        #[cfg(unix)]
        SignalAction::IgnoreHangup => {
            if let Some(console) = console {
                let _ = console.emit(ServiceConsoleEvent::HangupIgnored);
            }
            None
        }
        SignalAction::BeginShutdown(reason) => Some(reason),
    }
}

fn announce_startup_shutdown(console: Option<&mut ServiceConsole>, reason: ShutdownReason) {
    if let Some(console) = console {
        let _ = console.emit(ServiceConsoleEvent::ShutdownRequested(reason));
    }
}

fn finish_startup_without_app(
    mut console: Option<&mut ServiceConsole>,
    reason: ShutdownReason,
    service: bool,
) -> Result<(), Box<dyn Error>> {
    announce_startup_shutdown(console.as_deref_mut(), reason);
    let mut errors = Vec::new();
    if let Some(console) = console
        && let Err(error) = console.emit(ServiceConsoleEvent::ShutdownComplete)
    {
        errors.push(format!("service console finalization failed: {error}"));
    }
    finish_with_errors(errors, service)
}

fn build_app(
    config: &Config,
    python_policy: lethetic::python_policy::PythonPolicyState,
    cli: &Cli,
) -> Result<App, Box<dyn Error>> {
    let mut app = App::new_with_python_policy_state(config, python_policy);
    // Launch flags force remote control; the palette cannot change it.
    app.remote_control_target = cli.wfe_remote_control.clone();
    app.remote_control_locked = cli.wfe_remote_control.is_some();
    app.remote_control_open = cli.wfe_disable_authtoken;
    app.remote_control_files = cli.wfe_files.is_some();
    if (cli.new_session || cli.python_mode == Some(LiteralPythonMode::Nonlocal))
        && cli.session_id.is_none()
        && app.current_session_dir.is_none()
    {
        app.start_new_session_checked()?;
        app.show_session_manager = false;
    }
    Ok(app)
}

async fn run_frontend(
    app: &mut App,
    config: &mut Config,
    prepared_wfe: Option<PreparedWfe>,
    mode: RuntimeMode,
    mut signals: SignalListener,
    mut console: Option<ServiceConsole>,
) -> Result<(), Box<dyn Error>> {
    let service = mode == RuntimeMode::Service;
    if let Some(reason) = drain_startup_signals(&mut signals, mode, console.as_mut()) {
        announce_startup_shutdown(console.as_mut(), reason);
        return finalize(app, Ok(()), None, None, console.as_mut(), service).await;
    }

    let started = match prepared_wfe {
        Some(prepared) => {
            match wfe_startup::start(prepared, app, mode, &mut signals, console.as_mut()).await {
                Ok(WfeStartOutcome::Started(started)) => Some(*started),
                Ok(WfeStartOutcome::Shutdown { reason, server }) => {
                    announce_startup_shutdown(console.as_mut(), reason);
                    return finalize(app, Ok(()), server, None, console.as_mut(), service).await;
                }
                Err(error) => {
                    return finalize(
                        app,
                        Err(error.into()),
                        None,
                        None,
                        console.as_mut(),
                        service,
                    )
                    .await;
                }
            }
        }
        None => None,
    };
    if service && started.is_none() {
        return finalize(
            app,
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "--service requires an active WFE listener",
            )
            .into()),
            None,
            None,
            console.as_mut(),
            true,
        )
        .await;
    }

    let (wfe_runtime, wfe_status, mut wfe_server) = split_started_wfe(started);
    if let Some(reason) = drain_startup_signals(&mut signals, mode, console.as_mut()) {
        announce_startup_shutdown(console.as_mut(), reason);
        return finalize(app, Ok(()), wfe_server, None, console.as_mut(), service).await;
    }
    let surface = {
        let surface_result = if service {
            Ok(ActorSurface::service())
        } else if wfe_runtime.is_some() {
            wfe_startup::print_browser_control_active()
                .and_then(|()| BootstrapLineReader::spawn().map(ActorSurface::awaiting_tui))
        } else {
            ActorSurface::interactive()
        };
        match surface_result {
            Ok(surface) => surface,
            Err(error) => {
                return finalize(
                    app,
                    Err(error.into()),
                    wfe_server.take(),
                    None,
                    console.as_mut(),
                    service,
                )
                .await;
            }
        }
    };

    let (surface, actor_result) = run_actor(
        app,
        config,
        wfe_runtime,
        wfe_status,
        surface,
        signals,
        console.as_mut(),
    )
    .await;
    let mut terminal = surface.into_terminal();

    finalize(
        app,
        actor_result,
        wfe_server,
        terminal.as_mut(),
        console.as_mut(),
        service,
    )
    .await
}

fn split_started_wfe(
    started: Option<StartedWfe>,
) -> (
    Option<lethetic::wfe::runtime::WfeRuntime>,
    Option<tokio::sync::watch::Receiver<lethetic::wfe::server::WfeServerStatus>>,
    Option<lethetic::wfe::server::WfeServerHandle>,
) {
    match started {
        Some(started) => (
            Some(started.runtime),
            Some(started.status),
            Some(started.server),
        ),
        None => (None, None, None),
    }
}

async fn finalize(
    app: &mut App,
    actor_result: Result<(), Box<dyn Error>>,
    wfe_server: Option<lethetic::wfe::server::WfeServerHandle>,
    terminal: Option<&mut InteractiveTerminal>,
    console: Option<&mut ServiceConsole>,
    service: bool,
) -> Result<(), Box<dyn Error>> {
    let mut errors = Vec::new();
    if let Err(error) = actor_result {
        errors.push(error.to_string());
    }
    if let Err(error) = app.save_session_checked() {
        errors.push(format!("final session save failed: {error}"));
    }
    if let Err(error) = app.tool_runtime.unbind_session_checked().await {
        errors.push(format!("final Python runtime detach failed: {error}"));
    }
    if let Some(server) = wfe_server
        && let Err(error) = server.shutdown().await
    {
        errors.push(error);
    }
    if let Some(terminal) = terminal
        && let Err(error) = terminal.restore()
    {
        errors.push(format!("terminal restoration failed: {error}"));
    }
    if let Some(console) = console
        && let Err(error) = console.emit(ServiceConsoleEvent::ShutdownComplete)
    {
        errors.push(format!("service console finalization failed: {error}"));
    }

    finish_with_errors(errors, service)
}

fn finish_with_errors(errors: Vec<String>, service: bool) -> Result<(), Box<dyn Error>> {
    if errors.is_empty() {
        return Ok(());
    }
    if service {
        return Err(io::Error::other("Lethetic browser service failed").into());
    }
    let error = io::Error::other(errors.join("; "));
    eprintln!("{error}");
    Err(error.into())
}

#[cfg(target_os = "linux")]
async fn perform_startup_runtime_maintenance(
    service: bool,
    cancellation: tokio_util::sync::CancellationToken,
) {
    match lethetic::python::retained_runtime::sweep_retained_runtimes_with_cancellation(
        cancellation,
    )
    .await
    {
        Ok(report) => {
            if !service && (report.reconciled > 0 || report.removed > 0) {
                eprintln!(
                    "Lethetic Python runtime maintenance: {} reconciled, {} expired removed",
                    report.reconciled, report.removed
                );
            }
            if !service {
                for error in report.errors {
                    eprintln!("Lethetic Python runtime maintenance warning: {error}");
                }
            }
        }
        Err(error) if !service => {
            eprintln!("Lethetic Python runtime maintenance failed: {error}")
        }
        Err(_) => {}
    }
}

#[cfg(not(target_os = "linux"))]
async fn perform_startup_runtime_maintenance(
    _service: bool,
    _cancellation: tokio_util::sync::CancellationToken,
) {
}

async fn run_headless_with_maintenance(
    config: &Config,
    prompt: String,
    cli: &Cli,
) -> Result<(), Box<dyn Error>> {
    #[cfg(target_os = "linux")]
    let maintenance_cancellation = tokio_util::sync::CancellationToken::new();
    #[cfg(target_os = "linux")]
    let maintenance_task_cancellation = maintenance_cancellation.clone();
    #[cfg(target_os = "linux")]
    let mut maintenance_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;
                _ = maintenance_task_cancellation.cancelled() => break,
                _ = tokio::time::sleep(Duration::from_secs(60 * 60)) => {}
            }
            let result =
                lethetic::python::retained_runtime::sweep_retained_runtimes_with_cancellation(
                    maintenance_task_cancellation.clone(),
                )
                .await;
            if maintenance_task_cancellation.is_cancelled() {
                break;
            }
            match result {
                Ok(report) => {
                    for error in report.errors {
                        eprintln!("Lethetic Python runtime maintenance warning: {error}");
                    }
                }
                Err(error) => eprintln!("Lethetic Python runtime maintenance failed: {error}"),
            }
        }
    });

    let result = headless::run(config, prompt, cli).await;
    #[cfg(target_os = "linux")]
    {
        maintenance_cancellation.cancel();
        if tokio::time::timeout(Duration::from_secs(3), &mut maintenance_task)
            .await
            .is_err()
        {
            maintenance_task.abort();
            let _ = maintenance_task.await;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockStartupSignals {
        receiver: tokio::sync::mpsc::UnboundedReceiver<crate::lifecycle::ProcessSignal>,
    }

    impl StartupSignalSource for MockStartupSignals {
        fn recv_startup(
            &mut self,
        ) -> Pin<Box<dyn Future<Output = io::Result<crate::lifecycle::ProcessSignal>> + '_>>
        {
            Box::pin(async {
                self.receiver.recv().await.ok_or_else(|| {
                    io::Error::new(io::ErrorKind::BrokenPipe, "mock signal channel closed")
                })
            })
        }
    }

    #[tokio::test]
    async fn stalled_session_resume_is_cancelled_and_settled_before_startup_shutdown() {
        struct DropWitness {
            settled: std::sync::Arc<std::sync::atomic::AtomicBool>,
            dropped_before_settlement: std::sync::Arc<std::sync::atomic::AtomicBool>,
        }
        impl Drop for DropWitness {
            fn drop(&mut self) {
                if !self.settled.load(std::sync::atomic::Ordering::SeqCst) {
                    self.dropped_before_settlement
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                }
            }
        }

        let cancellation = tokio_util::sync::CancellationToken::new();
        let future_cancellation = cancellation.clone();
        let settled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let future_settled = settled.clone();
        let dropped_before_settlement =
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let future_dropped_before_settlement = dropped_before_settlement.clone();
        let resume = async move {
            let _witness = DropWitness {
                settled: future_settled.clone(),
                dropped_before_settlement: future_dropped_before_settlement,
            };
            future_cancellation.cancelled().await;
            tokio::task::yield_now().await;
            future_settled.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok::<(), String>(())
        };
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        sender
            .send(crate::lifecycle::ProcessSignal::Interrupt)
            .unwrap();
        let mut signals = MockStartupSignals { receiver };

        let stage = await_cancellable_startup_stage(
            resume,
            cancellation,
            &mut signals,
            RuntimeMode::Interactive,
            None,
        )
        .await
        .unwrap();

        assert!(matches!(
            stage,
            CancellableStartupStage::Shutdown(ShutdownReason::Interrupt)
        ));
        assert!(settled.load(std::sync::atomic::Ordering::SeqCst));
        assert!(
            !dropped_before_settlement.load(std::sync::atomic::Ordering::SeqCst),
            "stalled resumed-session future was dropped before containment"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn session_resume_signal_contains_fake_runtime_child_and_output_reader() {
        use std::os::unix::fs::PermissionsExt as _;
        use std::sync::atomic::{AtomicBool, Ordering};
        use tokio::io::AsyncReadExt as _;

        struct ReaderDropWitness(std::sync::Arc<AtomicBool>);
        impl Drop for ReaderDropWitness {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-runtime.sh");
        std::fs::write(
            &executable,
            "#!/bin/sh\nprintf 'fake runtime diagnostic\\n' >&2\nexec /bin/sleep 30\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&executable, permissions).unwrap();

        let cancellation = tokio_util::sync::CancellationToken::new();
        let future_cancellation = cancellation.clone();
        let child_started = std::sync::Arc::new(AtomicBool::new(false));
        let future_child_started = child_started.clone();
        let child_pid = std::sync::Arc::new(std::sync::Mutex::new(None));
        let future_child_pid = child_pid.clone();
        let reader_dropped = std::sync::Arc::new(AtomicBool::new(false));
        let future_reader_dropped = reader_dropped.clone();
        let resume_settled = std::sync::Arc::new(AtomicBool::new(false));
        let future_resume_settled = resume_settled.clone();
        let resume = async move {
            let mut child = tokio::process::Command::new(executable)
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .map_err(|error| error.to_string())?;
            *future_child_pid.lock().unwrap() = child.id();
            let mut stderr = child.stderr.take().expect("fake runtime stderr missing");
            let diagnostics = tokio::spawn(async move {
                let _witness = ReaderDropWitness(future_reader_dropped);
                let mut output = Vec::new();
                let _ = stderr.read_to_end(&mut output).await;
            });
            future_child_started.store(true, Ordering::SeqCst);

            future_cancellation.cancelled().await;
            let _ = child.start_kill();
            let _ = child.wait().await;
            diagnostics.abort();
            let _ = diagnostics.await;
            future_resume_settled.store(true, Ordering::SeqCst);
            Err::<(), String>("fake runtime startup cancelled".to_string())
        };

        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let signal_sender = tokio::spawn(async move {
            while !child_started.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
            sender
                .send(crate::lifecycle::ProcessSignal::Terminate)
                .unwrap();
        });
        let mut signals = MockStartupSignals { receiver };

        let stage = await_cancellable_startup_stage(
            resume,
            cancellation,
            &mut signals,
            RuntimeMode::Interactive,
            None,
        )
        .await
        .unwrap();
        signal_sender.await.unwrap();

        assert!(matches!(
            stage,
            CancellableStartupStage::Shutdown(ShutdownReason::Terminate)
        ));
        assert!(resume_settled.load(Ordering::SeqCst));
        assert!(reader_dropped.load(Ordering::SeqCst));
        let pid = child_pid
            .lock()
            .unwrap()
            .expect("fake runtime never spawned");
        let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
        assert_eq!(result, -1, "fake runtime child {pid} survived shutdown");
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH),
            "fake runtime child {pid} still exists"
        );
    }
}
