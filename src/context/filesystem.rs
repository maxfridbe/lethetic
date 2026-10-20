//! Native host adapter for bounded, consistent file snapshots.
use super::api::FileRead;
use std::{io::Read, path::Path};

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileSnapshot {
    len: u64,
    modified: Option<std::time::SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    modified_seconds: i64,
    #[cfg(unix)]
    modified_nanoseconds: i64,
    #[cfg(unix)]
    changed_seconds: i64,
    #[cfg(unix)]
    changed_nanoseconds: i64,
}

fn snapshot_metadata(metadata: &std::fs::Metadata) -> FileSnapshot {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;

    FileSnapshot {
        len: metadata.len(),
        modified: metadata.modified().ok(),
        #[cfg(unix)]
        device: metadata.dev(),
        #[cfg(unix)]
        inode: metadata.ino(),
        #[cfg(unix)]
        modified_seconds: metadata.mtime(),
        #[cfg(unix)]
        modified_nanoseconds: metadata.mtime_nsec(),
        #[cfg(unix)]
        changed_seconds: metadata.ctime(),
        #[cfg(unix)]
        changed_nanoseconds: metadata.ctime_nsec(),
    }
}

#[cfg(unix)]
fn open_for_bounded_read(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
}

#[cfg(not(unix))]
fn open_for_bounded_read(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::File::open(path)
}

pub(super) fn read_complete_file(path: &Path, byte_limit: usize) -> FileRead {
    // Open once, then budget and verify that exact handle. O_NONBLOCK prevents a
    // path replacement with a FIFO from stalling Unix request preparation.
    let mut file = match open_for_bounded_read(path) {
        Ok(file) => file,
        Err(_) => return FileRead::Unavailable,
    };
    let initial_metadata = match file.metadata() {
        Ok(metadata) if metadata.is_file() => metadata,
        _ => return FileRead::Unavailable,
    };
    let initial = snapshot_metadata(&initial_metadata);
    if initial.len > u64::try_from(byte_limit).unwrap_or(u64::MAX) {
        return FileRead::TooLarge;
    }

    let read_limit = u64::try_from(byte_limit)
        .unwrap_or(u64::MAX - 1)
        .saturating_add(1);
    let initial_capacity = usize::try_from(initial.len)
        .unwrap_or(byte_limit)
        .min(byte_limit)
        .min(64 * 1024);
    let mut bytes = Vec::with_capacity(initial_capacity);
    if file
        .by_ref()
        .take(read_limit)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return FileRead::Unavailable;
    }
    if bytes.len() > byte_limit {
        return FileRead::TooLarge;
    }
    let final_metadata = match file.metadata() {
        Ok(metadata) if metadata.is_file() => metadata,
        _ => return FileRead::Changed,
    };
    let final_snapshot = snapshot_metadata(&final_metadata);
    if final_snapshot != initial || initial.len != u64::try_from(bytes.len()).unwrap_or(u64::MAX) {
        return FileRead::Changed;
    }

    match String::from_utf8(bytes) {
        Ok(content) => FileRead::Complete(content),
        Err(_) => FileRead::NonUtf8,
    }
}
