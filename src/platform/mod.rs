//! Operating-system-specific code.
//!
//! Everything that touches processes, platform directories, or OS interfaces
//! (`/proc`, `which`, shells, …) lives behind this module so that a future
//! wasm32/web build only has to reimplement this surface instead of hunting
//! through the codebase.
//!
//! Layout:
//! - `native.rs` — implementations shared by all desktop OSes (subprocess
//!   execution, config directories). Compiled for everything except wasm32.
//! - `linux.rs` / `macos.rs` / `windows.rs` — code that genuinely differs per
//!   OS (currently process memory statistics).
//! - `web.rs` — wasm32 stubs. Subprocess spawning has no browser equivalent;
//!   capture-style helpers return `ErrorKind::Unsupported` so callers degrade
//!   gracefully. The streaming/piped spawn helpers exist only on native —
//!   their callers (`run_shell_command`, the LSP manager) are inherently
//!   native features and must be compiled out of a web build.

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
pub use native::*;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::*;

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub use web::*;
