//! Windows-specific code.

/// Resident set size of this process in MB.
///
/// Not implemented on Windows yet — would need `GetProcessMemoryInfo` via the
/// `windows`/`winapi` crate, which isn't worth a dependency for a status-line
/// number. The UI simply shows 0.
pub fn process_rss_mb() -> u64 {
    0
}
