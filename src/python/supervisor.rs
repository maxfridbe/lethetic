use super::WORKER_SOURCE;
use crate::python::egress_broker::{
    AdapterConfig, DEFAULT_ADAPTER_PORT, open_broker_readiness_connection, probe_broker,
    run_adapter_with_readiness,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::process::Command;
use tokio::sync::{Mutex, Semaphore, oneshot};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

pub const RUNTIME_ABI: &str = "lethetic-python-runtime-v4";
pub const PACKAGE_PROTOCOL_VERSION: u32 = 1;
pub const PACKAGE_SOCKET_PATH: &str = "/run/lethetic-pkg/control.sock";
pub const PACKAGE_AUDIT_PATH: &str = "/var/log/lethetic/package-transactions.jsonl";
pub const PYTHON_PATH: &str = "/usr/local/bin/python3";
pub const APT_GET_PATH: &str = "/usr/bin/apt-get";
pub const WORKER_HOME: &str = "/home/lethetic";

const PACKAGE_MAX_FRAME_BYTES: usize = 1024 * 1024;
const PACKAGE_MAX_NAMES: usize = 32;
const PACKAGE_MAX_NAME_BYTES: usize = 128;
const PACKAGE_MAX_OUTPUT_BYTES: usize = 64 * 1024;
const PACKAGE_MAX_AUDIT_BYTES: u64 = 16 * 1024 * 1024;
const PACKAGE_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const PACKAGE_TRANSACTION_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const BROKER_STARTUP_TIMEOUT: Duration = Duration::from_secs(60);
const SERVICE_STARTUP_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone)]
pub struct SupervisorConfig {
    pub runtime_id: String,
    pub capability_path: PathBuf,
    pub broker_socket_path: PathBuf,
    pub workspace: PathBuf,
    pub worker_uid: u32,
    pub worker_gid: u32,
}

impl SupervisorConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.worker_uid == 0 || self.worker_gid == 0 {
            return Err("runtime worker UID and GID must be nonzero".to_string());
        }
        if !self.capability_path.is_absolute() || !self.broker_socket_path.is_absolute() {
            return Err("runtime capability and broker socket paths must be absolute".to_string());
        }
        let canonical = self
            .workspace
            .canonicalize()
            .map_err(|error| format!("could not canonicalize runtime workspace: {error}"))?;
        if canonical != self.workspace || !canonical.is_dir() {
            return Err("runtime workspace must be an existing canonical directory".to_string());
        }
        let capability = read_capability(&self.capability_path)?;
        AdapterConfig {
            socket_path: self.broker_socket_path.clone(),
            runtime_id: self.runtime_id.clone(),
            capability,
            listen: SocketAddr::from((Ipv4Addr::LOCALHOST, DEFAULT_ADAPTER_PORT)),
            max_connections: 32,
        }
        .validate()
    }
}

enum SupervisorExit {
    Worker(std::process::ExitStatus),
    LeaseEnded,
    Signal(i32),
    Infrastructure(String),
}

fn classify_output_task_exit(
    result: Result<Result<(), String>, tokio::task::JoinError>,
) -> SupervisorExit {
    match result {
        Ok(Ok(())) => SupervisorExit::Infrastructure(
            "runtime worker protocol output reached EOF before the worker exited".to_string(),
        ),
        Ok(Err(error)) => SupervisorExit::Infrastructure(error),
        Err(error) => {
            SupervisorExit::Infrastructure(format!("runtime output proxy failed: {error}"))
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PackageOperation {
    Refresh,
    Install,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct PackageRequest {
    version: u32,
    operation: PackageOperation,
    packages: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PackageResponse {
    pub version: u32,
    pub ok: bool,
    pub exit_code: Option<i32>,
    pub output: String,
    pub truncated: bool,
}

struct AsyncProcessFd {
    fd: tokio::io::unix::AsyncFd<OwnedFd>,
}

impl AsyncProcessFd {
    fn duplicate(raw_fd: libc::c_int, label: &str) -> Result<Self, String> {
        let duplicated = unsafe { libc::fcntl(raw_fd, libc::F_DUPFD_CLOEXEC, 3) };
        if duplicated < 0 {
            return Err(format!(
                "could not duplicate runtime {label}: {}",
                std::io::Error::last_os_error()
            ));
        }
        let fd = unsafe { OwnedFd::from_raw_fd(duplicated) };
        let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
        if flags < 0 {
            return Err(format!(
                "could not inspect runtime {label} flags: {}",
                std::io::Error::last_os_error()
            ));
        }
        if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } != 0 {
            return Err(format!(
                "could not make runtime {label} nonblocking: {}",
                std::io::Error::last_os_error()
            ));
        }
        let fd = tokio::io::unix::AsyncFd::new(fd)
            .map_err(|error| format!("runtime {label} is not pollable: {error}"))?;
        Ok(Self { fd })
    }

    async fn read(&self, buffer: &mut [u8]) -> Result<usize, String> {
        loop {
            let mut ready = self
                .fd
                .readable()
                .await
                .map_err(|error| format!("runtime lease input readiness failed: {error}"))?;
            let result = unsafe {
                libc::read(
                    self.fd.get_ref().as_raw_fd(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                )
            };
            if result >= 0 {
                return Ok(result as usize);
            }
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            if error.kind() == std::io::ErrorKind::WouldBlock {
                ready.clear_ready();
                continue;
            }
            return Err(format!("runtime lease input read failed: {error}"));
        }
    }

    async fn write_all(&self, mut bytes: &[u8]) -> Result<(), String> {
        while !bytes.is_empty() {
            let mut ready = self
                .fd
                .writable()
                .await
                .map_err(|error| format!("runtime lease output readiness failed: {error}"))?;
            let result = unsafe {
                libc::write(
                    self.fd.get_ref().as_raw_fd(),
                    bytes.as_ptr().cast(),
                    bytes.len(),
                )
            };
            if result > 0 {
                bytes = &bytes[result as usize..];
                continue;
            }
            if result == 0 {
                return Err("runtime lease output accepted no bytes".to_string());
            }
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            if error.kind() == std::io::ErrorKind::WouldBlock {
                ready.clear_ready();
                continue;
            }
            return Err(format!("runtime lease output write failed: {error}"));
        }
        Ok(())
    }
}

#[derive(Default)]
struct ChildRegistry {
    tracked: StdMutex<HashSet<u32>>,
}

struct ChildRegistration {
    registry: Arc<ChildRegistry>,
    pid: u32,
}

impl ChildRegistry {
    fn spawn(
        self: &Arc<Self>,
        command: &mut Command,
        label: &str,
    ) -> Result<(tokio::process::Child, ChildRegistration), String> {
        let mut tracked = self
            .tracked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let child = command
            .spawn()
            .map_err(|error| format!("could not start {label}: {error}"))?;
        let pid = child
            .id()
            .ok_or_else(|| format!("{label} has no process ID"))?;
        if !tracked.insert(pid) {
            return Err(format!("{label} reused an already tracked process ID"));
        }
        drop(tracked);
        Ok((
            child,
            ChildRegistration {
                registry: self.clone(),
                pid,
            },
        ))
    }

    fn reap_untracked(&self) -> bool {
        let tracked = self
            .tracked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Ok(tasks) = std::fs::read_dir("/proc/self/task") else {
            return false;
        };
        let mut children = HashSet::new();
        for task in tasks.flatten() {
            let path = task.path().join("children");
            let Ok(value) = std::fs::read_to_string(path) else {
                continue;
            };
            children.extend(
                value
                    .split_ascii_whitespace()
                    .filter_map(|pid| pid.parse::<u32>().ok()),
            );
        }
        let mut pending = false;
        for pid in children {
            if tracked.contains(&pid) || pid > i32::MAX as u32 {
                continue;
            }
            let mut status = 0;
            let result = unsafe { libc::waitpid(pid as i32, &mut status, libc::WNOHANG) };
            if result == pid as i32 {
                eprintln!("reaped detached runtime descendant {pid}");
            } else if result == 0 {
                pending = true;
            }
        }
        pending
    }
}

impl Drop for ChildRegistration {
    fn drop(&mut self) {
        self.registry
            .tracked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.pid);
    }
}

fn spawn_orphan_reaper(
    registry: Arc<ChildRegistry>,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tokio::time::sleep(Duration::from_millis(250)) => {
                    let _ = registry.reap_untracked();
                },
            }
        }
        registry.reap_untracked();
    })
}

struct ProcessGroupGuard {
    process_group: u32,
}

impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        signal_process_group(self.process_group, libc::SIGKILL);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PackageCommandSpec {
    program: PathBuf,
    args: Vec<String>,
    env: Vec<(String, String)>,
}

#[derive(Serialize)]
struct PackageAuditRecord<'a> {
    timestamp: String,
    runtime_id: &'a str,
    phase: &'a str,
    operation: &'a PackageOperation,
    packages: &'a [String],
    ok: Option<bool>,
    exit_code: Option<i32>,
    truncated: Option<bool>,
}

struct PackageAuditLog {
    runtime_id: String,
    file: Mutex<tokio::fs::File>,
    bytes: AtomicU64,
    healthy: AtomicBool,
    failed: CancellationToken,
}

impl PackageAuditLog {
    fn open(runtime_id: String) -> Result<Self, String> {
        prepare_root_directory(Path::new("/var/log/lethetic"), 0o700)?;
        let path = Path::new(PACKAGE_AUDIT_PATH);
        if let Ok(metadata) = std::fs::symlink_metadata(path)
            && (!metadata.is_file() || metadata.file_type().is_symlink() || metadata.uid() != 0)
        {
            return Err("package audit path is not a trusted root-owned regular file".to_string());
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .map_err(|error| format!("could not open package audit log: {error}"))?;
        let metadata = file
            .metadata()
            .map_err(|error| format!("could not inspect package audit log: {error}"))?;
        if !metadata.is_file() || metadata.uid() != 0 || metadata.nlink() != 1 {
            return Err("package audit log is not a singly linked root-owned file".to_string());
        }
        if metadata.len() >= PACKAGE_MAX_AUDIT_BYTES {
            return Err("package audit log reached its safety limit".to_string());
        }
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("could not secure package audit log: {error}"))?;
        Ok(Self {
            runtime_id,
            file: Mutex::new(tokio::fs::File::from_std(file)),
            bytes: AtomicU64::new(metadata.len()),
            healthy: AtomicBool::new(true),
            failed: CancellationToken::new(),
        })
    }

    fn mark_failed(&self) {
        self.healthy.store(false, Ordering::Release);
        self.failed.cancel();
    }

    async fn record(
        &self,
        phase: &str,
        operation: &PackageOperation,
        packages: &[String],
        ok: Option<bool>,
        exit_code: Option<i32>,
        truncated: Option<bool>,
    ) -> Result<(), String> {
        if !self.healthy.load(Ordering::Acquire) {
            return Err("package audit log is unhealthy".to_string());
        }
        let record = PackageAuditRecord {
            timestamp: chrono::Utc::now().to_rfc3339(),
            runtime_id: &self.runtime_id,
            phase,
            operation,
            packages,
            ok,
            exit_code,
            truncated,
        };
        let mut encoded = serde_json::to_vec(&record)
            .map_err(|error| format!("could not encode package audit record: {error}"))?;
        encoded.push(b'\n');
        let result: Result<(), String> = async {
            let mut file = self.file.lock().await;
            if !self.healthy.load(Ordering::Acquire) {
                return Err("package audit log became unhealthy".to_string());
            }
            let current = self.bytes.load(Ordering::Acquire);
            let next = current
                .checked_add(encoded.len() as u64)
                .ok_or_else(|| "package audit byte count overflowed".to_string())?;
            if next > PACKAGE_MAX_AUDIT_BYTES {
                return Err("package audit log reached its safety limit".to_string());
            }
            file.write_all(&encoded)
                .await
                .map_err(|error| format!("could not write package audit record: {error}"))?;
            file.flush()
                .await
                .map_err(|error| format!("could not flush package audit record: {error}"))?;
            file.sync_data()
                .await
                .map_err(|error| format!("could not sync package audit record: {error}"))?;
            self.bytes.store(next, Ordering::Release);
            Ok(())
        }
        .await;
        if let Err(error) = result {
            self.mark_failed();
            return Err(error);
        }
        Ok(())
    }
}

pub fn validate_runtime_image_installation() -> Result<(), String> {
    validate_trusted_runtime_file(Path::new(PYTHON_PATH), true)?;
    validate_trusted_runtime_file(Path::new(APT_GET_PATH), true)?;
    let executable = std::env::current_exe()
        .map_err(|error| format!("could not resolve runtime executable: {error}"))?;
    let metadata = executable
        .metadata()
        .map_err(|error| format!("could not inspect runtime executable: {error}"))?;
    if !metadata.is_file()
        || metadata.uid() != 0
        || metadata.permissions().mode() & 0o022 != 0
        || metadata.permissions().mode() & 0o111 == 0
    {
        return Err("runtime executable is not a trusted root-owned executable".to_string());
    }
    let helper = Path::new("/usr/local/bin/lethetic-pkg");
    let helper_metadata = std::fs::symlink_metadata(helper)
        .map_err(|error| format!("could not inspect package helper link: {error}"))?;
    if !helper_metadata.file_type().is_symlink()
        || helper_metadata.uid() != 0
        || helper
            .canonicalize()
            .map_err(|error| format!("could not resolve package helper link: {error}"))?
            != executable
    {
        return Err(
            "package helper does not resolve to the trusted runtime executable".to_string(),
        );
    }
    Ok(())
}

pub async fn run_supervisor(config: SupervisorConfig) -> Result<i32, String> {
    if !cfg!(target_os = "linux") {
        return Err("retained Python supervisor requires Linux".to_string());
    }
    if rustix::process::geteuid().as_raw() != 0
        || rustix::process::getpid() != rustix::process::Pid::INIT
    {
        return Err("retained Python supervisor must run as container root PID 1".to_string());
    }
    if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } != 0 {
        return Err(format!(
            "could not enable runtime descendant reaping: {}",
            std::io::Error::last_os_error()
        ));
    }
    config.validate()?;
    validate_trusted_runtime_file(Path::new(PYTHON_PATH), true)?;
    validate_trusted_runtime_file(Path::new(APT_GET_PATH), true)?;
    prepare_worker_home(config.worker_uid, config.worker_gid)?;
    let capability = read_capability(&config.capability_path)?;
    let shutdown = CancellationToken::new();
    let child_registry = Arc::new(ChildRegistry::default());
    let reaper_task = spawn_orphan_reaper(child_registry.clone(), shutdown.child_token());
    let received_signal = Arc::new(AtomicI32::new(0));
    let signal_task = spawn_signal_handler(shutdown.clone(), received_signal.clone());

    if let Err(error) = wait_for_broker(
        &config.broker_socket_path,
        &config.runtime_id,
        &capability,
        &shutdown,
    )
    .await
    {
        let signal = received_signal.load(Ordering::Acquire);
        shutdown.cancel();
        signal_task.abort();
        let _ = timeout(Duration::from_secs(2), reaper_task).await;
        if signal != 0 {
            return Ok(128 + signal);
        }
        return Err(error);
    }
    let broker_lease = match open_broker_readiness_connection(
        &config.broker_socket_path,
        &config.runtime_id,
        &capability,
    )
    .await
    {
        Ok(lease) => lease,
        Err(error) => {
            let signal = received_signal.load(Ordering::Acquire);
            shutdown.cancel();
            signal_task.abort();
            let _ = timeout(Duration::from_secs(2), reaper_task).await;
            if signal != 0 {
                return Ok(128 + signal);
            }
            return Err(format!(
                "could not establish the host broker lease: {error}"
            ));
        }
    };
    let mut broker_lease_task =
        tokio::spawn(monitor_broker_lease(broker_lease, shutdown.child_token()));

    let adapter_config = AdapterConfig {
        socket_path: config.broker_socket_path.clone(),
        runtime_id: config.runtime_id.clone(),
        capability,
        listen: SocketAddr::from((Ipv4Addr::LOCALHOST, DEFAULT_ADAPTER_PORT)),
        max_connections: 32,
    };
    let (adapter_ready_tx, adapter_ready_rx) = oneshot::channel();
    let adapter_shutdown = shutdown.child_token();
    let mut adapter_task = tokio::spawn(run_adapter_with_readiness(
        adapter_config,
        adapter_shutdown,
        adapter_ready_tx,
    ));

    let package_audit = Arc::new(PackageAuditLog::open(config.runtime_id.clone())?);
    let (package_ready_tx, package_ready_rx) = oneshot::channel();
    let package_shutdown = shutdown.child_token();
    let mut package_task = tokio::spawn(run_package_server(
        config.worker_uid,
        config.worker_gid,
        package_audit,
        child_registry.clone(),
        package_shutdown,
        package_ready_tx,
    ));

    let startup = async {
        wait_service_readiness(adapter_ready_rx, "egress adapter", &shutdown).await?;
        wait_service_readiness(package_ready_rx, "package supervisor", &shutdown).await?;
        Ok::<(), String>(())
    }
    .await;
    if let Err(error) = startup {
        let signal = received_signal.load(Ordering::Acquire);
        shutdown.cancel();
        adapter_task.abort();
        package_task.abort();
        broker_lease_task.abort();
        let _ = adapter_task.await;
        let _ = package_task.await;
        let _ = broker_lease_task.await;
        signal_task.abort();
        let _ = timeout(Duration::from_secs(2), reaper_task).await;
        if signal != 0 {
            return Ok(128 + signal);
        }
        return Err(error);
    }

    let (mut worker, worker_registration) = spawn_worker_process(&config, &child_registry)?;
    let worker_process_group = worker
        .id()
        .ok_or_else(|| "runtime worker has no process ID".to_string())?;
    let worker_stdin = worker
        .stdin
        .take()
        .ok_or_else(|| "runtime worker has no stdin".to_string())?;
    let worker_stdout = worker
        .stdout
        .take()
        .ok_or_else(|| "runtime worker has no stdout".to_string())?;
    let lease_input = AsyncProcessFd::duplicate(libc::STDIN_FILENO, "stdin")?;
    let lease_output = AsyncProcessFd::duplicate(libc::STDOUT_FILENO, "stdout")?;
    let mut input_task = tokio::spawn(proxy_stdin(worker_stdin, lease_input));
    let mut output_task = tokio::spawn(proxy_stdout(worker_stdout, lease_output));
    let mut input_task_joined = false;
    let mut output_task_joined = false;
    let mut broker_lease_task_joined = false;

    let exit = tokio::select! {
        biased;
        _ = shutdown.cancelled() => {
            let signal = received_signal.load(Ordering::Acquire);
            if signal == 0 {
                SupervisorExit::Infrastructure("runtime shutdown was requested without a signal".to_string())
            } else {
                SupervisorExit::Signal(signal)
            }
        }
        status = worker.wait() => {
            SupervisorExit::Worker(status.map_err(|error| format!("could not wait for runtime worker: {error}"))?)
        }
        result = &mut input_task => {
            input_task_joined = true;
            let detail = match result {
                Ok(Ok(())) => "runtime attach input reached EOF".to_string(),
                Ok(Err(error)) => error,
                Err(error) => format!("runtime input proxy failed: {error}"),
            };
            if detail.contains("EOF") {
                SupervisorExit::LeaseEnded
            } else {
                SupervisorExit::Infrastructure(detail)
            }
        }
        result = &mut output_task => {
            output_task_joined = true;
            classify_output_task_exit(result)
        }
        result = &mut broker_lease_task => {
            broker_lease_task_joined = true;
            SupervisorExit::Infrastructure(match result {
                Ok(Ok(())) => "host broker lease ended unexpectedly".to_string(),
                Ok(Err(error)) => error,
                Err(error) => format!("host broker lease task failed: {error}"),
            })
        }
        result = &mut adapter_task => {
            SupervisorExit::Infrastructure(match result {
                Ok(Ok(())) => "egress adapter stopped unexpectedly".to_string(),
                Ok(Err(error)) => error,
                Err(error) => format!("egress adapter task failed: {error}"),
            })
        }
        result = &mut package_task => {
            SupervisorExit::Infrastructure(match result {
                Ok(Ok(())) => "package supervisor stopped unexpectedly".to_string(),
                Ok(Err(error)) => error,
                Err(error) => format!("package supervisor task failed: {error}"),
            })
        }
    };

    let _worker_status = match &exit {
        SupervisorExit::Worker(status) => {
            signal_process_group(worker_process_group, libc::SIGKILL);
            Some(*status)
        }
        SupervisorExit::LeaseEnded => {
            terminate_worker_group(
                &mut worker,
                worker_process_group,
                libc::SIGTERM,
                Duration::from_secs(5),
            )
            .await
        }
        SupervisorExit::Signal(signal) => {
            terminate_worker_group(
                &mut worker,
                worker_process_group,
                *signal,
                Duration::from_secs(5),
            )
            .await
        }
        SupervisorExit::Infrastructure(_) => {
            terminate_worker_group(
                &mut worker,
                worker_process_group,
                libc::SIGTERM,
                Duration::from_secs(2),
            )
            .await
        }
    };
    drop(worker_registration);
    shutdown.cancel();
    if !input_task_joined {
        input_task.abort();
        let _ = input_task.await;
    }
    if !output_task_joined
        && timeout(Duration::from_secs(5), &mut output_task)
            .await
            .is_err()
    {
        output_task.abort();
        let _ = output_task.await;
    }
    if !broker_lease_task_joined
        && timeout(Duration::from_secs(2), &mut broker_lease_task)
            .await
            .is_err()
    {
        broker_lease_task.abort();
        let _ = broker_lease_task.await;
    }
    if !adapter_task.is_finished()
        && timeout(Duration::from_secs(5), &mut adapter_task)
            .await
            .is_err()
    {
        adapter_task.abort();
        let _ = adapter_task.await;
    }
    if !package_task.is_finished()
        && timeout(Duration::from_secs(5), &mut package_task)
            .await
            .is_err()
    {
        package_task.abort();
        let _ = package_task.await;
    }
    signal_task.abort();
    let _ = timeout(Duration::from_secs(2), reaper_task).await;
    for _ in 0..20 {
        if !child_registry.reap_untracked() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    match exit {
        SupervisorExit::Worker(status) => Ok(status.code().unwrap_or(1)),
        SupervisorExit::LeaseEnded => Ok(0),
        SupervisorExit::Signal(signal) => Ok(128 + signal),
        SupervisorExit::Infrastructure(error) => Err(error),
    }
}

async fn wait_service_readiness<T>(
    receiver: oneshot::Receiver<T>,
    label: &str,
    shutdown: &CancellationToken,
) -> Result<T, String> {
    tokio::select! {
        biased;
        _ = shutdown.cancelled() => Err(format!("{label} startup was cancelled")),
        result = timeout(SERVICE_STARTUP_TIMEOUT, receiver) => result
            .map_err(|_| format!("{label} startup timed out"))?
            .map_err(|_| format!("{label} exited before readiness")),
    }
}

fn signal_process_group(process_group: u32, signal: i32) {
    if process_group > i32::MAX as u32 {
        return;
    }
    let result = unsafe { libc::kill(-(process_group as i32), signal) };
    if result != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            eprintln!("could not signal runtime worker process group: {error}");
        }
    }
}

async fn terminate_worker_group(
    worker: &mut tokio::process::Child,
    process_group: u32,
    signal: i32,
    grace: Duration,
) -> Option<std::process::ExitStatus> {
    signal_process_group(process_group, signal);
    match timeout(grace, worker.wait()).await {
        Ok(Ok(status)) => {
            signal_process_group(process_group, libc::SIGKILL);
            Some(status)
        }
        Ok(Err(error)) => {
            eprintln!("could not wait for signalled runtime worker: {error}");
            signal_process_group(process_group, libc::SIGKILL);
            let _ = worker.start_kill();
            None
        }
        Err(_) => {
            signal_process_group(process_group, libc::SIGKILL);
            let _ = worker.start_kill();
            timeout(Duration::from_secs(2), worker.wait())
                .await
                .ok()
                .and_then(Result::ok)
        }
    }
}

async fn monitor_broker_lease(
    mut lease: UnixStream,
    cancel: CancellationToken,
) -> Result<(), String> {
    let mut unexpected = [0_u8; 1];
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Ok(()),
        result = lease.read(&mut unexpected) => match result {
            Ok(0) => Err("host broker lease closed".to_string()),
            Ok(_) => Err("host broker lease returned unexpected data".to_string()),
            Err(error) => Err(format!("host broker lease failed: {error}")),
        },
    }
}

async fn wait_for_broker(
    socket_path: &Path,
    runtime_id: &str,
    capability: &str,
    cancel: &CancellationToken,
) -> Result<(), String> {
    let start = tokio::time::Instant::now();
    let mut last_error = "broker is not ready".to_string();
    loop {
        if start.elapsed() >= BROKER_STARTUP_TIMEOUT {
            return Err(format!("egress broker startup timed out: {last_error}"));
        }
        tokio::select! {
            _ = cancel.cancelled() => return Err("runtime cancelled while waiting for egress broker".to_string()),
            result = probe_broker(socket_path, runtime_id, capability) => match result {
                Ok(()) => return Ok(()),
                Err(error) => last_error = error,
            },
        }
        tokio::select! {
            _ = cancel.cancelled() => return Err("runtime cancelled while waiting for egress broker".to_string()),
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }
}

async fn run_package_server(
    worker_uid: u32,
    worker_gid: u32,
    audit: Arc<PackageAuditLog>,
    child_registry: Arc<ChildRegistry>,
    cancel: CancellationToken,
    ready: oneshot::Sender<()>,
) -> Result<(), String> {
    prepare_package_socket_parent(worker_gid)?;
    let socket_path = Path::new(PACKAGE_SOCKET_PATH);
    remove_stale_root_socket(socket_path)?;
    let listener = UnixListener::bind(socket_path)
        .map_err(|error| format!("could not bind package supervisor socket: {error}"))?;
    secure_package_socket_path(socket_path, worker_gid)?;
    ready
        .send(())
        .map_err(|_| "package supervisor readiness receiver was dropped".to_string())?;
    let transaction = Arc::new(Mutex::new(()));
    let semaphore = Arc::new(Semaphore::new(4));
    let mut handlers = tokio::task::JoinSet::new();
    let mut fatal_error = None;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = audit.failed.cancelled() => {
                fatal_error = Some("package audit sink failed".to_string());
                break;
            }
            joined = handlers.join_next(), if !handlers.is_empty() => {
                if joined.is_some_and(|result| result.is_err()) {
                    fatal_error = Some("package protocol handler panicked".to_string());
                    break;
                }
            }
            accepted = listener.accept() => {
                let (stream, _) = match accepted {
                    Ok(accepted) => accepted,
                    Err(error) => {
                        fatal_error = Some(format!("package supervisor accept failed: {error}"));
                        break;
                    }
                };
                let permit = match semaphore.clone().try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        drop(stream);
                        continue;
                    }
                };
                let transaction = transaction.clone();
                let audit = audit.clone();
                let child_registry = child_registry.clone();
                handlers.spawn(async move {
                    let _permit = permit;
                    let _ = handle_package_request(
                        stream,
                        worker_uid,
                        worker_gid,
                        transaction,
                        audit,
                        child_registry,
                    )
                    .await;
                });
            }
        }
    }
    handlers.abort_all();
    while handlers.join_next().await.is_some() {}
    let _ = remove_socket_if_owned(socket_path);
    match fatal_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

async fn handle_package_request(
    mut stream: UnixStream,
    worker_uid: u32,
    worker_gid: u32,
    transaction: Arc<Mutex<()>>,
    audit: Arc<PackageAuditLog>,
    child_registry: Arc<ChildRegistry>,
) -> Result<(), String> {
    let credentials = stream
        .peer_cred()
        .map_err(|error| format!("could not read package client credentials: {error}"))?;
    if credentials.uid() != worker_uid
        || credentials.gid() != worker_gid
        || credentials.pid().is_none()
    {
        return Err("package client credentials do not match the runtime worker".to_string());
    }
    let request = timeout(
        PACKAGE_REQUEST_TIMEOUT,
        read_package_frame::<_, PackageRequest>(&mut stream),
    )
    .await
    .map_err(|_| "package request timed out".to_string())??;
    validate_package_request(&request)?;
    let _transaction = transaction.lock().await;
    audit
        .record(
            "started",
            &request.operation,
            &request.packages,
            None,
            None,
            None,
        )
        .await?;
    let response = run_package_command(&request, &child_registry).await;
    audit
        .record(
            "finished",
            &request.operation,
            &request.packages,
            Some(response.ok),
            response.exit_code,
            Some(response.truncated),
        )
        .await?;
    write_package_frame(&mut stream, &response).await
}

fn validate_package_request(request: &PackageRequest) -> Result<(), String> {
    if request.version != PACKAGE_PROTOCOL_VERSION {
        return Err("package protocol version mismatch".to_string());
    }
    match request.operation {
        PackageOperation::Refresh if !request.packages.is_empty() => {
            return Err("package refresh does not accept package names".to_string());
        }
        PackageOperation::Install
            if request.packages.is_empty() || request.packages.len() > PACKAGE_MAX_NAMES =>
        {
            return Err("package install requires between 1 and 32 names".to_string());
        }
        _ => {}
    }
    let mut unique = HashSet::new();
    for package in &request.packages {
        validate_package_name(package)?;
        if !unique.insert(package) {
            return Err("duplicate package names are not accepted".to_string());
        }
    }
    Ok(())
}

pub fn validate_package_name(package: &str) -> Result<(), String> {
    if package.is_empty()
        || package.len() > PACKAGE_MAX_NAME_BYTES
        || package != package.to_ascii_lowercase()
        || package.starts_with(['-', '.', '+'])
        || !package.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'+' | b'-' | b'.')
        })
        || !package
            .bytes()
            .last()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'+')
    {
        return Err(format!("invalid Debian package name '{package}'"));
    }
    Ok(())
}

fn package_command_spec(request: &PackageRequest) -> Result<PackageCommandSpec, String> {
    validate_package_request(request)?;
    let proxy = format!("http://127.0.0.1:{DEFAULT_ADAPTER_PORT}");
    let mut args = vec![
        "-o".to_string(),
        format!("Acquire::http::Proxy={proxy}"),
        "-o".to_string(),
        format!("Acquire::https::Proxy={proxy}"),
        "-o".to_string(),
        "Acquire::ftp::Proxy=false".to_string(),
        "-o".to_string(),
        "Acquire::Retries=2".to_string(),
        "-o".to_string(),
        "Acquire::http::Timeout=30".to_string(),
        "-o".to_string(),
        "Acquire::https::Timeout=30".to_string(),
        "-o".to_string(),
        "Dpkg::Use-Pty=0".to_string(),
    ];
    match request.operation {
        PackageOperation::Refresh => args.push("update".to_string()),
        PackageOperation::Install => {
            args.extend([
                "install".to_string(),
                "--yes".to_string(),
                "--no-install-recommends".to_string(),
                "--".to_string(),
            ]);
            args.extend(request.packages.iter().cloned());
        }
    }
    Ok(PackageCommandSpec {
        program: PathBuf::from(APT_GET_PATH),
        args,
        env: root_package_environment(),
    })
}

async fn run_package_command(
    request: &PackageRequest,
    child_registry: &Arc<ChildRegistry>,
) -> PackageResponse {
    let spec = match package_command_spec(request) {
        Ok(spec) => spec,
        Err(error) => {
            return PackageResponse {
                version: PACKAGE_PROTOCOL_VERSION,
                ok: false,
                exit_code: None,
                output: error,
                truncated: false,
            };
        }
    };
    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .env_clear()
        .envs(spec.env.iter().cloned())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0);
    let (mut child, _registration) = match child_registry.spawn(&mut command, "apt-get") {
        Ok(child) => child,
        Err(error) => {
            return PackageResponse {
                version: PACKAGE_PROTOCOL_VERSION,
                ok: false,
                exit_code: None,
                output: format!("could not start apt-get: {error}"),
                truncated: false,
            };
        }
    };
    let process_group = match child.id() {
        Some(pid) => pid,
        None => return failed_package_response("apt-get has no process ID"),
    };
    let _process_group_guard = ProcessGroupGuard { process_group };
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let Some(stdout) = stdout else {
        signal_process_group(process_group, libc::SIGKILL);
        return failed_package_response("apt-get has no stdout");
    };
    let Some(stderr) = stderr else {
        signal_process_group(process_group, libc::SIGKILL);
        return failed_package_response("apt-get has no stderr");
    };
    let outcome = timeout(PACKAGE_TRANSACTION_TIMEOUT, async {
        let stdout = drain_bounded(stdout, PACKAGE_MAX_OUTPUT_BYTES);
        let stderr = drain_bounded(stderr, PACKAGE_MAX_OUTPUT_BYTES);
        let wait = child.wait();
        tokio::try_join!(async { stdout.await }, async { stderr.await }, async {
            wait.await
                .map_err(|error| format!("could not wait for apt-get: {error}"))
        })
    })
    .await;
    let ((stdout, stdout_truncated), (stderr, stderr_truncated), status) = match outcome {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(error)) => {
            signal_process_group(process_group, libc::SIGKILL);
            let _ = child.start_kill();
            let _ = child.wait().await;
            return failed_package_response(&error);
        }
        Err(_) => {
            signal_process_group(process_group, libc::SIGTERM);
            let stopped = matches!(
                timeout(Duration::from_secs(5), child.wait()).await,
                Ok(Ok(_))
            );
            if !stopped {
                signal_process_group(process_group, libc::SIGKILL);
                let _ = child.start_kill();
                let _ = child.wait().await;
            }
            return failed_package_response("apt-get transaction timed out");
        }
    };
    signal_process_group(process_group, libc::SIGKILL);
    let mut output = String::new();
    if !stdout.is_empty() {
        output.push_str(&String::from_utf8_lossy(&stdout));
    }
    if !stderr.is_empty() {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&String::from_utf8_lossy(&stderr));
    }
    PackageResponse {
        version: PACKAGE_PROTOCOL_VERSION,
        ok: status.success(),
        exit_code: status.code(),
        output,
        truncated: stdout_truncated || stderr_truncated,
    }
}

fn failed_package_response(message: &str) -> PackageResponse {
    PackageResponse {
        version: PACKAGE_PROTOCOL_VERSION,
        ok: false,
        exit_code: None,
        output: message.to_string(),
        truncated: false,
    }
}

async fn drain_bounded<R>(mut reader: R, maximum: usize) -> Result<(Vec<u8>, bool), String>
where
    R: AsyncRead + Unpin,
{
    let mut kept = Vec::new();
    let mut buffer = [0_u8; 16 * 1024];
    let mut truncated = false;
    loop {
        let read = reader
            .read(&mut buffer)
            .await
            .map_err(|error| format!("could not read package-manager output: {error}"))?;
        if read == 0 {
            return Ok((kept, truncated));
        }
        let remaining = maximum.saturating_sub(kept.len());
        let keep = remaining.min(read);
        kept.extend_from_slice(&buffer[..keep]);
        truncated |= keep < read;
    }
}

pub async fn run_package_client(
    operation: PackageOperation,
    packages: Vec<String>,
) -> Result<PackageResponse, String> {
    let request = PackageRequest {
        version: PACKAGE_PROTOCOL_VERSION,
        operation,
        packages,
    };
    validate_package_request(&request)?;
    let mut stream = UnixStream::connect(PACKAGE_SOCKET_PATH)
        .await
        .map_err(|error| format!("could not connect to package supervisor: {error}"))?;
    write_package_frame(&mut stream, &request).await?;
    timeout(
        PACKAGE_TRANSACTION_TIMEOUT + Duration::from_secs(30),
        read_package_frame::<_, PackageResponse>(&mut stream),
    )
    .await
    .map_err(|_| "package supervisor response timed out".to_string())?
}

fn spawn_worker_process(
    config: &SupervisorConfig,
    child_registry: &Arc<ChildRegistry>,
) -> Result<(tokio::process::Child, ChildRegistration), String> {
    let mut command = Command::new("/proc/self/exe");
    command
        .arg("worker")
        .arg(config.worker_uid.to_string())
        .arg(config.worker_gid.to_string())
        .arg(&config.workspace)
        .env_clear()
        .envs(worker_environment())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .process_group(0);
    child_registry.spawn(&mut command, "unprivileged runtime worker")
}

pub fn worker_environment() -> Vec<(String, String)> {
    let proxy = format!("http://127.0.0.1:{DEFAULT_ADAPTER_PORT}");
    vec![
        (
            "PATH".to_string(),
            "/usr/local/bin:/usr/bin:/bin".to_string(),
        ),
        ("HOME".to_string(), WORKER_HOME.to_string()),
        ("USER".to_string(), "lethetic".to_string()),
        ("LOGNAME".to_string(), "lethetic".to_string()),
        ("LANG".to_string(), "C.UTF-8".to_string()),
        ("PYTHONDONTWRITEBYTECODE".to_string(), "1".to_string()),
        ("PYTHONUNBUFFERED".to_string(), "1".to_string()),
        ("PIP_DISABLE_PIP_VERSION_CHECK".to_string(), "1".to_string()),
        ("CARGO_HOME".to_string(), format!("{WORKER_HOME}/.cargo")),
        ("RUSTUP_HOME".to_string(), format!("{WORKER_HOME}/.rustup")),
        ("HTTP_PROXY".to_string(), proxy.clone()),
        ("http_proxy".to_string(), proxy.clone()),
        ("HTTPS_PROXY".to_string(), proxy.clone()),
        ("https_proxy".to_string(), proxy),
        ("NO_PROXY".to_string(), String::new()),
        ("no_proxy".to_string(), String::new()),
    ]
}

fn root_package_environment() -> Vec<(String, String)> {
    vec![
        (
            "PATH".to_string(),
            "/usr/sbin:/usr/bin:/sbin:/bin".to_string(),
        ),
        ("HOME".to_string(), "/root".to_string()),
        ("LANG".to_string(), "C.UTF-8".to_string()),
        ("LC_ALL".to_string(), "C.UTF-8".to_string()),
        ("DEBIAN_FRONTEND".to_string(), "noninteractive".to_string()),
    ]
}

async fn proxy_stdin(
    mut worker: tokio::process::ChildStdin,
    input: AsyncProcessFd,
) -> Result<(), String> {
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = input.read(&mut buffer).await?;
        if read == 0 {
            worker
                .shutdown()
                .await
                .map_err(|error| format!("runtime worker stdin shutdown failed: {error}"))?;
            return Err("runtime attach input reached EOF".to_string());
        }
        worker
            .write_all(&buffer[..read])
            .await
            .map_err(|error| format!("runtime input proxy failed: {error}"))?;
    }
}

async fn proxy_stdout(
    mut worker: tokio::process::ChildStdout,
    output: AsyncProcessFd,
) -> Result<(), String> {
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = worker
            .read(&mut buffer)
            .await
            .map_err(|error| format!("runtime output proxy failed: {error}"))?;
        if read == 0 {
            return Ok(());
        }
        output.write_all(&buffer[..read]).await?;
    }
}

fn spawn_signal_handler(
    cancel: CancellationToken,
    received: Arc<AtomicI32>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        use tokio::signal::unix::{SignalKind, signal};
        let Ok(mut terminate) = signal(SignalKind::terminate()) else {
            cancel.cancel();
            return;
        };
        let Ok(mut interrupt) = signal(SignalKind::interrupt()) else {
            cancel.cancel();
            return;
        };
        let first = tokio::select! {
            _ = terminate.recv() => libc::SIGTERM,
            _ = interrupt.recv() => libc::SIGINT,
        };
        received.store(first, Ordering::Release);
        cancel.cancel();

        let _second = tokio::select! {
            _ = terminate.recv() => libc::SIGTERM,
            _ = interrupt.recv() => libc::SIGINT,
        };
        unsafe { libc::_exit(128 + first) }
    })
}

pub fn exec_worker_process(uid: u32, gid: u32, workspace: &Path) -> Result<(), String> {
    if !cfg!(target_os = "linux") {
        return Err("runtime worker privilege drop requires Linux".to_string());
    }
    if uid == 0 || gid == 0 || rustix::process::geteuid().as_raw() != 0 {
        return Err("runtime worker privilege transition requires container root".to_string());
    }
    if rustix::process::getppid() != Some(rustix::process::Pid::INIT) {
        return Err("runtime worker must be launched directly by supervisor PID 1".to_string());
    }
    let workspace = workspace
        .canonicalize()
        .map_err(|error| format!("could not canonicalize runtime worker workspace: {error}"))?;
    if !workspace.is_dir() {
        return Err("runtime worker workspace is not a directory".to_string());
    }
    validate_trusted_runtime_file(Path::new(PYTHON_PATH), true)?;

    let empty_groups = unsafe { libc::setgroups(0, std::ptr::null()) };
    if empty_groups != 0 {
        return Err(format!(
            "could not clear runtime worker supplementary groups: {}",
            std::io::Error::last_os_error()
        ));
    }
    for capability in 0..=63 {
        let result = unsafe { libc::prctl(libc::PR_CAPBSET_DROP, capability, 0, 0, 0) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EINVAL) {
                return Err(format!(
                    "could not drop worker bounding capability {capability}: {error}"
                ));
            }
        }
    }
    let _ = unsafe {
        libc::prctl(
            libc::PR_CAP_AMBIENT,
            libc::PR_CAP_AMBIENT_CLEAR_ALL,
            0,
            0,
            0,
        )
    };
    if unsafe { libc::setresgid(gid, gid, gid) } != 0 {
        return Err(format!(
            "could not set runtime worker GID: {}",
            std::io::Error::last_os_error()
        ));
    }
    if unsafe { libc::setresuid(uid, uid, uid) } != 0 {
        return Err(format!(
            "could not set runtime worker UID: {}",
            std::io::Error::last_os_error()
        ));
    }
    clear_process_capabilities()?;
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
        return Err(format!(
            "could not set no-new-privileges for runtime worker: {}",
            std::io::Error::last_os_error()
        ));
    }
    unsafe { libc::umask(0o077) };
    std::env::set_current_dir(&workspace)
        .map_err(|error| format!("could not enter runtime worker workspace: {error}"))?;
    verify_worker_security_status(uid, gid)?;

    let error = std::process::Command::new(PYTHON_PATH)
        .args(["-u", "-B", "-c", WORKER_SOURCE])
        .exec();
    Err(format!("could not exec Python runtime worker: {error}"))
}

fn clear_process_capabilities() -> Result<(), String> {
    #[repr(C)]
    struct CapabilityHeader {
        version: u32,
        pid: i32,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CapabilityData {
        effective: u32,
        permitted: u32,
        inheritable: u32,
    }
    let mut header = CapabilityHeader {
        version: 0x2008_0522,
        pid: 0,
    };
    let mut data = [
        CapabilityData {
            effective: 0,
            permitted: 0,
            inheritable: 0,
        },
        CapabilityData {
            effective: 0,
            permitted: 0,
            inheritable: 0,
        },
    ];
    let result = unsafe {
        libc::syscall(
            libc::SYS_capset,
            &mut header as *mut CapabilityHeader,
            data.as_mut_ptr(),
        )
    };
    if result != 0 {
        return Err(format!(
            "could not clear runtime worker capabilities: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

fn verify_worker_security_status(uid: u32, gid: u32) -> Result<(), String> {
    if rustix::process::geteuid().as_raw() != uid || rustix::process::getegid().as_raw() != gid {
        return Err("runtime worker credential verification failed".to_string());
    }
    let status = std::fs::read_to_string("/proc/self/status")
        .map_err(|error| format!("could not inspect runtime worker security status: {error}"))?;
    for field in ["CapInh:", "CapPrm:", "CapEff:", "CapAmb:"] {
        let value = status
            .lines()
            .find_map(|line| line.strip_prefix(field))
            .map(str::trim)
            .ok_or_else(|| format!("runtime worker status is missing {field}"))?;
        if value != "0000000000000000" {
            return Err(format!("runtime worker retained capabilities in {field}"));
        }
    }
    let no_new_privs = status
        .lines()
        .find_map(|line| line.strip_prefix("NoNewPrivs:"))
        .map(str::trim);
    if no_new_privs != Some("1") {
        return Err("runtime worker no-new-privileges verification failed".to_string());
    }
    Ok(())
}

fn read_capability(path: &Path) -> Result<String, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect broker capability file: {error}"))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 256 {
        return Err("broker capability file is not a small regular file".to_string());
    }
    let capability = std::fs::read_to_string(path)
        .map_err(|error| format!("could not read broker capability file: {error}"))?;
    Ok(capability.trim().to_string())
}

fn prepare_worker_home(uid: u32, gid: u32) -> Result<(), String> {
    let path = Path::new(WORKER_HOME);
    if let Ok(metadata) = std::fs::symlink_metadata(path)
        && (!metadata.is_dir() || metadata.file_type().is_symlink())
    {
        return Err("runtime worker home is not a trusted directory".to_string());
    }
    std::fs::create_dir_all(path)
        .map_err(|error| format!("could not create runtime worker home: {error}"))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("could not secure runtime worker home: {error}"))?;
    if unsafe { libc::chown(path_to_c_string(path)?.as_ptr(), uid, gid) } != 0 {
        return Err(format!(
            "could not assign runtime worker home ownership: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

fn secure_package_socket_path(path: &Path, worker_gid: u32) -> Result<(), String> {
    let c_path = path_to_c_string(path)?;
    if unsafe { libc::lchown(c_path.as_ptr(), 0, worker_gid) } != 0 {
        return Err(format!(
            "could not assign package supervisor socket group: {}",
            std::io::Error::last_os_error()
        ));
    }
    if unsafe { libc::chmod(c_path.as_ptr(), 0o660) } != 0 {
        return Err(format!(
            "could not secure package supervisor socket: {}",
            std::io::Error::last_os_error()
        ));
    }
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("could not verify package supervisor socket: {error}"))?;
    if !metadata.file_type().is_socket()
        || metadata.file_type().is_symlink()
        || metadata.uid() != 0
        || metadata.gid() != worker_gid
        || metadata.nlink() != 1
        || metadata.permissions().mode() & 0o777 != 0o660
    {
        return Err("package supervisor socket pathname metadata is unsafe".to_string());
    }
    Ok(())
}

fn prepare_package_socket_parent(worker_gid: u32) -> Result<(), String> {
    let path = Path::new("/run/lethetic-pkg");
    prepare_root_directory(path, 0o750)?;
    if unsafe { libc::chown(path_to_c_string(path)?.as_ptr(), 0, worker_gid) } != 0 {
        return Err(format!(
            "could not assign package socket directory group: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

fn prepare_root_directory(path: &Path, mode: u32) -> Result<(), String> {
    if let Ok(metadata) = std::fs::symlink_metadata(path)
        && (!metadata.is_dir() || metadata.file_type().is_symlink() || metadata.uid() != 0)
    {
        return Err(format!(
            "{} is not a trusted root-owned directory",
            path.display()
        ));
    }
    std::fs::create_dir_all(path)
        .map_err(|error| format!("could not create {}: {error}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|error| format!("could not secure {}: {error}", path.display()))
}

fn validate_trusted_runtime_file(path: &Path, executable: bool) -> Result<(), String> {
    let link_metadata = std::fs::symlink_metadata(path).map_err(|error| {
        format!(
            "could not inspect trusted runtime file {}: {error}",
            path.display()
        )
    })?;
    if !link_metadata.is_file() && !link_metadata.file_type().is_symlink() {
        return Err(format!("untrusted runtime file {}", path.display()));
    }
    if link_metadata.uid() != 0
        || (!link_metadata.file_type().is_symlink()
            && link_metadata.permissions().mode() & 0o022 != 0)
    {
        return Err(format!("untrusted runtime file {}", path.display()));
    }
    if link_metadata.file_type().is_symlink() && !executable {
        return Err(format!(
            "trusted runtime data file may not be a symlink: {}",
            path.display()
        ));
    }
    let canonical = path.canonicalize().map_err(|error| {
        format!(
            "could not resolve trusted runtime file {}: {error}",
            path.display()
        )
    })?;
    if executable && !(canonical.starts_with("/usr/bin") || canonical.starts_with("/usr/local/bin"))
    {
        return Err(format!(
            "runtime executable resolved outside trusted system paths: {}",
            path.display()
        ));
    }
    let metadata = canonical.metadata().map_err(|error| {
        format!(
            "could not inspect resolved runtime file {}: {error}",
            canonical.display()
        )
    })?;
    if !metadata.is_file()
        || metadata.uid() != 0
        || metadata.permissions().mode() & 0o022 != 0
        || (executable && metadata.permissions().mode() & 0o111 == 0)
    {
        return Err(format!("untrusted runtime file {}", path.display()));
    }
    Ok(())
}

fn remove_stale_root_socket(path: &Path) -> Result<(), String> {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return Ok(());
    };
    if !metadata.file_type().is_socket() || metadata.uid() != 0 {
        return Err("stale package control path is not a root-owned socket".to_string());
    }
    std::fs::remove_file(path)
        .map_err(|error| format!("could not remove stale package control socket: {error}"))
}

fn remove_socket_if_owned(path: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect package socket during cleanup: {error}"))?;
    if metadata.file_type().is_socket() && metadata.uid() == 0 {
        std::fs::remove_file(path)
            .map_err(|error| format!("could not remove package socket: {error}"))?;
    }
    Ok(())
}

fn path_to_c_string(path: &Path) -> Result<std::ffi::CString, String> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| format!("path contains NUL: {}", path.display()))
}

async fn read_package_frame<R, T>(reader: &mut R) -> Result<T, String>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let length = reader
        .read_u32()
        .await
        .map_err(|error| format!("could not read package frame length: {error}"))?
        as usize;
    if length == 0 || length > PACKAGE_MAX_FRAME_BYTES {
        return Err("package frame length is invalid".to_string());
    }
    let mut payload = vec![0_u8; length];
    reader
        .read_exact(&mut payload)
        .await
        .map_err(|error| format!("could not read package frame: {error}"))?;
    serde_json::from_slice(&payload).map_err(|error| format!("invalid package frame JSON: {error}"))
}

async fn write_package_frame<W, T>(writer: &mut W, value: &T) -> Result<(), String>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let payload = serde_json::to_vec(value)
        .map_err(|error| format!("could not encode package frame: {error}"))?;
    if payload.is_empty() || payload.len() > PACKAGE_MAX_FRAME_BYTES {
        return Err("encoded package frame is too large".to_string());
    }
    writer
        .write_u32(payload.len() as u32)
        .await
        .map_err(|error| format!("could not write package frame length: {error}"))?;
    writer
        .write_all(&payload)
        .await
        .map_err(|error| format!("could not write package frame: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(operation: PackageOperation, packages: &[&str]) -> PackageRequest {
        PackageRequest {
            version: PACKAGE_PROTOCOL_VERSION,
            operation,
            packages: packages
                .iter()
                .map(|package| (*package).to_string())
                .collect(),
        }
    }

    #[tokio::test]
    async fn protocol_output_eof_terminates_a_still_running_worker_group() {
        use tokio::io::AsyncReadExt;

        if !crate::platform::binary_on_path("python3") {
            return;
        }
        let mut child = Command::new("python3");
        child
            .arg("-c")
            .arg("import os, time; os.close(1); time.sleep(60)")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .process_group(0);
        let mut child = child.spawn().unwrap();
        let process_group = child.id().unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let output_task = tokio::spawn(async move {
            let mut output = Vec::new();
            stdout
                .read_to_end(&mut output)
                .await
                .map(|_| ())
                .map_err(|error| error.to_string())
        });
        let joined = timeout(Duration::from_secs(2), output_task)
            .await
            .expect("worker did not close protocol output");
        assert!(
            child.try_wait().unwrap().is_none(),
            "worker unexpectedly exited"
        );
        let exit = classify_output_task_exit(joined);
        assert!(matches!(
            exit,
            SupervisorExit::Infrastructure(ref error)
                if error.contains("protocol output reached EOF")
        ));

        let status = terminate_worker_group(
            &mut child,
            process_group,
            libc::SIGTERM,
            Duration::from_secs(1),
        )
        .await;
        assert!(
            status.is_some(),
            "worker group did not terminate within the bound"
        );
    }

    #[test]
    fn package_names_reject_options_urls_paths_and_shell_syntax() {
        for package in [
            "-o",
            "--allow-unauthenticated",
            "curl=https://evil",
            "https://evil/pkg.deb",
            "../pkg",
            "pkg/other",
            "pkg:amd64",
            "pkg;id",
            "pkg$(id)",
            "Pkg",
            ".hidden",
            "+feature",
            "pkg_underscore",
            "pkg ",
            "",
        ] {
            assert!(validate_package_name(package).is_err(), "{package:?}");
        }
        for package in ["rustc", "cargo", "libssl-dev", "python3.13", "g++"] {
            assert!(validate_package_name(package).is_ok(), "{package}");
        }
    }

    #[test]
    fn package_protocol_enforces_operation_shapes_and_duplicates() {
        assert!(validate_package_request(&request(PackageOperation::Refresh, &[])).is_ok());
        assert!(validate_package_request(&request(PackageOperation::Refresh, &["rustc"])).is_err());
        assert!(validate_package_request(&request(PackageOperation::Install, &[])).is_err());
        assert!(
            validate_package_request(&request(PackageOperation::Install, &["rustc", "rustc"]))
                .is_err()
        );
    }

    #[test]
    fn apt_command_is_fixed_and_proxy_only() {
        let spec =
            package_command_spec(&request(PackageOperation::Install, &["rustc", "cargo"])).unwrap();
        assert_eq!(spec.program, PathBuf::from(APT_GET_PATH));
        assert!(spec.args.iter().any(|arg| arg == "install"));
        assert!(spec.args.iter().any(|arg| arg == "--"));
        assert!(
            spec.args
                .ends_with(&["rustc".to_string(), "cargo".to_string()])
        );
        let joined = spec.args.join(" ");
        assert!(joined.contains("Acquire::http::Proxy=http://127.0.0.1:18080"));
        assert!(joined.contains("Acquire::https::Proxy=http://127.0.0.1:18080"));
        assert!(!joined.contains("sh -c"));
    }

    #[test]
    fn worker_environment_has_only_loopback_proxy_and_no_bypass() {
        let environment = worker_environment();
        let values = environment
            .iter()
            .cloned()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(
            values.get("HTTP_PROXY"),
            Some(&"http://127.0.0.1:18080".to_string())
        );
        assert_eq!(
            values.get("HTTPS_PROXY"),
            Some(&"http://127.0.0.1:18080".to_string())
        );
        assert_eq!(values.get("NO_PROXY"), Some(&String::new()));
        assert!(!values.contains_key("ALL_PROXY"));
        assert!(!values.contains_key("FTP_PROXY"));
    }

    #[tokio::test]
    async fn broker_lease_fails_closed_when_host_peer_disappears() {
        let (lease, peer) = UnixStream::pair().unwrap();
        let cancel = CancellationToken::new();
        let mut monitor = tokio::spawn(monitor_broker_lease(lease, cancel));
        assert!(
            timeout(Duration::from_millis(50), &mut monitor)
                .await
                .is_err()
        );
        drop(peer);
        let error = timeout(Duration::from_secs(1), monitor)
            .await
            .expect("broker lease monitor did not detect peer loss")
            .unwrap()
            .unwrap_err();
        assert!(error.contains("lease closed"), "{error}");
    }

    #[tokio::test]
    async fn package_frames_roundtrip_and_reject_oversize() {
        let request = request(PackageOperation::Install, &["rustc"]);
        let (mut left, mut right) = tokio::io::duplex(PACKAGE_MAX_FRAME_BYTES + 8);
        let writer = tokio::spawn(async move { write_package_frame(&mut left, &request).await });
        let decoded: PackageRequest = read_package_frame(&mut right).await.unwrap();
        writer.await.unwrap().unwrap();
        assert_eq!(decoded.packages, vec!["rustc"]);

        let (mut left, mut right) = tokio::io::duplex(8);
        left.write_u32((PACKAGE_MAX_FRAME_BYTES + 1) as u32)
            .await
            .unwrap();
        assert!(
            read_package_frame::<_, PackageRequest>(&mut right)
                .await
                .is_err()
        );
    }
}
