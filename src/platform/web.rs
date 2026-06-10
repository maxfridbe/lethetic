//! wasm32 (browser) stubs.
//!
//! There are no subprocesses, platform shells, or config directories in the
//! browser. Capture-style helpers return `ErrorKind::Unsupported` so tools
//! report a clean error to the model; a web build that wants these features
//! must route them to a remote execution backend instead.
//!
//! Note: `spawn_streaming_shell` and `spawn_piped_server` (which return
//! `tokio::process::Child`) intentionally do not exist here — their callers
//! (`run_shell_command`, the LSP manager) are native-only features that must
//! be compiled out of a web build.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::Output;

fn unsupported() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "subprocesses are not available in the browser",
    )
}

/// No process metrics in the browser.
pub fn process_rss_mb() -> u64 {
    0
}

/// No per-user config directory in the browser; use the (virtual) cwd.
pub fn lethetic_config_dir() -> PathBuf {
    PathBuf::from(".lethetic-config")
}

pub async fn command_output<I, S>(
    _program: &str,
    _args: I,
    _cwd: Option<&str>,
) -> std::io::Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Err(unsupported())
}

pub async fn shell_output(_command: &str, _cwd: Option<&str>) -> std::io::Result<Output> {
    Err(unsupported())
}

pub fn binary_on_path(_program: &str) -> bool {
    false
}
