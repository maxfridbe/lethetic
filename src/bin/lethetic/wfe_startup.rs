use std::io::{self, IsTerminal, Write as _};
use std::path::Path;

use lethetic::config::Config;
use lethetic::wfe::files::{DisclosurePolicy, RootedFiles};

use lethetic::app::App;
use lethetic::wfe::diagnostics::OperationalEvent;
use lethetic::wfe::runtime::WfeRuntime;
use lethetic::wfe::security::{
    ControllerAuthenticationMode, PreparedSecurity, SecurityFileOptions, SecurityProfile,
    WfeTarget, prepare_security_with_authentication,
};

use crate::cli::Cli;
use crate::formatting::{format_wfe_listener_addresses, validate_wfe_bootstrap_acknowledgement};
use crate::lifecycle::{RuntimeMode, ShutdownReason, SignalAction, SignalListener, signal_action};
use crate::line_reader::BootstrapLineReader;
use crate::service_console::ServiceConsole;
#[cfg(unix)]
use crate::service_console::ServiceConsoleEvent;

pub(crate) struct PreparedWfe {
    security: PreparedSecurity,
    bind_plan: lethetic::wfe::server::WfeBindPlan,
    files: Option<RootedFiles>,
}

pub(crate) struct StartedWfe {
    pub(crate) runtime: WfeRuntime,
    pub(crate) server: lethetic::wfe::server::WfeServerHandle,
    pub(crate) status: tokio::sync::watch::Receiver<lethetic::wfe::server::WfeServerStatus>,
}

pub(crate) enum WfeStartOutcome {
    Started(Box<StartedWfe>),
    Shutdown {
        reason: ShutdownReason,
        server: Option<lethetic::wfe::server::WfeServerHandle>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BootstrapOutcome {
    Acknowledged { ignored_hangups: u32 },
    Shutdown(ShutdownReason),
}

pub(crate) fn pin_files(cli: &Cli, launch_root: &Path) -> io::Result<Option<RootedFiles>> {
    if cli.wfe_files.is_none() {
        return Ok(None);
    }
    RootedFiles::open(launch_root, DisclosurePolicy::default())
        .map(Some)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput,
            "--wfe-files localonly requires an eligible launch directory and Linux openat2 confinement"))
}

pub(crate) fn protect_files(
    files: &mut Option<RootedFiles>,
    cli: &Cli,
    launch_root: &Path,
    config_path: &Path,
    config: &Config,
) -> io::Result<()> {
    let Some(files) = files.as_mut() else {
        return Ok(());
    };
    let mut paths = vec![
        config_path.to_path_buf(),
        config_path.with_file_name("config.local.yml"),
        lethetic::platform::lethetic_config_dir(),
        lethetic::platform::lethetic_state_dir(),
    ];
    paths.extend(
        [
            cli.wfe_tls_cert.as_ref(),
            cli.wfe_tls_key.as_ref(),
            cli.wfe_auth_token_file.as_ref(),
        ]
        .into_iter()
        .flatten()
        .cloned(),
    );
    if let Some(config_root) = dirs::config_dir() {
        for name in ["gcloud", "gh", "containers", "claude-code-proxy"] {
            paths.push(config_root.join(name));
        }
    }
    if let Some(home) = dirs::home_dir() {
        for name in [
            ".claude", ".codex", ".ssh", ".aws", ".azure", ".kube", ".docker",
        ] {
            paths.push(home.join(name));
        }
    }
    for path in paths {
        let path = if path.is_absolute() {
            path
        } else {
            launch_root.join(path)
        };
        files.protect_path(&path).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "could not establish the WFE file disclosure boundary",
            )
        })?;
    }
    for secret in config.api_key.as_deref().into_iter().chain(
        config
            .model_servers
            .iter()
            .filter_map(|server| server.api_key.as_deref()),
    ) {
        files.register_secret(secret).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "could not protect configured file-service credentials",
            )
        })?;
    }
    Ok(())
}

pub(crate) async fn prepare(
    cli: &Cli,
    files: Option<RootedFiles>,
) -> Result<Option<PreparedWfe>, io::Error> {
    let Some(value) = cli.wfe_remote_control.as_deref() else {
        return Ok(None);
    };
    require_attached_terminal(cli.service)?;

    let target = WfeTarget::parse(value)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let bind_plan = lethetic::wfe::server::resolve_target(&target)
        .await
        .map_err(io::Error::other)?;
    let security_files = SecurityFileOptions::new(
        cli.wfe_tls_cert.clone(),
        cli.wfe_tls_key.clone(),
        cli.wfe_auth_token_file.clone(),
    );
    let authentication = if cli.wfe_disable_authtoken {
        ControllerAuthenticationMode::Disabled
    } else {
        ControllerAuthenticationMode::TokenRequired
    };
    let security =
        prepare_security_with_authentication(&target, security_files, authentication, None)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    Ok(Some(PreparedWfe {
        security,
        bind_plan,
        files,
    }))
}

fn require_attached_terminal(service: bool) -> io::Result<()> {
    if io::stdin().is_terminal() && io::stdout().is_terminal() {
        return Ok(());
    }
    let message = if service {
        "--service requires an attached terminal for secure controller startup"
    } else {
        "--wfe-remote-control requires an attached terminal for secure controller startup"
    };
    Err(io::Error::new(io::ErrorKind::InvalidInput, message))
}

pub(crate) async fn start(
    prepared: PreparedWfe,
    app: &App,
    mode: RuntimeMode,
    signals: &mut SignalListener,
    mut console: Option<&mut ServiceConsole>,
) -> Result<WfeStartOutcome, io::Error> {
    let PreparedWfe {
        security,
        bind_plan,
        files,
    } = prepared;
    let authentication = security.controller_authentication_mode();
    let mut ignored_hangups = 0_u32;
    if files.is_some() {
        let mut output = io::stdout().lock();
        writeln!(
            output,
            "WFE files: read-only viewing and downloads share eligible files below the fixed launch directory; protected files and links are excluded."
        )?;
        writeln!(
            output,
            "localonly limits filesystem scope, not network access. Unknown project secrets may still be shared."
        )?;
        if authentication == ControllerAuthenticationMode::Disabled {
            writeln!(
                output,
                "WARNING: every peer able to reach this tokenless listener can view and download eligible launch-directory files."
            )?;
        }
        output.flush()?;
    }

    if authentication == ControllerAuthenticationMode::Disabled {
        match print_tokenless_warning(&security, &bind_plan, mode, signals, console.as_deref_mut())
            .await?
        {
            BootstrapOutcome::Acknowledged {
                ignored_hangups: count,
            } => ignored_hangups = ignored_hangups.saturating_add(count),
            BootstrapOutcome::Shutdown(reason) => {
                return Ok(WfeStartOutcome::Shutdown {
                    reason,
                    server: None,
                });
            }
        }
    }

    let sensitive_values = security
        .bootstrap_token_for_host()
        .map(|token| vec![token.to_string()])
        .unwrap_or_default();
    let (frontend, mut runtime) =
        WfeRuntime::new(app, sensitive_values).map_err(io::Error::other)?;
    let (server, mut host_info) = lethetic::wfe::server::start_with_options(
        security,
        bind_plan,
        frontend,
        lethetic::wfe::server::WfeServerOptions { files },
    )
    .await
    .map_err(io::Error::other)?;

    let bootstrap_result = print_started_host(&mut host_info, mode, signals, console).await;
    let bootstrap = match bootstrap_result {
        Ok(bootstrap) => bootstrap,
        Err(error) => {
            let _ = server.shutdown().await;
            return Err(error);
        }
    };
    match bootstrap {
        BootstrapOutcome::Shutdown(reason) => {
            drop(runtime);
            return Ok(WfeStartOutcome::Shutdown {
                reason,
                server: Some(server),
            });
        }
        BootstrapOutcome::Acknowledged {
            ignored_hangups: count,
        } => ignored_hangups = ignored_hangups.saturating_add(count),
    }
    if ignored_hangups > 0 {
        runtime.record_operational(OperationalEvent::HangupIgnored);
    }

    let status = server.status_receiver();
    Ok(WfeStartOutcome::Started(Box::new(StartedWfe {
        runtime,
        server,
        status,
    })))
}

/// A listener started from the command palette inside a running session.
pub(crate) struct InSessionWfe {
    pub(crate) runtime: WfeRuntime,
    pub(crate) server: lethetic::wfe::server::WfeServerHandle,
    pub(crate) status: tokio::sync::watch::Receiver<lethetic::wfe::server::WfeServerStatus>,
    pub(crate) url: String,
    pub(crate) lines: Vec<String>,
}

/// Start the HTTPS listener for the running session. The TUI confirmation
/// dialog stands in for the startup Enter gate, so nothing is printed to the
/// terminal; the URL and fingerprint go back to the caller for a popup.
pub(crate) async fn start_in_session(
    app: &App,
    config: &Config,
    target: &str,
    open: bool,
    share_files: bool,
) -> Result<InSessionWfe, String> {
    let target = WfeTarget::parse(target)?;
    let bind_plan = lethetic::wfe::server::resolve_target(&target).await?;
    let authentication = if open {
        ControllerAuthenticationMode::Disabled
    } else {
        ControllerAuthenticationMode::TokenRequired
    };
    let security = prepare_security_with_authentication(
        &target,
        SecurityFileOptions::new(None, None, None),
        authentication,
        None,
    )?;
    let files = if share_files {
        let launch_root = std::env::current_dir().map_err(|error| error.to_string())?;
        let mut cli = Cli::parse(&[]).map_err(|error| error.to_string())?;
        cli.wfe_files = Some(crate::cli::WfeFilesMode::LocalOnly);
        let mut files = pin_files(&cli, &launch_root).map_err(|error| error.to_string())?;
        let config_path = if Path::new("config.yml").exists() {
            std::path::PathBuf::from("config.yml")
        } else {
            lethetic::platform::lethetic_config_dir().join("config.yml")
        };
        protect_files(&mut files, &cli, &launch_root, &config_path, config)
            .map_err(|error| error.to_string())?;
        files
    } else {
        None
    };
    let sensitive_values = security
        .bootstrap_token_for_host()
        .map(|token| vec![token.to_string()])
        .unwrap_or_default();
    let (frontend, mut runtime) = WfeRuntime::new(app, sensitive_values)?;
    let (server, mut host_info) = lethetic::wfe::server::start_with_options(
        security,
        bind_plan,
        frontend,
        lethetic::wfe::server::WfeServerOptions { files },
    )
    .await?;
    runtime.record_operational(OperationalEvent::ActorReady);
    let url = match host_info.profile() {
        SecurityProfile::AutomaticGenerated => host_info
            .take_bootstrap_url()
            .map(|url| url.as_str().to_string())
            .unwrap_or_else(|| host_info.target().to_string()),
        SecurityProfile::ExplicitFiles => host_info.target().to_string(),
    };
    let mut lines = vec![
        format!("Controller URL: {url}"),
        format!(
            "Listening on: {}",
            format_wfe_listener_addresses(host_info.listener_addresses())
        ),
        format!(
            "TLS certificate SHA-256: {}",
            host_info.fingerprint_sha256()
        ),
    ];
    if open {
        lines.push(
            "WARNING: no controller token. Anyone who can reach this address has full control."
                .to_string(),
        );
    } else {
        lines.push("Keep this URL private: it contains the controller token.".to_string());
    }
    if share_files {
        lines.push("The launch directory is shared read-only in the browser.".to_string());
    }
    let status = server.status_receiver();
    Ok(InSessionWfe {
        runtime,
        server,
        status,
        url,
        lines,
    })
}

pub(crate) fn print_browser_control_active() -> io::Result<()> {
    let mut output = io::stdout().lock();
    writeln!(
        output,
        "Browser control is active. Continue in the browser, or press Enter at any time to enter the terminal UI…"
    )?;
    output.flush()
}

async fn print_tokenless_warning(
    security: &PreparedSecurity,
    bind_plan: &lethetic::wfe::server::WfeBindPlan,
    mode: RuntimeMode,
    signals: &mut SignalListener,
    console: Option<&mut ServiceConsole>,
) -> io::Result<BootstrapOutcome> {
    {
        let mut output = io::stdout().lock();
        writeln!(
            output,
            "WARNING: WFE controller-token authentication is DISABLED."
        )?;
        writeln!(
            output,
            "HTTPS encrypts traffic but does not authenticate the controller."
        )?;
        writeln!(
            output,
            "Any peer that can reach {} will receive TUI-equivalent authority.",
            security.target()
        )?;
        writeln!(
            output,
            "Restrict reachability with a host firewall and VPN ACLs before continuing."
        )?;
        writeln!(output, "Controller URL: {}", security.target())?;
        writeln!(
            output,
            "Pinned HTTPS listeners: {}",
            format_wfe_listener_addresses(bind_plan.addresses())
        )?;
        writeln!(
            output,
            "TLS certificate SHA-256: {}",
            security.fingerprint_sha256()
        )?;
        write!(output, "{}", tokenless_enter_prompt(mode))?;
        output.flush()?;
    }
    await_bootstrap_acknowledgement(mode, signals, console).await
}

async fn print_started_host(
    host_info: &mut lethetic::wfe::server::WfeHostInfo,
    mode: RuntimeMode,
    signals: &mut SignalListener,
    console: Option<&mut ServiceConsole>,
) -> io::Result<BootstrapOutcome> {
    let pinned = format_wfe_listener_addresses(host_info.listener_addresses());
    if host_info.authentication_mode() == ControllerAuthenticationMode::Disabled {
        let mut output = io::stdout().lock();
        writeln!(
            output,
            "Lethetic WFE listening at {} (HTTPS/WSS, tokenless)",
            host_info.target()
        )?;
        writeln!(output, "Pinned HTTPS listeners: {pinned}")?;
        output.flush()?;
        return Ok(BootstrapOutcome::Acknowledged { ignored_hangups: 0 });
    }

    {
        let mut output = io::stdout().lock();
        writeln!(
            output,
            "Lethetic WFE: {} (HTTPS/WSS only)",
            host_info.target()
        )?;
        writeln!(output, "Pinned HTTPS listeners: {pinned}")?;
        writeln!(
            output,
            "TLS certificate SHA-256: {}",
            host_info.fingerprint_sha256()
        )?;
        match host_info.profile() {
            SecurityProfile::AutomaticGenerated => {
                let bootstrap_url = host_info.take_bootstrap_url().ok_or_else(|| {
                    io::Error::other("automatic WFE bootstrap URL is unavailable")
                })?;
                writeln!(output, "Private controller URL: {}", bootstrap_url.as_str())?;
                writeln!(
                    output,
                    "Verify the fingerprint, then open or copy this URL before continuing."
                )?;
            }
            SecurityProfile::ExplicitFiles => {
                writeln!(
                    output,
                    "In the browser address bar, append the private token-file value as #token=<value>."
                )?;
            }
        }
        write!(output, "{}", authenticated_enter_prompt(mode))?;
        output.flush()?;
    }
    await_bootstrap_acknowledgement(mode, signals, console).await
}

async fn await_bootstrap_acknowledgement(
    mode: RuntimeMode,
    signals: &mut SignalListener,
    console: Option<&mut ServiceConsole>,
) -> io::Result<BootstrapOutcome> {
    #[cfg(unix)]
    let mut console = console;
    #[cfg(not(unix))]
    let _ = console;
    #[cfg(unix)]
    let mut ignored_hangups = 0_u32;
    #[cfg(not(unix))]
    let ignored_hangups = 0_u32;
    let mut input = BootstrapLineReader::spawn()?;
    loop {
        tokio::select! {
            biased;
            signal = signals.recv() => {
                let signal = signal?;
                match signal_action(mode, signal) {
                    #[cfg(unix)]
                    SignalAction::IgnoreHangup => {
                        ignored_hangups = ignored_hangups.saturating_add(1);
                        if let Some(console) = console.as_deref_mut() {
                            let _ = console.emit(ServiceConsoleEvent::HangupIgnored);
                        }
                    }
                    SignalAction::BeginShutdown(reason) => {
                        return Ok(BootstrapOutcome::Shutdown(reason));
                    }
                }
            }
            acknowledgement = input.receive() => {
                let acknowledgement = match acknowledgement {
                    Ok(acknowledgement) => acknowledgement,
                    Err(error) => {
                        return Ok(BootstrapOutcome::Shutdown(
                            bootstrap_input_shutdown_reason(&error),
                        ));
                    }
                };
                if let Err(error) = validate_wfe_bootstrap_acknowledgement(&acknowledgement) {
                    return Ok(BootstrapOutcome::Shutdown(
                        bootstrap_input_shutdown_reason(&error),
                    ));
                }
                return Ok(BootstrapOutcome::Acknowledged { ignored_hangups });
            }
        }
    }
}

pub(crate) fn bootstrap_input_shutdown_reason(error: &io::Error) -> ShutdownReason {
    if error.kind() == io::ErrorKind::UnexpectedEof {
        ShutdownReason::TerminalClosed
    } else {
        ShutdownReason::TerminalError
    }
}

fn tokenless_enter_prompt(mode: RuntimeMode) -> &'static str {
    match mode {
        RuntimeMode::Interactive => {
            "Press Enter to acknowledge this warning and activate browser control… "
        }
        RuntimeMode::Service => {
            "Press Enter to acknowledge this warning and start the foreground browser service… "
        }
    }
}

fn authenticated_enter_prompt(mode: RuntimeMode) -> &'static str {
    match mode {
        RuntimeMode::Interactive => {
            "Press Enter to acknowledge this controller information and activate browser control… "
        }
        RuntimeMode::Service => "Press Enter to start the foreground browser-only service… ",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn file_startup_protects_config_overlay_explicit_credentials_and_keys() {
        use lethetic::wfe::file_contracts::FilesReadRequest;
        use lethetic::wfe::files::FilesError;
        let root = tempfile::tempdir().unwrap();
        let config_path = root.path().join("settings.yaml");
        let token_path = root.path().join("private-controller");
        for name in [
            "settings.yaml",
            "config.local.yml",
            "private-controller",
            "ordinary.txt",
        ] {
            std::fs::write(root.path().join(name), "ordinary fixture").unwrap();
        }
        std::fs::write(root.path().join("copied.txt"), "configured-private-key").unwrap();
        let mut cli = Cli::parse(&[
            "--wfe-remote-control".into(),
            "https://127.0.0.1:11223".into(),
            "--wfe-files".into(),
            "localonly".into(),
        ])
        .unwrap();
        cli.wfe_auth_token_file = Some(token_path);
        let config = Config {
            api_key: Some("configured-private-key".into()),
            ..Config::default()
        };
        let mut files = pin_files(&cli, root.path()).unwrap();
        protect_files(&mut files, &cli, root.path(), &config_path, &config).unwrap();
        let files = files.unwrap();
        for name in ["settings.yaml", "config.local.yml", "private-controller"] {
            let result = files
                .read(
                    FilesReadRequest { path: name.into() },
                    tokio_util::sync::CancellationToken::new(),
                )
                .await;
            assert!(matches!(result, Err(FilesError::Protected)));
        }
        assert!(matches!(
            files
                .read(
                    FilesReadRequest {
                        path: "copied.txt".into()
                    },
                    tokio_util::sync::CancellationToken::new()
                )
                .await,
            Err(FilesError::SensitiveContent)
        ));
        assert!(
            files
                .read(
                    FilesReadRequest {
                        path: "ordinary.txt".into()
                    },
                    tokio_util::sync::CancellationToken::new()
                )
                .await
                .is_ok()
        );
    }

    /// Binds a real loopback listener and writes a generated certificate to
    /// the per-user state directory, so it only runs on request.
    #[tokio::test]
    #[ignore = "binds 127.0.0.1 and generates a TLS identity; run with --ignored"]
    async fn palette_listener_serves_and_stops() {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let config = Config::default();
        let app = App::new(&config);
        let target = format!("https://127.0.0.1:{port}");
        let started = start_in_session(&app, &config, &target, false, false)
            .await
            .unwrap();
        assert!(started.url.starts_with(&target), "{}", started.url);
        assert!(
            started.url.contains('#'),
            "token URL expected: {}",
            started.url
        );
        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap();
        let response = client.get(&target).send().await.unwrap();
        assert!(response.status().is_success(), "{}", response.status());
        started.server.shutdown().await.unwrap();
        assert!(client.get(&target).send().await.is_err());
    }

    #[test]
    fn disabled_files_do_not_open_or_inspect_a_root() {
        let cli = Cli::parse(&[]).unwrap();
        assert!(
            pin_files(&cli, Path::new("/does-not-exist/lethetic-test"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn bootstrap_prompts_are_mode_specific_without_changing_security_steps() {
        let interactive = authenticated_enter_prompt(RuntimeMode::Interactive);
        assert!(interactive.contains("acknowledge"));
        assert!(interactive.contains("browser control"));
        assert!(!interactive.contains("terminal UI"));
        assert!(authenticated_enter_prompt(RuntimeMode::Service).contains("browser-only"));
        assert!(tokenless_enter_prompt(RuntimeMode::Interactive).contains("acknowledge"));
        assert!(tokenless_enter_prompt(RuntimeMode::Interactive).contains("browser control"));
        assert!(tokenless_enter_prompt(RuntimeMode::Service).contains("acknowledge"));
        assert!(tokenless_enter_prompt(RuntimeMode::Service).contains("foreground"));
    }

    #[test]
    fn bootstrap_terminal_failures_map_to_graceful_shutdown_reasons() {
        assert_eq!(
            bootstrap_input_shutdown_reason(&io::Error::from(io::ErrorKind::UnexpectedEof)),
            ShutdownReason::TerminalClosed
        );
        assert_eq!(
            bootstrap_input_shutdown_reason(&io::Error::from(io::ErrorKind::InvalidData)),
            ShutdownReason::TerminalError
        );
    }

    #[test]
    fn every_bootstrap_acknowledgement_rejects_eof_and_partial_lines() {
        for incomplete in ["", "x", "\r"] {
            let error = validate_wfe_bootstrap_acknowledgement(incomplete).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
        }
        for complete in ["\n", "x\n", "\r\n"] {
            validate_wfe_bootstrap_acknowledgement(complete).unwrap();
        }
    }
}
