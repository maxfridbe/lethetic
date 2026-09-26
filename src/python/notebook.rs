use crate::python::PythonCellResult;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const NOTEBOOK_FILE_NAME: &str = "python.ipynb";
const MAX_NOTEBOOK_BYTES: usize = 32 * 1024 * 1024;
const MAX_SOURCE_BYTES: usize = 4 * 1024 * 1024;
const MAX_DESCRIPTION_BYTES: usize = 16 * 1024;
const MAX_TOOL_CALL_ID_BYTES: usize = 16 * 1024;
const MAX_HOST_CALLS_PER_ATTEMPT: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotebookAttemptStatus {
    Approved,
    Running,
    Denied,
    Cancelled,
    Interrupted,
    LaunchFailed,
    Error,
    Success,
}

impl NotebookAttemptStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Running => "running",
            Self::Denied => "denied",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
            Self::LaunchFailed => "launch_failed",
            Self::Error => "errored",
            Self::Success => "succeeded",
        }
    }
}

#[derive(Clone, Debug)]
pub struct NotebookAttemptStart<'a> {
    pub tool_call_id: &'a str,
    pub source: &'a str,
    pub description: &'a str,
    pub cwd: &'a str,
    pub policy_fingerprint: &'a str,
}

#[derive(Clone, Copy, Debug)]
pub struct NotebookHostCallOutcome<'a> {
    pub success: bool,
    pub error_code: Option<&'a str>,
    pub revision: Option<u64>,
    pub todo_count: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Notebook {
    cells: Vec<NotebookCell>,
    metadata: Value,
    nbformat: u32,
    nbformat_minor: u32,
}

impl Default for Notebook {
    fn default() -> Self {
        Self {
            cells: Vec::new(),
            metadata: json!({
                "kernelspec": {
                    "display_name": "Python 3 (Lethetic audit)",
                    "language": "python",
                    "name": "python3"
                },
                "language_info": { "name": "python" },
                "lethetic": {
                    "artifact": "python_tool_audit",
                    "schema": 1
                }
            }),
            nbformat: 4,
            nbformat_minor: 5,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct NotebookCell {
    cell_type: String,
    execution_count: Option<u64>,
    id: String,
    metadata: Value,
    outputs: Vec<Value>,
    source: String,
}

#[derive(Debug)]
struct NotebookLocation {
    root: PathBuf,
    directories: Vec<String>,
    path: PathBuf,
    identity: DirectoryIdentity,
}

#[derive(Debug)]
struct DirectoryIdentity {
    canonical_path: PathBuf,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl DirectoryIdentity {
    fn capture(path: &Path) -> Result<Self, String> {
        let link = std::fs::symlink_metadata(path).map_err(|error| {
            format!(
                "Could not inspect Python notebook root {}: {error}",
                path.display()
            )
        })?;
        if link.file_type().is_symlink() || !link.is_dir() {
            return Err(format!(
                "Python notebook root must be a real directory: {}",
                path.display()
            ));
        }
        let canonical_path = path.canonicalize().map_err(|error| {
            format!(
                "Could not canonicalize Python notebook root {}: {error}",
                path.display()
            )
        })?;
        if canonical_path != path {
            return Err(format!(
                "Python notebook root must already be canonical: {}",
                path.display()
            ));
        }
        #[cfg(unix)]
        let metadata = std::fs::metadata(&canonical_path).map_err(|error| {
            format!(
                "Could not inspect canonical Python notebook root {}: {error}",
                canonical_path.display()
            )
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.uid() != rustix::process::geteuid().as_raw() {
                return Err("Python notebook root is not owned by the invoking user".to_string());
            }
            Ok(Self {
                canonical_path,
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }
        #[cfg(not(unix))]
        {
            Ok(Self { canonical_path })
        }
    }

    fn verify(&self) -> Result<(), String> {
        let _current = Self::capture(&self.canonical_path)?;
        #[cfg(unix)]
        if _current.device != self.device || _current.inode != self.inode {
            return Err("Python notebook root directory identity changed".to_string());
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct PythonNotebook {
    location: NotebookLocation,
    notebook: Mutex<Notebook>,
}

impl PythonNotebook {
    pub fn in_existing_directory(directory: &Path) -> Result<Self, String> {
        let directory = directory.canonicalize().map_err(|error| {
            format!(
                "Could not canonicalize Python session directory {}: {error}",
                directory.display()
            )
        })?;
        let identity = DirectoryIdentity::capture(&directory)?;
        Self::open(NotebookLocation {
            root: directory.clone(),
            directories: Vec::new(),
            path: directory.join(NOTEBOOK_FILE_NAME),
            identity,
        })
    }

    pub fn non_durable(control_root: &Path, run_id: &str) -> Result<Self, String> {
        let root = control_root.canonicalize().map_err(|error| {
            format!(
                "Could not canonicalize Python notebook control root {}: {error}",
                control_root.display()
            )
        })?;
        let identity = DirectoryIdentity::capture(&root)?;
        validate_component(run_id, "Python notebook run ID")?;
        let directories = vec![
            ".lethetic".to_string(),
            "python-sessions".to_string(),
            run_id.to_string(),
        ];
        let path = root
            .join(".lethetic")
            .join("python-sessions")
            .join(run_id)
            .join(NOTEBOOK_FILE_NAME);
        Self::open(NotebookLocation {
            root,
            directories,
            path,
            identity,
        })
    }

    fn open(location: NotebookLocation) -> Result<Self, String> {
        location.identity.verify()?;
        let directory_refs = location
            .directories
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let bytes = crate::platform::read_file_nofollow(
            &location.root,
            &directory_refs,
            NOTEBOOK_FILE_NAME,
        )
        .map_err(|error| format!("Could not read Python notebook audit: {error}"))?;
        let mut notebook = match bytes {
            None => Notebook::default(),
            Some(bytes) => {
                if bytes.len() > MAX_NOTEBOOK_BYTES {
                    return Err(format!(
                        "Existing Python notebook exceeds {MAX_NOTEBOOK_BYTES} bytes"
                    ));
                }
                let notebook: Notebook = serde_json::from_slice(&bytes)
                    .map_err(|error| format!("Existing Python notebook is invalid: {error}"))?;
                validate_notebook(&notebook)?;
                notebook
            }
        };
        let repaired = repair_interrupted_cells(&mut notebook);
        let recorder = Self {
            location,
            notebook: Mutex::new(notebook),
        };
        if repaired > 0 {
            recorder.persist_locked(
                &recorder
                    .notebook
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            )?;
        }
        Ok(recorder)
    }

    pub fn path(&self) -> &Path {
        &self.location.path
    }

    pub fn begin_attempt(&self, start: NotebookAttemptStart<'_>) -> Result<(), String> {
        validate_attempt_start(&start)?;
        self.mutate_and_persist(|notebook| {
            if let Some(cell) = find_cell_mut(notebook, start.tool_call_id) {
                if cell.source != start.source {
                    return Err(format!(
                        "Python notebook tool-call ID '{}' was reused with different source",
                        start.tool_call_id
                    ));
                }
                return Ok(());
            }
            let execution_count = notebook
                .cells
                .iter()
                .filter_map(|cell| cell.execution_count)
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or_else(|| "Python notebook execution counter overflowed".to_string())?;
            let now = Utc::now().to_rfc3339();
            notebook.cells.push(NotebookCell {
                cell_type: "code".to_string(),
                execution_count: Some(execution_count),
                id: cell_id(start.tool_call_id, execution_count),
                metadata: json!({
                    "lethetic": {
                        "tool_call_id": start.tool_call_id,
                        "description": start.description,
                        "status": "pending",
                        "created_at": now,
                        "updated_at": now,
                        "cwd": start.cwd,
                        "policy_fingerprint": start.policy_fingerprint,
                        "host_calls": []
                    }
                }),
                outputs: Vec::new(),
                source: start.source.to_string(),
            });
            Ok(())
        })
    }

    pub fn mark_status(
        &self,
        tool_call_id: &str,
        status: NotebookAttemptStatus,
        message: Option<&str>,
    ) -> Result<(), String> {
        self.mutate_and_persist(|notebook| {
            let cell = find_cell_mut(notebook, tool_call_id).ok_or_else(|| {
                format!("Python notebook has no attempt for tool-call ID '{tool_call_id}'")
            })?;
            set_status(cell, status.as_str())?;
            if let Some(message) = message {
                cell.outputs = vec![error_output(status_error_name(status), message)];
            }
            Ok(())
        })
    }

    pub fn mark_running(
        &self,
        tool_call_id: &str,
        backend: &str,
        policy_fingerprint: &str,
    ) -> Result<(), String> {
        self.mutate_and_persist(|notebook| {
            let cell = find_cell_mut(notebook, tool_call_id).ok_or_else(|| {
                format!("Python notebook has no attempt for tool-call ID '{tool_call_id}'")
            })?;
            set_status(cell, NotebookAttemptStatus::Running.as_str())?;
            let metadata = lethetic_metadata_mut(cell)?;
            metadata.insert("backend".to_string(), Value::String(backend.to_string()));
            metadata.insert(
                "execution_policy_fingerprint".to_string(),
                Value::String(policy_fingerprint.to_string()),
            );
            Ok(())
        })
    }

    pub fn begin_host_call(
        &self,
        tool_call_id: &str,
        sub_id: u64,
        operation: &str,
    ) -> Result<(), String> {
        if !matches!(operation, "todo.get" | "todo.set") {
            return Err("Python notebook rejected an unknown host-call operation".to_string());
        }
        self.mutate_and_persist(|notebook| {
            let cell = find_cell_mut(notebook, tool_call_id).ok_or_else(|| {
                format!("Python notebook has no attempt for tool-call ID '{tool_call_id}'")
            })?;
            if current_status(cell)? != NotebookAttemptStatus::Running.as_str() {
                return Err("Python notebook host calls require a running attempt".to_string());
            }
            let metadata = lethetic_metadata_mut(cell)?;
            let host_calls = metadata
                .get_mut("host_calls")
                .and_then(Value::as_array_mut)
                .ok_or_else(|| "Python notebook host-call metadata is invalid".to_string())?;
            if host_calls.len() >= MAX_HOST_CALLS_PER_ATTEMPT {
                return Err(format!(
                    "Python notebook host-call count exceeds {MAX_HOST_CALLS_PER_ATTEMPT}"
                ));
            }
            let expected = u64::try_from(host_calls.len())
                .ok()
                .and_then(|count| count.checked_add(1))
                .ok_or_else(|| "Python notebook host-call counter overflowed".to_string())?;
            if sub_id != expected {
                return Err(format!(
                    "Python notebook host-call ID mismatch: expected {expected}, got {sub_id}"
                ));
            }
            let now = Utc::now().to_rfc3339();
            host_calls.push(json!({
                "sub_id": sub_id,
                "operation": operation,
                "status": "running",
                "started_at": now,
                "updated_at": now
            }));
            Ok(())
        })
    }

    pub fn finish_host_call(
        &self,
        tool_call_id: &str,
        sub_id: u64,
        operation: &str,
        outcome: NotebookHostCallOutcome<'_>,
    ) -> Result<(), String> {
        if outcome.success != outcome.error_code.is_none() {
            return Err("Python notebook host-call outcome is inconsistent".to_string());
        }
        if outcome.error_code.is_some_and(|code| {
            code.is_empty() || code.len() > 128 || code.chars().any(char::is_control)
        }) {
            return Err("Python notebook host-call error code is invalid".to_string());
        }
        self.mutate_and_persist(|notebook| {
            let cell = find_cell_mut(notebook, tool_call_id).ok_or_else(|| {
                format!("Python notebook has no attempt for tool-call ID '{tool_call_id}'")
            })?;
            if current_status(cell)? != NotebookAttemptStatus::Running.as_str() {
                return Err(
                    "Python notebook host-call outcomes require a running attempt".to_string(),
                );
            }
            let host_calls = lethetic_metadata_mut(cell)?
                .get_mut("host_calls")
                .and_then(Value::as_array_mut)
                .ok_or_else(|| "Python notebook host-call metadata is invalid".to_string())?;
            let host_call = host_calls
                .iter_mut()
                .find(|entry| entry["sub_id"].as_u64() == Some(sub_id))
                .and_then(Value::as_object_mut)
                .ok_or_else(|| format!("Python notebook has no host-call audit entry {sub_id}"))?;
            if host_call.get("operation").and_then(Value::as_str) != Some(operation)
                || host_call.get("status").and_then(Value::as_str) != Some("running")
            {
                return Err("Python notebook host-call audit entry is not finishable".to_string());
            }
            host_call.insert(
                "status".to_string(),
                Value::String(
                    if outcome.success {
                        "succeeded"
                    } else {
                        "errored"
                    }
                    .to_string(),
                ),
            );
            host_call.insert(
                "updated_at".to_string(),
                Value::String(Utc::now().to_rfc3339()),
            );
            if let Some(code) = outcome.error_code {
                host_call.insert("error_code".to_string(), Value::String(code.to_string()));
            }
            if let Some(revision) = outcome.revision {
                host_call.insert("revision".to_string(), Value::from(revision));
            }
            if let Some(todo_count) = outcome.todo_count {
                host_call.insert("todo_count".to_string(), Value::from(todo_count));
            }
            Ok(())
        })
    }

    pub fn record_result(
        &self,
        tool_call_id: &str,
        result: &PythonCellResult,
        backend: &str,
        policy_fingerprint: &str,
    ) -> Result<(), String> {
        self.mutate_and_persist(|notebook| {
            let cell = find_cell_mut(notebook, tool_call_id).ok_or_else(|| {
                format!("Python notebook has no attempt for tool-call ID '{tool_call_id}'")
            })?;
            let execution_count = cell.execution_count.unwrap_or(0);
            let mut outputs = Vec::new();
            if !result.stdout.is_empty() {
                outputs.push(json!({
                    "output_type": "stream",
                    "name": "stdout",
                    "text": result.stdout
                }));
            }
            if !result.stderr.is_empty() {
                outputs.push(json!({
                    "output_type": "stream",
                    "name": "stderr",
                    "text": result.stderr
                }));
            }
            if !result.value_repr.is_empty() {
                outputs.push(json!({
                    "output_type": "execute_result",
                    "execution_count": execution_count,
                    "data": { "text/plain": result.value_repr },
                    "metadata": {}
                }));
            }
            if result.is_error {
                let (ename, evalue) = traceback_name_value(&result.traceback);
                outputs.push(json!({
                    "output_type": "error",
                    "ename": ename,
                    "evalue": evalue,
                    "traceback": result.traceback.lines().map(str::to_string).collect::<Vec<_>>()
                }));
            }
            cell.outputs = outputs;
            set_status(
                cell,
                if result.is_error {
                    NotebookAttemptStatus::Error.as_str()
                } else {
                    NotebookAttemptStatus::Success.as_str()
                },
            )?;
            let metadata = lethetic_metadata_mut(cell)?;
            metadata.insert("backend".to_string(), Value::String(backend.to_string()));
            metadata.insert(
                "execution_policy_fingerprint".to_string(),
                Value::String(policy_fingerprint.to_string()),
            );
            metadata.insert("cwd_after".to_string(), Value::String(result.cwd.clone()));
            metadata.insert(
                "output_was_truncated".to_string(),
                Value::Bool(result.output_was_truncated),
            );
            metadata.insert(
                "output_metadata".to_string(),
                serde_json::to_value(&result.output_metadata).map_err(|error| {
                    format!("Could not encode Python output provenance: {error}")
                })?,
            );
            if let Some(notice) = &result.runtime_notice {
                metadata.insert(
                    "container_id".to_string(),
                    Value::String(notice.container_id.clone()),
                );
            }
            Ok(())
        })
    }

    fn mutate_and_persist(
        &self,
        mutation: impl FnOnce(&mut Notebook) -> Result<(), String>,
    ) -> Result<(), String> {
        self.location.identity.verify()?;
        let mut notebook = self
            .notebook
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = notebook.clone();
        mutation(&mut notebook)?;
        if let Err(error) = self.persist_locked(&notebook) {
            *notebook = previous;
            return Err(error);
        }
        Ok(())
    }

    fn persist_locked(&self, notebook: &Notebook) -> Result<(), String> {
        self.location.identity.verify()?;
        validate_notebook(notebook)?;
        let bytes = serde_json::to_vec_pretty(notebook)
            .map_err(|error| format!("Could not encode Python notebook audit: {error}"))?;
        if bytes.len() > MAX_NOTEBOOK_BYTES {
            return Err(format!(
                "Python notebook audit exceeds {MAX_NOTEBOOK_BYTES} bytes"
            ));
        }
        let directory_refs = self
            .location
            .directories
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let path = crate::platform::atomic_write_nofollow(
            &self.location.root,
            &directory_refs,
            NOTEBOOK_FILE_NAME,
            &bytes,
            0o600,
        )
        .map_err(|error| format!("Could not durably write Python notebook audit: {error}"))?;
        if path != self.location.path {
            return Err("Python notebook writer returned an unexpected path".to_string());
        }
        Ok(())
    }
}

fn validate_component(value: &str, label: &str) -> Result<(), String> {
    let path = Path::new(value);
    let mut components = path.components();
    if value.is_empty()
        || !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(format!("{label} is not a safe path component"));
    }
    Ok(())
}

fn validate_attempt_start(start: &NotebookAttemptStart<'_>) -> Result<(), String> {
    if start.tool_call_id.is_empty() || start.tool_call_id.len() > MAX_TOOL_CALL_ID_BYTES {
        return Err("Python notebook tool-call ID is empty or oversized".to_string());
    }
    if start.source.len() > MAX_SOURCE_BYTES {
        return Err(format!(
            "Python source exceeds the notebook audit limit of {MAX_SOURCE_BYTES} bytes"
        ));
    }
    if start.description.len() > MAX_DESCRIPTION_BYTES {
        return Err("Python notebook description is oversized".to_string());
    }
    Ok(())
}

fn validate_notebook(notebook: &Notebook) -> Result<(), String> {
    if notebook.nbformat != 4 || notebook.nbformat_minor != 5 {
        return Err("Python notebook must use nbformat 4.5".to_string());
    }
    if notebook.metadata["lethetic"]["artifact"] != "python_tool_audit"
        || notebook.metadata["lethetic"]["schema"] != 1
    {
        return Err("Python notebook has invalid Lethetic artifact metadata".to_string());
    }
    let mut call_ids = std::collections::HashSet::new();
    let mut cell_ids = std::collections::HashSet::new();
    let mut execution_counts = std::collections::HashSet::new();
    for cell in &notebook.cells {
        if cell.cell_type != "code" || cell.id.is_empty() || cell.id.len() > 64 {
            return Err("Python notebook contains an invalid audit cell".to_string());
        }
        if cell.source.len() > MAX_SOURCE_BYTES {
            return Err("Python notebook contains oversized source".to_string());
        }
        if !cell_ids.insert(&cell.id) {
            return Err("Python notebook contains duplicate cell IDs".to_string());
        }
        let execution_count = cell
            .execution_count
            .filter(|count| *count > 0)
            .ok_or_else(|| "Python notebook audit cell has no execution count".to_string())?;
        if !execution_counts.insert(execution_count) {
            return Err("Python notebook contains duplicate execution counts".to_string());
        }
        let metadata = lethetic_metadata(cell)?;
        let call_id = metadata
            .get("tool_call_id")
            .and_then(Value::as_str)
            .ok_or_else(|| "Python notebook cell has no tool-call ID".to_string())?;
        if call_id.is_empty() || call_id.len() > MAX_TOOL_CALL_ID_BYTES {
            return Err("Python notebook cell has an invalid tool-call ID".to_string());
        }
        if !call_ids.insert(call_id) {
            return Err("Python notebook contains duplicate tool-call IDs".to_string());
        }
        if cell.id != cell_id(call_id, execution_count) {
            return Err("Python notebook cell ID does not match its audit identity".to_string());
        }
        let status = metadata
            .get("status")
            .and_then(Value::as_str)
            .ok_or_else(|| "Python notebook cell has no attempt status".to_string())?;
        if !is_known_status(status) {
            return Err("Python notebook cell has an invalid attempt status".to_string());
        }
        if metadata
            .get("description")
            .and_then(Value::as_str)
            .is_none_or(|description| description.len() > MAX_DESCRIPTION_BYTES)
            || metadata.get("created_at").and_then(Value::as_str).is_none()
            || metadata.get("updated_at").and_then(Value::as_str).is_none()
            || metadata.get("cwd").and_then(Value::as_str).is_none()
            || metadata
                .get("policy_fingerprint")
                .and_then(Value::as_str)
                .is_none()
        {
            return Err("Python notebook cell has invalid audit metadata".to_string());
        }
        let host_calls = metadata
            .get("host_calls")
            .and_then(Value::as_array)
            .ok_or_else(|| "Python notebook host-call metadata is invalid".to_string())?;
        validate_host_calls(host_calls, status)?;
        for output in &cell.outputs {
            validate_output(output, execution_count)?;
        }
    }
    Ok(())
}

fn validate_host_calls(host_calls: &[Value], cell_status: &str) -> Result<(), String> {
    if host_calls.len() > MAX_HOST_CALLS_PER_ATTEMPT {
        return Err("Python notebook contains too many host-call entries".to_string());
    }
    for (index, host_call) in host_calls.iter().enumerate() {
        let host_call = host_call
            .as_object()
            .ok_or_else(|| "Python notebook host-call entry is not an object".to_string())?;
        let expected_sub_id = u64::try_from(index)
            .ok()
            .and_then(|index| index.checked_add(1))
            .ok_or_else(|| "Python notebook host-call counter overflowed".to_string())?;
        if host_call.get("sub_id").and_then(Value::as_u64) != Some(expected_sub_id)
            || !matches!(
                host_call.get("operation").and_then(Value::as_str),
                Some("todo.get" | "todo.set")
            )
            || host_call
                .get("started_at")
                .and_then(Value::as_str)
                .is_none()
            || host_call
                .get("updated_at")
                .and_then(Value::as_str)
                .is_none()
        {
            return Err("Python notebook contains invalid host-call identity metadata".to_string());
        }
        let status = host_call
            .get("status")
            .and_then(Value::as_str)
            .ok_or_else(|| "Python notebook host call has no status".to_string())?;
        match status {
            "running" if cell_status == "running" => {}
            "succeeded" => {
                if host_call.contains_key("error_code") {
                    return Err(
                        "Successful Python notebook host call has an error code".to_string()
                    );
                }
            }
            "errored" | "interrupted" => {
                if host_call
                    .get("error_code")
                    .and_then(Value::as_str)
                    .is_none_or(|code| code.is_empty() || code.len() > 128)
                {
                    return Err(
                        "Failed Python notebook host call has no valid error code".to_string()
                    );
                }
            }
            _ => return Err("Python notebook host call has an invalid status".to_string()),
        }
        if host_call
            .get("revision")
            .is_some_and(|value| value.as_u64().is_none())
            || host_call
                .get("todo_count")
                .is_some_and(|value| value.as_u64().is_none())
        {
            return Err("Python notebook host call has invalid result metadata".to_string());
        }
    }
    Ok(())
}

fn is_known_status(status: &str) -> bool {
    matches!(
        status,
        "pending"
            | "approved"
            | "running"
            | "denied"
            | "cancelled"
            | "interrupted"
            | "launch_failed"
            | "errored"
            | "succeeded"
    )
}

fn validate_output(output: &Value, execution_count: u64) -> Result<(), String> {
    let output = output
        .as_object()
        .ok_or_else(|| "Python notebook output is not an object".to_string())?;
    match output.get("output_type").and_then(Value::as_str) {
        Some("stream") => {
            if !matches!(
                output.get("name").and_then(Value::as_str),
                Some("stdout" | "stderr")
            ) || output.get("text").and_then(Value::as_str).is_none()
            {
                return Err("Python notebook contains an invalid stream output".to_string());
            }
        }
        Some("execute_result") => {
            if output.get("execution_count").and_then(Value::as_u64) != Some(execution_count)
                || !output.get("data").is_some_and(Value::is_object)
                || !output.get("metadata").is_some_and(Value::is_object)
            {
                return Err("Python notebook contains an invalid execute_result output".to_string());
            }
        }
        Some("error") => {
            let valid_traceback = output
                .get("traceback")
                .and_then(Value::as_array)
                .is_some_and(|lines| lines.iter().all(Value::is_string));
            if output.get("ename").and_then(Value::as_str).is_none()
                || output.get("evalue").and_then(Value::as_str).is_none()
                || !valid_traceback
            {
                return Err("Python notebook contains an invalid error output".to_string());
            }
        }
        _ => return Err("Python notebook contains an unsupported output type".to_string()),
    }
    Ok(())
}

fn repair_interrupted_cells(notebook: &mut Notebook) -> usize {
    let mut repaired = 0;
    for cell in &mut notebook.cells {
        let status = lethetic_metadata(cell)
            .ok()
            .and_then(|metadata| metadata.get("status"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if matches!(status, "pending" | "approved" | "running") {
            let now = Utc::now().to_rfc3339();
            if let Ok(metadata) = lethetic_metadata_mut(cell)
                && let Some(host_calls) =
                    metadata.get_mut("host_calls").and_then(Value::as_array_mut)
            {
                for host_call in host_calls {
                    if let Some(host_call) = host_call.as_object_mut()
                        && host_call.get("status").and_then(Value::as_str) == Some("running")
                    {
                        host_call.insert(
                            "status".to_string(),
                            Value::String("interrupted".to_string()),
                        );
                        host_call.insert(
                            "error_code".to_string(),
                            Value::String("interrupted".to_string()),
                        );
                        host_call.insert("updated_at".to_string(), Value::String(now.clone()));
                    }
                }
            }
            if set_status(cell, NotebookAttemptStatus::Interrupted.as_str()).is_ok() {
                cell.outputs = vec![error_output(
                    "LetheticInterrupted",
                    "Python attempt was interrupted before a durable terminal result was recorded. The operation may have partially completed.",
                )];
                repaired += 1;
            }
        }
    }
    repaired
}

fn find_cell_mut<'a>(
    notebook: &'a mut Notebook,
    tool_call_id: &str,
) -> Option<&'a mut NotebookCell> {
    notebook.cells.iter_mut().find(|cell| {
        lethetic_metadata(cell)
            .ok()
            .and_then(|metadata| metadata.get("tool_call_id"))
            .and_then(Value::as_str)
            == Some(tool_call_id)
    })
}

fn lethetic_metadata(cell: &NotebookCell) -> Result<&Map<String, Value>, String> {
    cell.metadata
        .get("lethetic")
        .and_then(Value::as_object)
        .ok_or_else(|| "Python notebook cell has invalid Lethetic metadata".to_string())
}

fn lethetic_metadata_mut(cell: &mut NotebookCell) -> Result<&mut Map<String, Value>, String> {
    cell.metadata
        .get_mut("lethetic")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| "Python notebook cell has invalid Lethetic metadata".to_string())
}

fn current_status(cell: &NotebookCell) -> Result<&str, String> {
    lethetic_metadata(cell)?
        .get("status")
        .and_then(Value::as_str)
        .ok_or_else(|| "Python notebook cell has no attempt status".to_string())
}

fn set_status(cell: &mut NotebookCell, status: &str) -> Result<(), String> {
    let metadata = lethetic_metadata_mut(cell)?;
    let current = metadata
        .get("status")
        .and_then(Value::as_str)
        .ok_or_else(|| "Python notebook cell has no attempt status".to_string())?;
    if !valid_status_transition(current, status) {
        return Err(format!(
            "Python notebook attempt cannot transition from '{current}' to '{status}'"
        ));
    }
    metadata.insert("status".to_string(), Value::String(status.to_string()));
    metadata.insert(
        "updated_at".to_string(),
        Value::String(Utc::now().to_rfc3339()),
    );
    Ok(())
}

fn valid_status_transition(current: &str, next: &str) -> bool {
    if current == next {
        return true;
    }
    match current {
        "pending" => matches!(next, "approved" | "denied" | "cancelled" | "interrupted"),
        "approved" => matches!(
            next,
            "running" | "cancelled" | "interrupted" | "launch_failed"
        ),
        "running" => matches!(
            next,
            "cancelled" | "interrupted" | "launch_failed" | "errored" | "succeeded"
        ),
        "denied" | "cancelled" | "interrupted" | "launch_failed" | "errored" | "succeeded" => false,
        _ => false,
    }
}

fn error_output(name: &str, message: &str) -> Value {
    json!({
        "output_type": "error",
        "ename": name,
        "evalue": message,
        "traceback": [message]
    })
}

fn status_error_name(status: NotebookAttemptStatus) -> &'static str {
    match status {
        NotebookAttemptStatus::Denied => "LetheticDenied",
        NotebookAttemptStatus::Cancelled => "LetheticCancelled",
        NotebookAttemptStatus::Interrupted => "LetheticInterrupted",
        NotebookAttemptStatus::LaunchFailed => "LetheticLaunchFailed",
        NotebookAttemptStatus::Error => "LetheticError",
        NotebookAttemptStatus::Approved
        | NotebookAttemptStatus::Running
        | NotebookAttemptStatus::Success => "LetheticStatus",
    }
}

fn traceback_name_value(traceback: &str) -> (String, String) {
    let last = traceback.lines().rev().find(|line| !line.trim().is_empty());
    match last.and_then(|line| line.split_once(':')) {
        Some((name, value)) => (name.trim().to_string(), value.trim().to_string()),
        None => (
            "PythonError".to_string(),
            last.unwrap_or_default().trim().to_string(),
        ),
    }
}

fn cell_id(tool_call_id: &str, execution_count: u64) -> String {
    let mut hash = Sha256::new();
    hash.update(tool_call_id.as_bytes());
    hash.update([0]);
    hash.update(execution_count.to_le_bytes());
    let digest = format!("{:x}", hash.finalize());
    format!("lethetic-{}", &digest[..24])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start<'a>(id: &'a str, source: &'a str) -> NotebookAttemptStart<'a> {
        NotebookAttemptStart {
            tool_call_id: id,
            source,
            description: "run",
            cwd: "/workspace",
            policy_fingerprint: "policy",
        }
    }

    fn output_metadata(cell: u64) -> crate::python::PythonOutputMetadata {
        crate::python::PythonOutputMetadata {
            cell,
            artifact_id: "00000000-0000-4000-8000-000000000001".to_string(),
            retained: true,
            sections: [
                crate::python::PythonOutputSection::Stdout,
                crate::python::PythonOutputSection::Stderr,
                crate::python::PythonOutputSection::Repr,
                crate::python::PythonOutputSection::Traceback,
            ]
            .into_iter()
            .map(|section| crate::python::PythonOutputSectionMetadata {
                section,
                captured_bytes: 0,
                original_bytes: 0,
                excerpt_bytes: 0,
                truncated: false,
            })
            .collect(),
        }
    }

    #[test]
    fn writes_nbformat_45_and_preserves_exact_source_and_outputs() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let recorder = PythonNotebook::non_durable(&root, "run-1").unwrap();
        let source = "x=1\nprint( x )\nx\n";
        recorder.begin_attempt(start("toolu-1", source)).unwrap();
        recorder
            .mark_status("toolu-1", NotebookAttemptStatus::Approved, None)
            .unwrap();
        recorder
            .mark_running("toolu-1", "Podman", "policy")
            .unwrap();
        recorder
            .record_result(
                "toolu-1",
                &PythonCellResult {
                    cell: 1,
                    stdout: "1\n".to_string(),
                    stderr: String::new(),
                    value_repr: "1".to_string(),
                    traceback: String::new(),
                    cwd: "/workspace".to_string(),
                    is_error: false,
                    output_was_truncated: false,
                    output_metadata: output_metadata(1),
                    runtime_notice: None,
                },
                "Podman",
                "policy",
            )
            .unwrap();
        let value: Value =
            serde_json::from_slice(&std::fs::read(recorder.path()).unwrap()).unwrap();
        assert_eq!(value["nbformat"], 4);
        assert_eq!(value["nbformat_minor"], 5);
        assert_eq!(value["cells"][0]["source"], source);
        assert_eq!(value["cells"][0]["outputs"][0]["output_type"], "stream");
        assert_eq!(
            value["cells"][0]["outputs"][1]["output_type"],
            "execute_result"
        );
        assert_eq!(
            value["cells"][0]["metadata"]["lethetic"]["status"],
            "succeeded"
        );
        assert_eq!(
            value["cells"][0]["metadata"]["lethetic"]["output_metadata"]["cell"],
            1
        );
        assert_eq!(
            value["cells"][0]["metadata"]["lethetic"]["output_metadata"]["sections"]
                .as_array()
                .unwrap()
                .len(),
            4
        );
    }

    #[test]
    fn duplicate_call_id_is_idempotent_but_source_reuse_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let recorder = PythonNotebook::non_durable(&root, "run-2").unwrap();
        recorder.begin_attempt(start("same", "one()")).unwrap();
        recorder.begin_attempt(start("same", "one()")).unwrap();
        assert!(recorder.begin_attempt(start("same", "two()")).is_err());
        let value: Value =
            serde_json::from_slice(&std::fs::read(recorder.path()).unwrap()).unwrap();
        assert_eq!(value["cells"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn opening_repairs_pending_attempt_without_replaying_it() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let path = {
            let recorder = PythonNotebook::non_durable(&root, "run-3").unwrap();
            recorder
                .begin_attempt(start("pending", "side_effect()"))
                .unwrap();
            recorder.path().to_path_buf()
        };
        let _reopened = PythonNotebook::non_durable(&root, "run-3").unwrap();
        let value: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(
            value["cells"][0]["metadata"]["lethetic"]["status"],
            "interrupted"
        );
        assert_eq!(value["cells"][0]["source"], "side_effect()");
        assert_eq!(value["cells"][0]["outputs"][0]["output_type"], "error");
    }

    #[test]
    fn records_denied_cancelled_launch_failed_and_errored_attempts() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let recorder = PythonNotebook::non_durable(&root, "run-statuses").unwrap();

        recorder.begin_attempt(start("denied", "deny()")).unwrap();
        recorder
            .mark_status(
                "denied",
                NotebookAttemptStatus::Denied,
                Some("approval denied"),
            )
            .unwrap();
        recorder
            .begin_attempt(start("cancelled", "cancel()"))
            .unwrap();
        recorder
            .mark_status(
                "cancelled",
                NotebookAttemptStatus::Cancelled,
                Some("cancelled before dispatch"),
            )
            .unwrap();
        recorder.begin_attempt(start("launch", "launch()")).unwrap();
        recorder
            .mark_status("launch", NotebookAttemptStatus::Approved, None)
            .unwrap();
        recorder
            .mark_status(
                "launch",
                NotebookAttemptStatus::LaunchFailed,
                Some("worker did not start"),
            )
            .unwrap();
        recorder.begin_attempt(start("error", "1 / 0")).unwrap();
        recorder
            .mark_status("error", NotebookAttemptStatus::Approved, None)
            .unwrap();
        recorder.mark_running("error", "Host", "policy").unwrap();
        recorder
            .record_result(
                "error",
                &PythonCellResult {
                    cell: 1,
                    stdout: String::new(),
                    stderr: "warning\n".to_string(),
                    value_repr: String::new(),
                    traceback:
                        "Traceback (most recent call last):\nZeroDivisionError: division by zero"
                            .to_string(),
                    cwd: "/workspace".to_string(),
                    is_error: true,
                    output_was_truncated: true,
                    output_metadata: output_metadata(1),
                    runtime_notice: None,
                },
                "Host",
                "policy",
            )
            .unwrap();

        let value: Value =
            serde_json::from_slice(&std::fs::read(recorder.path()).unwrap()).unwrap();
        let cells = value["cells"].as_array().unwrap();
        assert_eq!(cells.len(), 4);
        assert_eq!(cells[0]["metadata"]["lethetic"]["status"], "denied");
        assert_eq!(cells[0]["outputs"][0]["ename"], "LetheticDenied");
        assert_eq!(cells[1]["metadata"]["lethetic"]["status"], "cancelled");
        assert_eq!(cells[2]["metadata"]["lethetic"]["status"], "launch_failed");
        assert_eq!(cells[3]["metadata"]["lethetic"]["status"], "errored");
        assert_eq!(cells[3]["outputs"][0]["name"], "stderr");
        assert_eq!(cells[3]["outputs"][1]["output_type"], "error");
        assert_eq!(cells[3]["outputs"][1]["ename"], "ZeroDivisionError");
        assert_eq!(cells[3]["outputs"][1]["evalue"], "division by zero");
        assert_eq!(
            cells[3]["metadata"]["lethetic"]["output_was_truncated"],
            true
        );
        assert!(
            recorder
                .mark_status("denied", NotebookAttemptStatus::Approved, None)
                .is_err(),
            "terminal attempts must never become executable again"
        );
    }

    #[test]
    fn host_call_audit_is_sanitized_and_repairs_interrupted_outcomes() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let path = {
            let recorder = PythonNotebook::non_durable(&root, "run-host-calls").unwrap();
            recorder
                .begin_attempt(start(
                    "host-call",
                    "import lethetic_todo\nlethetic_todo.set(secret_todos, expected_revision=0)",
                ))
                .unwrap();
            recorder
                .mark_status("host-call", NotebookAttemptStatus::Approved, None)
                .unwrap();
            recorder
                .mark_running("host-call", "Host", "policy")
                .unwrap();
            recorder
                .begin_host_call("host-call", 1, "todo.set")
                .unwrap();
            recorder
                .finish_host_call(
                    "host-call",
                    1,
                    "todo.set",
                    NotebookHostCallOutcome {
                        success: true,
                        error_code: None,
                        revision: Some(1),
                        todo_count: Some(2),
                    },
                )
                .unwrap();
            recorder
                .begin_host_call("host-call", 2, "todo.get")
                .unwrap();
            recorder.path().to_path_buf()
        };

        let _reopened = PythonNotebook::non_durable(&root, "run-host-calls").unwrap();
        let value: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let host_calls = value["cells"][0]["metadata"]["lethetic"]["host_calls"]
            .as_array()
            .unwrap();
        assert_eq!(host_calls.len(), 2);
        assert_eq!(host_calls[0]["status"], "succeeded");
        assert_eq!(host_calls[0]["revision"], 1);
        assert_eq!(host_calls[0]["todo_count"], 2);
        assert!(host_calls[0].get("todos").is_none());
        assert_eq!(host_calls[1]["status"], "interrupted");
        assert_eq!(host_calls[1]["error_code"], "interrupted");
        assert_eq!(
            value["cells"][0]["metadata"]["lethetic"]["status"],
            "interrupted"
        );
    }

    #[test]
    fn execution_counts_continue_across_reopen() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        {
            let recorder = PythonNotebook::non_durable(&root, "run-counts").unwrap();
            recorder.begin_attempt(start("first", "first()")).unwrap();
            recorder
                .mark_status("first", NotebookAttemptStatus::Denied, Some("not approved"))
                .unwrap();
        }
        let recorder = PythonNotebook::non_durable(&root, "run-counts").unwrap();
        recorder.begin_attempt(start("second", "second()")).unwrap();
        let value: Value =
            serde_json::from_slice(&std::fs::read(recorder.path()).unwrap()).unwrap();
        assert_eq!(value["cells"][0]["execution_count"], 1);
        assert_eq!(value["cells"][1]["execution_count"], 2);
    }

    #[cfg(unix)]
    #[test]
    fn post_execution_symlink_swap_cannot_redirect_a_result_checkpoint() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let directory = root.join("session-result");
        std::fs::create_dir(&directory).unwrap();
        let recorder = PythonNotebook::in_existing_directory(&directory).unwrap();
        recorder
            .begin_attempt(start("result", "side_effect()"))
            .unwrap();
        recorder
            .mark_status("result", NotebookAttemptStatus::Approved, None)
            .unwrap();
        recorder.mark_running("result", "Host", "policy").unwrap();
        std::fs::remove_file(recorder.path()).unwrap();
        let outside = root.join("outside-result");
        std::fs::write(&outside, "outside-safe").unwrap();
        symlink(&outside, recorder.path()).unwrap();

        let error = recorder
            .record_result(
                "result",
                &PythonCellResult {
                    cell: 1,
                    stdout: "completed\n".to_string(),
                    stderr: String::new(),
                    value_repr: String::new(),
                    traceback: String::new(),
                    cwd: "/workspace".to_string(),
                    is_error: false,
                    output_was_truncated: false,
                    output_metadata: output_metadata(1),
                    runtime_notice: None,
                },
                "Host",
                "policy",
            )
            .unwrap_err();
        assert!(error.contains("durably write"), "{error}");
        assert_eq!(std::fs::read_to_string(outside).unwrap(), "outside-safe");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_notebook_destination() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let directory = root.join("session");
        std::fs::create_dir(&directory).unwrap();
        let outside = root.join("outside");
        std::fs::write(&outside, "unchanged").unwrap();
        symlink(&outside, directory.join(NOTEBOOK_FILE_NAME)).unwrap();
        assert!(PythonNotebook::in_existing_directory(&directory).is_err());
        assert_eq!(std::fs::read_to_string(outside).unwrap(), "unchanged");
    }
}
