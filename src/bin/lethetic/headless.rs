use std::{error::Error, io, time::Duration};

use reqwest::Client;
use tokio_util::sync::CancellationToken;

use lethetic::{config::Config, icons};

use crate::{cli::Cli, formatting::print_accounting_estimates};

fn validate_headless_session_requirements(config: &Config, cli: &Cli) -> Result<(), String> {
    let retained_nonlocal = lethetic::config::is_exact_retained_nonlocal_python_policy(
        config.tool_profile,
        &config.python_runtime,
    );
    if retained_nonlocal && !cli.new_session && cli.session_id.is_none() {
        return Err(
            "Retained Nonlocal Python headless mode requires --new-session or --session-id before provider work"
                .to_string(),
        );
    }
    Ok(())
}

pub(crate) async fn run(config: &Config, prompt: String, cli: &Cli) -> Result<(), Box<dyn Error>> {
    if let Some(error) = config.python_mode_validation_error() {
        return Err(format!("Python-only mode is not ready for headless use: {error}").into());
    }
    validate_headless_session_requirements(config, cli)?;
    println!("\n{} User: {}\n", icons::INPUT, prompt);
    let client = Client::new();
    if cli.new_session || cli.session_id.is_some() {
        #[cfg(target_os = "linux")]
        {
            let session = lethetic::headless_session::DurableHeadlessSession::open(
                &std::env::current_dir()?,
                config,
                cli.new_session,
                cli.session_id.as_deref(),
            )
            .await?;
            println!("Session ID: {}", session.session_id);
            println!("Session path: {}", session.session_path.display());
            println!("Managed workspace: {}", session.workspace.display());
            if let Some(runtime_id) = &session.runtime_id {
                println!("Python runtime ID: {runtime_id}");
            }
            if let Some(container_id) = &session.container_id {
                println!("Podman container ID: {container_id}");
            }
            if let Some(audit_path) = &session.audit_path {
                println!("Egress audit path: {}", audit_path.display());
            }
            let run = session
                .run(
                    prompt,
                    &client,
                    config,
                    true,
                    Duration::from_secs(cli.timeout_seconds),
                )
                .await?;
            print_accounting_estimates(
                &run.state.accounting.latest_logical_turn,
                &run.state.accounting.session,
            );
            println!("\n[DONE]");
            return Ok(());
        }
        #[cfg(not(target_os = "linux"))]
        {
            return Err("durable headless sessions are supported only on Linux".into());
        }
    }

    let cancellation = CancellationToken::new();
    let mut agent_future = Box::pin(lethetic::headless::run_agent_accounted_with_cancellation(
        prompt,
        &client,
        config,
        true,
        None,
        Some(cancellation.clone()),
    ));
    let run = tokio::select! {
        result = &mut agent_future => {
            result.map_err(|error| -> Box<dyn Error> { error.into() })?
        }
        _ = tokio::time::sleep(Duration::from_secs(cli.timeout_seconds)) => {
            cancellation.cancel();
            let settled = tokio::time::timeout(Duration::from_secs(30), &mut agent_future)
                .await
                .is_ok();
            let suffix = if settled {
                String::new()
            } else {
                " and request cancellation did not settle within 30s".to_string()
            };
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "headless operation timed out after {}s{suffix}",
                    cli.timeout_seconds
                ),
            )
            .into());
        }
    };
    drop(agent_future);
    let mut totals = lethetic::accounting::AccountingTotals::default();
    for request in run.requests {
        totals.record(&request.into_logical_turn("headless-turn".to_string()));
    }
    print_accounting_estimates(&totals, &totals);
    println!("\n[DONE]");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::DEFAULT_HEADLESS_TIMEOUT_SECONDS;

    #[test]
    fn configured_nonlocal_mode_requires_identity_before_provider_work() {
        let mut config = Config::default();
        config.tool_profile = lethetic::config::ToolProfile::PythonOnly;
        config.python_runtime.target = Some(lethetic::config::PythonExecutionTarget::Sandbox);
        config.python_runtime.sandbox.backend = Some(lethetic::config::SandboxBackend::Podman);
        config.python_runtime.sandbox.network = Some(lethetic::config::NetworkAccess::Nonlocal);
        config.python_runtime.sandbox.workspace_access =
            Some(lethetic::config::AccessMode::ReadWrite);
        config.python_runtime.sandbox.package_access = lethetic::config::PackageAccess::Session;
        let mut cli = Cli {
            command: Some("test".to_string()),
            python_mode: None,
            new_session: false,
            session_id: None,
            timeout_seconds: DEFAULT_HEADLESS_TIMEOUT_SECONDS,
            wfe_remote_control: None,
            rc_wizard: false,
            wfe_files: None,
            wfe_tls_cert: None,
            wfe_tls_key: None,
            wfe_auth_token_file: None,
            wfe_disable_authtoken: false,
            service: false,
            help: false,
        };

        let error = validate_headless_session_requirements(&config, &cli).unwrap_err();
        assert!(error.contains("before provider work"));
        cli.new_session = true;
        validate_headless_session_requirements(&config, &cli).unwrap();
        cli.new_session = false;
        cli.session_id = Some("11111111-2222-4333-8444-555555555555".to_string());
        validate_headless_session_requirements(&config, &cli).unwrap();

        let mut stale_host = config;
        stale_host.python_runtime.target = Some(lethetic::config::PythonExecutionTarget::Host);
        cli.session_id = None;
        validate_headless_session_requirements(&stale_host, &cli).unwrap();
    }
}
