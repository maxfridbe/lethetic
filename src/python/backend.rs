#[cfg(target_os = "linux")]
use super::PythonContainerIdentity;
use super::process_tree::{DEFAULT_SETTLE_TIMEOUT, ProcessTree, force_kill_and_reap};
use super::{LaunchKind, LaunchSpec, WORKER_SOURCE};
use crate::client::StreamEvent;
#[cfg(target_os = "linux")]
use crate::config::PythonWorkspaceExposure;
use crate::config::{
    AccessMode, Config, NetworkAccess, PathGrant, PythonExecutionTarget, SandboxBackend,
    ToolProfile,
};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Stdio;
#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
#[cfg(target_os = "linux")]
const PODMAN_IMAGE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const MAX_PULL_PROGRESS_CHUNK: usize = 4096;
#[cfg(target_os = "linux")]
static CONTAINER_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PythonBackendChoice {
    Host,
    Bubblewrap,
    Podman,
}

impl PythonBackendChoice {
    pub fn label(self) -> &'static str {
        match self {
            Self::Host => "Host",
            Self::Bubblewrap => "Bubblewrap",
            Self::Podman => "Podman",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendCapability {
    pub choice: PythonBackendChoice,
    pub available: bool,
    pub reason: String,
}

impl BackendCapability {
    fn available(choice: PythonBackendChoice, reason: impl Into<String>) -> Self {
        Self {
            choice,
            available: true,
            reason: reason.into(),
        }
    }

    fn unavailable(choice: PythonBackendChoice, reason: impl Into<String>) -> Self {
        Self {
            choice,
            available: false,
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedMount {
    pub path: PathBuf,
    pub access: AccessMode,
    pub is_directory: bool,
    pub is_workspace: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLaunch {
    pub spec: LaunchSpec,
    pub fingerprint: String,
    pub choice: PythonBackendChoice,
    pub network: Option<NetworkAccess>,
    pub workspace_access: Option<AccessMode>,
    pub host_visible_roots: Vec<PathBuf>,
    pub launch_cwd: PathBuf,
}

pub async fn resolve_launch(
    config: &Config,
    workspace_root: &Path,
    current_cwd: &Path,
) -> Result<ResolvedLaunch, String> {
    resolve_launch_with_cancellation(
        config,
        workspace_root,
        current_cwd,
        CancellationToken::new(),
    )
    .await
}

pub async fn resolve_launch_with_cancellation(
    config: &Config,
    workspace_root: &Path,
    current_cwd: &Path,
    cancellation: CancellationToken,
) -> Result<ResolvedLaunch, String> {
    if cancellation.is_cancelled() {
        return Err("Python runtime launch preparation was cancelled".to_string());
    }
    if config.tool_profile != ToolProfile::PythonOnly {
        return Err("The Python runspace is available only in Python-only mode".to_string());
    }
    if let Some(error) = config.python_mode_validation_error() {
        return Err(error);
    }

    let workspace = canonical_directory(workspace_root, "workspace")?;
    let host_cwd = canonical_directory(current_cwd, "current working directory")
        .unwrap_or_else(|_| workspace.clone());
    let policy_fingerprint = config.python_policy_fingerprint();

    match config.python_runtime.target {
        Some(PythonExecutionTarget::Host) => {
            let capability = probe_backend_with_cancellation(
                config,
                &workspace,
                PythonBackendChoice::Host,
                cancellation.clone(),
            )
            .await?;
            if !capability.available {
                return Err(capability.reason);
            }
            let spec = LaunchSpec::host(
                config.python_runtime.python_executable.clone(),
                host_cwd.clone(),
            );
            Ok(ResolvedLaunch {
                spec,
                fingerprint: format!("{policy_fingerprint}:host"),
                choice: PythonBackendChoice::Host,
                network: None,
                workspace_access: None,
                host_visible_roots: Vec::new(),
                launch_cwd: host_cwd,
            })
        }
        Some(PythonExecutionTarget::Sandbox) => {
            let workspace_access = config
                .python_runtime
                .sandbox
                .workspace_access
                .ok_or_else(|| "Python sandbox workspace access is not configured".to_string())?;
            let network = config
                .python_runtime
                .sandbox
                .network
                .ok_or_else(|| "Python sandbox network access is not configured".to_string())?;
            let mounts = validate_mounts(
                &workspace,
                workspace_access,
                &config.python_runtime.sandbox.grants,
            )?;
            let launch_cwd = visible_cwd(&host_cwd, &workspace, &mounts);
            let roots = mounts
                .iter()
                .map(|mount| mount.path.clone())
                .collect::<Vec<_>>();
            let mount_fingerprint = mounts
                .iter()
                .map(|mount| format!("{}:{:?}", mount.path.to_string_lossy(), mount.access))
                .collect::<Vec<_>>()
                .join("|");

            match config.python_runtime.sandbox.backend {
                Some(SandboxBackend::Bubblewrap) => {
                    let capability = probe_backend_with_cancellation(
                        config,
                        &workspace,
                        PythonBackendChoice::Bubblewrap,
                        cancellation.clone(),
                    )
                    .await?;
                    if !capability.available {
                        return Err(capability.reason);
                    }
                    let bwrap = resolve_executable("bwrap")?;
                    let python = resolve_executable(&config.python_runtime.python_executable)?;
                    let spec =
                        build_bubblewrap_spec(&bwrap, &python, &mounts, &launch_cwd, network)?;
                    Ok(ResolvedLaunch {
                        spec,
                        fingerprint: format!(
                            "{policy_fingerprint}:bubblewrap:{network:?}:{mount_fingerprint}"
                        ),
                        choice: PythonBackendChoice::Bubblewrap,
                        network: Some(network),
                        workspace_access: Some(workspace_access),
                        host_visible_roots: roots,
                        launch_cwd,
                    })
                }
                Some(SandboxBackend::Podman) => {
                    #[cfg(target_os = "linux")]
                    {
                        let mask_lethetic = config.python_invocation.workspace_exposure
                            == PythonWorkspaceExposure::SharedLaunchCwd;
                        if mask_lethetic {
                            let validated =
                                super::retained_podman::validate_external_workspace(&workspace)?;
                            if validated != workspace {
                                return Err(format!(
                                    "Validated shared Python workspace changed identity: {} -> {}",
                                    workspace.display(),
                                    validated.display()
                                ));
                            }
                        }
                        let capability = Box::pin(probe_backend_with_cancellation(
                            config,
                            &workspace,
                            PythonBackendChoice::Podman,
                            cancellation.clone(),
                        ))
                        .await?;
                        if !capability.available {
                            return Err(capability.reason);
                        }
                        let podman = resolve_real_podman()?;
                        let image = super::retained_podman::with_podman_command_cancellation(
                            cancellation.clone(),
                            super::retained_podman::resolve_local_image_exact(
                                &podman,
                                &config.python_runtime.sandbox.podman_image,
                            ),
                        )
                        .await?;
                        let spec = build_podman_spec_with_mask(
                            &podman,
                            &image.image_id,
                            &mounts,
                            &launch_cwd,
                            network,
                            mask_lethetic,
                        )?;
                        Ok(ResolvedLaunch {
                            spec,
                            fingerprint: format!(
                                "{policy_fingerprint}:podman:{}:{network:?}:mask-lethetic={mask_lethetic}:{mount_fingerprint}",
                                image.image_id
                            ),
                            choice: PythonBackendChoice::Podman,
                            network: Some(network),
                            workspace_access: Some(workspace_access),
                            host_visible_roots: roots,
                            launch_cwd,
                        })
                    }
                    #[cfg(not(target_os = "linux"))]
                    {
                        let _ = (
                            config,
                            workspace,
                            mounts,
                            launch_cwd,
                            network,
                            workspace_access,
                            roots,
                            mount_fingerprint,
                            policy_fingerprint,
                        );
                        Err("Podman sandboxing is supported only on Linux".to_string())
                    }
                }
                None => Err("Python sandbox backend is not configured".to_string()),
            }
        }
        None => Err("Python execution target is not configured".to_string()),
    }
}

pub async fn probe_backend(
    config: &Config,
    workspace_root: &Path,
    choice: PythonBackendChoice,
) -> BackendCapability {
    probe_backend_inner(config, workspace_root, choice, CancellationToken::new()).await
}

async fn probe_backend_inner(
    config: &Config,
    workspace_root: &Path,
    choice: PythonBackendChoice,
    cancellation: CancellationToken,
) -> BackendCapability {
    let result = match choice {
        PythonBackendChoice::Host => Box::pin(probe_host(config, &cancellation)).await,
        PythonBackendChoice::Bubblewrap => {
            Box::pin(probe_bubblewrap(config, workspace_root, &cancellation)).await
        }
        PythonBackendChoice::Podman => {
            Box::pin(probe_podman(config, workspace_root, &cancellation)).await
        }
    };
    match result {
        Ok(reason) => BackendCapability::available(choice, reason),
        Err(reason) => BackendCapability::unavailable(choice, reason),
    }
}

pub async fn probe_all_backends(config: &Config, workspace_root: &Path) -> Vec<BackendCapability> {
    futures_util::future::join_all([
        probe_backend(config, workspace_root, PythonBackendChoice::Host),
        probe_backend(config, workspace_root, PythonBackendChoice::Bubblewrap),
        probe_backend(config, workspace_root, PythonBackendChoice::Podman),
    ])
    .await
}

pub async fn probe_all_backends_with_cancellation(
    config: &Config,
    workspace_root: &Path,
    cancellation: CancellationToken,
) -> Result<Vec<BackendCapability>, String> {
    if cancellation.is_cancelled() {
        return Err("Python backend probing was cancelled".to_string());
    }
    let capabilities = futures_util::future::join_all([
        probe_backend_inner(
            config,
            workspace_root,
            PythonBackendChoice::Host,
            cancellation.clone(),
        ),
        probe_backend_inner(
            config,
            workspace_root,
            PythonBackendChoice::Bubblewrap,
            cancellation.clone(),
        ),
        probe_backend_inner(
            config,
            workspace_root,
            PythonBackendChoice::Podman,
            cancellation.clone(),
        ),
    ])
    .await;
    if cancellation.is_cancelled() {
        Err("Python backend probing was cancelled".to_string())
    } else {
        Ok(capabilities)
    }
}

pub async fn probe_backend_with_cancellation(
    config: &Config,
    workspace_root: &Path,
    choice: PythonBackendChoice,
    cancellation: CancellationToken,
) -> Result<BackendCapability, String> {
    if cancellation.is_cancelled() {
        return Err("Python backend validation was cancelled".to_string());
    }
    let capability =
        probe_backend_inner(config, workspace_root, choice, cancellation.clone()).await;
    if cancellation.is_cancelled() {
        Err("Python backend validation was cancelled".to_string())
    } else {
        Ok(capability)
    }
}

pub fn validate_mounts(
    workspace_root: &Path,
    workspace_access: AccessMode,
    grants: &[PathGrant],
) -> Result<Vec<ValidatedMount>, String> {
    let workspace = canonical_directory(workspace_root, "workspace")?;
    let mut mounts = BTreeMap::<PathBuf, (AccessMode, bool, bool)>::new();
    mounts.insert(workspace.clone(), (workspace_access, true, true));

    for grant in grants {
        let metadata = std::fs::symlink_metadata(&grant.path).map_err(|error| {
            format!(
                "Could not inspect Python sandbox grant {}: {error}",
                grant.path.display()
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "Python sandbox grants cannot be symlinks: {}",
                grant.path.display()
            ));
        }
        let canonical = grant.path.canonicalize().map_err(|error| {
            format!(
                "Could not canonicalize Python sandbox grant {}: {error}",
                grant.path.display()
            )
        })?;
        if canonical.to_str().is_none() {
            return Err(format!(
                "Python sandbox grant is not valid UTF-8: {}",
                canonical.display()
            ));
        }
        let metadata = std::fs::metadata(&canonical).map_err(|error| {
            format!(
                "Could not inspect canonical Python sandbox grant {}: {error}",
                canonical.display()
            )
        })?;
        reject_special_file(&canonical, &metadata)?;
        let is_directory = metadata.is_dir();
        if !is_directory && !metadata.is_file() {
            return Err(format!(
                "Python sandbox grant must be a regular file or directory: {}",
                canonical.display()
            ));
        }
        let is_workspace = canonical == workspace;
        mounts.insert(canonical, (grant.access, is_directory, is_workspace));
    }

    let mut result = mounts
        .into_iter()
        .map(
            |(path, (access, is_directory, is_workspace))| ValidatedMount {
                path,
                access,
                is_directory,
                is_workspace,
            },
        )
        .collect::<Vec<_>>();
    result.sort_by(|left, right| {
        left.path
            .components()
            .count()
            .cmp(&right.path.components().count())
            .then_with(|| left.path.cmp(&right.path))
    });
    Ok(result)
}

fn reject_special_file(path: &Path, metadata: &std::fs::Metadata) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        let file_type = metadata.file_type();
        if file_type.is_socket()
            || file_type.is_fifo()
            || file_type.is_block_device()
            || file_type.is_char_device()
        {
            return Err(format!(
                "Python sandbox grant cannot be a socket, FIFO, or device: {}",
                path.display()
            ));
        }
    }
    let _ = (path, metadata);
    Ok(())
}

fn canonical_directory(path: &Path, label: &str) -> Result<PathBuf, String> {
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("Could not canonicalize {label} {}: {error}", path.display()))?;
    let metadata = std::fs::metadata(&canonical)
        .map_err(|error| format!("Could not inspect {label} {}: {error}", canonical.display()))?;
    if !metadata.is_dir() {
        return Err(format!(
            "{label} is not a directory: {}",
            canonical.display()
        ));
    }
    if canonical.to_str().is_none() {
        return Err(format!(
            "{label} is not valid UTF-8: {}",
            canonical.display()
        ));
    }
    Ok(canonical)
}

fn visible_cwd(current: &Path, workspace: &Path, mounts: &[ValidatedMount]) -> PathBuf {
    if mounts
        .iter()
        .any(|mount| mount.is_directory && current.starts_with(&mount.path))
    {
        current.to_path_buf()
    } else {
        workspace.to_path_buf()
    }
}

pub fn build_bubblewrap_spec(
    bwrap: &Path,
    python: &Path,
    mounts: &[ValidatedMount],
    launch_cwd: &Path,
    network: NetworkAccess,
) -> Result<LaunchSpec, String> {
    if !cfg!(target_os = "linux") {
        return Err("Bubblewrap sandboxing is supported only on Linux".to_string());
    }
    ensure_absolute_executable(bwrap, "Bubblewrap")?;
    ensure_absolute_executable(python, "Python")?;
    if network == NetworkAccess::Nonlocal {
        return Err(
            "Nonlocal networking requires the retained Podman runtime; Bubblewrap cannot provide it"
                .to_string(),
        );
    }
    if !mounts
        .iter()
        .any(|mount| mount.is_directory && launch_cwd.starts_with(&mount.path))
    {
        return Err(format!(
            "Python sandbox cwd is not covered by a path grant: {}",
            launch_cwd.display()
        ));
    }

    let runtime_mounts = bubblewrap_runtime_mounts(network);
    let mut destinations = BTreeSet::new();
    for path in runtime_mounts
        .iter()
        .chain(mounts.iter().map(|mount| &mount.path))
    {
        add_destination_directories(path, &mut destinations);
        if path.is_dir() {
            destinations.insert(path.clone());
        }
    }
    for path in [
        Path::new("/tmp"),
        Path::new("/home"),
        Path::new("/home/lethetic"),
        Path::new("/proc"),
        Path::new("/dev"),
    ] {
        add_destination_directories(path, &mut destinations);
        destinations.insert(path.to_path_buf());
    }

    let mut args = os_args(&[
        "--die-with-parent",
        "--new-session",
        "--unshare-user",
        "--unshare-pid",
        "--unshare-ipc",
        "--unshare-uts",
        "--clearenv",
        "--hostname",
        "lethetic-python",
    ]);
    if network == NetworkAccess::None {
        args.push(OsString::from("--unshare-net"));
    }

    let mut destinations = destinations.into_iter().collect::<Vec<_>>();
    destinations.sort_by_key(|path| path.components().count());
    for destination in destinations {
        if destination == Path::new("/")
            || destination == Path::new("/tmp")
            || destination == Path::new("/proc")
            || destination == Path::new("/dev")
        {
            continue;
        }
        args.push(OsString::from("--dir"));
        args.push(destination.into_os_string());
    }

    args.extend(os_args(&[
        "--proc",
        "/proc",
        "--dev",
        "/dev",
        "--tmpfs",
        "/tmp",
        "--setenv",
        "HOME",
        "/home/lethetic",
        "--setenv",
        "PATH",
        "/usr/local/bin:/usr/bin:/bin",
        "--setenv",
        "LANG",
        "C.UTF-8",
        "--setenv",
        "PYTHONNOUSERSITE",
        "1",
        "--setenv",
        "PYTHONDONTWRITEBYTECODE",
        "1",
    ]));

    for path in runtime_mounts {
        args.push(OsString::from("--ro-bind"));
        args.push(path.clone().into_os_string());
        args.push(path.into_os_string());
    }
    for mount in mounts {
        args.push(match mount.access {
            AccessMode::ReadOnly => OsString::from("--ro-bind"),
            AccessMode::ReadWrite => OsString::from("--bind"),
        });
        args.push(mount.path.clone().into_os_string());
        args.push(mount.path.clone().into_os_string());
    }

    args.push(OsString::from("--chdir"));
    args.push(launch_cwd.as_os_str().to_os_string());
    args.push(OsString::from("--"));
    args.push(python.as_os_str().to_os_string());
    args.extend(os_args(&["-u", "-B", "-c"]));
    args.push(OsString::from(WORKER_SOURCE));

    Ok(LaunchSpec {
        kind: LaunchKind::Direct,
        container_identity: None,
        program: bwrap.as_os_str().to_os_string(),
        args,
        cwd: None,
        clear_env: true,
        env: Vec::new(),
        cleanup: None,
        startup_notice: None,
        startup_timeout: std::time::Duration::from_secs(10),
        graceful_shutdown: None,
    })
}

fn bubblewrap_runtime_mounts(network: NetworkAccess) -> Vec<PathBuf> {
    let mut candidates = vec![
        PathBuf::from("/usr"),
        PathBuf::from("/bin"),
        PathBuf::from("/lib"),
        PathBuf::from("/lib64"),
        PathBuf::from("/etc/alternatives"),
        PathBuf::from("/etc/ld.so.cache"),
        PathBuf::from("/etc/localtime"),
    ];
    if network == NetworkAccess::Full {
        candidates.extend([
            PathBuf::from("/etc/resolv.conf"),
            PathBuf::from("/etc/hosts"),
            PathBuf::from("/etc/nsswitch.conf"),
            PathBuf::from("/etc/ssl"),
            PathBuf::from("/etc/pki"),
        ]);
    }
    candidates.retain(|path| path.exists());
    candidates.sort();
    candidates.dedup();
    candidates
}

fn add_destination_directories(path: &Path, destinations: &mut BTreeSet<PathBuf>) {
    let mut current = path.parent();
    while let Some(parent) = current {
        if parent == Path::new("") || parent == Path::new("/") {
            break;
        }
        destinations.insert(parent.to_path_buf());
        current = parent.parent();
    }
}

#[cfg(all(test, target_os = "linux"))]
fn podman_user_arg() -> Result<OsString, String> {
    #[cfg(unix)]
    {
        let uid = rustix::process::geteuid().as_raw();
        let gid = rustix::process::getegid().as_raw();
        Ok(OsString::from(format!("--user={uid}:{gid}")))
    }
    #[cfg(not(unix))]
    {
        Err("Podman user mapping is supported only on Unix".to_string())
    }
}

pub fn build_podman_spec(
    podman: &Path,
    image: &str,
    mounts: &[ValidatedMount],
    launch_cwd: &Path,
    network: NetworkAccess,
) -> Result<LaunchSpec, String> {
    build_podman_spec_with_mask(podman, image, mounts, launch_cwd, network, false)
}

#[cfg(target_os = "linux")]
pub fn build_podman_spec_with_mask(
    podman: &Path,
    image: &str,
    mounts: &[ValidatedMount],
    launch_cwd: &Path,
    network: NetworkAccess,
    mask_lethetic: bool,
) -> Result<LaunchSpec, String> {
    if !cfg!(target_os = "linux") {
        return Err("Podman sandboxing is supported only on Linux".to_string());
    }
    validate_podman_executable(podman)?;
    validate_image_name(image)?;
    if !mounts
        .iter()
        .any(|mount| mount.is_directory && launch_cwd.starts_with(&mount.path))
    {
        return Err(format!(
            "Python sandbox cwd is not covered by a path grant: {}",
            launch_cwd.display()
        ));
    }
    for mount in mounts {
        let path = mount
            .path
            .to_str()
            .ok_or_else(|| format!("Podman grant is not valid UTF-8: {}", mount.path.display()))?;
        if path.contains(':') {
            return Err(format!(
                "Podman cannot safely encode a grant containing ':': {path}"
            ));
        }
    }

    let name = format!(
        "lethetic-python-transient-{}-{}",
        std::process::id(),
        CONTAINER_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    if network == NetworkAccess::Nonlocal {
        return Err(
            "Nonlocal networking requires the retained Podman runtime; the transient Podman backend cannot provide it"
                .to_string(),
        );
    }
    let workspace = mounts
        .iter()
        .find(|mount| mount.is_workspace && mount.is_directory)
        .ok_or_else(|| "transient Podman mode has no workspace mount".to_string())?;
    let worker_uid = rustix::process::geteuid().as_raw();
    let worker_gid = rustix::process::getegid().as_raw();
    use sha2::{Digest, Sha256};
    let mut fingerprint = Sha256::new();
    for value in [
        image.as_bytes(),
        name.as_bytes(),
        launch_cwd.as_os_str().as_encoded_bytes(),
        format!("{network:?}:{mask_lethetic}:{worker_uid}:{worker_gid}").as_bytes(),
    ] {
        fingerprint.update(value);
        fingerprint.update([0]);
    }
    for mount in mounts {
        fingerprint.update(mount.path.as_os_str().as_encoded_bytes());
        fingerprint.update([0]);
        fingerprint.update(format!("{:?}:{}", mount.access, mount.is_directory));
        fingerprint.update([0]);
    }
    let security_fingerprint = format!("{:x}", fingerprint.finalize());
    let transient = super::retained_podman::TransientPodmanConfig {
        podman: podman.to_path_buf(),
        image_id: image.to_string(),
        container_name: name,
        security_fingerprint,
        workspace: workspace.path.clone(),
        launch_cwd: launch_cwd.to_path_buf(),
        mounts: mounts
            .iter()
            .map(|mount| super::retained_podman::TransientPodmanMount {
                path: mount.path.clone(),
                access: mount.access,
                is_directory: mount.is_directory,
            })
            .collect(),
        network,
        mask_lethetic,
        worker_uid,
        worker_gid,
    };
    let container_identity = PythonContainerIdentity::transient(&transient.container_name)
        .ok_or_else(|| "transient Podman container name is not canonical".to_string())?;
    let create = super::retained_podman::build_transient_create_spec(&transient)?;
    Ok(LaunchSpec {
        kind: LaunchKind::TransientPodman(transient),
        container_identity: Some(container_identity),
        program: create.program.into_os_string(),
        args: create.args,
        cwd: None,
        clear_env: false,
        env: Vec::new(),
        cleanup: None,
        startup_notice: None,
        startup_timeout: std::time::Duration::from_secs(10),
        graceful_shutdown: None,
    })
}

#[cfg(not(target_os = "linux"))]
pub fn build_podman_spec_with_mask(
    podman: &Path,
    image: &str,
    mounts: &[ValidatedMount],
    launch_cwd: &Path,
    network: NetworkAccess,
    mask_lethetic: bool,
) -> Result<LaunchSpec, String> {
    let _ = (podman, image, mounts, launch_cwd, network, mask_lethetic);
    Err("Podman sandboxing is supported only on Linux".to_string())
}

async fn probe_host(config: &Config, cancellation: &CancellationToken) -> Result<String, String> {
    let executable = config.python_runtime.python_executable.trim();
    if executable.is_empty() {
        return Err("Host Python executable is empty".to_string());
    }
    let output = run_output_with_cancellation(
        OsStr::new(executable),
        &os_args(&[
            "-I",
            "-B",
            "-c",
            "import ast,json,os,tempfile; print(os.path.realpath(os.getcwd()))",
        ]),
        PROBE_TIMEOUT,
        cancellation,
    )
    .await
    .map_err(|error| format!("Host Python probe failed: {error}"))?;
    if !output.status.success() {
        return Err(command_failure("Host Python probe", &output));
    }
    let version = run_output_with_cancellation(
        OsStr::new(executable),
        &os_args(&["--version"]),
        PROBE_TIMEOUT,
        cancellation,
    )
    .await
    .map_err(|error| format!("Host Python version probe failed: {error}"))?;
    let text = combined_output(&version);
    Ok(if text.is_empty() {
        format!("Host Python '{}' is available", executable)
    } else {
        format!("{} is available", text.trim())
    })
}

async fn probe_bubblewrap(
    config: &Config,
    workspace_root: &Path,
    cancellation: &CancellationToken,
) -> Result<String, String> {
    if !cfg!(target_os = "linux") {
        return Err("Bubblewrap sandboxing is supported only on Linux".to_string());
    }
    if cancellation.is_cancelled() {
        return Err("Python backend probing was cancelled".to_string());
    };
    let bwrap = resolve_executable("bwrap")
        .map_err(|error| format!("Bubblewrap is unavailable: {error}"))?;
    let python = resolve_executable(&config.python_runtime.python_executable)
        .map_err(|error| format!("Bubblewrap Python is unavailable: {error}"))?;
    let workspace = canonical_directory(workspace_root, "workspace")?;
    let mounts = validate_mounts(&workspace, AccessMode::ReadOnly, &[])?;
    let spec = build_bubblewrap_spec(&bwrap, &python, &mounts, &workspace, NetworkAccess::None)?;
    run_launch_probe(spec, "Bubblewrap", cancellation).await?;
    Ok(format!("Bubblewrap is available with {}", python.display()))
}

#[cfg(target_os = "linux")]
async fn probe_podman(
    config: &Config,
    workspace_root: &Path,
    cancellation: &CancellationToken,
) -> Result<String, String> {
    if !cfg!(target_os = "linux") {
        return Err("Podman sandboxing is supported only on Linux".to_string());
    }
    let podman = resolve_real_podman()?;
    Box::pin(require_rootless_podman_with_cancellation(
        &podman,
        cancellation,
    ))
    .await?;
    let image =
        super::retained_podman::with_podman_command_cancellation(cancellation.clone(), async {
            Box::pin(super::retained_podman::retry_pending_transient_cleanups(
                &podman,
            ))
            .await?;
            let image = config.python_runtime.sandbox.podman_image.trim();
            validate_image_name(image)?;
            Box::pin(super::retained_podman::resolve_local_image_exact(
                &podman, image,
            ))
            .await
        })
        .await?;

    let workspace = canonical_directory(workspace_root, "workspace")?;
    let mask_lethetic =
        config.python_invocation.workspace_exposure == PythonWorkspaceExposure::SharedLaunchCwd;
    if mask_lethetic {
        super::retained_podman::validate_external_workspace(&workspace)?;
    }
    let workspace_access = if mask_lethetic {
        config
            .python_runtime
            .sandbox
            .workspace_access
            .unwrap_or(AccessMode::ReadWrite)
    } else {
        AccessMode::ReadOnly
    };
    let mounts = validate_mounts(&workspace, workspace_access, &[])?;
    let probe_network = match config.python_runtime.sandbox.network {
        Some(NetworkAccess::Full) => NetworkAccess::Full,
        _ => NetworkAccess::None,
    };
    let spec = build_podman_spec_with_mask(
        &podman,
        &image.image_id,
        &mounts,
        &workspace,
        probe_network,
        mask_lethetic,
    )?;
    Box::pin(run_launch_probe(spec, "Podman", cancellation)).await?;
    Ok(format!(
        "Rootless Podman and image '{}' are available",
        image.requested
    ))
}

#[cfg(not(target_os = "linux"))]
async fn probe_podman(
    _config: &Config,
    _workspace_root: &Path,
    cancellation: &CancellationToken,
) -> Result<String, String> {
    if cancellation.is_cancelled() {
        return Err("Python backend probing was cancelled".to_string());
    }
    Err("Podman sandboxing is supported only on Linux".to_string())
}

async fn run_launch_probe(
    spec: LaunchSpec,
    label: &str,
    cancellation: &CancellationToken,
) -> Result<(), String> {
    let worker = Box::pin(super::Worker::spawn(spec, cancellation.clone()))
        .await
        .map_err(|error| format!("{label} hardened smoke probe failed: {error}"))?;
    Box::pin(worker.terminate())
        .await
        .map_err(|error| format!("{label} hardened smoke probe cleanup failed: {error}"))
}

pub(crate) fn resolve_real_podman() -> Result<PathBuf, String> {
    // Nonlocal cleanup and runtime management must not resolve a project- or
    // environment-controlled PATH entry. Keep the command-specific invocation
    // path intact because Distrobox selects the forwarded host command from the
    // `podman` symlink basename.
    for candidate in ["/usr/local/bin/podman", "/usr/bin/podman"] {
        let path = PathBuf::from(candidate);
        match std::fs::symlink_metadata(&path) {
            Ok(_) => {
                validate_podman_executable(&path)?;
                validate_trusted_system_podman_path(&path)?;
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!("Could not inspect Podman {candidate}: {error}"));
            }
        }
    }
    Err("Podman is unavailable at a trusted system invocation path".to_string())
}

#[cfg(target_os = "linux")]
fn validate_trusted_system_podman_path(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let invoked = std::fs::symlink_metadata(path)
        .map_err(|error| format!("Could not inspect Podman invocation path: {error}"))?;
    if invoked.uid() == rustix::process::geteuid().as_raw() {
        return Err("Podman system invocation path is owned by the invoking user".to_string());
    }
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("Could not canonicalize Podman invocation path: {error}"))?;
    let target = std::fs::metadata(&canonical)
        .map_err(|error| format!("Could not inspect Podman executable target: {error}"))?;
    let mode = target.permissions().mode();
    if !target.is_file()
        || mode & 0o111 == 0
        || mode & 0o022 != 0
        || (target.uid() == rustix::process::geteuid().as_raw() && mode & 0o200 != 0)
    {
        return Err(format!(
            "Podman executable target is not a trusted non-writable system executable: {}",
            canonical.display()
        ));
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn validate_trusted_system_podman_path(_path: &Path) -> Result<(), String> {
    Err("trusted Podman execution is supported only on Linux".to_string())
}

pub fn validate_podman_executable(path: &Path) -> Result<(), String> {
    ensure_absolute_executable(path, "Podman")?;
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("Could not canonicalize Podman {}: {error}", path.display()))?;
    let canonical_name = canonical
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let invoked_name = path
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase();

    if canonical_name.contains("distrobox-host-exec") {
        // Distrobox installs command-specific compatibility symlinks whose
        // basename selects the host command. Invoking the `podman` link keeps
        // argv shell-free and still passes through the rootless/info/image and
        // hardened smoke probes below. Calling the generic forwarder directly
        // would not identify which host command to run.
        if invoked_name == "podman" {
            return Ok(());
        }
        return Err(format!(
            "Podman host forwarder {} must be invoked through a 'podman' compatibility symlink",
            canonical.display()
        ));
    }
    if canonical_name.contains("host-spawn") {
        return Err(format!(
            "Podman path {} resolves to unsupported host-forwarding shim {}",
            path.display(),
            canonical.display()
        ));
    }
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
pub(crate) async fn require_rootless_podman(podman: &Path) -> Result<(), String> {
    require_rootless_podman_with_cancellation(podman, &CancellationToken::new()).await
}

pub(crate) async fn require_rootless_podman_with_cancellation(
    podman: &Path,
    cancellation: &CancellationToken,
) -> Result<(), String> {
    let output = run_output_with_cancellation(
        podman.as_os_str(),
        &os_args(&["info", "--format=json"]),
        PROBE_TIMEOUT,
        cancellation,
    )
    .await
    .map_err(|error| format!("Could not query Podman: {error}"))?;
    if !output.status.success() {
        return Err(command_failure("Podman info", &output));
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("Podman info did not return valid JSON: {error}"))?;
    if !contains_rootless_true(&value) {
        return Err(
            "Podman runtime is not rootless; refusing Python sandbox activation".to_string(),
        );
    }
    Ok(())
}

fn contains_rootless_true(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(object) => object.iter().any(|(key, value)| {
            (key.eq_ignore_ascii_case("rootless") && value.as_bool() == Some(true))
                || contains_rootless_true(value)
        }),
        serde_json::Value::Array(values) => values.iter().any(contains_rootless_true),
        _ => false,
    }
}

#[cfg(target_os = "linux")]
pub(crate) async fn require_podman_image_with_cancellation(
    podman: &Path,
    image: &str,
    cancellation: &CancellationToken,
) -> Result<(), String> {
    let output = run_output_with_cancellation(
        podman.as_os_str(),
        &[
            OsString::from("image"),
            OsString::from("exists"),
            OsString::from(image),
        ],
        PODMAN_IMAGE_TIMEOUT,
        cancellation,
    )
    .await
    .map_err(|error| format!("Could not inspect Podman image '{image}': {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Podman image '{image}' is not installed. Use the explicit Pull action; Lethetic will not pull it automatically."
        ));
    }
    Ok(())
}

pub async fn pull_podman_image(
    image: &str,
    cancellation_token: CancellationToken,
    progress_tx: Option<mpsc::UnboundedSender<StreamEvent>>,
) -> Result<(), String> {
    validate_image_name(image)?;
    let podman = resolve_real_podman()?;
    Box::pin(require_rootless_podman_with_cancellation(
        &podman,
        &cancellation_token,
    ))
    .await?;

    let mut child = Command::new(&podman)
        .arg("pull")
        .arg("--")
        .arg(image)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("Could not start Podman pull for '{image}': {error}"))?;
    let Some(stdout) = child.stdout.take() else {
        let _ = child.start_kill();
        let _ = child.wait().await;
        return Err("Podman pull did not expose stdout".to_string());
    };
    let Some(stderr) = child.stderr.take() else {
        drop(stdout);
        let _ = child.start_kill();
        let _ = child.wait().await;
        return Err("Podman pull did not expose stderr".to_string());
    };
    let stdout_task = spawn_progress_reader(stdout, progress_tx.clone(), "Podman");
    let stderr_task = spawn_progress_reader(stderr, progress_tx, "Podman");

    enum PullWait {
        Exited(std::io::Result<std::process::ExitStatus>),
        Cancelled,
    }
    let outcome = tokio::select! {
        biased;
        _ = cancellation_token.cancelled() => PullWait::Cancelled,
        status = child.wait() => PullWait::Exited(status),
    };
    let status = match outcome {
        PullWait::Exited(Ok(status)) => status,
        PullWait::Exited(Err(error)) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            stdout_task.abort();
            stderr_task.abort();
            let _ = stdout_task.await;
            let _ = stderr_task.await;
            return Err(format!("Could not wait for Podman pull '{image}': {error}"));
        }
        PullWait::Cancelled => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            stdout_task.abort();
            stderr_task.abort();
            let _ = stdout_task.await;
            let _ = stderr_task.await;
            return Err(format!("Podman pull for '{image}' was cancelled"));
        }
    };
    let _ = stdout_task.await;
    let _ = stderr_task.await;
    if !status.success() {
        return Err(format!(
            "Podman pull for '{image}' failed with status {status}"
        ));
    }
    Ok(())
}

fn spawn_progress_reader<R>(
    mut reader: R,
    tx: Option<mpsc::UnboundedSender<StreamEvent>>,
    label: &'static str,
) -> tokio::task::JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut buffer = vec![0_u8; MAX_PULL_PROGRESS_CHUNK];
        while let Ok(read) = reader.read(&mut buffer).await {
            if read == 0 {
                break;
            }
            if let Some(tx) = &tx {
                let text = String::from_utf8_lossy(&buffer[..read]);
                let _ = tx.send(StreamEvent::ToolProgress(format!(
                    "{label}: {}",
                    text.trim_end()
                )));
            }
        }
    })
}

fn validate_image_name(image: &str) -> Result<(), String> {
    if image.trim().is_empty() {
        return Err("Podman image cannot be empty".to_string());
    }
    if image.starts_with('-') || image.chars().any(char::is_whitespace) || image.contains('\0') {
        return Err(format!("Unsafe Podman image name: {image:?}"));
    }
    Ok(())
}

#[cfg(all(test, unix))]
fn resolve_invocation_path_with_search_path(
    program: &str,
    search_path: &OsStr,
) -> Result<PathBuf, String> {
    if program.trim().is_empty() {
        return Err("Executable name is empty".to_string());
    }
    let make_absolute = |candidate: PathBuf| -> Result<PathBuf, String> {
        if candidate.is_absolute() {
            Ok(candidate)
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(candidate))
                .map_err(|error| format!("Could not resolve the current directory: {error}"))
        }
    };

    let candidate = PathBuf::from(program);
    if candidate.is_absolute() || candidate.components().count() > 1 {
        let candidate = make_absolute(candidate)?;
        ensure_absolute_executable(&candidate, "Executable")?;
        return Ok(candidate);
    }
    for directory in std::env::split_paths(search_path) {
        let candidate = make_absolute(directory.join(program))?;
        if ensure_absolute_executable(&candidate, "Executable").is_ok() {
            return Ok(candidate);
        }
    }
    Err(format!("'{program}' was not found on PATH"))
}

fn resolve_executable(program: &str) -> Result<PathBuf, String> {
    if program.trim().is_empty() {
        return Err("Executable name is empty".to_string());
    }
    let candidate = PathBuf::from(program);
    if candidate.is_absolute() || candidate.components().count() > 1 {
        return canonical_executable(&candidate);
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(program);
        if (candidate.is_file() || candidate.symlink_metadata().is_ok())
            && let Ok(executable) = canonical_executable(&candidate)
        {
            return Ok(executable);
        }
    }
    Err(format!("'{program}' was not found on PATH"))
}

fn canonical_executable(path: &Path) -> Result<PathBuf, String> {
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("Could not resolve executable {}: {error}", path.display()))?;
    ensure_absolute_executable(&canonical, "Executable")?;
    Ok(canonical)
}

fn ensure_absolute_executable(path: &Path, label: &str) -> Result<(), String> {
    if !path.is_absolute() {
        return Err(format!("{label} path must be absolute: {}", path.display()));
    }
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("Could not inspect {label} {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{label} is not a regular file: {}", path.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(format!("{label} is not executable: {}", path.display()));
        }
    }
    Ok(())
}

async fn run_output_with_cancellation(
    program: &OsStr,
    args: &[OsString],
    timeout: std::time::Duration,
    cancellation: &CancellationToken,
) -> Result<std::process::Output, String> {
    if cancellation.is_cancelled() {
        return Err(format!("Command {:?} was cancelled before start", program));
    }

    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut process_group = ProcessTree::prepare(&mut command, "probe")?;
    let mut child = command
        .spawn()
        .map_err(|error| format!("Could not start {:?}: {error}", program))?;
    if let Err(error) = process_group.attach_and_resume(&child, "probe") {
        let containment =
            force_kill_and_reap(child, process_group, DEFAULT_SETTLE_TIMEOUT, "probe").await;
        return Err(match containment {
            Ok(()) => error,
            Err(containment) => format!("{error}; containment failed: {containment}"),
        });
    }
    let Some(stdout) = child.stdout.take() else {
        let containment =
            force_kill_and_reap(child, process_group, DEFAULT_SETTLE_TIMEOUT, "probe").await;
        return Err(match containment {
            Ok(()) => format!("Command {:?} did not expose stdout", program),
            Err(error) => format!(
                "Command {:?} did not expose stdout; containment failed: {error}",
                program
            ),
        });
    };
    let Some(stderr) = child.stderr.take() else {
        drop(stdout);
        let containment =
            force_kill_and_reap(child, process_group, DEFAULT_SETTLE_TIMEOUT, "probe").await;
        return Err(match containment {
            Ok(()) => format!("Command {:?} did not expose stderr", program),
            Err(error) => format!(
                "Command {:?} did not expose stderr; containment failed: {error}",
                program
            ),
        });
    };
    let stdout_task = spawn_output_reader(stdout);
    let stderr_task = spawn_output_reader(stderr);

    enum WaitOutcome {
        Exited(std::io::Result<std::process::ExitStatus>),
        Cancelled,
        TimedOut,
    }

    let outcome = tokio::select! {
        biased;
        _ = cancellation.cancelled() => WaitOutcome::Cancelled,
        status = child.wait() => WaitOutcome::Exited(status),
        _ = tokio::time::sleep(timeout) => WaitOutcome::TimedOut,
    };

    let (status, containment) = match outcome {
        WaitOutcome::Exited(Ok(status)) => (
            Some(status),
            process_group
                .terminate_remaining(DEFAULT_SETTLE_TIMEOUT, "probe")
                .await,
        ),
        WaitOutcome::Exited(Err(_)) | WaitOutcome::Cancelled | WaitOutcome::TimedOut => (
            None,
            force_kill_and_reap(child, process_group, DEFAULT_SETTLE_TIMEOUT, "probe").await,
        ),
    };
    let stdout = settle_output_reader(stdout_task).await;
    let stderr = settle_output_reader(stderr_task).await;

    if let Err(containment) = containment {
        return Err(match &outcome {
            WaitOutcome::Exited(Ok(_)) => format!(
                "Command {:?} completed but process-tree containment failed: {containment}",
                program
            ),
            WaitOutcome::Exited(Err(error)) => format!(
                "Could not wait for {:?}: {error}; process-tree containment failed: {containment}",
                program
            ),
            WaitOutcome::Cancelled => format!(
                "Command {:?} was cancelled; process-tree containment failed: {containment}",
                program
            ),
            WaitOutcome::TimedOut => format!(
                "Command {:?} timed out after {}s; process-tree containment failed: {containment}",
                program,
                timeout.as_secs()
            ),
        });
    }

    match outcome {
        WaitOutcome::Exited(Ok(_)) => Ok(std::process::Output {
            status: status.expect("successful child wait retained its exit status"),
            stdout: stdout?,
            stderr: stderr?,
        }),
        WaitOutcome::Exited(Err(error)) => {
            Err(format!("Could not wait for {:?}: {error}", program))
        }
        WaitOutcome::Cancelled => Err(format!("Command {:?} was cancelled", program)),
        WaitOutcome::TimedOut => Err(format!(
            "Command {:?} timed out after {}s",
            program,
            timeout.as_secs()
        )),
    }
}

fn spawn_output_reader<R>(mut reader: R) -> tokio::task::JoinHandle<Result<Vec<u8>, String>>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut output = Vec::new();
        reader
            .read_to_end(&mut output)
            .await
            .map_err(|error| format!("Could not read command output: {error}"))?;
        Ok(output)
    })
}

async fn settle_output_reader(
    mut task: tokio::task::JoinHandle<Result<Vec<u8>, String>>,
) -> Result<Vec<u8>, String> {
    match tokio::time::timeout(std::time::Duration::from_secs(1), &mut task).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => Err(format!("Command output reader failed: {error}")),
        Err(_) => {
            task.abort();
            let _ = task.await;
            Err("Command output reader did not settle after process exit".to_string())
        }
    }
}

fn command_failure(label: &str, output: &std::process::Output) -> String {
    let details = combined_output(output);
    if details.is_empty() {
        format!("{label} failed with status {}", output.status)
    } else {
        format!("{label} failed with status {}: {details}", output.status)
    }
}

fn combined_output(output: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    format!("{}{}", stdout.trim(), stderr.trim())
}

fn os_args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

#[cfg(test)]
mod tests;
