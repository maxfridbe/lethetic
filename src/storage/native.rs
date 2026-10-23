//! Native durable adapter. A database lock serializes its users across
//! processes; all database and WAL opens retain the existing no-follow policy.
use super::{ContentDatabase, DATABASE_NAME};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
};
use turso_core::{Buffer, Clock, Completion, IO, MonotonicInstant, OpenFlags, WallClockInstant};

const LOCK_NAME: &str = "lethetic.db.lock";

fn read_legacy(directory: &Path, name: &str) -> Result<Option<Vec<u8>>, String> {
    if matches!(name, DATABASE_NAME | "lethetic.db-wal" | LOCK_NAME) {
        return Err("database infrastructure names are reserved".to_owned());
    }
    let bytes = crate::platform::read_file_nofollow(directory, &[], name).map_err(message)?;
    if bytes.is_some() {
        // Reuse the descriptor-based owner/type/link checks before migrating or
        // replacing a legacy destination, as the former atomic writer did.
        drop(
            crate::platform::open_lock_file_nofollow(directory, &[], name, 0o600)
                .map_err(message)?,
        );
    }
    Ok(bytes)
}

pub fn read(directory: &Path, name: &str) -> Result<Option<Vec<u8>>, String> {
    match std::fs::symlink_metadata(directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(message(error)),
        Ok(_) => {}
    }
    with_database(directory, |db, scope, directory| {
        if name == "session_state.json" {
            migrate_session_content(db, scope, directory)?;
        }
        if let Some(bytes) = db.read(scope, name)? {
            return Ok(Some(bytes));
        }
        let legacy = read_legacy(directory, name)?;
        if let Some(bytes) = &legacy {
            db.put(scope, name, bytes)?;
            // Keep the source until the database commit is durable. A cleanup
            // failure is visible and never replaces the committed data.
            crate::platform::remove_file_nofollow(directory, &[], name).map_err(message)?;
        }
        Ok(legacy)
    })
}

fn migrate_session_content(
    db: &ContentDatabase,
    scope: &str,
    directory: &Path,
) -> Result<(), String> {
    let mut legacy = Vec::new();
    for name in [
        "session_state.json",
        "ui_state.json",
        "context.json",
        "ui_log.txt",
        "ui_log_original.txt",
        "logs.txt",
        "tokens.jsonl",
        "last_raw_prompt.txt",
        "last_request.json",
    ] {
        if let Some(bytes) = read_legacy(directory, name)? {
            legacy.push((name, bytes));
        }
    }
    if legacy.is_empty() {
        return Ok(());
    }
    db.transaction(|| {
        for (name, bytes) in &legacy {
            if !db.contains(scope, name)? {
                db.append_in_transaction(scope, name, bytes)?;
            }
        }
        Ok(())
    })?;
    for (name, _) in legacy {
        crate::platform::remove_file_nofollow(directory, &[], name).map_err(message)?;
    }
    Ok(())
}

pub fn write(directory: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    with_database(directory, |db, scope, directory| {
        // Validate an existing legacy destination even when replacing it.
        let legacy = read_legacy(directory, name)?;
        db.put(scope, name, bytes)?;
        if legacy.is_some() {
            crate::platform::remove_file_nofollow(directory, &[], name).map_err(message)?;
        }
        Ok(())
    })
}

pub fn append(directory: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    with_database(directory, |db, scope, directory| {
        let legacy = read_legacy(directory, name)?;
        db.transaction(|| {
            if !db.contains(scope, name)?
                && let Some(legacy) = &legacy
            {
                db.append_in_transaction(scope, name, legacy)?;
            }
            db.append_in_transaction(scope, name, bytes)
        })?;
        if legacy.is_some() {
            crate::platform::remove_file_nofollow(directory, &[], name).map_err(message)?;
        }
        Ok(())
    })
}

/// Read a database-backed artifact by its historical logical path, or an
/// ordinary workspace/export file. Content consumers retain their path API.
pub fn read_path(path: &Path) -> Result<Vec<u8>, String> {
    if is_content_path(path) {
        let directory = path.parent().ok_or("missing artifact directory")?;
        let name = path
            .file_name()
            .and_then(|v| v.to_str())
            .ok_or("invalid artifact name")?;
        return read(directory, name)?
            .ok_or_else(|| format!("artifact not found: {}", path.display()));
    }
    std::fs::read(path).map_err(message)
}

fn is_content_path(path: &Path) -> bool {
    let Some(root) = path
        .ancestors()
        .find(|p| p.file_name().is_some_and(|v| v == ".lethetic"))
    else {
        return false;
    };
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    let parts = relative.components().collect::<Vec<_>>();
    let name = path.file_name().and_then(|v| v.to_str()).unwrap_or("");
    match parts.first().and_then(|v| v.as_os_str().to_str()) {
        Some("tool_responses" | "background") => true,
        Some("sessions" | "python-sessions") => matches!(
            name,
            "session_state.json"
                | "ui_state.json"
                | "context.json"
                | "ui_log.txt"
                | "ui_log_original.txt"
                | "logs.txt"
                | "tokens.jsonl"
                | "last_raw_prompt.txt"
                | "last_request.json"
        ),
        _ => {
            parts.len() == 1
                && matches!(
                    name,
                    "history.json"
                        | "todos.json"
                        | "tokens.jsonl"
                        | "last_request.json"
                        | "last_raw_prompt.txt"
                        | "logs.txt"
                        | "ui_log.txt"
                        | "ui_log_original.txt"
                        | "session_state.json"
                        | "ui_state.json"
                        | "context.json"
                )
        }
    }
}

pub fn read_text_path(path: &Path) -> Result<String, String> {
    String::from_utf8(read_path(path)?).map_err(message)
}

pub fn remove_scope(directory: &Path) -> Result<(), String> {
    with_database(directory, |db, scope, _| db.remove_scope(scope))
}

fn with_database<T>(
    directory: &Path,
    operation: impl FnOnce(&ContentDatabase, &str, &Path) -> Result<T, String>,
) -> Result<T, String> {
    // Resolve/create components through the same platform helper as the previous
    // session writer. Do not canonicalize away an untrusted final symlink.
    let mut components = Vec::new();
    let mut base = PathBuf::new();
    for component in directory.components() {
        match component {
            Component::Normal(value) => {
                components.push(value.to_str().ok_or("storage path must be UTF-8")?)
            }
            Component::Prefix(prefix) if directory.is_absolute() => base.push(prefix.as_os_str()),
            Component::RootDir if directory.is_absolute() => base.push(component.as_os_str()),
            _ => {
                return Err(
                    "storage path must not contain parent or current-directory components"
                        .to_owned(),
                );
            }
        }
    }
    if components.is_empty() {
        return Err("storage directory must not be empty".to_owned());
    }
    if base.as_os_str().is_empty() {
        base.push(".");
    }
    // The probe also rejects symlink/hardlink directory replacements before I/O.
    let probe = crate::platform::open_lock_file_nofollow(&base, &components, LOCK_NAME, 0o600)
        .map_err(message)?;
    drop(probe);
    // The descriptor walk has already validated every component. Do not
    // canonicalize here: a replacement symlink must be rejected, not followed.
    let directory = if directory.is_absolute() {
        directory.to_path_buf()
    } else {
        std::env::current_dir().map_err(message)?.join(directory)
    };
    let project = directory
        .ancestors()
        .find(|path| path.file_name().is_some_and(|name| name == ".lethetic"));
    // Durable sessions own their database. Project-wide todos, history and
    // unbound diagnostics keep a separate shared project database.
    let root = directory
        .ancestors()
        .find(|path| {
            path.parent().is_some_and(|parent| {
                parent
                    .file_name()
                    .is_some_and(|name| name == "sessions" || name == "python-sessions")
                    && parent.parent() == project
            })
        })
        .or(project)
        .unwrap_or(&directory);
    let scope = directory
        .strip_prefix(root)
        .map_err(message)?
        .to_str()
        .ok_or("storage scope must be UTF-8")?;
    #[cfg(windows)]
    let normalized_scope = scope.replace('\\', "/");
    #[cfg(windows)]
    let scope = normalized_scope.as_str();
    let lock =
        crate::platform::open_lock_file_nofollow(root, &[], LOCK_NAME, 0o600).map_err(message)?;
    lock.lock().map_err(message)?;
    let db = ContentDatabase::open(Arc::new(NativeIo(root.to_path_buf())), DATABASE_NAME)?;
    let result = (|| {
        if let Some(project) = project.filter(|project| *project != root) {
            migrate_project_session(project, root, &db)?;
        }
        operation(&db, scope, &directory)
    })();
    let close = db.close();
    match (result, close) {
        (Ok(result), Ok(())) => Ok(result),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}

/// Upgrade the earlier aggregate layout without ever deleting the only copy.
/// The session lock is already held; shared-project operations never acquire a
/// session lock, so this lock order cannot form a cycle.
fn migrate_project_session(
    project: &Path,
    session: &Path,
    destination: &ContentDatabase,
) -> Result<(), String> {
    const MARKER: &str = "project-import-v1";
    if destination.contains("_storage", MARKER)? {
        return Ok(());
    }
    if project.join(DATABASE_NAME).try_exists().map_err(message)? {
        let lock = crate::platform::open_lock_file_nofollow(project, &[], LOCK_NAME, 0o600)
            .map_err(message)?;
        lock.lock().map_err(message)?;
        let source =
            ContentDatabase::open(Arc::new(NativeIo(project.to_path_buf())), DATABASE_NAME)?;
        let scope = session
            .strip_prefix(project)
            .map_err(message)?
            .to_str()
            .ok_or("storage scope must be UTF-8")?
            .replace('\\', "/");
        let copied = destination.transaction(|| {
            for name in source.names(&scope)? {
                if name == "python.ipynb" {
                    if read_legacy(session, &name)?.is_none()
                        && let Some(bytes) = source.read(&scope, &name)?
                    {
                        crate::platform::atomic_write_nofollow(session, &[], &name, &bytes, 0o600)
                            .map_err(message)?;
                    }
                    continue;
                }
                if !destination.contains("", &name)?
                    && let Some(bytes) = source.read(&scope, &name)?
                {
                    destination.append_in_transaction("", &name, &bytes)?;
                }
            }
            Ok(())
        });
        let cleaned = copied.and_then(|()| source.remove_scope(&scope));
        let closed = source.close();
        cleaned?;
        closed?;
    }
    destination.put("_storage", MARKER, b"")
}

struct NativeIo(PathBuf);
impl Clock for NativeIo {
    fn current_time_monotonic(&self) -> MonotonicInstant {
        turso_core::io::clock::DefaultClock.current_time_monotonic()
    }
    fn current_time_wall_clock(&self) -> WallClockInstant {
        turso_core::io::clock::DefaultClock.current_time_wall_clock()
    }
}
impl IO for NativeIo {
    fn open_file(
        &self,
        name: &str,
        _flags: OpenFlags,
        _direct: bool,
    ) -> turso_core::Result<Arc<dyn turso_core::File>> {
        if !matches!(name, DATABASE_NAME | "lethetic.db-wal") {
            return Err(turso_core::LimboError::InvalidArgument(format!(
                "unexpected database file: {name}"
            )));
        }
        let file = crate::platform::open_lock_file_nofollow(&self.0, &[], name, 0o600)
            .map_err(|e| turso_core::io_error(e, "open content database"))?;
        Ok(Arc::new(NativeFile(Mutex::new(file))))
    }
    fn remove_file(&self, name: &str) -> turso_core::Result<()> {
        if name != "lethetic.db-wal" {
            return Err(turso_core::LimboError::InvalidArgument(
                "unexpected database removal".to_owned(),
            ));
        }
        crate::platform::remove_file_nofollow(&self.0, &[], name)
            .map_err(|e| turso_core::io_error(e, "remove database WAL"))?;
        Ok(())
    }
}

struct NativeFile(Mutex<File>);
impl NativeFile {
    fn access<T>(&self, f: impl FnOnce(&mut File) -> std::io::Result<T>) -> turso_core::Result<T> {
        let mut file = self.0.lock().map_err(|_| {
            turso_core::LimboError::InternalError("database file lock poisoned".to_owned())
        })?;
        f(&mut file).map_err(|e| turso_core::io_error(e, "content database I/O"))
    }
}
impl turso_core::File for NativeFile {
    // The project-wide OS lock is held for the entire connection lifetime.
    fn lock_file(&self, _exclusive: bool) -> turso_core::Result<()> {
        Ok(())
    }
    fn unlock_file(&self) -> turso_core::Result<()> {
        Ok(())
    }
    fn pread(&self, pos: u64, completion: Completion) -> turso_core::Result<Completion> {
        let size = self.access(|file| {
            file.seek(SeekFrom::Start(pos))?;
            let buffer = completion.as_read().buf();
            let bytes = buffer.as_mut_slice();
            bytes.fill(0);
            let mut size = 0;
            while size < bytes.len() {
                let count = file.read(&mut bytes[size..])?;
                if count == 0 {
                    break;
                }
                size += count;
            }
            Ok(size)
        })?;
        completion.complete(size as i32);
        Ok(completion)
    }
    fn pwrite(
        &self,
        pos: u64,
        buffer: Arc<Buffer>,
        completion: Completion,
    ) -> turso_core::Result<Completion> {
        self.access(|file| {
            file.seek(SeekFrom::Start(pos))?;
            file.write_all(buffer.as_slice())
        })?;
        completion.complete(buffer.len() as i32);
        Ok(completion)
    }
    fn sync(
        &self,
        completion: Completion,
        _kind: turso_core::io::FileSyncType,
    ) -> turso_core::Result<Completion> {
        self.access(|file| file.sync_all())?;
        completion.complete(0);
        Ok(completion)
    }
    fn size(&self) -> turso_core::Result<u64> {
        self.access(|file| Ok(file.metadata()?.len()))
    }
    fn truncate(&self, size: u64, completion: Completion) -> turso_core::Result<Completion> {
        self.access(|file| file.set_len(size))?;
        completion.complete(0);
        Ok(completion)
    }
}

fn message(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(test)]
#[path = "native_tests.rs"]
mod tests;
