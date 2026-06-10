//! macOS-specific code.

/// Resident set size of this process in MB. macOS has no `/proc`; shell out
/// to `ps` (rss column is reported in kB).
pub fn process_rss_mb() -> u64 {
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u64>().ok())
        .map(|kb| kb / 1024)
        .unwrap_or(0)
}
