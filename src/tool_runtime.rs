use crate::config::{Config, PythonExecutionTarget};
use crate::python::backend::{self, PythonBackendChoice, ResolvedLaunch};
use crate::python::notebook::{NotebookAttemptStart, NotebookAttemptStatus, PythonNotebook};
use crate::python::{
    PythonContainerIdentity, PythonHostCallContext, PythonRunspace, PythonRuntimeNotice,
};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

#[cfg(target_os = "linux")]
type NotebookSessionLeaseGuard = Arc<crate::session_store::SessionLease>;
#[cfg(not(target_os = "linux"))]
type NotebookSessionLeaseGuard = ();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSurface {
    Interactive,
    Headless,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeExecution {
    pub output: String,
    pub cwd: String,
    pub is_error: bool,
    pub provenance: crate::tools::ToolOutputProvenance,
}

impl RuntimeExecution {
    fn error(message: impl Into<String>, cwd: &str) -> Self {
        Self {
            output: format!("ERROR: {}", message.into()),
            cwd: cwd.to_string(),
            is_error: true,
            provenance: crate::tools::ToolOutputProvenance::OrdinaryHost,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedWorkspaceBinding {
    pub canonical_path: PathBuf,
    pub device: u64,
    pub inode: u64,
    pub binding_hash: String,
}

#[cfg(target_os = "linux")]
impl SharedWorkspaceBinding {
    fn from_runtime_identity(identity: crate::python::runtime_store::WorkspaceIdentity) -> Self {
        Self {
            canonical_path: identity.canonical_path,
            device: identity.device,
            inode: identity.inode,
            binding_hash: identity.binding_hash,
        }
    }

    pub fn to_runtime_identity(&self) -> crate::python::runtime_store::WorkspaceIdentity {
        crate::python::runtime_store::WorkspaceIdentity {
            canonical_path: self.canonical_path.clone(),
            device: self.device,
            inode: self.inode,
            binding_hash: self.binding_hash.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionBinding {
    pub session_id: String,
    pub runtime_id: Option<String>,
    pub managed_workspace: PathBuf,
    pub workspace_device: u64,
    pub workspace_inode: u64,
    pub workspace_binding_hash: String,
    pub shared_workspace: Option<SharedWorkspaceBinding>,
    pub surface: ToolSurface,
}

#[derive(Clone)]
pub struct ToolRuntime {
    runspace: Arc<PythonRunspace>,
    cached_launch: Arc<Mutex<Option<ResolvedLaunch>>>,
    session_binding: Arc<Mutex<Option<SessionBinding>>>,
    workspace_root: Arc<PathBuf>,
    notebook: Arc<StdMutex<Option<Arc<PythonNotebook>>>>,
    notebook_run_id: Arc<String>,
    notebook_notice_emitted: Arc<StdMutex<Option<PathBuf>>>,
    pending_runtime_notices: Arc<StdMutex<VecDeque<PythonRuntimeNotice>>>,
    notebook_fault: Arc<StdMutex<Option<(PathBuf, String)>>>,
    #[cfg(target_os = "linux")]
    notebook_session_lease:
        Arc<StdMutex<Option<std::sync::Weak<crate::session_store::SessionLease>>>>,
    surface: ToolSurface,
}

impl ToolRuntime {
    pub fn new(surface: ToolSurface, workspace_root: impl Into<PathBuf>) -> Self {
        let workspace_root = workspace_root.into();
        // Resolve the startup workspace once. Python-reported cwd changes must
        // never retarget host-side runtime storage.
        let workspace_root = workspace_root.canonicalize().unwrap_or(workspace_root);
        Self {
            runspace: Arc::new(PythonRunspace::new()),
            cached_launch: Arc::new(Mutex::new(None)),
            session_binding: Arc::new(Mutex::new(None)),
            workspace_root: Arc::new(workspace_root),
            notebook: Arc::new(StdMutex::new(None)),
            notebook_run_id: Arc::new(uuid::Uuid::new_v4().to_string()),
            notebook_notice_emitted: Arc::new(StdMutex::new(None)),
            pending_runtime_notices: Arc::new(StdMutex::new(VecDeque::new())),
            notebook_fault: Arc::new(StdMutex::new(None)),
            #[cfg(target_os = "linux")]
            notebook_session_lease: Arc::new(StdMutex::new(None)),
            surface,
        }
    }

    pub fn interactive(workspace_root: impl Into<PathBuf>) -> Self {
        Self::new(ToolSurface::Interactive, workspace_root)
    }

    pub fn headless(workspace_root: impl Into<PathBuf>) -> Self {
        Self::new(ToolSurface::Headless, workspace_root)
    }

    pub fn surface(&self) -> ToolSurface {
        self.surface
    }

    pub fn workspace_root(&self) -> &Path {
        self.workspace_root.as_path()
    }

    pub fn python_container_identity(&self) -> Option<PythonContainerIdentity> {
        self.runspace.operational_identity()
    }

    #[cfg(target_os = "linux")]
    pub fn bind_python_notebook_session_lease(
        &self,
        lease: &Arc<crate::session_store::SessionLease>,
    ) -> Result<(), String> {
        lease.verify_current()?;
        *self
            .notebook_session_lease
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::downgrade(lease));
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn python_notebook_session_lease_guard(
        &self,
        required: bool,
    ) -> Result<Option<NotebookSessionLeaseGuard>, String> {
        let lease = self
            .notebook_session_lease
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some(lease) = lease else {
            return if required {
                Err("Durable Python notebook audit has no active session lease".to_string())
            } else {
                Ok(None)
            };
        };
        let lease = lease
            .upgrade()
            .ok_or_else(|| "Durable Python notebook audit session lease expired".to_string())?;
        lease.verify_current()?;
        Ok(Some(lease))
    }

    #[cfg(not(target_os = "linux"))]
    fn python_notebook_session_lease_guard(
        &self,
        required: bool,
    ) -> Result<Option<NotebookSessionLeaseGuard>, String> {
        if required {
            Err("Durable Python notebook auditing requires secure Linux session leases".to_string())
        } else {
            Ok(None)
        }
    }

    fn ensure_python_notebook_healthy(&self, path: &Path) -> Result<(), String> {
        let fault = self
            .notebook_fault
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((fault_path, reason)) = fault.as_ref()
            && fault_path == path
        {
            return Err(format!(
                "Python notebook audit is unavailable after a prior terminal checkpoint failure: {reason}. Restart or resume the session to repair the interrupted attempt"
            ));
        }
        Ok(())
    }

    fn poison_python_notebook(&self, path: &Path, reason: &str) {
        let reason = reason.chars().take(4096).collect::<String>();
        *self
            .notebook_fault
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some((path.to_path_buf(), reason));
    }

    fn python_notebook(
        &self,
        session_directory: Option<&Path>,
    ) -> Result<Arc<PythonNotebook>, String> {
        let _lease_guard = self.python_notebook_session_lease_guard(session_directory.is_some())?;
        #[cfg(target_os = "linux")]
        if let Some(directory) = session_directory {
            let canonical = directory.canonicalize().map_err(|error| {
                format!(
                    "Could not canonicalize Python notebook session directory {}: {error}",
                    directory.display()
                )
            })?;
            let lease = _lease_guard.as_ref().ok_or_else(|| {
                "Durable Python notebook audit has no active session lease".to_string()
            })?;
            if canonical != lease.canonical_path() {
                return Err(
                    "Durable Python notebook directory does not match its active session lease"
                        .to_string(),
                );
            }
        }
        let desired_path = match session_directory {
            Some(directory) => directory
                .canonicalize()
                .map_err(|error| {
                    format!(
                        "Could not canonicalize Python notebook session directory {}: {error}",
                        directory.display()
                    )
                })?
                .join("python.ipynb"),
            None => self
                .workspace_root
                .join(".lethetic")
                .join("python-sessions")
                .join(self.notebook_run_id.as_str())
                .join("python.ipynb"),
        };
        self.ensure_python_notebook_healthy(&desired_path)?;
        let mut current = self
            .notebook
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(notebook) = current.as_ref()
            && notebook.path() == desired_path
        {
            return Ok(notebook.clone());
        }
        let notebook = Arc::new(match session_directory {
            Some(directory) => PythonNotebook::in_existing_directory(directory)?,
            None => PythonNotebook::non_durable(
                self.workspace_root.as_path(),
                self.notebook_run_id.as_str(),
            )?,
        });
        *current = Some(notebook.clone());
        *self
            .notebook_fault
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        *self
            .notebook_notice_emitted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        Ok(notebook)
    }

    pub fn begin_python_audit_attempt(
        &self,
        session_directory: Option<&Path>,
        config: &Config,
        tool_call_id: &str,
        source: &str,
        description: &str,
        cwd: &str,
    ) -> Result<PathBuf, String> {
        let notebook = self.python_notebook(session_directory)?;
        let policy_fingerprint = config.python_policy_fingerprint();
        notebook.begin_attempt(NotebookAttemptStart {
            tool_call_id,
            source,
            description,
            cwd,
            policy_fingerprint: &policy_fingerprint,
        })?;
        Ok(notebook.path().to_path_buf())
    }

    pub fn mark_python_audit_status(
        &self,
        tool_call_id: &str,
        status: NotebookAttemptStatus,
        message: Option<&str>,
    ) -> Result<(), String> {
        let _lease_guard = self.python_notebook_session_lease_guard(false)?;
        let notebook = self
            .notebook
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(|| "Python notebook audit has not been initialized".to_string())?;
        self.ensure_python_notebook_healthy(notebook.path())?;
        notebook.mark_status(tool_call_id, status, message)
    }

    pub fn python_notebook_path(&self) -> Option<PathBuf> {
        self.notebook
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|notebook| notebook.path().to_path_buf())
    }

    pub fn take_python_notebook_notice(&self) -> Option<PathBuf> {
        let path = self.python_notebook_path()?;
        let mut emitted = self
            .notebook_notice_emitted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if emitted.as_ref() == Some(&path) {
            None
        } else {
            *emitted = Some(path.clone());
            Some(path)
        }
    }

    pub fn take_python_runtime_notice(&self) -> Option<PythonRuntimeNotice> {
        self.pending_runtime_notices
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
    }

    fn queue_python_runtime_notice(&self, notice: PythonRuntimeNotice) {
        self.pending_runtime_notices
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(notice);
    }

    pub async fn bind_session(&self, binding: SessionBinding) -> Result<(), String> {
        validate_session_binding(&binding, self.surface)?;
        let mut current = self.session_binding.lock().await;
        if current.as_ref() == Some(&binding) {
            return Ok(());
        }
        let previous = current.clone();
        self.reset_and_reconcile(previous.as_ref()).await?;
        *current = Some(binding);
        Ok(())
    }

    #[cfg(target_os = "linux")]
    pub async fn bind_managed_session(
        &self,
        session_id: String,
        runtime_id: Option<String>,
        workspace: crate::python::runtime_store::WorkspaceIdentity,
    ) -> Result<(), String> {
        self.bind_managed_session_with_shared(session_id, runtime_id, workspace, None)
            .await
    }

    #[cfg(target_os = "linux")]
    pub async fn bind_managed_session_with_shared(
        &self,
        session_id: String,
        runtime_id: Option<String>,
        workspace: crate::python::runtime_store::WorkspaceIdentity,
        shared_workspace: Option<crate::python::runtime_store::WorkspaceIdentity>,
    ) -> Result<(), String> {
        self.bind_session(SessionBinding {
            session_id,
            runtime_id,
            managed_workspace: workspace.canonical_path,
            workspace_device: workspace.device,
            workspace_inode: workspace.inode,
            workspace_binding_hash: workspace.binding_hash,
            shared_workspace: shared_workspace.map(SharedWorkspaceBinding::from_runtime_identity),
            surface: self.surface,
        })
        .await
    }

    pub async fn session_binding(&self) -> Option<SessionBinding> {
        self.session_binding.lock().await.clone()
    }

    pub async fn update_bound_runtime_id(&self, runtime_id: String) -> Result<(), String> {
        validate_uuid(&runtime_id, "runtime ID")?;
        let mut binding = self.session_binding.lock().await;
        let binding = binding
            .as_mut()
            .ok_or_else(|| "Python runtime has no bound chat session".to_string())?;
        binding.runtime_id = Some(runtime_id);
        Ok(())
    }

    pub async fn execute_python(
        &self,
        config: &Config,
        current_cwd: &str,
        code: &str,
        cancellation_token: CancellationToken,
        progress_tx: Option<&tokio::sync::mpsc::UnboundedSender<crate::client::StreamEvent>>,
    ) -> RuntimeExecution {
        Box::pin(self.execute_python_impl(
            config,
            current_cwd,
            code,
            None,
            cancellation_token,
            progress_tx,
        ))
        .await
    }

    pub async fn execute_python_audited(
        &self,
        config: &Config,
        current_cwd: &str,
        code: &str,
        tool_call_id: &str,
        cancellation_token: CancellationToken,
        progress_tx: Option<&tokio::sync::mpsc::UnboundedSender<crate::client::StreamEvent>>,
    ) -> RuntimeExecution {
        Box::pin(self.execute_python_impl(
            config,
            current_cwd,
            code,
            Some(tool_call_id),
            cancellation_token,
            progress_tx,
        ))
        .await
    }

    async fn execute_python_impl(
        &self,
        config: &Config,
        current_cwd: &str,
        code: &str,
        tool_call_id: Option<&str>,
        cancellation_token: CancellationToken,
        progress_tx: Option<&tokio::sync::mpsc::UnboundedSender<crate::client::StreamEvent>>,
    ) -> RuntimeExecution {
        let _notebook_session_lease = if tool_call_id.is_some() {
            match self.python_notebook_session_lease_guard(false) {
                Ok(lease) => lease,
                Err(error) => {
                    return RuntimeExecution::error(
                        format!(
                            "Python execution was not started because its notebook session lease could not be verified: {error}"
                        ),
                        current_cwd,
                    );
                }
            }
        } else {
            None
        };
        let notebook = if let Some(tool_call_id) = tool_call_id {
            match self
                .notebook
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
            {
                Some(notebook) => Some((tool_call_id, notebook)),
                None => {
                    return RuntimeExecution::error(
                        "Python execution was not started because its notebook audit checkpoint is missing",
                        current_cwd,
                    );
                }
            }
        } else {
            None
        };
        if let Some((_, notebook)) = &notebook
            && let Err(error) = self.ensure_python_notebook_healthy(notebook.path())
        {
            return RuntimeExecution::error(
                format!("Python execution was not started: {error}"),
                current_cwd,
            );
        }
        let launch =
            match Box::pin(self.launch_for(config, Path::new(current_cwd), &cancellation_token))
                .await
            {
                Ok(launch) => launch,
                Err(error) => {
                    if let Some((tool_call_id, notebook)) = &notebook
                        && let Err(audit_error) = notebook.mark_status(
                            tool_call_id,
                            NotebookAttemptStatus::LaunchFailed,
                            Some(&error),
                        )
                    {
                        return RuntimeExecution::error(
                            format!(
                                "{error}; notebook audit persistence also failed: {audit_error}"
                            ),
                            current_cwd,
                        );
                    }
                    return RuntimeExecution::error(error, current_cwd);
                }
            };
        let policy_fingerprint = config.python_policy_fingerprint();
        let host_context = if let Some((tool_call_id, notebook)) = &notebook {
            match PythonHostCallContext::new(
                self.workspace_root.as_path(),
                notebook.clone(),
                tool_call_id,
                progress_tx.cloned(),
            ) {
                Ok(context) => Some(context),
                Err(error) => {
                    if let Err(audit_error) = notebook.mark_status(
                        tool_call_id,
                        NotebookAttemptStatus::LaunchFailed,
                        Some(&error),
                    ) {
                        return RuntimeExecution::error(
                            format!(
                                "{error}; notebook audit persistence also failed: {audit_error}"
                            ),
                            current_cwd,
                        );
                    }
                    return RuntimeExecution::error(error, current_cwd);
                }
            }
        } else {
            None
        };
        if let Some((tool_call_id, notebook)) = &notebook
            && let Err(error) =
                notebook.mark_running(tool_call_id, launch.choice.label(), &policy_fingerprint)
        {
            return RuntimeExecution::error(
                format!(
                    "Python execution was not started because its notebook running checkpoint failed: {error}"
                ),
                current_cwd,
            );
        }

        if let Some(tx) = progress_tx {
            let _ = tx.send(crate::client::StreamEvent::ToolProgress(format!(
                "Python cell running via {}",
                launch.choice.label()
            )));
        }

        let worker_was_running = self.runspace.is_running().await;
        let readiness = self
            .runspace
            .ensure_ready(
                launch.spec.clone(),
                launch.fingerprint.clone(),
                cancellation_token.clone(),
            )
            .await;
        let result = match readiness {
            Ok(notice) => {
                if let Some(notice) = notice {
                    if let Some(tx) = progress_tx {
                        let _ = tx.send(crate::client::StreamEvent::PythonRuntimeNotice(notice));
                    } else {
                        self.queue_python_runtime_notice(notice);
                    }
                }
                self.runspace
                    .execute_with_host_context(
                        launch.spec.clone(),
                        launch.fingerprint.clone(),
                        code,
                        cancellation_token,
                        host_context.clone(),
                    )
                    .await
            }
            Err(error) => Err(error),
        };
        match result {
            Ok(cell) => {
                if let Some((tool_call_id, notebook)) = &notebook
                    && let Err(error) = notebook.record_result(
                        tool_call_id,
                        &cell,
                        launch.choice.label(),
                        &policy_fingerprint,
                    )
                {
                    self.poison_python_notebook(notebook.path(), &error);
                    let cleanup = self.runspace.reset_checked().await.err();
                    *self.cached_launch.lock().await = None;
                    return RuntimeExecution::error(
                        format!(
                            "Python operation may have completed, but its notebook result could not be persisted: {error}{}",
                            cleanup
                                .map(|cleanup| format!("; runspace cleanup failed: {cleanup}"))
                                .unwrap_or_default()
                        ),
                        current_cwd,
                    );
                }
                let cwd = visible_result_cwd(&launch, current_cwd, &cell.cwd);
                let output = cell.render();
                RuntimeExecution {
                    output,
                    cwd,
                    is_error: cell.is_error,
                    provenance: crate::tools::ToolOutputProvenance::PythonCell(
                        cell.output_metadata,
                    ),
                }
            }
            Err(error) => {
                if let Some((tool_call_id, notebook)) = &notebook {
                    let status = if error.to_ascii_lowercase().contains("cancel") {
                        NotebookAttemptStatus::Cancelled
                    } else if worker_was_running {
                        NotebookAttemptStatus::Error
                    } else {
                        NotebookAttemptStatus::LaunchFailed
                    };
                    if let Err(audit_error) =
                        notebook.mark_status(tool_call_id, status, Some(&error))
                    {
                        self.poison_python_notebook(notebook.path(), &audit_error);
                        let cleanup = self.runspace.reset_checked().await.err();
                        *self.cached_launch.lock().await = None;
                        return RuntimeExecution::error(
                            format!(
                                "{error}; notebook terminal checkpoint failed: {audit_error}. The Python operation may have partially completed{}",
                                cleanup
                                    .map(|cleanup| format!("; runspace cleanup failed: {cleanup}"))
                                    .unwrap_or_default()
                            ),
                            current_cwd,
                        );
                    }
                }
                *self.cached_launch.lock().await = None;
                RuntimeExecution::error(error, current_cwd)
            }
        }
    }

    pub async fn ensure_ready(&self, config: &Config, current_cwd: &Path) -> Result<(), String> {
        self.ensure_ready_with_cancellation(config, current_cwd, CancellationToken::new())
            .await
    }

    pub async fn ensure_ready_with_cancellation(
        &self,
        config: &Config,
        current_cwd: &Path,
        cancellation: CancellationToken,
    ) -> Result<(), String> {
        if cancellation.is_cancelled() {
            return Err("Python runtime startup was cancelled".to_string());
        }
        let launch = Box::pin(self.launch_for(config, current_cwd, &cancellation)).await?;
        if cancellation.is_cancelled() {
            *self.cached_launch.lock().await = None;
            return Err("Python runtime startup was cancelled".to_string());
        }
        let ready = self
            .runspace
            .ensure_ready(launch.spec, launch.fingerprint, cancellation.clone())
            .await;
        if cancellation.is_cancelled() {
            let cleanup = self.runspace.reset_checked().await;
            *self.cached_launch.lock().await = None;
            return match cleanup {
                Ok(()) => Err("Python runtime startup was cancelled".to_string()),
                Err(cleanup) => Err(format!(
                    "Python runtime startup was cancelled; cleanup failed: {cleanup}"
                )),
            };
        }
        match ready {
            Ok(Some(notice)) => {
                self.queue_python_runtime_notice(notice);
                Ok(())
            }
            Ok(None) => Ok(()),
            Err(error) => {
                *self.cached_launch.lock().await = None;
                Err(error)
            }
        }
    }

    pub async fn reset_checked(&self) -> Result<(), String> {
        let binding = self.session_binding.lock().await.clone();
        self.reset_and_reconcile(binding.as_ref()).await
    }

    pub async fn reset(&self) {
        let _ = self.reset_checked().await;
    }

    pub async fn unbind_session_checked(&self) -> Result<(), String> {
        let mut binding = self.session_binding.lock().await;
        self.reset_and_reconcile(binding.as_ref()).await?;
        *binding = None;
        Ok(())
    }

    pub async fn unbind_session(&self) {
        let _ = self.unbind_session_checked().await;
    }

    async fn reset_and_reconcile(&self, _binding: Option<&SessionBinding>) -> Result<(), String> {
        let reset = self.runspace.reset_checked().await;
        *self.cached_launch.lock().await = None;
        #[cfg(target_os = "linux")]
        let reconciliation = match _binding.and_then(|binding| {
            binding
                .runtime_id
                .as_deref()
                .map(|runtime_id| (runtime_id, binding.session_id.as_str()))
        }) {
            Some((runtime_id, session_id)) => {
                crate::python::retained_runtime::reconcile_retained_runtime(runtime_id, session_id)
                    .await
            }
            None => Ok(()),
        };
        #[cfg(not(target_os = "linux"))]
        let reconciliation: Result<(), String> = Ok(());
        match (reset, reconciliation) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) => Err(error),
            (Ok(()), Err(error)) => Err(format!(
                "retained runtime detach reconciliation failed: {error}"
            )),
            (Err(reset), Err(reconciliation)) => Err(format!(
                "{reset}; retained runtime detach reconciliation failed: {reconciliation}"
            )),
        }
    }

    pub async fn is_running(&self) -> bool {
        self.runspace.is_running().await
    }

    async fn launch_for(
        &self,
        config: &Config,
        current_cwd: &Path,
        cancellation: &CancellationToken,
    ) -> Result<ResolvedLaunch, String> {
        if let Some(error) = config.python_mode_validation_error() {
            return Err(format!("Invalid Python-only policy: {error}"));
        }
        let policy_fingerprint = config.python_policy_fingerprint();
        if self.runspace.is_running().await
            && let Some(cached) = self.cached_launch.lock().await.as_ref()
            && cached
                .fingerprint
                .starts_with(&format!("{policy_fingerprint}:"))
        {
            return Ok(cached.clone());
        }

        let resolved = if crate::config::is_exact_retained_nonlocal_python_policy(
            config.tool_profile,
            &config.python_runtime,
        ) {
            #[cfg(target_os = "linux")]
            {
                let binding = self.session_binding.lock().await.clone().ok_or_else(|| {
                    "Nonlocal Python requires a manifest-bound chat session".to_string()
                })?;
                Box::pin(
                    crate::python::retained_runtime::prepare_retained_launch_with_cancellation(
                        config,
                        &binding,
                        cancellation.clone(),
                    ),
                )
                .await?
            }
            #[cfg(not(target_os = "linux"))]
            {
                return Err("Nonlocal retained Python is supported only on Linux".to_string());
            }
        } else {
            Box::pin(backend::resolve_launch_with_cancellation(
                config,
                &self.workspace_root,
                current_cwd,
                cancellation.clone(),
            ))
            .await?
        };
        if cancellation.is_cancelled() {
            return Err("Python runtime launch preparation was cancelled".to_string());
        }
        *self.cached_launch.lock().await = Some(resolved.clone());
        Ok(resolved)
    }
}

fn validate_session_binding(
    binding: &SessionBinding,
    expected_surface: ToolSurface,
) -> Result<(), String> {
    validate_uuid(&binding.session_id, "session ID")?;
    if let Some(runtime_id) = &binding.runtime_id {
        validate_uuid(runtime_id, "runtime ID")?;
    }
    if binding.surface != expected_surface {
        return Err("Python session binding belongs to a different tool surface".to_string());
    }

    #[cfg(target_os = "linux")]
    {
        let identity = crate::python::runtime_store::WorkspaceIdentity {
            canonical_path: binding.managed_workspace.clone(),
            device: binding.workspace_device,
            inode: binding.workspace_inode,
            binding_hash: binding.workspace_binding_hash.clone(),
        };
        identity.verify_current()?;
        if let Some(shared) = &binding.shared_workspace {
            let shared = shared.to_runtime_identity();
            shared.verify_current()?;
            if shared.canonical_path == identity.canonical_path {
                return Err(
                    "shared launch cwd must be distinct from the managed workspace".to_string(),
                );
            }
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err("retained Python session bindings are supported only on Linux".to_string())
    }
}

fn validate_uuid(value: &str, label: &str) -> Result<(), String> {
    if value.len() != 36
        || !value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()
            }
        })
    {
        return Err(format!("{label} is not a canonical lowercase UUID"));
    }
    Ok(())
}

fn visible_result_cwd(launch: &ResolvedLaunch, previous: &str, reported: &str) -> String {
    let reported_path = Path::new(reported);
    if !reported_path.is_absolute() {
        return previous.to_string();
    }
    if launch.choice == PythonBackendChoice::Host {
        return reported.to_string();
    }
    if launch.network == Some(crate::config::NetworkAccess::Nonlocal)
        && let Ok(relative) = reported_path.strip_prefix("/workspace")
    {
        let host_path = launch.launch_cwd.join(relative);
        if host_path.starts_with(&launch.launch_cwd) {
            return host_path.to_string_lossy().into_owned();
        }
    }
    if launch
        .host_visible_roots
        .iter()
        .any(|root| reported_path.starts_with(root))
    {
        reported.to_string()
    } else {
        previous.to_string()
    }
}

pub fn backend_choice(config: &Config) -> Option<PythonBackendChoice> {
    if config.tool_profile != crate::config::ToolProfile::PythonOnly {
        return None;
    }
    match config.python_runtime.target {
        Some(PythonExecutionTarget::Host) => Some(PythonBackendChoice::Host),
        Some(PythonExecutionTarget::Sandbox) => match config.python_runtime.sandbox.backend {
            Some(crate::config::SandboxBackend::Bubblewrap) => {
                Some(PythonBackendChoice::Bubblewrap)
            }
            Some(crate::config::SandboxBackend::Podman) => Some(PythonBackendChoice::Podman),
            None => None,
        },
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{PythonRuntimeConfig, ToolProfile};

    fn host_config() -> Config {
        Config {
            tool_profile: ToolProfile::PythonOnly,
            python_runtime: PythonRuntimeConfig {
                target: Some(PythonExecutionTarget::Host),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn general_profile_has_no_python_backend_choice() {
        let mut config = host_config();
        config.tool_profile = ToolProfile::General;
        assert_eq!(backend_choice(&config), None);
    }

    #[tokio::test]
    async fn pre_cancelled_runtime_startup_never_installs_worker() {
        let workspace = tempfile::tempdir().unwrap();
        let runtime = ToolRuntime::interactive(workspace.path());
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let error = runtime
            .ensure_ready_with_cancellation(&host_config(), workspace.path(), cancellation)
            .await
            .unwrap_err();

        assert!(error.contains("cancelled"), "{error}");
        assert!(!runtime.is_running().await);
        assert!(runtime.cached_launch.lock().await.is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runtime_launch_preparation_uses_caller_cancellation() {
        use std::os::unix::fs::PermissionsExt;

        let workspace = tempfile::tempdir().unwrap();
        let executable = workspace.path().join("hanging-python");
        let marker = workspace.path().join("probe-pids");
        std::fs::write(
            &executable,
            format!(
                "#!/bin/sh\n/bin/sleep 30 &\nchild=$!\nprintf '%s %s' \"$$\" \"$child\" > '{}'\nwait \"$child\"\n",
                marker.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut config = host_config();
        config.python_runtime.python_executable = executable.to_string_lossy().into_owned();
        let runtime = ToolRuntime::interactive(workspace.path());
        let task_runtime = runtime.clone();
        let task_workspace = workspace.path().to_path_buf();
        let cancellation = CancellationToken::new();
        let task_cancellation = cancellation.clone();
        let task = tokio::spawn(async move {
            task_runtime
                .ensure_ready_with_cancellation(&config, &task_workspace, task_cancellation)
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !marker.exists() {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("runtime launch probe never started");
        let pids = std::fs::read_to_string(&marker)
            .unwrap()
            .split_whitespace()
            .map(|value| value.parse::<libc::pid_t>().unwrap())
            .collect::<Vec<_>>();

        cancellation.cancel();
        let error = tokio::time::timeout(std::time::Duration::from_secs(2), task)
            .await
            .expect("runtime launch cancellation did not settle")
            .expect("runtime launch task panicked")
            .unwrap_err();
        assert!(error.contains("cancelled"), "{error}");
        assert!(!runtime.is_running().await);
        assert!(runtime.cached_launch.lock().await.is_none());
        for pid in pids {
            let alive = unsafe { libc::kill(pid, 0) } == 0;
            assert!(!alive, "runtime launch process {pid} survived cancellation");
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ESRCH)
            );
        }
    }

    #[tokio::test]
    async fn tool_runtime_preserves_python_state_and_cwd() {
        if !crate::platform::binary_on_path("python3") {
            return;
        }
        let workspace = std::env::current_dir().unwrap();
        let runtime = ToolRuntime::interactive(workspace.clone());
        let config = host_config();
        let first = runtime
            .execute_python(
                &config,
                workspace.to_str().unwrap(),
                "runtime_value = 41",
                CancellationToken::new(),
                None,
            )
            .await;
        assert!(!first.is_error, "{}", first.output);
        let second = runtime
            .execute_python(
                &config,
                &first.cwd,
                "runtime_value + 1",
                CancellationToken::new(),
                None,
            )
            .await;
        assert!(!second.is_error, "{}", second.output);
        assert!(second.output.contains("42"));
        let crate::tools::ToolOutputProvenance::PythonCell(metadata) = second.provenance else {
            panic!("Python execution lost worker-local output provenance");
        };
        assert_eq!(metadata.cell, 2);
        assert!(metadata.retained);
    }

    #[tokio::test]
    async fn host_cwd_change_does_not_reset_globals() {
        if !crate::platform::binary_on_path("python3") {
            return;
        }
        let workspace = tempfile::tempdir().unwrap();
        let nested = workspace.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        let runtime = ToolRuntime::interactive(workspace.path());
        let config = host_config();
        let code = format!(
            "import os\nkept_after_chdir = 7\nos.chdir({:?})",
            nested.to_string_lossy()
        );
        let first = runtime
            .execute_python(
                &config,
                workspace.path().to_str().unwrap(),
                &code,
                CancellationToken::new(),
                None,
            )
            .await;
        assert!(!first.is_error, "{}", first.output);
        assert_eq!(Path::new(&first.cwd), nested.as_path());
        let second = runtime
            .execute_python(
                &config,
                &first.cwd,
                "kept_after_chdir",
                CancellationToken::new(),
                None,
            )
            .await;
        assert!(!second.is_error, "{}", second.output);
        assert!(second.output.contains('7'));
    }

    #[tokio::test]
    async fn lethetic_todo_persists_across_cwd_change_and_worker_reset() {
        if !crate::platform::binary_on_path("python3") {
            return;
        }
        let workspace = tempfile::tempdir().unwrap();
        let workspace = workspace.path().canonicalize().unwrap();
        let runtime = ToolRuntime::interactive(&workspace);
        let config = host_config();
        let cwd = workspace.to_string_lossy().into_owned();
        let (progress, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();

        let get_source = "import lethetic_todo\ntodo_snapshot = lethetic_todo.get()\ntodo_snapshot";
        runtime
            .begin_python_audit_attempt(None, &config, "todo-get-1", get_source, "read todos", &cwd)
            .unwrap();
        runtime
            .mark_python_audit_status("todo-get-1", NotebookAttemptStatus::Approved, None)
            .unwrap();
        let first = runtime
            .execute_python_audited(
                &config,
                &cwd,
                get_source,
                "todo-get-1",
                CancellationToken::new(),
                Some(&progress),
            )
            .await;
        assert!(!first.is_error, "{}", first.output);
        assert!(first.output.contains("'revision': 0"), "{}", first.output);

        let set_source = "import os\nos.makedirs('nested', exist_ok=True)\nos.chdir('nested')\nlethetic_todo.set([{'id': 'bridge', 'content': 'Persist safely', 'status': 'in_progress', 'priority': 'high'}], expected_revision=todo_snapshot['revision'])";
        runtime
            .begin_python_audit_attempt(
                None,
                &config,
                "todo-set-1",
                set_source,
                "replace todos",
                &first.cwd,
            )
            .unwrap();
        runtime
            .mark_python_audit_status("todo-set-1", NotebookAttemptStatus::Approved, None)
            .unwrap();
        let second = runtime
            .execute_python_audited(
                &config,
                &first.cwd,
                set_source,
                "todo-set-1",
                CancellationToken::new(),
                Some(&progress),
            )
            .await;
        assert!(!second.is_error, "{}", second.output);
        assert!(second.output.contains("'revision': 1"), "{}", second.output);
        assert!(workspace.join(".lethetic/todos.json").is_file());
        assert!(!workspace.join("nested/.lethetic/todos.json").exists());
        let progress_event = progress_rx.try_recv().unwrap();
        assert!(matches!(
            progress_event,
            crate::client::StreamEvent::ToolProgress(message)
                if message.contains("Python cell running")
        ));
        let mut todo_update = None;
        while let Ok(event) = progress_rx.try_recv() {
            if let crate::client::StreamEvent::TodoUpdated(snapshot) = event {
                todo_update = Some(snapshot);
            }
        }
        let todo_update = todo_update.expect("todo.set must emit a typed refresh event");
        assert_eq!(todo_update.revision, 1);
        assert_eq!(todo_update.todos.len(), 1);
        assert_eq!(todo_update.todos[0].content, "Persist safely");

        runtime.reset_checked().await.unwrap();
        let get_after_reset = "import lethetic_todo\nlethetic_todo.get()";
        runtime
            .begin_python_audit_attempt(
                None,
                &config,
                "todo-get-2",
                get_after_reset,
                "read persisted todos",
                &second.cwd,
            )
            .unwrap();
        runtime
            .mark_python_audit_status("todo-get-2", NotebookAttemptStatus::Approved, None)
            .unwrap();
        let third = runtime
            .execute_python_audited(
                &config,
                &second.cwd,
                get_after_reset,
                "todo-get-2",
                CancellationToken::new(),
                None,
            )
            .await;
        assert!(!third.is_error, "{}", third.output);
        assert!(third.output.contains("Persist safely"), "{}", third.output);

        let conflict_source = "import lethetic_todo\nlethetic_todo.set([], expected_revision=0)";
        runtime
            .begin_python_audit_attempt(
                None,
                &config,
                "todo-conflict",
                conflict_source,
                "reject stale revision",
                &third.cwd,
            )
            .unwrap();
        runtime
            .mark_python_audit_status("todo-conflict", NotebookAttemptStatus::Approved, None)
            .unwrap();
        let conflict = runtime
            .execute_python_audited(
                &config,
                &third.cwd,
                conflict_source,
                "todo-conflict",
                CancellationToken::new(),
                None,
            )
            .await;
        assert!(conflict.is_error);
        assert!(conflict.output.contains("Todo revision conflict"));

        let notebook: serde_json::Value = serde_json::from_slice(
            &std::fs::read(runtime.python_notebook_path().unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            notebook["cells"][1]["metadata"]["lethetic"]["host_calls"][0]["operation"],
            "todo.set"
        );
        assert_eq!(
            notebook["cells"][1]["metadata"]["lethetic"]["host_calls"][0]["status"],
            "succeeded"
        );
        assert_eq!(
            notebook["cells"][3]["metadata"]["lethetic"]["host_calls"][0]["error_code"],
            "revision_conflict"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn terminal_notebook_write_failure_poisoned_until_restart() {
        use std::os::unix::fs::symlink;

        if !crate::platform::binary_on_path("python3") {
            return;
        }
        let workspace = tempfile::tempdir().unwrap();
        let workspace = workspace.path().canonicalize().unwrap();
        let runtime = ToolRuntime::interactive(&workspace);
        let config = host_config();
        let cwd = workspace.to_string_lossy().into_owned();
        let source = "from pathlib import Path\nPath('started').write_text('yes')\nimport time\ntime.sleep(1)\n42";
        runtime
            .begin_python_audit_attempt(
                None,
                &config,
                "terminal-write",
                source,
                "test terminal checkpoint failure",
                &cwd,
            )
            .unwrap();
        runtime
            .mark_python_audit_status("terminal-write", NotebookAttemptStatus::Approved, None)
            .unwrap();

        let execution_runtime = runtime.clone();
        let execution_config = config.clone();
        let execution_cwd = cwd.clone();
        let execution = tokio::spawn(async move {
            execution_runtime
                .execute_python_audited(
                    &execution_config,
                    &execution_cwd,
                    source,
                    "terminal-write",
                    CancellationToken::new(),
                    None,
                )
                .await
        });
        for _ in 0..200 {
            if workspace.join("started").exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(workspace.join("started").exists());
        let notebook_path = runtime.python_notebook_path().unwrap();
        std::fs::remove_file(&notebook_path).unwrap();
        let outside = workspace.join("outside-notebook-target");
        std::fs::write(&outside, "outside-safe").unwrap();
        symlink(&outside, &notebook_path).unwrap();

        let result = execution.await.unwrap();
        assert!(result.is_error);
        assert!(result.output.contains("operation may have completed"));
        assert!(!runtime.runspace.is_running().await);
        assert_eq!(std::fs::read_to_string(outside).unwrap(), "outside-safe");
        let error = runtime
            .begin_python_audit_attempt(
                None,
                &config,
                "must-not-run",
                "raise AssertionError('must not execute')",
                "poisoned recorder",
                &cwd,
            )
            .unwrap_err();
        assert!(
            error.contains("prior terminal checkpoint failure"),
            "{error}"
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn session_binding_tracks_runtime_and_survives_failed_reconciliation() {
        let workspace = tempfile::tempdir().unwrap();
        let workspace = workspace.path().canonicalize().unwrap();
        let runtime = ToolRuntime::interactive(&workspace);
        let identity =
            crate::python::runtime_store::WorkspaceIdentity::capture(&workspace).unwrap();
        let binding = SessionBinding {
            session_id: "11111111-2222-4333-8444-555555555555".to_string(),
            runtime_id: None,
            managed_workspace: identity.canonical_path,
            workspace_device: identity.device,
            workspace_inode: identity.inode,
            workspace_binding_hash: identity.binding_hash,
            shared_workspace: None,
            surface: ToolSurface::Interactive,
        };
        runtime.bind_session(binding.clone()).await.unwrap();
        assert_eq!(runtime.session_binding().await, Some(binding));
        runtime
            .update_bound_runtime_id("01234567-89ab-4def-8123-456789abcdef".to_string())
            .await
            .unwrap();
        assert_eq!(
            runtime
                .session_binding()
                .await
                .unwrap()
                .runtime_id
                .as_deref(),
            Some("01234567-89ab-4def-8123-456789abcdef")
        );
        let error = runtime.unbind_session_checked().await.unwrap_err();
        assert!(error.contains("detach reconciliation failed"), "{error}");
        assert!(runtime.session_binding().await.is_some());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn replaced_workspace_cannot_reuse_session_binding() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let workspace = workspace.canonicalize().unwrap();
        let identity =
            crate::python::runtime_store::WorkspaceIdentity::capture(&workspace).unwrap();
        std::fs::rename(&workspace, root.path().join("original-workspace")).unwrap();
        std::fs::create_dir(&workspace).unwrap();

        let runtime = ToolRuntime::interactive(&workspace);
        let error = runtime
            .bind_session(SessionBinding {
                session_id: "11111111-2222-4333-8444-555555555555".to_string(),
                runtime_id: None,
                managed_workspace: identity.canonical_path,
                workspace_device: identity.device,
                workspace_inode: identity.inode,
                workspace_binding_hash: identity.binding_hash,
                shared_workspace: None,
                surface: ToolSurface::Interactive,
            })
            .await
            .unwrap_err();
        assert!(error.contains("identity no longer matches"));
    }

    #[test]
    fn private_sandbox_cwd_does_not_escape_to_host_state() {
        let launch = ResolvedLaunch {
            spec: crate::python::LaunchSpec::host("python3", PathBuf::from("/workspace")),
            fingerprint: "x".to_string(),
            choice: PythonBackendChoice::Bubblewrap,
            network: Some(crate::config::NetworkAccess::None),
            workspace_access: Some(crate::config::AccessMode::ReadOnly),
            host_visible_roots: vec![PathBuf::from("/workspace")],
            launch_cwd: PathBuf::from("/workspace"),
        };
        assert_eq!(
            visible_result_cwd(&launch, "/workspace", "/tmp/private"),
            "/workspace"
        );
        assert_eq!(
            visible_result_cwd(&launch, "/workspace", "/workspace/data"),
            "/workspace/data"
        );

        let retained = ResolvedLaunch {
            network: Some(crate::config::NetworkAccess::Nonlocal),
            launch_cwd: PathBuf::from("/host/managed-workspace"),
            host_visible_roots: vec![PathBuf::from("/host/managed-workspace")],
            ..launch
        };
        assert_eq!(
            visible_result_cwd(&retained, "/host/managed-workspace", "/workspace/data"),
            "/host/managed-workspace/data"
        );
        assert_eq!(
            visible_result_cwd(&retained, "/host/managed-workspace", "/tmp/private"),
            "/host/managed-workspace"
        );
    }
}
