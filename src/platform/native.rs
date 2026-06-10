//! Desktop implementations shared by Linux, macOS, and Windows.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::Output;

/// Process handle types, re-exported so callers of the spawn helpers don't
/// import `tokio::process` directly (native-only; absent on wasm32).
pub use tokio::process::{Child, ChildStdin, ChildStdout};

#[cfg(not(windows))]
const SHELL: (&str, &str) = ("bash", "-c");
#[cfg(windows)]
const SHELL: (&str, &str) = ("cmd", "/C");

#[cfg(not(windows))]
const WHICH: &str = "which";
#[cfg(windows)]
const WHICH: &str = "where";

/// Root directory for lethetic's per-user configuration
/// (e.g. `~/.config/lethetic` on Linux).
pub fn lethetic_config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .map(|h| h.join(".config"))
                .unwrap_or_else(|| PathBuf::from("."))
        })
        .join("lethetic")
}

/// Run a program with arguments and capture its output. The child is killed
/// if the returned future is dropped (e.g. by a cancellation `select!`).
pub async fn command_output<I, S>(
    program: &str,
    args: I,
    cwd: Option<&str>,
) -> std::io::Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.spawn()?.wait_with_output().await
}

/// Run a command line through the platform shell and capture its output.
/// The child is killed if the returned future is dropped.
pub async fn shell_output(command: &str, cwd: Option<&str>) -> std::io::Result<Output> {
    command_output(SHELL.0, [SHELL.1, command], cwd).await
}

/// Spawn a command line through the platform shell with piped stdout/stderr
/// for line-by-line streaming. The child is killed when dropped.
pub fn spawn_streaming_shell(command: &str, cwd: &str) -> std::io::Result<tokio::process::Child> {
    tokio::process::Command::new(SHELL.0)
        .arg(SHELL.1)
        .arg(command)
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
}

/// Spawn a long-lived server process (e.g. an LSP server) with piped
/// stdin/stdout for JSON-RPC style communication. The child is killed when
/// dropped.
pub fn spawn_piped_server(program: &str, args: &[&str]) -> std::io::Result<tokio::process::Child> {
    tokio::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
}

/// True when `program` resolves on the user's PATH.
pub fn binary_on_path(program: &str) -> bool {
    std::process::Command::new(WHICH)
        .arg(program)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
