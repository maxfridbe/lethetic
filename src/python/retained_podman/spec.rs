use super::model::{
    CONTAINER_BROKER_SOCKET, CONTAINER_CAPABILITY_PATH, CONTAINER_ENTRYPOINT,
    CONTAINER_HOST_BRIDGE, CONTAINER_WORKSPACE, LEGACY_CONTAINER_BROKER_SOCKET, PodmanCommandSpec,
    REQUIRED_CAPABILITIES, RetainedPodmanConfig, TransientPodmanConfig,
};
use super::workspace::{
    ExternalWorkspacePlan, classify_external_workspace, validate_managed_directory,
};
use crate::config::{AccessMode, NetworkAccess};
use crate::python::runtime_store::{
    LEGACY_RUNTIME_ABI_V2, LEGACY_RUNTIME_ABI_V3, RuntimeLayoutProfile, RuntimeWorkspaceOwnership,
};
use crate::python::supervisor::RUNTIME_ABI;
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::Path;

impl TransientPodmanConfig {
    pub(super) fn validate_cleanup_record_identity(&self) -> Result<(), String> {
        if !self.podman.is_absolute()
            || self.podman.to_str().is_none()
            || self.podman.file_name().is_none()
            || self.podman.components().any(|component| {
                !matches!(
                    component,
                    std::path::Component::RootDir | std::path::Component::Normal(_)
                )
            })
        {
            return Err("transient cleanup Podman path must be an absolute normalized UTF-8 executable path".to_string());
        }
        validate_image_id(&self.image_id)?;
        validate_transient_container_name(&self.container_name)?;
        validate_lower_hex(
            &self.security_fingerprint,
            64,
            "transient security fingerprint",
        )?;
        if !matches!(self.network, NetworkAccess::None | NetworkAccess::Full) {
            return Err("transient Podman supports only None or Full networking".to_string());
        }
        if self.worker_uid == 0 || self.worker_gid == 0 {
            return Err("transient Podman worker UID and GID must be nonzero".to_string());
        }
        if self.worker_uid != rustix::process::geteuid().as_raw()
            || self.worker_gid != rustix::process::getegid().as_raw()
        {
            return Err(
                "transient Podman worker identity must match the invoking user".to_string(),
            );
        }
        Ok(())
    }

    pub(super) fn validate_cleanup_identity(&self) -> Result<(), String> {
        self.validate_cleanup_record_identity()?;
        crate::python::backend::validate_podman_executable(&self.podman)?;
        Ok(())
    }

    pub(super) fn validate(&self) -> Result<(), String> {
        self.validate_without_workspace_commit().map(|_| ())
    }

    pub(super) fn prepare_workspace_for_create(&self) -> Result<(), String> {
        if let Some(workspace_plan) = self.validate_without_workspace_commit()? {
            workspace_plan.commit()?;
        }
        Ok(())
    }

    fn validate_without_workspace_commit(&self) -> Result<Option<ExternalWorkspacePlan>, String> {
        self.validate_cleanup_identity()?;
        if self.mounts.is_empty() {
            return Err("transient Podman requires at least the workspace mount".to_string());
        }
        let workspace = self
            .workspace
            .canonicalize()
            .map_err(|error| format!("could not canonicalize transient workspace: {error}"))?;
        if workspace != self.workspace || !workspace.is_dir() {
            return Err("transient Podman workspace must be a canonical directory".to_string());
        }
        let workspace_plan = if self.mask_lethetic {
            Some(classify_external_workspace(&workspace)?)
        } else {
            None
        };
        if !self.launch_cwd.is_absolute()
            || self.launch_cwd.to_str().is_none()
            || !self
                .mounts
                .iter()
                .any(|mount| mount.is_directory && self.launch_cwd.starts_with(&mount.path))
        {
            return Err("transient Podman cwd is not covered by a directory mount".to_string());
        }
        let mut paths = BTreeSet::new();
        let mut workspace_seen = false;
        for mount in &self.mounts {
            let metadata = std::fs::symlink_metadata(&mount.path).map_err(|error| {
                format!(
                    "could not inspect transient Podman mount {}: {error}",
                    mount.path.display()
                )
            })?;
            if metadata.file_type().is_symlink() {
                return Err(format!(
                    "transient Podman mount cannot be a symlink: {}",
                    mount.path.display()
                ));
            }
            let canonical = mount.path.canonicalize().map_err(|error| {
                format!(
                    "could not canonicalize transient Podman mount {}: {error}",
                    mount.path.display()
                )
            })?;
            if canonical != mount.path
                || mount.path.to_str().is_none()
                || metadata.is_dir() != mount.is_directory
                || !metadata.is_dir() && !metadata.is_file()
            {
                return Err(format!(
                    "transient Podman mount must be a canonical regular file or directory: {}",
                    mount.path.display()
                ));
            }
            validate_volume_source(
                mount
                    .path
                    .to_str()
                    .ok_or_else(|| "transient mount is not UTF-8".to_string())?,
                "transient Podman mount",
            )?;
            if !paths.insert(mount.path.clone()) {
                return Err("transient Podman mount set contains duplicates".to_string());
            }
            if mount.path == workspace {
                workspace_seen = true;
            }
        }
        if !workspace_seen {
            return Err("transient Podman mount set omits the workspace".to_string());
        }
        if self.mask_lethetic {
            let mask = self.workspace.join(".lethetic");
            let mask = mask
                .to_str()
                .ok_or_else(|| "Lethetic control-state mask is not valid UTF-8".to_string())?;
            validate_volume_source(mask, "Lethetic control-state mask")?;
        }
        Ok(workspace_plan)
    }
}

impl RetainedPodmanConfig {
    pub fn validate(&self) -> Result<(), String> {
        self.validate_without_workspace_commit().map(|_| ())
    }

    pub(super) fn prepare_workspace_for_create(&self) -> Result<(), String> {
        if let Some(workspace_plan) = self.validate_without_workspace_commit()? {
            workspace_plan.commit()?;
        }
        Ok(())
    }

    fn validate_without_workspace_commit(&self) -> Result<Option<ExternalWorkspacePlan>, String> {
        self.validate_static()?;
        let (workspace, workspace_plan) = match self.workspace_ownership {
            RuntimeWorkspaceOwnership::Managed => (
                validate_managed_directory(&self.workspace, "managed workspace", true)?,
                None,
            ),
            RuntimeWorkspaceOwnership::ExternalLaunchCwd => {
                let plan = classify_external_workspace(&self.workspace)?;
                (plan.path().to_path_buf(), Some(plan))
            }
        };
        let broker =
            validate_managed_directory(&self.broker_directory, "runtime broker directory", true)?;
        if workspace == broker || workspace.starts_with(&broker) || broker.starts_with(&workspace) {
            return Err("workspace and broker directories must be disjoint".to_string());
        }
        Ok(workspace_plan)
    }

    pub(super) fn validate_static(&self) -> Result<(), String> {
        crate::python::backend::validate_podman_executable(&self.podman)?;
        validate_image_id(&self.image_id)?;
        validate_container_name(&self.container_name, &self.runtime_id)?;
        validate_uuid(&self.runtime_id, "runtime ID")?;
        validate_uuid(&self.session_id, "session ID")?;
        validate_lower_hex(&self.security_fingerprint, 64, "security fingerprint")?;
        validate_runtime_abi(&self.runtime_abi)?;
        if self.layout_profile == RuntimeLayoutProfile::LegacyV2Unclassified {
            return Err("retained runtime layout has not been exactly classified".to_string());
        }
        if self.worker_uid == 0 || self.worker_gid == 0 {
            return Err("retained runtime worker UID and GID must be nonzero".to_string());
        }
        #[cfg(unix)]
        {
            let current_uid = rustix::process::geteuid().as_raw();
            let current_gid = rustix::process::getegid().as_raw();
            if self.worker_uid != current_uid || self.worker_gid != current_gid {
                return Err(
                    "retained runtime worker identity must match the invoking user".to_string(),
                );
            }
        }
        if !self.workspace.is_absolute()
            || !self.workspace_destination.is_absolute()
            || !self.broker_directory.is_absolute()
            || self.workspace.to_str().is_none()
            || self.workspace_destination.to_str().is_none()
            || self.broker_directory.to_str().is_none()
            || self.workspace == self.broker_directory
            || self.workspace.starts_with(&self.broker_directory)
            || self.broker_directory.starts_with(&self.workspace)
        {
            return Err(
                "retained workspace, destination, and broker paths must be absolute UTF-8 and disjoint"
                    .to_string(),
            );
        }
        match self.workspace_ownership {
            RuntimeWorkspaceOwnership::Managed => {
                if self.workspace_destination != Path::new(CONTAINER_WORKSPACE)
                    || self.mask_lethetic
                    || self.layout_profile == RuntimeLayoutProfile::ShortSiblingSharedCwdV1
                {
                    return Err("managed retained workspace profile is inconsistent".to_string());
                }
            }
            RuntimeWorkspaceOwnership::ExternalLaunchCwd => {
                if self.workspace_destination != self.workspace
                    || !self.mask_lethetic
                    || self.layout_profile != RuntimeLayoutProfile::ShortSiblingSharedCwdV1
                {
                    return Err("external retained workspace profile is inconsistent".to_string());
                }
            }
        }
        validate_workspace_destination(&self.workspace_destination)?;
        validate_retained_argument_paths(self)?;
        Ok(())
    }
}

pub(crate) fn build_create_spec(
    config: &RetainedPodmanConfig,
) -> Result<PodmanCommandSpec, String> {
    config.validate_without_workspace_commit()?;
    require_current_runtime_abi(&config.runtime_abi)?;
    build_create_spec_without_live_path_validation(config)
}

pub(super) fn build_create_spec_without_live_path_validation(
    config: &RetainedPodmanConfig,
) -> Result<PodmanCommandSpec, String> {
    config.validate_static()?;
    let workspace = config.workspace.to_str().ok_or_else(|| {
        format!(
            "runtime workspace is not valid UTF-8: {}",
            config.workspace.display()
        )
    })?;
    let workspace_destination = config.workspace_destination.to_str().ok_or_else(|| {
        format!(
            "runtime workspace destination is not valid UTF-8: {}",
            config.workspace_destination.display()
        )
    })?;
    let broker_directory = config.broker_directory.to_str().ok_or_else(|| {
        format!(
            "runtime broker directory is not valid UTF-8: {}",
            config.broker_directory.display()
        )
    })?;
    validate_volume_source(workspace, "runtime workspace")?;
    validate_volume_source(workspace_destination, "runtime workspace destination")?;
    validate_volume_source(broker_directory, "runtime broker directory")?;

    let mut args = vec![
        OsString::from("create"),
        OsString::from("--interactive"),
        OsString::from("--pull=never"),
        OsString::from(format!("--name={}", config.container_name)),
        OsString::from("--userns=keep-id"),
        OsString::from("--user=0:0"),
        OsString::from("--network=none"),
        OsString::from("--ipc=private"),
        OsString::from("--image-volume=ignore"),
        OsString::from("--log-driver=none"),
        OsString::from("--cap-drop=ALL"),
        OsString::from("--security-opt=no-new-privileges"),
        OsString::from("--pids-limit=512"),
        OsString::from("--stop-signal=SIGTERM"),
        OsString::from("--stop-timeout=15"),
        OsString::from("--tmpfs=/tmp:rw,nosuid,nodev,noexec,mode=1777"),
        OsString::from("--tmpfs=/run:rw,nosuid,nodev,noexec,mode=0755"),
        OsString::from(format!("--workdir={workspace_destination}")),
        OsString::from(format!("--entrypoint={CONTAINER_ENTRYPOINT}")),
    ];
    match config.workspace_ownership {
        RuntimeWorkspaceOwnership::Managed => {
            args.extend([
                OsString::from("--security-opt=label=type:container_t"),
                OsString::from("--security-opt=label=filetype:container_file_t"),
                OsString::from(format!("--volume={workspace}:{workspace_destination}:rw,Z")),
                OsString::from(format!(
                    "--volume={broker_directory}:{CONTAINER_HOST_BRIDGE}:ro,Z"
                )),
            ]);
        }
        RuntimeWorkspaceOwnership::ExternalLaunchCwd => {
            args.extend([
                OsString::from("--security-opt=label=disable"),
                OsString::from(format!("--volume={workspace}:{workspace_destination}:rw")),
                OsString::from(format!(
                    "--volume={broker_directory}:{CONTAINER_HOST_BRIDGE}:ro"
                )),
            ]);
        }
    }
    if config.mask_lethetic {
        let mask = config.workspace_destination.join(".lethetic");
        let mask = mask
            .to_str()
            .ok_or_else(|| "Lethetic control-state mask is not valid UTF-8".to_string())?;
        validate_volume_source(mask, "Lethetic control-state mask")?;
        args.push(OsString::from(format!(
            "--tmpfs={mask}:ro,nosuid,nodev,noexec,notmpcopyup,mode=000,size=1048576"
        )));
    }
    for capability in REQUIRED_CAPABILITIES {
        args.push(OsString::from(format!("--cap-add={capability}")));
    }
    for (key, value) in config.expected_labels() {
        args.push(OsString::from(format!("--label={key}={value}")));
    }
    args.extend([
        OsString::from(&config.image_id),
        OsString::from("supervisor"),
        OsString::from(&config.runtime_abi),
        OsString::from(&config.runtime_id),
        OsString::from(CONTAINER_CAPABILITY_PATH),
        OsString::from(config.container_broker_socket()),
        OsString::from(workspace_destination),
        OsString::from(config.worker_uid.to_string()),
        OsString::from(config.worker_gid.to_string()),
    ]);
    Ok(PodmanCommandSpec {
        program: config.podman.clone(),
        args,
    })
}

pub(crate) fn build_transient_create_spec(
    config: &TransientPodmanConfig,
) -> Result<PodmanCommandSpec, String> {
    config.validate_without_workspace_commit()?;
    let network = match config.network {
        NetworkAccess::None => "none",
        NetworkAccess::Full => "host",
        NetworkAccess::Nonlocal => {
            return Err("Nonlocal networking requires the retained Podman runtime".to_string());
        }
    };
    let mut args = vec![
        OsString::from("create"),
        OsString::from("--interactive"),
        OsString::from("--rm"),
        OsString::from("--pull=never"),
        OsString::from(format!("--name={}", config.container_name)),
        OsString::from("--read-only"),
        OsString::from("--cap-drop=ALL"),
        OsString::from("--security-opt=no-new-privileges"),
        OsString::from("--security-opt=label=disable"),
        OsString::from("--userns=keep-id"),
        OsString::from(format!(
            "--user={}:{}",
            config.worker_uid, config.worker_gid
        )),
        OsString::from("--ipc=private"),
        OsString::from("--image-volume=ignore"),
        OsString::from("--log-driver=none"),
        OsString::from("--pids-limit=256"),
        OsString::from("--stop-signal=SIGTERM"),
        OsString::from("--stop-timeout=10"),
        OsString::from(format!("--network={network}")),
        OsString::from("--tmpfs=/tmp:rw,nosuid,nodev,noexec,mode=1777"),
        OsString::from("--tmpfs=/home/lethetic:rw,nosuid,nodev,mode=1777"),
        OsString::from("--env=HOME=/home/lethetic"),
        OsString::from("--env=LANG=C.UTF-8"),
        OsString::from("--env=PYTHONNOUSERSITE=1"),
        OsString::from("--env=PYTHONDONTWRITEBYTECODE=1"),
        OsString::from(format!("--workdir={}", config.launch_cwd.display())),
    ];
    for mount in &config.mounts {
        let access = match mount.access {
            AccessMode::ReadOnly => "ro",
            AccessMode::ReadWrite => "rw",
        };
        let path = mount
            .path
            .to_str()
            .ok_or_else(|| "transient Podman mount is not UTF-8".to_string())?;
        args.push(OsString::from(format!("--volume={path}:{path}:{access}")));
    }
    if config.mask_lethetic {
        let mask = config.workspace.join(".lethetic");
        let mask = mask
            .to_str()
            .ok_or_else(|| "Lethetic control-state mask is not valid UTF-8".to_string())?;
        validate_volume_source(mask, "Lethetic control-state mask")?;
        args.push(OsString::from(format!(
            "--tmpfs={mask}:ro,nosuid,nodev,noexec,notmpcopyup,mode=000,size=1048576"
        )));
    }
    for (key, value) in config.expected_labels() {
        args.push(OsString::from(format!("--label={key}={value}")));
    }
    args.extend([
        OsString::from("--entrypoint=python3"),
        OsString::from(&config.image_id),
        OsString::from("-u"),
        OsString::from("-B"),
        OsString::from("-c"),
        OsString::from(crate::python::WORKER_SOURCE),
    ]);
    Ok(PodmanCommandSpec {
        program: config.podman.clone(),
        args,
    })
}
pub(crate) fn build_start_attach_spec(
    podman: &Path,
    container_id: &str,
) -> Result<PodmanCommandSpec, String> {
    crate::python::backend::validate_podman_executable(podman)?;
    validate_container_id(container_id)?;
    Ok(PodmanCommandSpec {
        program: podman.to_path_buf(),
        args: vec![
            OsString::from("start"),
            OsString::from("--attach"),
            OsString::from("--interactive"),
            OsString::from("--"),
            OsString::from(container_id),
        ],
    })
}

pub(crate) fn build_stop_spec(
    podman: &Path,
    container_id: &str,
) -> Result<PodmanCommandSpec, String> {
    crate::python::backend::validate_podman_executable(podman)?;
    validate_container_id(container_id)?;
    Ok(PodmanCommandSpec {
        program: podman.to_path_buf(),
        args: vec![
            OsString::from("stop"),
            OsString::from("--time=15"),
            OsString::from("--"),
            OsString::from(container_id),
        ],
    })
}

pub(crate) fn build_remove_spec(
    podman: &Path,
    container_id: &str,
) -> Result<PodmanCommandSpec, String> {
    crate::python::backend::validate_podman_executable(podman)?;
    validate_container_id(container_id)?;
    Ok(PodmanCommandSpec {
        program: podman.to_path_buf(),
        args: vec![
            OsString::from("rm"),
            OsString::from("--"),
            OsString::from(container_id),
        ],
    })
}
pub(crate) fn build_force_remove_spec(
    podman: &Path,
    container_id: &str,
) -> Result<PodmanCommandSpec, String> {
    crate::python::backend::validate_podman_executable(podman)?;
    validate_container_id(container_id)?;
    Ok(PodmanCommandSpec {
        program: podman.to_path_buf(),
        args: vec![
            OsString::from("rm"),
            OsString::from("--force"),
            OsString::from("--time=10"),
            OsString::from("--"),
            OsString::from(container_id),
        ],
    })
}

pub(super) fn validate_workspace_destination(destination: &Path) -> Result<(), String> {
    for protected in [
        CONTAINER_ENTRYPOINT,
        CONTAINER_HOST_BRIDGE,
        CONTAINER_CAPABILITY_PATH,
        CONTAINER_BROKER_SOCKET,
        LEGACY_CONTAINER_BROKER_SOCKET,
    ] {
        let protected = Path::new(protected);
        if destination.starts_with(protected) || protected.starts_with(destination) {
            return Err(format!(
                "retained workspace destination overlaps protected container path {}",
                protected.display()
            ));
        }
    }
    for fixed_mount in [Path::new("/tmp"), Path::new("/run")] {
        if fixed_mount.starts_with(destination) {
            return Err(format!(
                "retained workspace destination conflicts with fixed container mount {}",
                fixed_mount.display()
            ));
        }
    }
    Ok(())
}

fn validate_retained_argument_paths(config: &RetainedPodmanConfig) -> Result<(), String> {
    let workspace = config.workspace.to_str().ok_or_else(|| {
        format!(
            "runtime workspace is not valid UTF-8: {}",
            config.workspace.display()
        )
    })?;
    let workspace_destination = config.workspace_destination.to_str().ok_or_else(|| {
        format!(
            "runtime workspace destination is not valid UTF-8: {}",
            config.workspace_destination.display()
        )
    })?;
    let broker_directory = config.broker_directory.to_str().ok_or_else(|| {
        format!(
            "runtime broker directory is not valid UTF-8: {}",
            config.broker_directory.display()
        )
    })?;
    validate_volume_source(workspace, "runtime workspace")?;
    validate_volume_source(workspace_destination, "runtime workspace destination")?;
    validate_volume_source(broker_directory, "runtime broker directory")?;
    if config.mask_lethetic {
        let mask = config.workspace_destination.join(".lethetic");
        let mask = mask
            .to_str()
            .ok_or_else(|| "Lethetic control-state mask is not valid UTF-8".to_string())?;
        validate_volume_source(mask, "Lethetic control-state mask")?;
    }
    Ok(())
}

fn validate_runtime_abi(runtime_abi: &str) -> Result<(), String> {
    if matches!(
        runtime_abi,
        RUNTIME_ABI | LEGACY_RUNTIME_ABI_V2 | LEGACY_RUNTIME_ABI_V3
    ) {
        Ok(())
    } else {
        Err("retained runtime ABI is unknown or unsupported".to_string())
    }
}

pub(super) fn require_current_runtime_abi(runtime_abi: &str) -> Result<(), String> {
    validate_runtime_abi(runtime_abi)?;
    if runtime_abi != RUNTIME_ABI {
        return Err("new retained containers require the current runtime ABI".to_string());
    }
    Ok(())
}

fn validate_volume_source(path: &str, label: &str) -> Result<(), String> {
    if path.contains([':', ',', '\n', '\r', '\0']) {
        return Err(format!(
            "{label} cannot be encoded safely as a Podman volume"
        ));
    }
    Ok(())
}

pub(super) fn validate_image_reference(image: &str) -> Result<(), String> {
    if image.is_empty()
        || image != image.trim()
        || image.starts_with('-')
        || image.chars().any(char::is_whitespace)
        || image.contains(['\0', '\n', '\r'])
    {
        return Err("Podman image reference is unsafe".to_string());
    }
    Ok(())
}

pub(super) fn normalize_image_id(image_id: &str) -> Result<String, String> {
    let value = image_id.strip_prefix("sha256:").unwrap_or(image_id);
    validate_lower_hex(value, 64, "image ID")?;
    Ok(format!("sha256:{value}"))
}

pub(super) fn validate_image_id(image_id: &str) -> Result<(), String> {
    if image_id.strip_prefix("sha256:").is_none() {
        return Err("runtime image must use an exact sha256 image ID".to_string());
    }
    normalize_image_id(image_id).map(|_| ())
}

pub(super) fn normalize_container_id(container_id: &str) -> Result<String, String> {
    validate_lower_hex(container_id, 64, "container ID")?;
    Ok(container_id.to_string())
}

pub fn validate_container_id(container_id: &str) -> Result<(), String> {
    normalize_container_id(container_id).map(|_| ())
}

pub(super) fn validate_transient_container_name(name: &str) -> Result<(), String> {
    let suffix = name
        .strip_prefix("lethetic-python-transient-")
        .ok_or_else(|| "transient container name has an invalid prefix".to_string())?;
    if suffix.is_empty()
        || name.len() > 128
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err("transient container name is unsafe".to_string());
    }
    Ok(())
}

pub(super) fn validate_container_name(name: &str, runtime_id: &str) -> Result<(), String> {
    let expected = format!("lethetic-python-{runtime_id}");
    if name != expected {
        return Err(format!(
            "retained container name must be exactly '{expected}'"
        ));
    }
    Ok(())
}

pub(super) fn validate_uuid(value: &str, label: &str) -> Result<(), String> {
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

fn validate_lower_hex(value: &str, length: usize, label: &str) -> Result<(), String> {
    if value.len() != length
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(format!(
            "{label} must be exactly {length} lowercase hexadecimal bytes"
        ));
    }
    Ok(())
}
