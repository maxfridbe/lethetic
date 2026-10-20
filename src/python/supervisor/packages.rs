use super::DEFAULT_ADAPTER_PORT;
use super::security::{
    prepare_package_socket_parent, prepare_root_directory, remove_socket_if_owned,
    remove_stale_root_socket, secure_package_socket_path,
};
use super::{
    APT_GET_PATH, ChildRegistry, PACKAGE_AUDIT_PATH, PACKAGE_PROTOCOL_VERSION, PACKAGE_SOCKET_PATH,
    ProcessGroupGuard, signal_process_group,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::HashSet;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::process::Command;
use tokio::sync::{Mutex, Semaphore, oneshot};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

pub(super) const PACKAGE_MAX_FRAME_BYTES: usize = 1024 * 1024;
const PACKAGE_MAX_NAMES: usize = 32;
const PACKAGE_MAX_NAME_BYTES: usize = 128;
const PACKAGE_MAX_OUTPUT_BYTES: usize = 64 * 1024;
const PACKAGE_MAX_AUDIT_BYTES: u64 = 16 * 1024 * 1024;
const PACKAGE_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const PACKAGE_TRANSACTION_TIMEOUT: Duration = Duration::from_secs(20 * 60);

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PackageOperation {
    Refresh,
    Install,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct PackageRequest {
    pub(super) version: u32,
    pub(super) operation: PackageOperation,
    pub(super) packages: Vec<String>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PackageCommandSpec {
    pub(super) program: PathBuf,
    pub(super) args: Vec<String>,
    pub(super) env: Vec<(String, String)>,
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

pub(super) struct PackageAuditLog {
    pub(super) runtime_id: String,
    pub(super) file: Mutex<tokio::fs::File>,
    pub(super) bytes: AtomicU64,
    pub(super) healthy: AtomicBool,
    pub(super) failed: CancellationToken,
}

impl PackageAuditLog {
    pub(super) fn open(runtime_id: String) -> Result<Self, String> {
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

pub(super) async fn run_package_server(
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

pub(super) async fn handle_package_request(
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

pub(super) fn validate_package_request(request: &PackageRequest) -> Result<(), String> {
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

pub(super) fn package_command_spec(request: &PackageRequest) -> Result<PackageCommandSpec, String> {
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

pub(super) async fn run_package_command(
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
        tokio::try_join!(stdout, stderr, async {
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

pub(super) fn failed_package_response(message: &str) -> PackageResponse {
    PackageResponse {
        version: PACKAGE_PROTOCOL_VERSION,
        ok: false,
        exit_code: None,
        output: message.to_string(),
        truncated: false,
    }
}

pub(super) async fn drain_bounded<R>(
    mut reader: R,
    maximum: usize,
) -> Result<(Vec<u8>, bool), String>
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

pub(super) fn root_package_environment() -> Vec<(String, String)> {
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

pub(super) async fn read_package_frame<R, T>(reader: &mut R) -> Result<T, String>
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

pub(super) async fn write_package_frame<W, T>(writer: &mut W, value: &T) -> Result<(), String>
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
