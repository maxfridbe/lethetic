use std::path::PathBuf;

use lethetic::config::Config;

pub(crate) const DEFAULT_HEADLESS_TIMEOUT_SECONDS: u64 = 900;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LiteralPythonMode {
    FullyIsolated,
    Nonlocal,
    Permissive,
}

impl LiteralPythonMode {
    fn flag(self) -> &'static str {
        match self {
            Self::FullyIsolated => "--python-fully-isolated",
            Self::Nonlocal => "--python-isolated-with-nonlocal-network",
            Self::Permissive => "--python-isolated-permissive",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WfeFilesMode {
    LocalOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Cli {
    pub(crate) command: Option<String>,
    pub(crate) python_mode: Option<LiteralPythonMode>,
    pub(crate) new_session: bool,
    pub(crate) session_id: Option<String>,
    pub(crate) timeout_seconds: u64,
    pub(crate) wfe_remote_control: Option<String>,
    pub(crate) wfe_files: Option<WfeFilesMode>,
    pub(crate) wfe_tls_cert: Option<PathBuf>,
    pub(crate) wfe_tls_key: Option<PathBuf>,
    pub(crate) wfe_auth_token_file: Option<PathBuf>,
    pub(crate) wfe_disable_authtoken: bool,
    pub(crate) service: bool,
    pub(crate) help: bool,
}

impl Cli {
    pub(crate) fn parse(arguments: &[String]) -> Result<Self, String> {
        let mut cli = Self {
            command: None,
            python_mode: None,
            new_session: false,
            session_id: None,
            timeout_seconds: DEFAULT_HEADLESS_TIMEOUT_SECONDS,
            wfe_remote_control: None,
            wfe_files: None,
            wfe_tls_cert: None,
            wfe_tls_key: None,
            wfe_auth_token_file: None,
            wfe_disable_authtoken: false,
            service: false,
            help: false,
        };
        let mut index = 0;
        while index < arguments.len() {
            match arguments[index].as_str() {
                "--python-fully-isolated"
                | "--python-isolated-with-nonlocal-network"
                | "--python-isolated-permissive" => {
                    let mode = match arguments[index].as_str() {
                        "--python-fully-isolated" => LiteralPythonMode::FullyIsolated,
                        "--python-isolated-with-nonlocal-network" => LiteralPythonMode::Nonlocal,
                        "--python-isolated-permissive" => LiteralPythonMode::Permissive,
                        _ => unreachable!("literal flag was matched above"),
                    };
                    if let Some(existing) = cli.python_mode {
                        return Err(format!(
                            "{} conflicts with {}",
                            mode.flag(),
                            existing.flag()
                        ));
                    }
                    cli.python_mode = Some(mode);
                }
                "--new-session" => cli.new_session = true,
                "--session-id" => {
                    index += 1;
                    let value = arguments
                        .get(index)
                        .ok_or_else(|| "--session-id requires a UUID".to_string())?;
                    let parsed = uuid::Uuid::parse_str(value).map_err(|_| {
                        "--session-id requires a canonical lowercase UUID".to_string()
                    })?;
                    if parsed.to_string() != *value {
                        return Err("--session-id requires a canonical lowercase UUID".to_string());
                    }
                    cli.session_id = Some(value.clone());
                }
                "--timeout-seconds" => {
                    index += 1;
                    let value = arguments.get(index).ok_or_else(|| {
                        "--timeout-seconds requires a positive integer".to_string()
                    })?;
                    cli.timeout_seconds = value
                        .parse::<u64>()
                        .ok()
                        .filter(|seconds| *seconds > 0)
                        .ok_or_else(|| {
                            "--timeout-seconds requires a positive integer".to_string()
                        })?;
                }
                "--wfe-remote-control" => {
                    if cli.wfe_remote_control.is_some() {
                        return Err("--wfe-remote-control may be supplied only once".to_string());
                    }
                    index += 1;
                    let value = arguments
                        .get(index)
                        .filter(|value| !value.trim().is_empty())
                        .ok_or_else(|| "--wfe-remote-control requires an HTTPS URL".to_string())?;
                    cli.wfe_remote_control = Some(value.clone());
                }
                "--wfe-files" => {
                    if cli.wfe_files.is_some() {
                        return Err("--wfe-files may be supplied only once".to_string());
                    }
                    index += 1;
                    if arguments.get(index).map(String::as_str) != Some("localonly") {
                        return Err("--wfe-files requires the exact value localonly".to_string());
                    }
                    cli.wfe_files = Some(WfeFilesMode::LocalOnly);
                }
                "--wfe-tls-cert" => {
                    if cli.wfe_tls_cert.is_some() {
                        return Err("--wfe-tls-cert may be supplied only once".to_string());
                    }
                    index += 1;
                    let value = arguments
                        .get(index)
                        .filter(|value| !value.is_empty())
                        .ok_or_else(|| "--wfe-tls-cert requires a file path".to_string())?;
                    cli.wfe_tls_cert = Some(PathBuf::from(value));
                }
                "--wfe-tls-key" => {
                    if cli.wfe_tls_key.is_some() {
                        return Err("--wfe-tls-key may be supplied only once".to_string());
                    }
                    index += 1;
                    let value = arguments
                        .get(index)
                        .filter(|value| !value.is_empty())
                        .ok_or_else(|| "--wfe-tls-key requires a file path".to_string())?;
                    cli.wfe_tls_key = Some(PathBuf::from(value));
                }
                "--wfe-auth-token-file" => {
                    if cli.wfe_auth_token_file.is_some() {
                        return Err("--wfe-auth-token-file may be supplied only once".to_string());
                    }
                    index += 1;
                    let value = arguments
                        .get(index)
                        .filter(|value| !value.is_empty())
                        .ok_or_else(|| "--wfe-auth-token-file requires a file path".to_string())?;
                    cli.wfe_auth_token_file = Some(PathBuf::from(value));
                }
                "--wfe-disable-authtoken" => {
                    if cli.wfe_disable_authtoken {
                        return Err("--wfe-disable-authtoken may be supplied only once".to_string());
                    }
                    cli.wfe_disable_authtoken = true;
                }
                "--service" => {
                    if cli.service {
                        return Err("--service may be supplied only once".to_string());
                    }
                    cli.service = true;
                }
                "--command" => {
                    if cli.command.is_some() {
                        return Err("--command may be supplied only once".to_string());
                    }
                    let prompt = arguments[index + 1..].join(" ");
                    if prompt.trim().is_empty() {
                        return Err("--command requires a prompt".to_string());
                    }
                    cli.command = Some(prompt);
                    break;
                }
                "--help" | "-h" => cli.help = true,
                argument => return Err(format!("unknown argument {argument:?}")),
            }
            index += 1;
        }
        if cli.new_session && cli.session_id.is_some() {
            return Err("--new-session conflicts with --session-id".to_string());
        }
        if cli.service && cli.command.is_some() {
            return Err("--service conflicts with --command".to_string());
        }
        if cli.service && cli.wfe_remote_control.is_none() {
            return Err("--service requires --wfe-remote-control <HTTPS_URL>".to_string());
        }
        if cli.wfe_files.is_some() && cli.wfe_remote_control.is_none() {
            return Err("--wfe-files requires --wfe-remote-control <HTTPS_URL>".to_string());
        }
        let has_wfe_cert = cli.wfe_tls_cert.is_some();
        let has_wfe_key = cli.wfe_tls_key.is_some();
        let has_wfe_token = cli.wfe_auth_token_file.is_some();
        if cli.wfe_remote_control.is_none()
            && (has_wfe_cert || has_wfe_key || has_wfe_token || cli.wfe_disable_authtoken)
        {
            return Err(
                "WFE TLS/auth options require --wfe-remote-control <HTTPS_URL>".to_string(),
            );
        }
        if cli.wfe_disable_authtoken && has_wfe_token {
            return Err("--wfe-disable-authtoken conflicts with --wfe-auth-token-file".to_string());
        }
        if has_wfe_cert != has_wfe_key {
            return Err("--wfe-tls-cert and --wfe-tls-key must be supplied together".to_string());
        }
        if has_wfe_token && !(has_wfe_cert && has_wfe_key) {
            return Err(
                "--wfe-auth-token-file requires --wfe-tls-cert and --wfe-tls-key".to_string(),
            );
        }
        if has_wfe_cert && !has_wfe_token && !cli.wfe_disable_authtoken {
            return Err(
                "explicit WFE TLS requires --wfe-auth-token-file or --wfe-disable-authtoken"
                    .to_string(),
            );
        }
        if cli.command.is_some() && cli.wfe_remote_control.is_some() {
            return Err("--wfe-remote-control cannot be used with --command".to_string());
        }
        if cli.command.is_some()
            && cli.python_mode == Some(LiteralPythonMode::Nonlocal)
            && !cli.new_session
            && cli.session_id.is_none()
        {
            return Err(
                "Nonlocal headless mode requires --new-session or --session-id <uuid>".to_string(),
            );
        }
        Ok(cli)
    }

    pub(crate) fn print_help() {
        println!(
            "Lethetic\n\nUSAGE:\n  lethetic [OPTIONS]\n  lethetic [OPTIONS] --service --wfe-remote-control <HTTPS_URL>\n  lethetic [OPTIONS] --command <PROMPT>\n\nOPTIONS:\n  --python-fully-isolated\n      Python-only transient rootless Podman mode. The canonical launch cwd is mounted R/W;\n      direct network access is disabled and packages are not installed automatically.\n  --python-isolated-with-nonlocal-network\n      Python-only retained Podman mode. The canonical launch cwd is mounted R/W; public\n      HTTP(S) uses the constrained broker while direct/local/LAN/VPN access stays blocked.\n  --python-isolated-permissive\n      Python-only transient rootless Podman mode with the canonical launch cwd mounted R/W\n      and full host/localhost/LAN/VPN/Internet network reachability. No engine socket is mounted.\n  --new-session\n      Create a durable chat identity.\n  --session-id <UUID>\n      Resume one exact durable chat identity.\n  --timeout-seconds <SECONDS>\n      Positive headless timeout (default: {DEFAULT_HEADLESS_TIMEOUT_SECONDS}).\n  --wfe-remote-control <HTTPS_URL>\n      Mirror and control the active chat over one exact HTTPS/WSS identity.\n      Accepted concrete IPs and lowercase DNS names get generated TLS and a fresh process token.\n  --wfe-files localonly\n      Enable read-only Monaco viewing and file/folder downloads below the fixed launch cwd.\n      Requires --wfe-remote-control and Linux; protected files and links are excluded.\n  --service\n      Run as a foreground browser-only service. Requires --wfe-remote-control, an attached\n      terminal for secure bootstrap, and no --command.\n  --wfe-tls-cert <PATH>\n  --wfe-tls-key <PATH>\n      Optional exact-target TLS override; both private credential files are required together.\n  --wfe-auth-token-file <PATH>\n      Use a private controller token file with explicit TLS.\n  --wfe-disable-authtoken\n      Disable controller-token authentication. HTTPS remains required, but every peer that can\n      reach any pinned listener receives TUI-equivalent authority. Restrict access with firewall/VPN ACLs.\n  --command <PROMPT>\n      Run one headless agent request. Put this option last.\n  -h, --help\n      Show this help."
        );
    }
}

pub(crate) fn apply_literal_python_mode(
    config: &mut Config,
    mode: LiteralPythonMode,
) -> Result<(), String> {
    use lethetic::config::{
        AccessMode, NetworkAccess, PackageAccess, PythonExecutionTarget, PythonWorkspaceExposure,
        SandboxBackend, ToolProfile,
    };

    config.tool_profile = ToolProfile::PythonOnly;
    config.python_runtime.target = Some(PythonExecutionTarget::Sandbox);
    config.python_runtime.sandbox.backend = Some(SandboxBackend::Podman);
    config.python_runtime.sandbox.workspace_access = Some(AccessMode::ReadWrite);
    config.python_runtime.sandbox.grants.clear();
    config.python_invocation.workspace_exposure = PythonWorkspaceExposure::SharedLaunchCwd;

    match mode {
        LiteralPythonMode::FullyIsolated => {
            config.python_runtime.sandbox.network = Some(NetworkAccess::None);
            config.python_runtime.sandbox.package_access = PackageAccess::Disabled;
        }
        LiteralPythonMode::Nonlocal => {
            config.python_runtime.sandbox.network = Some(NetworkAccess::Nonlocal);
            config.python_runtime.sandbox.package_access = PackageAccess::Session;
            if config.python_runtime.sandbox.podman_image.trim().is_empty()
                || config.python_runtime.sandbox.podman_image
                    == "docker.io/library/python:3.13-slim"
            {
                config.python_runtime.sandbox.podman_image =
                    lethetic::config::DEFAULT_RETAINED_PODMAN_IMAGE.to_string();
            }
        }
        LiteralPythonMode::Permissive => {
            config.python_runtime.sandbox.network = Some(NetworkAccess::Full);
            config.python_runtime.sandbox.package_access = PackageAccess::Disabled;
        }
    }

    config.python_mode_validation_error().map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn service_requires_wfe_and_conflicts_with_command() {
        assert_eq!(
            Cli::parse(&arguments(&["--service"])).unwrap_err(),
            "--service requires --wfe-remote-control <HTTPS_URL>"
        );
        assert_eq!(
            Cli::parse(&arguments(&[
                "--service",
                "--wfe-remote-control",
                "https://127.0.0.1:11223",
                "--command",
                "hello",
            ]))
            .unwrap_err(),
            "--service conflicts with --command"
        );
    }

    #[test]
    fn service_is_unique_and_parses_as_foreground_mode() {
        let cli = Cli::parse(&arguments(&[
            "--service",
            "--wfe-remote-control",
            "https://127.0.0.1:11223",
        ]))
        .unwrap();
        assert!(cli.service);
        assert!(cli.command.is_none());

        let error = Cli::parse(&arguments(&[
            "--service",
            "--service",
            "--wfe-remote-control",
            "https://127.0.0.1:11223",
        ]))
        .unwrap_err();
        assert_eq!(error, "--service may be supplied only once");
    }

    #[test]
    fn files_mode_is_exact_unique_and_requires_remote_control() {
        for values in [
            vec!["--wfe-files"],
            vec!["--wfe-files", "all"],
            vec!["--wfe-files", "LocalOnly"],
        ] {
            assert_eq!(
                Cli::parse(&arguments(&values)).unwrap_err(),
                "--wfe-files requires the exact value localonly"
            );
        }
        assert_eq!(
            Cli::parse(&arguments(&["--wfe-files", "localonly"])).unwrap_err(),
            "--wfe-files requires --wfe-remote-control <HTTPS_URL>"
        );
        assert_eq!(
            Cli::parse(&arguments(&[
                "--wfe-files",
                "localonly",
                "--wfe-files",
                "localonly"
            ]))
            .unwrap_err(),
            "--wfe-files may be supplied only once"
        );
        let cli = Cli::parse(&arguments(&[
            "--wfe-remote-control",
            "https://127.0.0.1:11223",
            "--wfe-files",
            "localonly",
            "--service",
        ]))
        .unwrap();
        assert_eq!(cli.wfe_files, Some(WfeFilesMode::LocalOnly));
        assert!(!cli.wfe_disable_authtoken);
        assert!(cli.service);
        assert_eq!(Cli::parse(&[]).unwrap().wfe_files, None);
    }

    #[test]
    fn literal_modes_remain_mutually_exclusive() {
        let error = Cli::parse(&arguments(&[
            "--python-fully-isolated",
            "--python-isolated-permissive",
        ]))
        .unwrap_err();
        assert!(error.contains("conflicts with"));
    }

    #[test]
    fn nonlocal_headless_still_requires_durable_identity() {
        let error = Cli::parse(&arguments(&[
            "--python-isolated-with-nonlocal-network",
            "--command",
            "test",
        ]))
        .unwrap_err();
        assert!(error.contains("requires --new-session or --session-id"));
    }

    #[test]
    fn service_preserves_wfe_tls_auth_validation() {
        let error = Cli::parse(&arguments(&[
            "--service",
            "--wfe-remote-control",
            "https://brainiac:9443",
            "--wfe-tls-cert",
            "/private/cert.pem",
        ]))
        .unwrap_err();
        assert!(error.contains("must be supplied together"));

        let cli = Cli::parse(&arguments(&[
            "--service",
            "--wfe-remote-control",
            "https://brainiac:9443",
            "--wfe-tls-cert",
            "/private/cert.pem",
            "--wfe-tls-key",
            "/private/key.pem",
            "--wfe-auth-token-file",
            "/private/token",
        ]))
        .unwrap();
        assert!(cli.service);
        assert_eq!(
            cli.wfe_tls_key.as_deref(),
            Some(std::path::Path::new("/private/key.pem"))
        );
    }

    #[test]
    fn literal_python_overrides_remain_exact() {
        use lethetic::config::{
            AccessMode, NetworkAccess, PackageAccess, PythonExecutionTarget,
            PythonWorkspaceExposure, SandboxBackend, ToolProfile,
        };

        for (mode, network, packages) in [
            (
                LiteralPythonMode::FullyIsolated,
                NetworkAccess::None,
                PackageAccess::Disabled,
            ),
            (
                LiteralPythonMode::Nonlocal,
                NetworkAccess::Nonlocal,
                PackageAccess::Session,
            ),
            (
                LiteralPythonMode::Permissive,
                NetworkAccess::Full,
                PackageAccess::Disabled,
            ),
        ] {
            let mut config = Config::default();
            config.python_runtime.sandbox.podman_image = "localhost/runtime@sha256:abc".to_string();
            apply_literal_python_mode(&mut config, mode).unwrap();
            assert_eq!(config.tool_profile, ToolProfile::PythonOnly);
            assert_eq!(
                config.python_runtime.target,
                Some(PythonExecutionTarget::Sandbox)
            );
            assert_eq!(
                config.python_runtime.sandbox.backend,
                Some(SandboxBackend::Podman)
            );
            assert_eq!(config.python_runtime.sandbox.network, Some(network));
            assert_eq!(
                config.python_runtime.sandbox.workspace_access,
                Some(AccessMode::ReadWrite)
            );
            assert_eq!(config.python_runtime.sandbox.package_access, packages);
            assert!(config.python_runtime.sandbox.grants.is_empty());
            assert_eq!(
                config.python_invocation.workspace_exposure,
                PythonWorkspaceExposure::SharedLaunchCwd
            );
            assert_eq!(
                config.python_runtime.sandbox.podman_image,
                "localhost/runtime@sha256:abc"
            );
        }
    }
}
