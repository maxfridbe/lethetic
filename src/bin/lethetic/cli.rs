use std::path::PathBuf;

use lethetic::config::Config;

use crate::rc_wizard::DEFAULT_RC_PORT;

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
            Self::FullyIsolated => "--python-only isolated",
            Self::Nonlocal => "--python-only nonlocal",
            Self::Permissive => "--python-only permissive",
        }
    }

    fn from_mode_name(value: &str) -> Option<Self> {
        match value {
            "isolated" | "none" | "offline" => Some(Self::FullyIsolated),
            "nonlocal" | "packages" | "public" => Some(Self::Nonlocal),
            "permissive" | "full" | "network" => Some(Self::Permissive),
            _ => None,
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
    /// Exact HTTPS controller target; `--rc` and `--wfe-remote-control` both fill this.
    pub(crate) wfe_remote_control: Option<String>,
    /// `--rc` was given without a target: ask interactively before startup.
    pub(crate) rc_wizard: bool,
    pub(crate) wfe_files: Option<WfeFilesMode>,
    pub(crate) wfe_tls_cert: Option<PathBuf>,
    pub(crate) wfe_tls_key: Option<PathBuf>,
    pub(crate) wfe_auth_token_file: Option<PathBuf>,
    pub(crate) wfe_disable_authtoken: bool,
    pub(crate) service: bool,
    pub(crate) help: bool,
}

fn looks_like_flag(value: &str) -> bool {
    value.starts_with("--") || value == "-h"
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
            rc_wizard: false,
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
            let argument = arguments[index].as_str();
            match argument {
                "--python-only" | "--sandbox-python-only" => {
                    let mode = match arguments.get(index + 1).map(String::as_str) {
                        Some(value) if !looks_like_flag(value) => {
                            index += 1;
                            LiteralPythonMode::from_mode_name(value).ok_or_else(|| {
                                format!(
                                    "{argument} accepts isolated, nonlocal, or permissive (got {value:?})"
                                )
                            })?
                        }
                        _ => LiteralPythonMode::FullyIsolated,
                    };
                    cli.set_python_mode(mode)?;
                }
                "--python-fully-isolated" => {
                    cli.set_python_mode(LiteralPythonMode::FullyIsolated)?;
                }
                "--python-isolated-with-nonlocal-network" => {
                    cli.set_python_mode(LiteralPythonMode::Nonlocal)?;
                }
                "--python-isolated-permissive" => {
                    cli.set_python_mode(LiteralPythonMode::Permissive)?;
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
                "--rc" => {
                    if cli.wfe_remote_control.is_some() || cli.rc_wizard {
                        return Err("--rc may be supplied only once".to_string());
                    }
                    match arguments.get(index + 1).map(String::as_str) {
                        Some(value) if !looks_like_flag(value) && !value.trim().is_empty() => {
                            index += 1;
                            cli.wfe_remote_control =
                                Some(crate::rc_wizard::normalize_rc_target(value));
                        }
                        _ => cli.rc_wizard = true,
                    }
                }
                "--wfe-remote-control" => {
                    if cli.wfe_remote_control.is_some() || cli.rc_wizard {
                        return Err("--rc may be supplied only once".to_string());
                    }
                    index += 1;
                    let value = arguments
                        .get(index)
                        .filter(|value| !value.trim().is_empty())
                        .ok_or_else(|| "--wfe-remote-control requires an HTTPS URL".to_string())?;
                    cli.wfe_remote_control = Some(value.clone());
                }
                "--rc-files" => {
                    if cli.wfe_files.is_some() {
                        return Err("--rc-files may be supplied only once".to_string());
                    }
                    cli.wfe_files = Some(WfeFilesMode::LocalOnly);
                }
                "--wfe-files" => {
                    if cli.wfe_files.is_some() {
                        return Err("--rc-files may be supplied only once".to_string());
                    }
                    index += 1;
                    if arguments.get(index).map(String::as_str) != Some("localonly") {
                        return Err("--wfe-files requires the exact value localonly".to_string());
                    }
                    cli.wfe_files = Some(WfeFilesMode::LocalOnly);
                }
                "--rc-tls-cert" | "--wfe-tls-cert" => {
                    if cli.wfe_tls_cert.is_some() {
                        return Err("--rc-tls-cert may be supplied only once".to_string());
                    }
                    index += 1;
                    let value = arguments
                        .get(index)
                        .filter(|value| !value.is_empty())
                        .ok_or_else(|| "--rc-tls-cert requires a file path".to_string())?;
                    cli.wfe_tls_cert = Some(PathBuf::from(value));
                }
                "--rc-tls-key" | "--wfe-tls-key" => {
                    if cli.wfe_tls_key.is_some() {
                        return Err("--rc-tls-key may be supplied only once".to_string());
                    }
                    index += 1;
                    let value = arguments
                        .get(index)
                        .filter(|value| !value.is_empty())
                        .ok_or_else(|| "--rc-tls-key requires a file path".to_string())?;
                    cli.wfe_tls_key = Some(PathBuf::from(value));
                }
                "--rc-token-file" | "--wfe-auth-token-file" => {
                    if cli.wfe_auth_token_file.is_some() {
                        return Err("--rc-token-file may be supplied only once".to_string());
                    }
                    index += 1;
                    let value = arguments
                        .get(index)
                        .filter(|value| !value.is_empty())
                        .ok_or_else(|| "--rc-token-file requires a file path".to_string())?;
                    cli.wfe_auth_token_file = Some(PathBuf::from(value));
                }
                "--rc-open" | "--wfe-disable-authtoken" => {
                    if cli.wfe_disable_authtoken {
                        return Err("--rc-open may be supplied only once".to_string());
                    }
                    cli.wfe_disable_authtoken = true;
                }
                "--rc-only" | "--service" => {
                    if cli.service {
                        return Err("--rc-only may be supplied only once".to_string());
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
        cli.validate()?;
        Ok(cli)
    }

    fn set_python_mode(&mut self, mode: LiteralPythonMode) -> Result<(), String> {
        if let Some(existing) = self.python_mode {
            return Err(format!(
                "{} conflicts with {}",
                mode.flag(),
                existing.flag()
            ));
        }
        self.python_mode = Some(mode);
        Ok(())
    }

    fn has_remote_control(&self) -> bool {
        self.wfe_remote_control.is_some() || self.rc_wizard
    }

    fn validate(&self) -> Result<(), String> {
        if self.new_session && self.session_id.is_some() {
            return Err("--new-session conflicts with --session-id".to_string());
        }
        if self.service && self.command.is_some() {
            return Err("--rc-only conflicts with --command".to_string());
        }
        if self.service && !self.has_remote_control() {
            return Err("--rc-only requires --rc <TARGET>".to_string());
        }
        if self.wfe_files.is_some() && !self.has_remote_control() {
            return Err("--rc-files requires --rc <TARGET>".to_string());
        }
        let has_cert = self.wfe_tls_cert.is_some();
        let has_key = self.wfe_tls_key.is_some();
        let has_token = self.wfe_auth_token_file.is_some();
        if !self.has_remote_control()
            && (has_cert || has_key || has_token || self.wfe_disable_authtoken)
        {
            return Err("remote-control TLS/auth options require --rc <TARGET>".to_string());
        }
        if self.wfe_disable_authtoken && has_token {
            return Err("--rc-open conflicts with --rc-token-file".to_string());
        }
        if has_cert != has_key {
            return Err("--rc-tls-cert and --rc-tls-key must be supplied together".to_string());
        }
        if has_token && !(has_cert && has_key) {
            return Err("--rc-token-file requires --rc-tls-cert and --rc-tls-key".to_string());
        }
        if has_cert && !has_token && !self.wfe_disable_authtoken {
            return Err(
                "explicit remote-control TLS requires --rc-token-file or --rc-open".to_string(),
            );
        }
        if self.command.is_some() && self.has_remote_control() {
            return Err("--rc cannot be used with --command".to_string());
        }
        if self.command.is_some()
            && self.python_mode == Some(LiteralPythonMode::Nonlocal)
            && !self.new_session
            && self.session_id.is_none()
        {
            return Err(
                "Nonlocal headless mode requires --new-session or --session-id <uuid>".to_string(),
            );
        }
        Ok(())
    }

    /// The non-interactive flags equivalent to the current remote-control choices.
    pub(crate) fn rc_flags_summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(target) = &self.wfe_remote_control {
            parts.push(format!("--rc {target}"));
        }
        if self.wfe_disable_authtoken {
            parts.push("--rc-open".to_string());
        }
        if self.wfe_files.is_some() {
            parts.push("--rc-files".to_string());
        }
        if self.service {
            parts.push("--rc-only".to_string());
        }
        if let Some(mode) = self.python_mode {
            parts.push(mode.flag().to_string());
        }
        parts.join(" ")
    }

    pub(crate) fn print_help() {
        println!(
            "Lethetic\n\n\
USAGE:\n  \
  lethetic [OPTIONS]                       terminal UI\n  \
  lethetic --rc [TARGET] [RC OPTIONS]      terminal UI plus browser control\n  \
  lethetic --rc TARGET --rc-only           browser control only\n  \
  lethetic [OPTIONS] --command <PROMPT>    one headless request\n\n\
REMOTE CONTROL (browser):\n  \
  --rc [TARGET]\n      \
      Expose the chat to a browser over HTTPS. TARGET is host, host:port, or https://host:port\n      \
      (default port {DEFAULT_RC_PORT}; concrete IPs or lowercase DNS names of this machine).\n      \
      Without TARGET, an interactive chooser lists this machine's addresses and asks how to\n      \
      bind: address, port, authentication, file sharing, and surface.\n  \
  --rc-open\n      \
      No controller token: everyone who can reach the address gets full control.\n      \
      HTTPS stays on. Lethetic prints a warning and waits for Enter before binding.\n  \
  --rc-files\n      \
      Also share the launch directory read-only in the browser (Linux). Protected files are excluded.\n  \
  --rc-only\n      \
      Browser only; never enters the terminal UI. Needs an attached terminal for the startup gate.\n  \
  --rc-tls-cert <PATH> --rc-tls-key <PATH> [--rc-token-file <PATH>]\n      \
      Bring your own TLS identity and controller token instead of the generated ones.\n\n\
PYTHON-ONLY AGENT (rootless Podman sandbox, launch cwd mounted read/write):\n  \
  --python-only [MODE]      MODE is one of:\n      \
      isolated    (default) no network, no package installs, transient container\n      \
      nonlocal    public HTTP(S) only through the broker; `lethetic-pkg` installs; retained 14 days\n      \
      permissive  full host/LAN/VPN/Internet reachability; no package installs; transient\n\n\
SESSIONS:\n  \
  --new-session             Create a durable chat identity.\n  \
  --session-id <UUID>       Resume one exact durable chat identity.\n\n\
HEADLESS:\n  \
  --command <PROMPT>        Run one agent request and exit. Put this option last.\n  \
  --timeout-seconds <N>     Headless deadline (default: {DEFAULT_HEADLESS_TIMEOUT_SECONDS}).\n\n\
OTHER:\n  \
  -h, --help                Show this help.\n\n\
Legacy spellings still work: --wfe-remote-control <HTTPS_URL>, --wfe-files localonly,\n\
--wfe-disable-authtoken, --service, --wfe-tls-cert/--wfe-tls-key/--wfe-auth-token-file,\n\
--python-fully-isolated, --python-isolated-with-nonlocal-network, --python-isolated-permissive."
        );
    }
}

pub(crate) fn apply_literal_python_mode(
    config: &mut Config,
    mode: LiteralPythonMode,
) -> Result<(), String> {
    use lethetic::config::{PythonPreset, PythonWorkspaceExposure};

    config.apply_python_preset(match mode {
        LiteralPythonMode::FullyIsolated => PythonPreset::Isolated,
        LiteralPythonMode::Nonlocal => PythonPreset::Nonlocal,
        LiteralPythonMode::Permissive => PythonPreset::Permissive,
    });
    // Literal flags share the canonical launch cwd; TUI presets keep the
    // ordinary managed-workspace exposure.
    config.python_invocation.workspace_exposure = PythonWorkspaceExposure::SharedLaunchCwd;
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
            "--rc-only requires --rc <TARGET>"
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
            "--rc-only conflicts with --command"
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
        assert_eq!(error, "--rc-only may be supplied only once");
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
            "--rc-files requires --rc <TARGET>"
        );
        assert_eq!(
            Cli::parse(&arguments(&[
                "--wfe-files",
                "localonly",
                "--wfe-files",
                "localonly"
            ]))
            .unwrap_err(),
            "--rc-files may be supplied only once"
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
    fn short_remote_control_flags_map_onto_the_wfe_fields() {
        let cli = Cli::parse(&arguments(&[
            "--rc",
            "brainiac",
            "--rc-open",
            "--rc-files",
            "--rc-only",
        ]))
        .unwrap();
        assert_eq!(
            cli.wfe_remote_control.as_deref(),
            Some("https://brainiac:11223")
        );
        assert!(cli.wfe_disable_authtoken);
        assert_eq!(cli.wfe_files, Some(WfeFilesMode::LocalOnly));
        assert!(cli.service);
        assert!(!cli.rc_wizard);
        assert_eq!(
            cli.rc_flags_summary(),
            "--rc https://brainiac:11223 --rc-open --rc-files --rc-only"
        );

        let cli = Cli::parse(&arguments(&["--rc", "10.0.0.5:9443"])).unwrap();
        assert_eq!(
            cli.wfe_remote_control.as_deref(),
            Some("https://10.0.0.5:9443")
        );
        assert_eq!(
            Cli::parse(&arguments(&[
                "--rc",
                "x",
                "--wfe-remote-control",
                "https://y:1"
            ]))
            .unwrap_err(),
            "--rc may be supplied only once"
        );
    }

    #[test]
    fn bare_rc_requests_the_interactive_chooser() {
        let cli = Cli::parse(&arguments(&["--rc"])).unwrap();
        assert!(cli.rc_wizard);
        assert!(cli.wfe_remote_control.is_none());
        let cli = Cli::parse(&arguments(&["--rc", "--rc-only"])).unwrap();
        assert!(cli.rc_wizard);
        assert!(cli.service);
        assert_eq!(
            Cli::parse(&arguments(&["--rc", "--command", "hi"])).unwrap_err(),
            "--rc cannot be used with --command"
        );
    }

    #[test]
    fn python_only_takes_an_optional_mode() {
        assert_eq!(
            Cli::parse(&arguments(&["--python-only"]))
                .unwrap()
                .python_mode,
            Some(LiteralPythonMode::FullyIsolated)
        );
        assert_eq!(
            Cli::parse(&arguments(&["--python-only", "nonlocal"]))
                .unwrap()
                .python_mode,
            Some(LiteralPythonMode::Nonlocal)
        );
        assert_eq!(
            Cli::parse(&arguments(&[
                "--sandbox-python-only",
                "permissive",
                "--new-session"
            ]))
            .unwrap()
            .python_mode,
            Some(LiteralPythonMode::Permissive)
        );
        assert_eq!(
            Cli::parse(&arguments(&["--python-only", "--new-session"]))
                .unwrap()
                .python_mode,
            Some(LiteralPythonMode::FullyIsolated)
        );
        assert!(
            Cli::parse(&arguments(&["--python-only", "host"]))
                .unwrap_err()
                .contains("accepts isolated, nonlocal, or permissive")
        );
        assert_eq!(
            Cli::parse(&arguments(&["--python-isolated-permissive"]))
                .unwrap()
                .python_mode,
            Some(LiteralPythonMode::Permissive)
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
