mod packages;
mod security;

use packages::*;
pub use packages::{PackageOperation, PackageResponse, run_package_client, validate_package_name};
use security::*;
pub use security::{exec_worker_process, validate_runtime_image_installation};

use super::WORKER_SOURCE;
use crate::python::egress_broker::{
    AdapterConfig, DEFAULT_ADAPTER_PORT, open_broker_readiness_connection, probe_broker,
    run_adapter_with_readiness,
};
use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::process::Command;
use tokio::sync::oneshot;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

pub const RUNTIME_ABI: &str = "lethetic-python-runtime-v4";
pub const PACKAGE_PROTOCOL_VERSION: u32 = 1;
pub const PACKAGE_SOCKET_PATH: &str = "/run/lethetic-pkg/control.sock";
pub const PACKAGE_AUDIT_PATH: &str = "/var/log/lethetic/package-transactions.jsonl";
pub const PYTHON_PATH: &str = "/usr/local/bin/python3";
pub const APT_GET_PATH: &str = "/usr/bin/apt-get";
pub const WORKER_HOME: &str = "/home/lethetic";

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
