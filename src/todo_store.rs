use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const TODO_DIRECTORY: &str = ".lethetic";
const TODO_FILE_NAME: &str = "todos.json";
const TODO_LOCK_FILE_NAME: &str = "todos.lock";
const MAX_TODO_FILE_BYTES: usize = 1024 * 1024;
const MAX_TODOS: usize = 512;
const MAX_TODO_ID_BYTES: usize = 256;
const MAX_TODO_CONTENT_BYTES: usize = 16 * 1024;
const LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const LOCK_RETRY: Duration = Duration::from_millis(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TodoStoreErrorCode {
    Invalid,
    Conflict,
    Oversized,
    Busy,
    Storage,
}

impl TodoStoreErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Invalid => "invalid",
            Self::Conflict => "revision_conflict",
            Self::Oversized => "oversized",
            Self::Busy => "busy",
            Self::Storage => "storage",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TodoStoreError {
    pub code: TodoStoreErrorCode,
    pub message: String,
}

impl TodoStoreError {
    fn new(code: TodoStoreErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn storage(context: &str, error: impl fmt::Display) -> Self {
        Self::new(TodoStoreErrorCode::Storage, format!("{context}: {error}"))
    }
}

impl fmt::Display for TodoStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for TodoStoreError {}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

impl TodoStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TodoPriority {
    High,
    Medium,
    Low,
}

impl TodoPriority {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TodoItem {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub content: String,
    pub status: TodoStatus,
    pub priority: TodoPriority,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TodoSnapshot {
    pub revision: u64,
    pub todos: Vec<TodoItem>,
}

impl Default for TodoSnapshot {
    fn default() -> Self {
        Self {
            revision: 0,
            todos: Vec::new(),
        }
    }
}

#[derive(Debug)]
struct RootIdentity {
    canonical_path: PathBuf,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl RootIdentity {
    fn capture(path: &Path) -> Result<Self, TodoStoreError> {
        let link = std::fs::symlink_metadata(path)
            .map_err(|error| TodoStoreError::storage("Could not inspect todo root", error))?;
        if link.file_type().is_symlink() || !link.is_dir() {
            return Err(TodoStoreError::new(
                TodoStoreErrorCode::Storage,
                format!("Todo root must be a real directory: {}", path.display()),
            ));
        }
        let canonical_path = path
            .canonicalize()
            .map_err(|error| TodoStoreError::storage("Could not canonicalize todo root", error))?;
        let metadata = std::fs::metadata(&canonical_path).map_err(|error| {
            TodoStoreError::storage("Could not inspect canonical todo root", error)
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.uid() != rustix::process::geteuid().as_raw() {
                return Err(TodoStoreError::new(
                    TodoStoreErrorCode::Storage,
                    "Todo root is not owned by the invoking user",
                ));
            }
            Ok(Self {
                canonical_path,
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }
        #[cfg(not(unix))]
        {
            let _ = metadata;
            Ok(Self { canonical_path })
        }
    }

    fn verify(&self) -> Result<(), TodoStoreError> {
        let _current = Self::capture(&self.canonical_path)?;
        #[cfg(unix)]
        if _current.device != self.device || _current.inode != self.inode {
            return Err(TodoStoreError::new(
                TodoStoreErrorCode::Storage,
                "Todo root directory identity changed",
            ));
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct TodoStore {
    root: RootIdentity,
}

impl TodoStore {
    pub fn open(root: &Path) -> Result<Self, TodoStoreError> {
        Ok(Self {
            root: RootIdentity::capture(root)?,
        })
    }

    pub fn path(&self) -> PathBuf {
        self.root
            .canonical_path
            .join(TODO_DIRECTORY)
            .join(TODO_FILE_NAME)
    }

    pub fn parse_todos(value: &serde_json::Value) -> Result<Vec<TodoItem>, TodoStoreError> {
        let todos = serde_json::from_value::<Vec<TodoItem>>(value.clone()).map_err(|error| {
            TodoStoreError::new(
                TodoStoreErrorCode::Invalid,
                format!("Invalid todo list: {error}"),
            )
        })?;
        validate_todos(&todos)?;
        Ok(todos)
    }

    pub fn get(&self) -> Result<TodoSnapshot, TodoStoreError> {
        self.root.verify()?;
        let _lock = self.acquire_lock()?;
        self.root.verify()?;
        self.read_locked()
    }

    pub fn replace(
        &self,
        todos: Vec<TodoItem>,
        expected_revision: u64,
    ) -> Result<TodoSnapshot, TodoStoreError> {
        validate_todos(&todos)?;
        self.root.verify()?;
        let _lock = self.acquire_lock()?;
        self.root.verify()?;
        let current = self.read_locked()?;
        if current.revision != expected_revision {
            return Err(TodoStoreError::new(
                TodoStoreErrorCode::Conflict,
                format!(
                    "Todo revision conflict: expected {expected_revision}, current revision is {}",
                    current.revision
                ),
            ));
        }
        self.write_next_locked(current.revision, todos)
    }

    pub fn replace_current(&self, todos: Vec<TodoItem>) -> Result<TodoSnapshot, TodoStoreError> {
        validate_todos(&todos)?;
        self.root.verify()?;
        let _lock = self.acquire_lock()?;
        self.root.verify()?;
        let current = self.read_locked()?;
        self.write_next_locked(current.revision, todos)
    }

    fn read_locked(&self) -> Result<TodoSnapshot, TodoStoreError> {
        let bytes = crate::platform::read_file_nofollow(
            &self.root.canonical_path,
            &[TODO_DIRECTORY],
            TODO_FILE_NAME,
        )
        .map_err(|error| TodoStoreError::storage("Could not safely read todos", error))?;
        let Some(bytes) = bytes else {
            return Ok(TodoSnapshot::default());
        };
        if bytes.len() > MAX_TODO_FILE_BYTES {
            return Err(TodoStoreError::new(
                TodoStoreErrorCode::Oversized,
                format!("Todo file exceeds {MAX_TODO_FILE_BYTES} bytes"),
            ));
        }
        let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
            TodoStoreError::new(
                TodoStoreErrorCode::Invalid,
                format!("Todo file is invalid JSON: {error}"),
            )
        })?;
        let snapshot = if value.is_array() {
            TodoSnapshot {
                revision: 0,
                todos: Self::parse_todos(&value)?,
            }
        } else {
            serde_json::from_value::<TodoSnapshot>(value).map_err(|error| {
                TodoStoreError::new(
                    TodoStoreErrorCode::Invalid,
                    format!("Todo file has an invalid envelope: {error}"),
                )
            })?
        };
        validate_todos(&snapshot.todos)?;
        Ok(snapshot)
    }

    fn write_next_locked(
        &self,
        current_revision: u64,
        todos: Vec<TodoItem>,
    ) -> Result<TodoSnapshot, TodoStoreError> {
        let revision = current_revision.checked_add(1).ok_or_else(|| {
            TodoStoreError::new(TodoStoreErrorCode::Invalid, "Todo revision overflowed")
        })?;
        let snapshot = TodoSnapshot { revision, todos };
        let encoded = serde_json::to_vec_pretty(&snapshot).map_err(|error| {
            TodoStoreError::new(
                TodoStoreErrorCode::Invalid,
                format!("Could not encode todos: {error}"),
            )
        })?;
        if encoded.len() > MAX_TODO_FILE_BYTES {
            return Err(TodoStoreError::new(
                TodoStoreErrorCode::Oversized,
                format!("Encoded todo file exceeds {MAX_TODO_FILE_BYTES} bytes"),
            ));
        }
        crate::platform::atomic_write_nofollow(
            &self.root.canonical_path,
            &[TODO_DIRECTORY],
            TODO_FILE_NAME,
            &encoded,
            0o600,
        )
        .map_err(|error| TodoStoreError::storage("Could not safely write todos", error))?;
        self.root.verify()?;
        Ok(snapshot)
    }

    fn acquire_lock(&self) -> Result<TodoFileLock, TodoStoreError> {
        let file = crate::platform::open_lock_file_nofollow(
            &self.root.canonical_path,
            &[TODO_DIRECTORY],
            TODO_LOCK_FILE_NAME,
            0o600,
        )
        .map_err(|error| TodoStoreError::storage("Could not open todo transaction lock", error))?;
        lock_file(file)
    }
}

fn validate_todos(todos: &[TodoItem]) -> Result<(), TodoStoreError> {
    if todos.len() > MAX_TODOS {
        return Err(TodoStoreError::new(
            TodoStoreErrorCode::Oversized,
            format!("Todo list exceeds {MAX_TODOS} items"),
        ));
    }
    let mut ids = HashSet::new();
    for (index, todo) in todos.iter().enumerate() {
        if todo.content.trim().is_empty() || todo.content.len() > MAX_TODO_CONTENT_BYTES {
            return Err(TodoStoreError::new(
                TodoStoreErrorCode::Invalid,
                format!(
                    "Todo item {index} content must be non-empty and at most {MAX_TODO_CONTENT_BYTES} bytes"
                ),
            ));
        }
        if let Some(id) = &todo.id {
            if id.is_empty()
                || id.len() > MAX_TODO_ID_BYTES
                || id.trim() != id
                || id.chars().any(char::is_control)
            {
                return Err(TodoStoreError::new(
                    TodoStoreErrorCode::Invalid,
                    format!("Todo item {index} has an invalid ID"),
                ));
            }
            if !ids.insert(id) {
                return Err(TodoStoreError::new(
                    TodoStoreErrorCode::Invalid,
                    format!("Todo ID {id:?} is duplicated"),
                ));
            }
        }
    }
    let encoded = serde_json::to_vec(todos).map_err(|error| {
        TodoStoreError::new(
            TodoStoreErrorCode::Invalid,
            format!("Could not validate encoded todos: {error}"),
        )
    })?;
    if encoded.len() > MAX_TODO_FILE_BYTES {
        return Err(TodoStoreError::new(
            TodoStoreErrorCode::Oversized,
            format!("Todo list exceeds {MAX_TODO_FILE_BYTES} encoded bytes"),
        ));
    }
    Ok(())
}

#[derive(Debug)]
struct TodoFileLock {
    file: std::fs::File,
}

#[cfg(unix)]
fn lock_file(file: std::fs::File) -> Result<TodoFileLock, TodoStoreError> {
    use std::os::fd::AsRawFd;

    let deadline = Instant::now() + LOCK_TIMEOUT;
    loop {
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result == 0 {
            return Ok(TodoFileLock { file });
        }
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        if error.kind() == std::io::ErrorKind::WouldBlock {
            if Instant::now() >= deadline {
                return Err(TodoStoreError::new(
                    TodoStoreErrorCode::Busy,
                    "Timed out waiting for the todo transaction lock",
                ));
            }
            std::thread::sleep(LOCK_RETRY);
            continue;
        }
        return Err(TodoStoreError::storage(
            "Could not acquire todo transaction lock",
            error,
        ));
    }
}

#[cfg(windows)]
fn lock_file(file: std::fs::File) -> Result<TodoFileLock, TodoStoreError> {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LockFile(
            file: *mut c_void,
            offset_low: u32,
            offset_high: u32,
            bytes_low: u32,
            bytes_high: u32,
        ) -> i32;
    }

    let deadline = Instant::now() + LOCK_TIMEOUT;
    loop {
        let result = unsafe { LockFile(file.as_raw_handle().cast(), 0, 0, u32::MAX, u32::MAX) };
        if result != 0 {
            return Ok(TodoFileLock { file });
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(33) {
            if Instant::now() >= deadline {
                return Err(TodoStoreError::new(
                    TodoStoreErrorCode::Busy,
                    "Timed out waiting for the todo transaction lock",
                ));
            }
            std::thread::sleep(LOCK_RETRY);
            continue;
        }
        return Err(TodoStoreError::storage(
            "Could not acquire todo transaction lock",
            error,
        ));
    }
}

#[cfg(all(not(unix), not(windows)))]
fn lock_file(_file: std::fs::File) -> Result<TodoFileLock, TodoStoreError> {
    Err(TodoStoreError::new(
        TodoStoreErrorCode::Storage,
        "Todo compare-and-swap locking is unsupported on this platform",
    ))
}

#[cfg(unix)]
impl Drop for TodoFileLock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
}

#[cfg(windows)]
impl Drop for TodoFileLock {
    fn drop(&mut self) {
        use std::ffi::c_void;
        use std::os::windows::io::AsRawHandle;

        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn UnlockFile(
                file: *mut c_void,
                offset_low: u32,
                offset_high: u32,
                bytes_low: u32,
                bytes_high: u32,
            ) -> i32;
        }
        let _ = unsafe { UnlockFile(self.file.as_raw_handle().cast(), 0, 0, u32::MAX, u32::MAX) };
    }
}

#[cfg(all(not(unix), not(windows)))]
impl Drop for TodoFileLock {
    fn drop(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, content: &str) -> TodoItem {
        TodoItem {
            id: Some(id.to_string()),
            content: content.to_string(),
            status: TodoStatus::Pending,
            priority: TodoPriority::Medium,
        }
    }

    #[test]
    fn imports_legacy_array_and_writes_revisioned_envelope() {
        let root = tempfile::tempdir().unwrap();
        let store = TodoStore::open(root.path()).unwrap();
        crate::platform::atomic_write_nofollow(
            root.path(),
            &[TODO_DIRECTORY],
            TODO_FILE_NAME,
            br#"[{"id":"old","content":"legacy","status":"pending","priority":"low"}]"#,
            0o600,
        )
        .unwrap();

        let legacy = store.get().unwrap();
        assert_eq!(legacy.revision, 0);
        assert_eq!(legacy.todos.len(), 1);
        let updated = store.replace(vec![item("new", "replacement")], 0).unwrap();
        assert_eq!(updated.revision, 1);
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(store.path()).unwrap()).unwrap();
        assert_eq!(value["revision"], 1);
        assert!(value["todos"].is_array());
    }

    #[test]
    fn compare_and_swap_rejects_stale_revision() {
        let root = tempfile::tempdir().unwrap();
        let store = TodoStore::open(root.path()).unwrap();
        let first = store.replace(vec![item("one", "first")], 0).unwrap();
        assert_eq!(first.revision, 1);
        let error = store.replace(vec![item("two", "stale")], 0).unwrap_err();
        assert_eq!(error.code, TodoStoreErrorCode::Conflict);
        assert_eq!(store.get().unwrap(), first);
    }

    #[test]
    fn concurrent_compare_and_swap_has_exactly_one_winner() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let mut workers = Vec::new();
        for id in ["one", "two"] {
            let root = root.clone();
            let barrier = barrier.clone();
            workers.push(std::thread::spawn(move || {
                let store = TodoStore::open(&root).unwrap();
                barrier.wait();
                store.replace(vec![item(id, id)], 0)
            }));
        }
        barrier.wait();
        let results = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(
                    result,
                    Err(error) if error.code == TodoStoreErrorCode::Conflict
                ))
                .count(),
            1
        );
        let snapshot = TodoStore::open(&root).unwrap().get().unwrap();
        assert_eq!(snapshot.revision, 1);
        assert_eq!(snapshot.todos.len(), 1);
    }

    #[test]
    fn rejects_unknown_fields_duplicate_ids_and_malformed_existing_data() {
        let root = tempfile::tempdir().unwrap();
        let store = TodoStore::open(root.path()).unwrap();
        let unknown = serde_json::json!([{
            "id": "x",
            "content": "task",
            "status": "pending",
            "priority": "low",
            "path": "/tmp/forbidden"
        }]);
        assert_eq!(
            TodoStore::parse_todos(&unknown).unwrap_err().code,
            TodoStoreErrorCode::Invalid
        );
        assert_eq!(
            store
                .replace(vec![item("same", "one"), item("same", "two")], 0)
                .unwrap_err()
                .code,
            TodoStoreErrorCode::Invalid
        );
        crate::platform::atomic_write_nofollow(
            root.path(),
            &[TODO_DIRECTORY],
            TODO_FILE_NAME,
            b"not-json",
            0o600,
        )
        .unwrap();
        assert_eq!(store.get().unwrap_err().code, TodoStoreErrorCode::Invalid);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_destination_is_rejected_without_overwrite() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(TODO_DIRECTORY)).unwrap();
        let outside = root.path().join("outside");
        std::fs::write(&outside, "safe").unwrap();
        symlink(
            &outside,
            root.path().join(TODO_DIRECTORY).join(TODO_FILE_NAME),
        )
        .unwrap();
        let store = TodoStore::open(root.path()).unwrap();
        assert_eq!(
            store
                .replace(vec![item("one", "first")], 0)
                .unwrap_err()
                .code,
            TodoStoreErrorCode::Storage
        );
        assert_eq!(std::fs::read_to_string(outside).unwrap(), "safe");
    }
}
