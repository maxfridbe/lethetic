pub mod backend;
pub mod display;
#[cfg(target_os = "linux")]
pub mod egress_broker;
pub mod notebook;
mod process_tree;
#[cfg(target_os = "linux")]
pub mod retained_podman;
#[cfg(target_os = "linux")]
pub mod retained_runtime;
#[cfg(target_os = "linux")]
pub mod runtime_store;
#[cfg(target_os = "linux")]
pub mod selinux;
#[cfg(target_os = "linux")]
pub mod supervisor;

use self::process_tree::{
    DEFAULT_SETTLE_TIMEOUT, ProcessTree, force_kill_and_reap, force_kill_and_reap_blocking,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub(crate) const WORKER_SOURCE: &str = include_str!("worker.py");
const PROTOCOL_NAME: &str = "LETHETIC_PYTHON";
const PROTOCOL_VERSION: u32 = 3;
const WORKER_ABI: &str = "lethetic-python-worker-v4";
const OUTPUT_RECOVERY_CAPABILITY: &str = "lethetic-output-v2";
const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;
const MAX_FRAME_HEADER_BYTES: usize = 128;
pub const PYTHON_OUTPUT_READ_MAX_BYTES: u64 = 64 * 1024;
const MAX_OUTPUT_SECTION_EXCERPT_BYTES: u64 = 64 * 1024;
const MAX_STREAM_CAPTURE_BYTES: u64 = 1024 * 1024;
const MAX_REPR_CAPTURE_BYTES: u64 = 256 * 1024;
const MAX_TRACEBACK_CAPTURE_BYTES: u64 = 256 * 1024;
const MAX_HOST_CALLS_PER_CELL: u64 = 64;
const MAX_HOST_ERROR_BYTES: usize = 4096;
const MAX_DIAGNOSTIC_BYTES: usize = 64 * 1024;
const STARTUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const DROP_SETTLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CleanupCommand {
    pub program: OsString,
    pub args: Vec<OsString>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CleanupAction {
    Command(CleanupCommand),
    #[cfg(target_os = "linux")]
    TransientPodman {
        config: retained_podman::TransientPodmanConfig,
        container_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchKind {
    Direct,
    #[cfg(target_os = "linux")]
    TransientPodman(retained_podman::TransientPodmanConfig),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeLaunchAction {
    Created,
    Resumed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PythonRuntimeNotice {
    pub container_id: String,
    pub container_name: String,
    pub action: RuntimeLaunchAction,
    pub network: crate::config::NetworkAccess,
    pub mounted_cwd: PathBuf,
}

impl PythonRuntimeNotice {
    pub fn render(&self) -> String {
        let action = match self.action {
            RuntimeLaunchAction::Created => "created",
            RuntimeLaunchAction::Resumed => "resumed",
        };
        let network = match self.network {
            crate::config::NetworkAccess::None => "network: none",
            crate::config::NetworkAccess::Nonlocal => {
                "direct network: disabled; constrained public HTTP(S) broker: available"
            }
            crate::config::NetworkAccess::Full => {
                "network: full (host/localhost/LAN/VPN/Internet reachable)"
            }
        };
        format!(
            "Podman container {} {action}; {network}; mounted R/W cwd: {}",
            self.container_name,
            self.mounted_cwd.display()
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PythonContainerKind {
    Retained,
    Transient,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PythonContainerIdentity {
    pub kind: PythonContainerKind,
    pub name: String,
    pub active: bool,
}

impl PythonContainerIdentity {
    pub fn retained(runtime_id: &str, active: bool) -> Option<Self> {
        let parsed = uuid::Uuid::parse_str(runtime_id).ok()?;
        (parsed.hyphenated().to_string() == runtime_id).then(|| Self {
            kind: PythonContainerKind::Retained,
            name: format!("lethetic-python-{runtime_id}"),
            active,
        })
    }

    pub fn transient(name: &str) -> Option<Self> {
        let suffix = name.strip_prefix("lethetic-python-transient-")?;
        let (pid, counter) = suffix.split_once('-')?;
        let pid = pid.parse::<u32>().ok().filter(|pid| *pid > 0)?;
        let counter = counter.parse::<u64>().ok()?;
        (suffix == format!("{pid}-{counter}")).then(|| Self {
            kind: PythonContainerKind::Transient,
            name: name.to_string(),
            active: true,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchSpec {
    pub kind: LaunchKind,
    pub container_identity: Option<PythonContainerIdentity>,
    pub program: OsString,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
    pub clear_env: bool,
    pub env: Vec<(OsString, OsString)>,
    pub cleanup: Option<CleanupAction>,
    pub startup_notice: Option<PythonRuntimeNotice>,
    pub startup_timeout: std::time::Duration,
    pub graceful_shutdown: Option<std::time::Duration>,
}

impl LaunchSpec {
    pub fn host(program: impl Into<OsString>, cwd: PathBuf) -> Self {
        Self {
            kind: LaunchKind::Direct,
            container_identity: None,
            program: program.into(),
            args: vec![
                OsString::from("-u"),
                OsString::from("-B"),
                OsString::from("-c"),
                OsString::from(WORKER_SOURCE),
            ],
            cwd: Some(cwd),
            clear_env: false,
            env: Vec::new(),
            cleanup: None,
            startup_notice: None,
            startup_timeout: STARTUP_TIMEOUT,
            graceful_shutdown: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PythonOutputSection {
    Stdout,
    Stderr,
    Repr,
    Traceback,
}

impl PythonOutputSection {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
            Self::Repr => "repr",
            Self::Traceback => "traceback",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PythonOutputSectionMetadata {
    pub section: PythonOutputSection,
    pub captured_bytes: u64,
    pub original_bytes: u64,
    pub excerpt_bytes: u64,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PythonOutputMetadata {
    /// Notebook-style execution count shown for the worker result.
    pub cell: u64,
    /// Opaque UUID identifying this artifact across worker epochs.
    pub artifact_id: String,
    /// Whether the artifact was retained when this result frame was emitted.
    pub retained: bool,
    pub sections: Vec<PythonOutputSectionMetadata>,
}

impl PythonOutputMetadata {
    pub fn has_recoverable_output(&self) -> bool {
        self.retained
            && self
                .sections
                .iter()
                .any(|section| section.captured_bytes > section.excerpt_bytes)
    }

    pub fn has_capture_loss(&self) -> bool {
        self.sections.iter().any(|section| section.truncated)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PythonCellResult {
    pub cell: u64,
    pub stdout: String,
    pub stderr: String,
    pub value_repr: String,
    pub traceback: String,
    pub cwd: String,
    pub is_error: bool,
    pub output_was_truncated: bool,
    pub output_metadata: PythonOutputMetadata,
    pub runtime_notice: Option<PythonRuntimeNotice>,
}

impl PythonCellResult {
    pub fn render(&self) -> String {
        let mut sections = Vec::new();
        if !self.stdout.is_empty() {
            sections.push(format!("stdout:\n{}", self.stdout.trim_end()));
        }
        if !self.stderr.is_empty() {
            sections.push(format!("stderr:\n{}", self.stderr.trim_end()));
        }
        if !self.value_repr.is_empty() {
            sections.push(format!("Out[{}]:\n{}", self.cell, self.value_repr));
        }
        if !self.traceback.is_empty() {
            sections.push(self.traceback.trim_end().to_string());
        }
        if sections.is_empty() {
            format!("In [{}] completed with no output.", self.cell)
        } else {
            sections.join("\n\n")
        }
    }
}

#[derive(Clone)]
pub(crate) struct PythonHostCallContext {
    todo_store: Arc<crate::todo_store::TodoStore>,
    notebook: Arc<notebook::PythonNotebook>,
    tool_call_id: Arc<str>,
    todo_update_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::client::StreamEvent>>,
}

#[derive(Debug)]
enum HostCallRequest {
    TodoGet {
        sub_id: u64,
    },
    TodoSet {
        sub_id: u64,
        todos: serde_json::Value,
        expected_revision: u64,
    },
}

impl HostCallRequest {
    fn sub_id(&self) -> u64 {
        match self {
            Self::TodoGet { sub_id } | Self::TodoSet { sub_id, .. } => *sub_id,
        }
    }

    fn operation(&self) -> &'static str {
        match self {
            Self::TodoGet { .. } => "todo.get",
            Self::TodoSet { .. } => "todo.set",
        }
    }
}

enum HostCallReply {
    Success(serde_json::Value),
    Error { code: &'static str, message: String },
}

impl PythonHostCallContext {
    pub(crate) fn new(
        todo_root: &std::path::Path,
        notebook: Arc<notebook::PythonNotebook>,
        tool_call_id: &str,
        todo_update_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::client::StreamEvent>>,
    ) -> Result<Self, String> {
        let todo_store = crate::todo_store::TodoStore::open(todo_root)
            .map_err(|error| format!("Could not initialize lethetic_todo: {error}"))?;
        Ok(Self {
            todo_store: Arc::new(todo_store),
            notebook,
            tool_call_id: Arc::from(tool_call_id),
            todo_update_tx,
        })
    }

    fn handle(&self, request: HostCallRequest) -> Result<HostCallReply, String> {
        let sub_id = request.sub_id();
        let operation = request.operation();
        self.notebook
            .begin_host_call(&self.tool_call_id, sub_id, operation)
            .map_err(|error| {
                format!(
                    "lethetic_todo host call was not executed because its audit checkpoint failed: {error}"
                )
            })?;

        let is_set = matches!(request, HostCallRequest::TodoSet { .. });
        let result = match request {
            HostCallRequest::TodoGet { .. } => self.todo_store.get(),
            HostCallRequest::TodoSet {
                todos,
                expected_revision,
                ..
            } => crate::todo_store::TodoStore::parse_todos(&todos)
                .and_then(|todos| self.todo_store.replace(todos, expected_revision)),
        };
        match result {
            Ok(snapshot) => {
                self.notebook
                    .finish_host_call(
                        &self.tool_call_id,
                        sub_id,
                        operation,
                        notebook::NotebookHostCallOutcome {
                            success: true,
                            error_code: None,
                            revision: Some(snapshot.revision),
                            todo_count: Some(snapshot.todos.len()),
                        },
                    )
                    .map_err(|error| {
                        let ambiguity = if is_set {
                            " The todo update may have completed."
                        } else {
                            ""
                        };
                        format!(
                            "lethetic_todo host-call outcome could not be audited: {error}.{ambiguity}"
                        )
                    })?;
                if is_set && let Some(tx) = &self.todo_update_tx {
                    let _ = tx.send(crate::client::StreamEvent::TodoUpdated(snapshot.clone()));
                }
                let value = serde_json::to_value(snapshot).map_err(|error| {
                    format!("Could not encode lethetic_todo host response: {error}")
                })?;
                Ok(HostCallReply::Success(value))
            }
            Err(error) => {
                let code = error.code.as_str();
                self.notebook
                    .finish_host_call(
                        &self.tool_call_id,
                        sub_id,
                        operation,
                        notebook::NotebookHostCallOutcome {
                            success: false,
                            error_code: Some(code),
                            revision: None,
                            todo_count: None,
                        },
                    )
                    .map_err(|audit_error| {
                        format!(
                            "lethetic_todo rejection could not be audited: {audit_error}; original error: {error}"
                        )
                    })?;
                Ok(HostCallReply::Error {
                    code,
                    message: bounded_host_error(&error.to_string()),
                })
            }
        }
    }
}

fn bounded_host_error(message: &str) -> String {
    if message.len() <= MAX_HOST_ERROR_BYTES {
        return message.to_string();
    }
    let mut end = MAX_HOST_ERROR_BYTES;
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &message[..end])
}

#[derive(Default)]
pub struct PythonRunspace {
    state: Mutex<RunspaceState>,
    operational_identity: StdMutex<Option<PythonContainerIdentity>>,
}

#[derive(Default)]
struct RunspaceState {
    worker: Option<Worker>,
    launch_fingerprint: String,
}

#[cfg(target_os = "linux")]
struct TransientCreationGuard {
    config: Option<retained_podman::TransientPodmanConfig>,
    container_id: Option<String>,
}

#[cfg(target_os = "linux")]
impl TransientCreationGuard {
    fn new(config: retained_podman::TransientPodmanConfig) -> Self {
        Self {
            config: Some(config),
            container_id: None,
        }
    }

    fn set_container_id(&mut self, container_id: String) {
        self.container_id = Some(container_id);
    }

    async fn cleanup_now(&mut self) -> Result<(), String> {
        let Some(config) = self.config.as_ref() else {
            return Ok(());
        };
        Box::pin(retained_podman::cleanup_interrupted_transient_create(
            config,
            self.container_id.as_deref(),
        ))
        .await?;
        self.config.take();
        self.container_id.take();
        Ok(())
    }

    fn disarm(&mut self) {
        self.config.take();
        self.container_id.take();
    }
}

#[cfg(target_os = "linux")]
impl Drop for TransientCreationGuard {
    fn drop(&mut self) {
        let Some(config) = self.config.take() else {
            return;
        };
        let container_id = self.container_id.take();
        run_transient_creation_drop_cleanup(config, container_id);
    }
}

struct CleanupOwner {
    action: Option<CleanupAction>,
}

impl CleanupOwner {
    fn new(action: Option<CleanupAction>) -> Self {
        Self { action }
    }

    async fn cleanup_now(&mut self) -> Result<(), String> {
        let Some(action) = self.action.clone() else {
            return Ok(());
        };
        Box::pin(run_cleanup_action(Some(action))).await?;
        self.action.take();
        Ok(())
    }

    fn transfer(&mut self) -> Option<CleanupAction> {
        self.action.take()
    }
}

impl Drop for CleanupOwner {
    fn drop(&mut self) {
        if let Some(action) = self.action.take() {
            run_drop_cleanup(action);
        }
    }
}

impl PythonRunspace {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn operational_identity(&self) -> Option<PythonContainerIdentity> {
        match self.operational_identity.lock() {
            Ok(identity) => identity.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    fn set_operational_identity(&self, identity: Option<PythonContainerIdentity>) {
        match self.operational_identity.lock() {
            Ok(mut current) => *current = identity,
            Err(poisoned) => *poisoned.into_inner() = identity,
        }
    }

    pub async fn execute(
        &self,
        launch: LaunchSpec,
        launch_fingerprint: String,
        code: &str,
        cancellation_token: CancellationToken,
    ) -> Result<PythonCellResult, String> {
        self.execute_with_host_context(launch, launch_fingerprint, code, cancellation_token, None)
            .await
    }

    pub(crate) async fn execute_with_host_context(
        &self,
        launch: LaunchSpec,
        launch_fingerprint: String,
        code: &str,
        cancellation_token: CancellationToken,
        host_context: Option<PythonHostCallContext>,
    ) -> Result<PythonCellResult, String> {
        let mut state = self.state.lock().await;
        Box::pin(self.ensure_worker_locked(
            &mut state,
            launch,
            launch_fingerprint,
            &cancellation_token,
        ))
        .await?;

        let result = {
            let worker = state.worker.as_mut().expect("worker initialized");
            tokio::select! {
                biased;
                _ = cancellation_token.cancelled() => {
                    Err("Python cell cancelled; runspace globals were reset.".to_string())
                }
                result = worker.execute(code, host_context.as_ref()) => result,
            }
        };

        match result {
            Ok(result) => Ok(result),
            Err(error) => {
                self.set_operational_identity(None);
                let cleanup = match state.worker.take() {
                    Some(worker) => worker.terminate().await,
                    None => Ok(()),
                };
                state.launch_fingerprint.clear();
                match cleanup {
                    Ok(()) => Err(error),
                    Err(cleanup) => Err(format!("{error} Runspace cleanup failed: {cleanup}")),
                }
            }
        }
    }

    pub async fn ensure_ready(
        &self,
        launch: LaunchSpec,
        launch_fingerprint: String,
        cancellation_token: CancellationToken,
    ) -> Result<Option<PythonRuntimeNotice>, String> {
        let mut state = self.state.lock().await;
        Box::pin(self.ensure_worker_locked(
            &mut state,
            launch,
            launch_fingerprint,
            &cancellation_token,
        ))
        .await?;
        Ok(state
            .worker
            .as_mut()
            .and_then(Worker::take_runtime_announcement))
    }

    async fn ensure_worker_locked(
        &self,
        state: &mut RunspaceState,
        launch: LaunchSpec,
        launch_fingerprint: String,
        cancellation_token: &CancellationToken,
    ) -> Result<(), String> {
        if cancellation_token.is_cancelled() {
            self.set_operational_identity(None);
            let cleanup = match state.worker.take() {
                Some(worker) => worker.terminate().await,
                None => Ok(()),
            };
            state.launch_fingerprint.clear();
            return match cleanup {
                Ok(()) => Err(
                    "Python cell cancelled before execution; runspace state was reset.".to_string(),
                ),
                Err(cleanup) => Err(format!(
                    "Python cell cancelled before execution; runspace cleanup failed: {cleanup}"
                )),
            };
        }
        if state.worker.is_some() && state.launch_fingerprint != launch_fingerprint {
            self.set_operational_identity(None);
            if let Some(worker) = state.worker.take() {
                worker.terminate().await?;
            }
            state.launch_fingerprint.clear();
        }
        if state.worker.is_none() {
            self.set_operational_identity(None);
            let worker = Worker::spawn(launch, cancellation_token.clone()).await?;
            let identity = worker.container_identity.clone();
            state.worker = Some(worker);
            state.launch_fingerprint = launch_fingerprint;
            self.set_operational_identity(identity);
        }
        Ok(())
    }

    pub async fn reset_checked(&self) -> Result<(), String> {
        let mut state = self.state.lock().await;
        self.set_operational_identity(None);
        let result = match state.worker.take() {
            Some(worker) => worker.terminate().await,
            None => Ok(()),
        };
        state.launch_fingerprint.clear();
        result
    }

    pub async fn reset(&self) {
        let _ = self.reset_checked().await;
    }

    pub async fn diagnostics_text(&self) -> String {
        let state = self.state.lock().await;
        state
            .worker
            .as_ref()
            .map(Worker::diagnostics_text)
            .unwrap_or_default()
    }

    pub async fn is_running(&self) -> bool {
        self.state.lock().await.worker.is_some()
    }
}

struct Worker {
    child: Option<Child>,
    process_tree: Option<ProcessTree>,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    diagnostics: Arc<StdMutex<VecDeque<u8>>>,
    diagnostics_task: tokio::task::JoinHandle<()>,
    next_request_id: u64,
    container_identity: Option<PythonContainerIdentity>,
    cleanup: Option<CleanupAction>,
    runtime_notice: Option<PythonRuntimeNotice>,
    runtime_notice_announced: bool,
    #[cfg(unix)]
    graceful_shutdown: Option<std::time::Duration>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HelloFrame {
    #[serde(rename = "type")]
    frame_type: String,
    protocol: u32,
    worker_abi: String,
    capabilities: Vec<String>,
    python: String,
    cwd: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultFrame {
    #[serde(rename = "type")]
    frame_type: String,
    id: u64,
    ok: bool,
    cell: u64,
    stdout: String,
    stderr: String,
    #[serde(rename = "repr")]
    value_repr: String,
    traceback: String,
    cwd: String,
    output_metadata: PythonOutputMetadata,
}

fn validate_result_output_metadata(
    response: &ResultFrame,
    expected_artifact_id: &str,
) -> Result<(), String> {
    let metadata = &response.output_metadata;
    if metadata.cell != response.cell {
        return Err(format!(
            "Python output artifact cell mismatch: result={}, artifact={}",
            response.cell, metadata.cell
        ));
    }
    if metadata.artifact_id != expected_artifact_id {
        return Err("Python output artifact ID did not match its execution request".to_string());
    }
    let artifact_id = uuid::Uuid::parse_str(&metadata.artifact_id)
        .map_err(|_| "Python output artifact ID is not a UUID".to_string())?;
    let artifact_bytes = artifact_id.as_bytes();
    if artifact_id.hyphenated().to_string() != metadata.artifact_id
        || artifact_bytes[6] >> 4 != 4
        || artifact_bytes[8] >> 6 != 2
    {
        return Err("Python output artifact ID is not a canonical UUIDv4".to_string());
    }
    if !metadata.retained {
        return Err(format!(
            "Python worker did not retain output artifact for cell {} despite advertising {OUTPUT_RECOVERY_CAPABILITY}",
            response.cell
        ));
    }
    if metadata.sections.len() != 4 {
        return Err("Python output artifact must describe exactly four sections".to_string());
    }
    for (expected, displayed, capture_limit) in [
        (
            PythonOutputSection::Stdout,
            response.stdout.as_str(),
            MAX_STREAM_CAPTURE_BYTES,
        ),
        (
            PythonOutputSection::Stderr,
            response.stderr.as_str(),
            MAX_STREAM_CAPTURE_BYTES,
        ),
        (
            PythonOutputSection::Repr,
            response.value_repr.as_str(),
            MAX_REPR_CAPTURE_BYTES,
        ),
        (
            PythonOutputSection::Traceback,
            response.traceback.as_str(),
            MAX_TRACEBACK_CAPTURE_BYTES,
        ),
    ] {
        let mut matches = metadata
            .sections
            .iter()
            .filter(|section| section.section == expected);
        let section = matches.next().ok_or_else(|| {
            format!(
                "Python output artifact omitted the {} section",
                expected.as_str()
            )
        })?;
        if matches.next().is_some() {
            return Err(format!(
                "Python output artifact duplicated the {} section",
                expected.as_str()
            ));
        }
        let displayed_bytes = u64::try_from(displayed.len())
            .map_err(|_| "Python output excerpt length exceeded u64".to_string())?;
        if section.excerpt_bytes != displayed_bytes {
            return Err(format!(
                "Python output {} excerpt byte count mismatch: reported {}, decoded {displayed_bytes}",
                expected.as_str(),
                section.excerpt_bytes
            ));
        }
        if section.excerpt_bytes > MAX_OUTPUT_SECTION_EXCERPT_BYTES {
            return Err(format!(
                "Python output {} excerpt exceeded {} bytes",
                expected.as_str(),
                MAX_OUTPUT_SECTION_EXCERPT_BYTES
            ));
        }
        if section.captured_bytes > capture_limit {
            return Err(format!(
                "Python output {} artifact exceeded its {capture_limit}-byte capture limit",
                expected.as_str()
            ));
        }
        if section.excerpt_bytes > section.captured_bytes {
            return Err(format!(
                "Python output {} excerpt was larger than its retained artifact",
                expected.as_str()
            ));
        }
    }
    Ok(())
}

fn parse_host_call_frame(
    frame: serde_json::Value,
    request_id: u64,
    expected_sub_id: u64,
) -> Result<HostCallRequest, String> {
    let object = frame
        .as_object()
        .ok_or_else(|| "Python host-call frame is not an object".to_string())?;
    let id = object
        .get("id")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "Python host-call frame has no valid execution ID".to_string())?;
    if id != request_id {
        return Err(format!(
            "Python host-call execution ID mismatch: expected {request_id}, got {id}"
        ));
    }
    let sub_id = object
        .get("sub_id")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "Python host-call frame has no valid sub-ID".to_string())?;
    if sub_id != expected_sub_id {
        return Err(format!(
            "Python host-call sub-ID mismatch: expected {expected_sub_id}, got {sub_id}"
        ));
    }
    let operation = object
        .get("operation")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "Python host-call frame has no operation".to_string())?;
    match operation {
        "todo.get" => {
            if object.len() != 4
                || !object
                    .keys()
                    .all(|key| matches!(key.as_str(), "type" | "id" | "sub_id" | "operation"))
            {
                return Err("Python todo.get host call contains unexpected fields".to_string());
            }
            Ok(HostCallRequest::TodoGet { sub_id })
        }
        "todo.set" => {
            if object.len() != 6
                || !object.keys().all(|key| {
                    matches!(
                        key.as_str(),
                        "type" | "id" | "sub_id" | "operation" | "todos" | "expected_revision"
                    )
                })
            {
                return Err("Python todo.set host call contains unexpected fields".to_string());
            }
            let todos = object
                .get("todos")
                .cloned()
                .ok_or_else(|| "Python todo.set host call has no todo list".to_string())?;
            let expected_revision = object
                .get("expected_revision")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| {
                    "Python todo.set host call has no valid expected revision".to_string()
                })?;
            Ok(HostCallRequest::TodoSet {
                sub_id,
                todos,
                expected_revision,
            })
        }
        _ => Err(format!(
            "Python worker requested unknown host operation {operation:?}"
        )),
    }
}

impl Worker {
    async fn materialize_launch(
        mut spec: LaunchSpec,
        cancellation: &CancellationToken,
    ) -> Result<LaunchSpec, String> {
        let kind = std::mem::replace(&mut spec.kind, LaunchKind::Direct);
        match kind {
            LaunchKind::Direct => {
                if cancellation.is_cancelled() {
                    Err(
                        "Python cell cancelled before worker launch; runspace state was reset."
                            .to_string(),
                    )
                } else {
                    Ok(spec)
                }
            }
            #[cfg(target_os = "linux")]
            LaunchKind::TransientPodman(config) => {
                let mut creation_guard = TransientCreationGuard::new(config.clone());
                let cleanup_error = |error: String, cleanup: Result<(), String>| match cleanup {
                    Ok(()) => error,
                    Err(cleanup) => format!("{error}; exact container cleanup failed: {cleanup}"),
                };
                let container_id =
                    match Box::pin(retained_podman::create_transient_container(&config)).await {
                        Ok(container_id) => {
                            creation_guard.set_container_id(container_id.clone());
                            container_id
                        }
                        Err(error) => {
                            let cleanup = Box::pin(creation_guard.cleanup_now()).await;
                            return Err(cleanup_error(error, cleanup));
                        }
                    };
                if let Err(error) = Box::pin(retained_podman::attest_new_transient_container(
                    &config,
                    &container_id,
                ))
                .await
                {
                    let cleanup = Box::pin(creation_guard.cleanup_now()).await;
                    return Err(cleanup_error(error, cleanup));
                }
                if cancellation.is_cancelled() {
                    let cleanup = Box::pin(creation_guard.cleanup_now()).await;
                    return Err(cleanup_error(
                        "Python cell cancelled before transient Podman start".to_string(),
                        cleanup,
                    ));
                }
                let start =
                    match retained_podman::build_start_attach_spec(&config.podman, &container_id) {
                        Ok(start) => start,
                        Err(error) => {
                            let cleanup = Box::pin(creation_guard.cleanup_now()).await;
                            return Err(cleanup_error(error, cleanup));
                        }
                    };
                spec.program = start.program.into_os_string();
                spec.args = start.args;
                spec.cwd = None;
                spec.clear_env = false;
                spec.env.clear();
                spec.cleanup = Some(CleanupAction::TransientPodman {
                    config: config.clone(),
                    container_id: container_id.clone(),
                });
                spec.startup_notice = config.mask_lethetic.then(|| PythonRuntimeNotice {
                    container_id,
                    container_name: config.container_name.clone(),
                    action: RuntimeLaunchAction::Created,
                    network: config.network,
                    mounted_cwd: config.launch_cwd,
                });
                creation_guard.disarm();
                Ok(spec)
            }
        }
    }

    async fn spawn(spec: LaunchSpec, cancellation: CancellationToken) -> Result<Self, String> {
        let mut spec = Box::pin(Self::materialize_launch(spec, &cancellation)).await?;
        let mut cleanup_owner = CleanupOwner::new(spec.cleanup.take());
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = &spec.cwd {
            command.current_dir(cwd);
        }
        if spec.clear_env {
            command.env_clear();
        }
        command.envs(spec.env.iter().cloned());

        let mut process_tree = match ProcessTree::prepare(&mut command, "Python worker") {
            Ok(process_tree) => process_tree,
            Err(error) => {
                return match Box::pin(cleanup_owner.cleanup_now()).await {
                    Ok(()) => Err(error),
                    Err(cleanup) => Err(format!("{error}; cleanup failed: {cleanup}")),
                };
            }
        };
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                let original = format!(
                    "Could not start Python runspace with {:?}: {error}",
                    spec.program
                );
                return match Box::pin(cleanup_owner.cleanup_now()).await {
                    Ok(()) => Err(original),
                    Err(cleanup) => Err(format!("{original}; cleanup failed: {cleanup}")),
                };
            }
        };
        if let Err(error) = process_tree.attach_and_resume(&child, "Python worker") {
            let containment =
                force_kill_and_reap(child, process_tree, DEFAULT_SETTLE_TIMEOUT, "Python worker")
                    .await;
            let cleanup = Box::pin(cleanup_owner.cleanup_now()).await;
            let mut errors = vec![error];
            if let Err(error) = containment {
                errors.push(format!("containment failed: {error}"));
            }
            if let Err(error) = cleanup {
                errors.push(format!("cleanup failed: {error}"));
            }
            return Err(errors.join("; "));
        }
        let streams = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let (Some(stdin), Some(stdout), Some(stderr)) = streams else {
            let containment =
                force_kill_and_reap(child, process_tree, DEFAULT_SETTLE_TIMEOUT, "Python worker")
                    .await;
            let cleanup = Box::pin(cleanup_owner.cleanup_now()).await;
            let mut errors = vec!["Python runspace did not expose all control streams".to_string()];
            if let Err(error) = containment {
                errors.push(format!("containment failed: {error}"));
            }
            if let Err(error) = cleanup {
                errors.push(format!("cleanup failed: {error}"));
            }
            return Err(errors.join("; "));
        };

        let diagnostics = Arc::new(StdMutex::new(VecDeque::new()));
        let diagnostics_task = spawn_diagnostic_reader(stderr, diagnostics.clone());
        let mut worker = Self {
            child: Some(child),
            process_tree: Some(process_tree),
            stdin: BufWriter::new(stdin),
            stdout: BufReader::new(stdout),
            diagnostics,
            diagnostics_task,
            next_request_id: 1,
            container_identity: spec.container_identity.take(),
            cleanup: cleanup_owner.transfer(),
            runtime_notice: spec.startup_notice,
            runtime_notice_announced: false,
            #[cfg(unix)]
            graceful_shutdown: spec.graceful_shutdown,
        };

        let hello_result = tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                Err("Python cell cancelled during runspace startup".to_string())
            }
            result = tokio::time::timeout(
                spec.startup_timeout,
                worker.read_json_frame::<HelloFrame>(),
            ) => result
                .map_err(|_| "Python runspace startup timed out".to_string())
                .and_then(|result| result),
        };
        let hello = match hello_result {
            Ok(hello) => hello,
            Err(error) => {
                let diagnostics = worker.diagnostics_text();
                let cleanup = worker.terminate().await.err();
                return Err(format!(
                    "{error}. {}{}",
                    diagnostics,
                    cleanup
                        .map(|cleanup| format!(" Cleanup failed: {cleanup}"))
                        .unwrap_or_default()
                ));
            }
        };
        if hello.frame_type != "hello" || hello.protocol != PROTOCOL_VERSION {
            let diagnostics = worker.diagnostics_text();
            let cleanup = worker.terminate().await.err();
            return Err(format!(
                "Python runspace protocol mismatch (type={}, version={}, python={}, cwd={}). {}{}",
                hello.frame_type,
                hello.protocol,
                hello.python,
                hello.cwd,
                diagnostics,
                cleanup
                    .map(|error| format!(" Cleanup failed: {error}"))
                    .unwrap_or_default()
            ));
        }
        let has_output_recovery = hello
            .capabilities
            .iter()
            .any(|capability| capability == OUTPUT_RECOVERY_CAPABILITY);
        if hello.worker_abi != WORKER_ABI || !has_output_recovery {
            let diagnostics = worker.diagnostics_text();
            let cleanup = worker.terminate().await.err();
            return Err(format!(
                "Python runspace worker ABI mismatch (reported {:?}, requires {:?} with capability {:?}). Incompatible retained runtimes must be explicitly deleted before creating a replacement. {}{}",
                hello.worker_abi,
                WORKER_ABI,
                OUTPUT_RECOVERY_CAPABILITY,
                diagnostics,
                cleanup
                    .map(|error| format!(" Cleanup failed: {error}"))
                    .unwrap_or_default()
            ));
        }

        Ok(worker)
    }

    fn take_runtime_announcement(&mut self) -> Option<PythonRuntimeNotice> {
        if self.runtime_notice_announced {
            return None;
        }
        let notice = self.runtime_notice.clone()?;
        self.runtime_notice_announced = true;
        Some(notice)
    }

    async fn execute(
        &mut self,
        code: &str,
        host_context: Option<&PythonHostCallContext>,
    ) -> Result<PythonCellResult, String> {
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        let artifact_id = uuid::Uuid::new_v4().hyphenated().to_string();
        let request = serde_json::to_vec(&json!({
            "id": request_id,
            "op": "execute",
            "artifact_id": artifact_id.clone(),
            "code": code,
        }))
        .map_err(|error| format!("Could not encode Python cell: {error}"))?;
        if request.len() > MAX_FRAME_BYTES {
            return Err(format!(
                "Encoded Python cell exceeded the {MAX_FRAME_BYTES}-byte worker frame limit"
            ));
        }
        self.stdin
            .write_all(&request)
            .await
            .map_err(|error| format!("Could not write Python cell: {error}"))?;
        self.stdin
            .write_all(b"\n")
            .await
            .map_err(|error| format!("Could not terminate Python request: {error}"))?;
        self.stdin
            .flush()
            .await
            .map_err(|error| format!("Could not flush Python cell: {error}"))?;

        let mut expected_sub_id = 1_u64;
        let response = loop {
            let frame = self
                .read_json_frame::<serde_json::Value>()
                .await
                .map_err(|error| {
                    let diagnostics = self.diagnostics_text();
                    if diagnostics.is_empty() {
                        error
                    } else {
                        format!("{error}. Worker diagnostics: {diagnostics}")
                    }
                })?;
            match frame.get("type").and_then(serde_json::Value::as_str) {
                Some("result") => {
                    let response = serde_json::from_value::<ResultFrame>(frame)
                        .map_err(|error| format!("Malformed Python result frame: {error}"))?;
                    break response;
                }
                Some("host_call") => {
                    let context = host_context.ok_or_else(|| {
                        "Python worker requested a host capability outside an audited model call"
                            .to_string()
                    })?;
                    if expected_sub_id > MAX_HOST_CALLS_PER_CELL {
                        return Err(format!(
                            "Python worker exceeded {MAX_HOST_CALLS_PER_CELL} host calls in one cell"
                        ));
                    }
                    let request = parse_host_call_frame(frame, request_id, expected_sub_id)?;
                    let sub_id = request.sub_id();
                    let reply = context.handle(request)?;
                    self.write_host_response(request_id, sub_id, reply).await?;
                    expected_sub_id = expected_sub_id
                        .checked_add(1)
                        .ok_or_else(|| "Python host-call counter overflowed".to_string())?;
                }
                Some(frame_type) => {
                    return Err(format!(
                        "Unexpected Python runspace frame type '{frame_type}'"
                    ));
                }
                None => return Err("Python runspace frame has no type".to_string()),
            }
        };
        if response.frame_type != "result" {
            return Err(format!(
                "Unexpected Python runspace frame type '{}'",
                response.frame_type
            ));
        }
        if response.id != request_id {
            return Err(format!(
                "Python runspace response id mismatch: expected {request_id}, got {}",
                response.id
            ));
        }

        validate_result_output_metadata(&response, &artifact_id)?;
        let output_was_truncated = response.output_metadata.has_capture_loss()
            || response.output_metadata.has_recoverable_output();

        Ok(PythonCellResult {
            cell: response.cell,
            stdout: response.stdout,
            stderr: response.stderr,
            value_repr: response.value_repr,
            traceback: response.traceback,
            cwd: response.cwd,
            is_error: !response.ok,
            output_was_truncated,
            output_metadata: response.output_metadata,
            runtime_notice: self.runtime_notice.take(),
        })
    }

    async fn write_host_response(
        &mut self,
        request_id: u64,
        sub_id: u64,
        reply: HostCallReply,
    ) -> Result<(), String> {
        let response = match reply {
            HostCallReply::Success(result) => json!({
                "id": request_id,
                "op": "host_response",
                "sub_id": sub_id,
                "ok": true,
                "result": result
            }),
            HostCallReply::Error { code, message } => json!({
                "id": request_id,
                "op": "host_response",
                "sub_id": sub_id,
                "ok": false,
                "error": {
                    "code": code,
                    "message": message
                }
            }),
        };
        let encoded = serde_json::to_vec(&response)
            .map_err(|error| format!("Could not encode Python host response: {error}"))?;
        if encoded.len() > MAX_FRAME_BYTES {
            return Err(format!(
                "Python host response exceeded {MAX_FRAME_BYTES} bytes"
            ));
        }
        self.stdin
            .write_all(&encoded)
            .await
            .map_err(|error| format!("Could not write Python host response: {error}"))?;
        self.stdin
            .write_all(b"\n")
            .await
            .map_err(|error| format!("Could not terminate Python host response: {error}"))?;
        self.stdin
            .flush()
            .await
            .map_err(|error| format!("Could not flush Python host response: {error}"))
    }

    async fn read_json_frame<T: for<'de> Deserialize<'de>>(&mut self) -> Result<T, String> {
        let mut header = Vec::with_capacity(MAX_FRAME_HEADER_BYTES);
        loop {
            if header.len() >= MAX_FRAME_HEADER_BYTES {
                return Err(format!(
                    "Python frame header exceeded {MAX_FRAME_HEADER_BYTES} bytes"
                ));
            }
            let byte = self
                .stdout
                .read_u8()
                .await
                .map_err(|error| format!("Could not read Python frame header: {error}"))?;
            header.push(byte);
            if byte == b'\n' {
                break;
            }
        }
        let header = std::str::from_utf8(&header)
            .map_err(|_| "Python frame header was not ASCII".to_string())?;
        if !header.is_ascii() {
            return Err("Python frame header was not ASCII".to_string());
        }
        let header_fields = header
            .strip_suffix('\n')
            .ok_or_else(|| "Python frame header had no terminator".to_string())?;
        let mut fields = header_fields.split(' ');
        let name = fields.next().unwrap_or_default();
        let version_text = fields.next().unwrap_or_default();
        let length_text = fields.next().unwrap_or_default();
        if fields.next().is_some()
            || name.is_empty()
            || version_text.is_empty()
            || length_text.is_empty()
        {
            return Err(format!("Malformed Python frame header: {header:?}"));
        }
        let version = version_text
            .parse::<u32>()
            .map_err(|_| format!("Malformed Python frame version: {header:?}"))?;
        let length = length_text
            .parse::<usize>()
            .map_err(|_| format!("Malformed Python frame length: {header:?}"))?;
        if version.to_string() != version_text || length.to_string() != length_text {
            return Err(format!("Non-canonical Python frame header: {header:?}"));
        }
        if name != PROTOCOL_NAME || version != PROTOCOL_VERSION {
            return Err(format!("Unexpected Python frame header: {header:?}"));
        }
        if length == 0 || length > MAX_FRAME_BYTES {
            return Err(format!(
                "Python frame exceeded {} bytes (reported {length})",
                MAX_FRAME_BYTES
            ));
        }
        let mut body = vec![0; length];
        self.stdout
            .read_exact(&mut body)
            .await
            .map_err(|error| format!("Could not read Python frame body: {error}"))?;
        serde_json::from_slice(&body)
            .map_err(|error| format!("Could not decode Python frame: {error}"))
    }

    fn diagnostics_text(&self) -> String {
        let mut diagnostics = self
            .diagnostics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        String::from_utf8_lossy(diagnostics.make_contiguous())
            .trim()
            .to_string()
    }

    async fn terminate(mut self) -> Result<(), String> {
        let mut errors = Vec::new();
        match (self.child.take(), self.process_tree.take()) {
            (Some(child), Some(process_tree)) => {
                #[cfg(unix)]
                let (mut child, mut process_tree) = (child, process_tree);
                #[cfg(not(unix))]
                let (child, process_tree) = (child, process_tree);
                #[cfg(unix)]
                let leader_exited = if let Some(grace) = self.graceful_shutdown {
                    match process_tree.signal_terminate("Python worker") {
                        Ok(()) => match tokio::time::timeout(grace, child.wait()).await {
                            Ok(Ok(_)) => {
                                if let Err(error) = process_tree
                                    .terminate_remaining(DEFAULT_SETTLE_TIMEOUT, "Python worker")
                                    .await
                                {
                                    errors.push(error);
                                }
                                true
                            }
                            Ok(Err(error)) => {
                                errors.push(format!(
                                    "Could not wait for graceful Python worker shutdown: {error}"
                                ));
                                false
                            }
                            Err(_) => false,
                        },
                        Err(error) => {
                            errors.push(error);
                            false
                        }
                    }
                } else {
                    false
                };
                #[cfg(not(unix))]
                let leader_exited = false;

                if !leader_exited
                    && let Err(error) = force_kill_and_reap(
                        child,
                        process_tree,
                        DEFAULT_SETTLE_TIMEOUT,
                        "Python worker",
                    )
                    .await
                {
                    errors.push(error);
                }
            }
            (child, process_tree) => {
                errors.push("Python worker containment state was incomplete".to_string());
                if let Some(mut process_tree) = process_tree
                    && let Err(error) = process_tree
                        .terminate_remaining(DEFAULT_SETTLE_TIMEOUT, "Python worker")
                        .await
                {
                    errors.push(error);
                }
                if let Some(mut child) = child {
                    let _ = child.start_kill();
                    match tokio::time::timeout(DEFAULT_SETTLE_TIMEOUT, child.wait()).await {
                        Ok(Ok(_)) => {}
                        Ok(Err(error)) => {
                            errors.push(format!("Could not reap Python worker: {error}"));
                        }
                        Err(_) => errors.push(
                            "Python worker did not reap within the containment deadline"
                                .to_string(),
                        ),
                    }
                }
            }
        }
        abort_and_join_diagnostic_reader(&mut self.diagnostics_task).await;
        if let Some(cleanup) = self.cleanup.clone() {
            match run_cleanup_action(Some(cleanup)).await {
                Ok(()) => {
                    self.cleanup.take();
                }
                Err(error) => errors.push(error),
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            let diagnostics = self.diagnostics_text();
            if !diagnostics.is_empty() {
                errors.push(format!("worker diagnostics: {diagnostics}"));
            }
            Err(errors.join("; "))
        }
    }
}

async fn run_cleanup_action(cleanup: Option<CleanupAction>) -> Result<(), String> {
    let Some(cleanup) = cleanup else {
        return Ok(());
    };
    match cleanup {
        CleanupAction::Command(cleanup) => {
            match Command::new(cleanup.program)
                .args(cleanup.args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .status()
                .await
            {
                Ok(status) if status.success() => Ok(()),
                Ok(status) => Err(format!(
                    "Python backend cleanup exited unsuccessfully: {status}"
                )),
                Err(error) => Err(format!("could not run Python backend cleanup: {error}")),
            }
        }
        #[cfg(target_os = "linux")]
        CleanupAction::TransientPodman {
            config,
            container_id,
        } => retained_podman::cleanup_transient_container(&config.podman, &container_id).await,
    }
}

#[cfg(target_os = "linux")]
fn run_transient_creation_drop_cleanup(
    config: retained_podman::TransientPodmanConfig,
    container_id: Option<String>,
) {
    let cleanup_record = match retained_podman::persist_transient_cleanup_record(
        &config,
        container_id.as_deref(),
    ) {
        Ok(file_name) => Some(file_name),
        Err(error) => {
            eprintln!(
                "WARNING: interrupted transient cleanup identity could not be persisted: {error}"
            );
            None
        }
    };
    let cleanup_thread = std::thread::Builder::new()
        .name("lethetic-python-create-cleanup".to_string())
        .spawn(move || -> Result<(), String> {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| format!("could not start creation-cleanup runtime: {error}"))?;
            runtime.block_on(async {
                let mut failures = Vec::new();
                for attempt in 1..=3 {
                    match retained_podman::cleanup_interrupted_transient_create(
                        &config,
                        container_id.as_deref(),
                    )
                    .await
                    {
                        Ok(()) => {
                            if let Some(file_name) = cleanup_record.as_deref() {
                                retained_podman::clear_transient_cleanup_record(file_name)?;
                            }
                            return Ok(());
                        }
                        Err(error) => failures.push(format!("attempt {attempt}: {error}")),
                    }
                    if attempt < 3 {
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                }
                Err(format!(
                    "interrupted transient creation cleanup failed after 3 attempts: {}",
                    failures.join("; ")
                ))
            })
        });
    match cleanup_thread {
        Ok(cleanup_thread) => match cleanup_thread.join() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => eprintln!(
                "WARNING: transient Python creation cleanup could not verify exact resource removal: {error}"
            ),
            Err(_) => eprintln!("WARNING: transient Python creation cleanup thread panicked"),
        },
        Err(error) => eprintln!(
            "WARNING: transient Python creation cleanup thread could not start; exact resource removal is unverified: {error}"
        ),
    }
}

fn run_drop_cleanup(cleanup: CleanupAction) {
    #[cfg(target_os = "linux")]
    let cleanup_record = match &cleanup {
        CleanupAction::TransientPodman {
            config,
            container_id,
        } => match retained_podman::persist_transient_cleanup_record(config, Some(container_id)) {
            Ok(file_name) => Some(file_name),
            Err(error) => {
                eprintln!(
                    "WARNING: transient worker cleanup identity could not be persisted: {error}"
                );
                None
            }
        },
        CleanupAction::Command(_) => None,
    };
    let cleanup_thread = std::thread::Builder::new()
        .name("lethetic-python-drop-cleanup".to_string())
        .spawn(move || -> Result<(), String> {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| format!("could not start cleanup runtime: {error}"))?;
            #[cfg(target_os = "linux")]
            if matches!(cleanup, CleanupAction::TransientPodman { .. }) {
                return runtime.block_on(async {
                    let mut failures = Vec::new();
                    for attempt in 1..=3 {
                        match run_cleanup_action(Some(cleanup.clone())).await {
                            Ok(()) => {
                                if let Some(file_name) = cleanup_record.as_deref() {
                                    retained_podman::clear_transient_cleanup_record(file_name)?;
                                }
                                return Ok(());
                            }
                            Err(error) => failures.push(format!("attempt {attempt}: {error}")),
                        }
                        if attempt < 3 {
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        }
                    }
                    Err(format!(
                        "exact transient cleanup failed after 3 attempts: {}",
                        failures.join("; ")
                    ))
                });
            }
            runtime.block_on(run_cleanup_action(Some(cleanup)))
        });
    match cleanup_thread {
        Ok(cleanup_thread) => match cleanup_thread.join() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => eprintln!(
                "WARNING: Python worker drop cleanup could not verify exact resource removal: {error}"
            ),
            Err(_) => eprintln!("WARNING: Python worker drop cleanup thread panicked"),
        },
        Err(error) => eprintln!(
            "WARNING: Python worker drop cleanup thread could not start; exact resource removal is unverified: {error}"
        ),
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Drop has no async caller, so use bounded synchronous polling to reap
        // the leader and verify that the inherited group/Job is empty.
        match (self.child.as_mut(), self.process_tree.as_mut()) {
            (Some(child), Some(process_tree)) => {
                if let Err(error) = force_kill_and_reap_blocking(
                    child,
                    process_tree,
                    DROP_SETTLE_TIMEOUT,
                    "Python worker",
                ) {
                    eprintln!(
                        "WARNING: Python worker drop could not verify process-tree settlement: {error}"
                    );
                }
            }
            (child, process_tree) => {
                if let Some(process_tree) = process_tree {
                    let _ = process_tree.signal_force("Python worker");
                }
                if let Some(child) = child {
                    let _ = child.start_kill();
                }
            }
        }
        self.diagnostics_task.abort();
        if let Some(cleanup) = self.cleanup.take() {
            run_drop_cleanup(cleanup);
        }
    }
}

async fn abort_and_join_diagnostic_reader(task: &mut tokio::task::JoinHandle<()>) {
    task.abort();
    let _ = task.await;
}

fn spawn_diagnostic_reader(
    mut stderr: tokio::process::ChildStderr,
    diagnostics: Arc<StdMutex<VecDeque<u8>>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut buffer = [0_u8; 4096];
        while let Ok(read) = stderr.read(&mut buffer).await {
            if read == 0 {
                break;
            }
            let mut diagnostics = diagnostics
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            diagnostics.extend(&buffer[..read]);
            while diagnostics.len() > MAX_DIAGNOSTIC_BYTES {
                diagnostics.pop_front();
            }
        }
    })
}

#[cfg(test)]
mod tests;
