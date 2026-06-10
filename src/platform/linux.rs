//! Linux-specific code.

/// Resident set size of this process in MB, read from `/proc/self/status`
/// (the `VmRSS` field is reported in kB).
pub fn process_rss_mb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            let line = s.lines().find(|l| l.starts_with("VmRSS:"))?;
            line.split_whitespace().nth(1)?.parse::<u64>().ok()
        })
        .map(|kb| kb / 1024)
        .unwrap_or(0)
}
