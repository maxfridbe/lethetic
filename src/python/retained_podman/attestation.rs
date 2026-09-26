use super::command::{load_container_inspection, podman_selinux_enabled};
use super::model::{
    CONTAINER_CAPABILITY_PATH, CONTAINER_ENTRYPOINT, CONTAINER_HOST_BRIDGE, ContainerInspection,
    ContainerProcessIdentity, InspectedMount, InspectedNetworkSettings, REQUIRED_CAPABILITIES,
    RUNTIME_IMAGE_LABEL, RUNTIME_IMAGE_LABEL_VALUE, RetainedPodmanConfig, TransientPodmanConfig,
};
use super::spec::{
    build_create_spec_without_live_path_validation, build_transient_create_spec,
    normalize_container_id, normalize_image_id, validate_container_id,
};
use crate::config::{AccessMode, NetworkAccess};
use crate::python::runtime_store::{
    LEGACY_RUNTIME_ABI_V2, RuntimeLayoutProfile, RuntimeWorkspaceOwnership,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Clone, Copy)]
enum RetainedAttestationPhase {
    New,
    Existing,
}

fn inspected_process_identity(
    config: &RetainedPodmanConfig,
    container_id: &str,
    inspection: &ContainerInspection,
    selinux_enabled: bool,
    phase: RetainedAttestationPhase,
) -> Result<ContainerProcessIdentity, String> {
    let inspected_id = normalize_container_id(&inspection.id)?;
    if inspected_id != container_id {
        return Err("Podman inspected a different container ID".to_string());
    }
    if inspection.state.paused
        || inspection.state.restarting
        || inspection.state.oom_killed
        || inspection.state.dead
        || inspection.state.running && inspection.state.status != "running"
        || !inspection.state.running
            && !matches!(
                inspection.state.status.as_str(),
                "configured" | "created" | "exited" | "stopped"
            )
    {
        return Err("retained container is in an unsafe or inconsistent runtime state".to_string());
    }
    if matches!(phase, RetainedAttestationPhase::New)
        && (inspection.state.running
            || inspection.state.pid != 0
            || !matches!(inspection.state.status.as_str(), "configured" | "created"))
    {
        return Err("new retained container is not in its pristine post-create state".to_string());
    }
    let selinux_labels = validate_inspected_selinux_labels(
        &inspection.process_label,
        &inspection.mount_label,
        selinux_enabled,
        config.workspace_ownership == RuntimeWorkspaceOwnership::Managed,
    )?;
    let host_pid = if inspection.state.running {
        let pid = u32::try_from(inspection.state.pid)
            .map_err(|_| "Podman returned an invalid container PID".to_string())?;
        if pid == 0 {
            return Err("running retained container has no host PID".to_string());
        }
        Some(pid)
    } else {
        if inspection.state.pid != 0 {
            return Err("stopped retained container unexpectedly has a host PID".to_string());
        }
        None
    };
    Ok(ContainerProcessIdentity {
        container_id: inspected_id,
        running: inspection.state.running,
        host_pid,
        selinux_labels,
    })
}

pub(crate) async fn attest_new_retained_container(
    config: &RetainedPodmanConfig,
    container_id: &str,
    workspace: &crate::python::runtime_store::WorkspaceIdentity,
) -> Result<ContainerProcessIdentity, String> {
    inspect_attested_container(
        config,
        container_id,
        workspace,
        RetainedAttestationPhase::New,
    )
    .await
}

pub(crate) async fn attest_new_transient_container(
    config: &TransientPodmanConfig,
    container_id: &str,
) -> Result<(), String> {
    validate_container_id(container_id)?;
    config.validate()?;
    let inspection = load_container_inspection(&config.podman, container_id).await?;
    validate_transient_inspection(config, container_id, &inspection)
}
struct TransientInspectionAttestation<'a> {
    config: &'a TransientPodmanConfig,
    container_id: &'a str,
    inspection: &'a ContainerInspection,
}

impl TransientInspectionAttestation<'_> {
    fn validate(self) -> Result<(), String> {
        self.validate_identity_and_state()?;
        self.validate_container_config()?;
        self.validate_host_config()
    }

    fn validate_identity_and_state(&self) -> Result<(), String> {
        if normalize_container_id(&self.inspection.id)? != self.container_id {
            return Err("Podman inspected a different transient container ID".to_string());
        }
        if self.inspection.state.running
            || self.inspection.state.pid != 0
            || self.inspection.state.paused
            || self.inspection.state.restarting
            || self.inspection.state.oom_killed
            || self.inspection.state.dead
            || !matches!(
                self.inspection.state.status.as_str(),
                "configured" | "created"
            )
        {
            return Err("transient container was not safely stopped after creation".to_string());
        }
        if !self.inspection.process_label.is_empty() {
            return Err(
                "label-disabled transient container unexpectedly has a process label".to_string(),
            );
        }
        if !self.inspection.mount_label.is_empty() {
            crate::python::selinux::validate_mount_label_only(&self.inspection.mount_label)?;
        }
        let worker_args = transient_worker_args();
        if self.inspection.name != self.config.container_name
            || self.inspection.path != "python3"
            || self.inspection.args != worker_args
            || normalize_image_id(&self.inspection.image)? != self.config.image_id
            || normalize_image_id(&self.inspection.config.image)? != self.config.image_id
        {
            return Err(
                "transient container identity, image, or worker command changed".to_string(),
            );
        }
        Ok(())
    }

    fn validate_container_config(&self) -> Result<(), String> {
        let expected_create = expected_transient_create_command(self.config)?;
        let container = &self.inspection.config;
        let worker_args = transient_worker_args();
        let expected_user = format!("{}:{}", self.config.worker_uid, self.config.worker_gid);
        if container.create_command != expected_create
            || container.entrypoint != ["python3"]
            || container.cmd != worker_args
            || container.user != expected_user
            || !container.open_stdin
            || container.stdin_once
            || container.tty
            || container.working_dir != self.config.launch_cwd.to_string_lossy()
            || container.stop_signal != "SIGTERM"
            || container.stop_timeout != 10
        {
            return Err("transient container immutable create configuration changed".to_string());
        }
        let labels = self.config.expected_labels();
        for (key, value) in &labels {
            if container.labels.get(key) != Some(value) {
                return Err(format!(
                    "transient container label {key} is missing or changed"
                ));
            }
        }
        if container
            .labels
            .keys()
            .any(|key| key.starts_with("org.lethetic.transient-") && !labels.contains_key(key))
        {
            return Err("transient container has an unexpected Lethetic-owned label".to_string());
        }
        for required in [
            "HOME=/home/lethetic",
            "LANG=C.UTF-8",
            "PYTHONNOUSERSITE=1",
            "PYTHONDONTWRITEBYTECODE=1",
        ] {
            if !container.env.iter().any(|entry| entry == required) {
                return Err(format!("transient container environment lost {required}"));
            }
        }
        if container.env.iter().any(|entry| {
            let key = entry.split_once('=').map(|(key, _)| key).unwrap_or(entry);
            matches!(
                key.to_ascii_uppercase().as_str(),
                "HTTP_PROXY" | "HTTPS_PROXY" | "ALL_PROXY" | "NO_PROXY"
            )
        }) {
            return Err(
                "transient container unexpectedly has proxy environment variables".to_string(),
            );
        }
        Ok(())
    }

    fn validate_host_config(&self) -> Result<(), String> {
        let expected_annotations = BTreeMap::from([
            (
                "io.podman.annotations.autoremove".to_string(),
                "TRUE".to_string(),
            ),
            (
                "io.podman.annotations.label".to_string(),
                "disable".to_string(),
            ),
            (
                "io.podman.annotations.pids-limit".to_string(),
                "256".to_string(),
            ),
            (
                "io.podman.annotations.userns".to_string(),
                "keep-id".to_string(),
            ),
        ]);
        validate_podman_annotations(&self.inspection.config.annotations, &expected_annotations)?;
        validate_podman_annotations(
            &self.inspection.host_config.annotations,
            &expected_annotations,
        )?;
        let host = &self.inspection.host_config;
        let expected_network = match self.config.network {
            NetworkAccess::None => "none",
            NetworkAccess::Full => "host",
            NetworkAccess::Nonlocal => unreachable!("validated above"),
        };
        if !host.auto_remove
            || host.privileged
            || host.publish_all_ports
            || !host.readonly_rootfs
            || host.network_mode != expected_network
            || host.ipc_mode != "private"
            || host.pid_mode != "private"
            || host.uts_mode != "private"
            || host.cgroup_mode != "private"
            || host.userns_mode != "private"
            || host.pids_limit != 256
            || host.log_config.log_type != "none"
            || host.restart_policy.name != "no"
            || host.restart_policy.maximum_retry_count != 0
            || !host.devices.is_empty()
            || !host.group_add.is_empty()
            || !host.port_bindings.is_empty()
        {
            return Err("transient container host isolation configuration changed".to_string());
        }
        let security = host
            .security_opt
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        if security != BTreeSet::from(["label=disable", "no-new-privileges"])
            || security.len() != host.security_opt.len()
        {
            return Err("transient container security options changed".to_string());
        }
        if !self.inspection.effective_caps.is_empty() {
            return Err("transient container unexpectedly has effective capabilities".to_string());
        }
        validate_id_map(&host.id_mappings.uid_map, self.config.worker_uid, "UID")?;
        validate_id_map(&host.id_mappings.gid_map, self.config.worker_gid, "GID")?;
        validate_transient_tmpfs(self.config, &host.tmpfs)?;
        validate_transient_binds(self.config, &host.binds)?;
        validate_transient_mounts(self.config, &self.inspection.mounts)?;
        match self.config.network {
            NetworkAccess::None => validate_none_network(&self.inspection.network_settings)?,
            NetworkAccess::Full => validate_host_network(&self.inspection.network_settings)?,
            NetworkAccess::Nonlocal => unreachable!("validated above"),
        }
        Ok(())
    }
}

fn transient_worker_args() -> Vec<String> {
    vec![
        "-u".to_string(),
        "-B".to_string(),
        "-c".to_string(),
        crate::python::WORKER_SOURCE.to_string(),
    ]
}

pub(super) fn validate_transient_inspection(
    config: &TransientPodmanConfig,
    container_id: &str,
    inspection: &ContainerInspection,
) -> Result<(), String> {
    TransientInspectionAttestation {
        config,
        container_id,
        inspection,
    }
    .validate()
}

pub(super) fn expected_transient_create_command(
    config: &TransientPodmanConfig,
) -> Result<Vec<String>, String> {
    let spec = build_transient_create_spec(config)?;
    let mut command = vec!["podman".to_string()];
    for argument in spec.args {
        command.push(
            argument
                .into_string()
                .map_err(|_| "transient Podman create argument is not UTF-8".to_string())?,
        );
    }
    Ok(command)
}

fn validate_transient_tmpfs(
    config: &TransientPodmanConfig,
    tmpfs: &BTreeMap<String, String>,
) -> Result<(), String> {
    let expected_len = if config.mask_lethetic { 3 } else { 2 };
    if tmpfs.len() != expected_len {
        return Err("transient container tmpfs set changed".to_string());
    }
    validate_option_set(
        tmpfs
            .get("/tmp")
            .ok_or_else(|| "transient container /tmp tmpfs is missing".to_string())?,
        &[
            "rw",
            "nosuid",
            "nodev",
            "noexec",
            "mode=1777",
            "rprivate",
            "tmpcopyup",
        ],
        "transient container /tmp tmpfs",
    )?;
    validate_option_set(
        tmpfs
            .get("/home/lethetic")
            .ok_or_else(|| "transient container /home/lethetic tmpfs is missing".to_string())?,
        &[
            "rw",
            "nosuid",
            "nodev",
            "mode=1777",
            "rprivate",
            "tmpcopyup",
        ],
        "transient container /home/lethetic tmpfs",
    )?;
    if config.mask_lethetic {
        let mask_path = config.workspace.join(".lethetic");
        let mask = mask_path.to_string_lossy();
        validate_option_set(
            tmpfs
                .get(mask.as_ref())
                .ok_or_else(|| "transient container .lethetic mask tmpfs is missing".to_string())?,
            &[
                "ro",
                "nosuid",
                "nodev",
                "noexec",
                "mode=000",
                "size=1048576",
                "rprivate",
            ],
            "transient container .lethetic mask tmpfs",
        )?;
    }
    Ok(())
}

fn validate_transient_binds(
    config: &TransientPodmanConfig,
    binds: &[String],
) -> Result<(), String> {
    if binds.len() != config.mounts.len() {
        return Err("transient container bind set changed".to_string());
    }
    for mount in &config.mounts {
        let destination = mount
            .path
            .to_str()
            .ok_or_else(|| "transient mount destination is not UTF-8".to_string())?;
        let access = match mount.access {
            AccessMode::ReadOnly => "ro",
            AccessMode::ReadWrite => "rw",
        };
        validate_bind(binds, &mount.path, destination, access, false)?;
    }
    Ok(())
}

fn validate_transient_mounts(
    config: &TransientPodmanConfig,
    mounts: &[InspectedMount],
) -> Result<(), String> {
    if mounts.len() != config.mounts.len() {
        return Err("transient container mount set changed".to_string());
    }
    for expected in &config.mounts {
        let destination = expected
            .path
            .to_str()
            .ok_or_else(|| "transient mount destination is not UTF-8".to_string())?;
        validate_mount(
            mounts,
            &expected.path,
            destination,
            expected.access == AccessMode::ReadWrite,
            false,
        )?;
    }
    Ok(())
}

fn validate_host_network(network: &InspectedNetworkSettings) -> Result<(), String> {
    if network.networks.len() != 1 {
        return Err("transient full-network attachment set changed".to_string());
    }
    let host = network
        .networks
        .get("host")
        .ok_or_else(|| "transient container is not attached only to host networking".to_string())?;
    if host.network_id != "host"
        || !host.gateway.is_empty()
        || !host.ip_address.is_empty()
        || !host.ipv6_gateway.is_empty()
        || !host.global_ipv6_address.is_empty()
    {
        return Err("transient host network unexpectedly has container addressing".to_string());
    }
    Ok(())
}

pub(crate) async fn attest_retained_container_with_unbound_labels(
    config: &RetainedPodmanConfig,
    container_id: &str,
    workspace: &crate::python::runtime_store::WorkspaceIdentity,
) -> Result<ContainerProcessIdentity, String> {
    inspect_attested_container(
        config,
        container_id,
        workspace,
        RetainedAttestationPhase::Existing,
    )
    .await
}

pub(crate) async fn attest_retained_container(
    config: &RetainedPodmanConfig,
    container_id: &str,
    workspace: &crate::python::runtime_store::WorkspaceIdentity,
    expected_selinux_labels: Option<&crate::python::selinux::SelinuxLabels>,
) -> Result<ContainerProcessIdentity, String> {
    let identity =
        attest_retained_container_with_unbound_labels(config, container_id, workspace).await?;
    enforce_expected_selinux_labels(identity, expected_selinux_labels)
}

pub(super) fn enforce_expected_selinux_labels(
    identity: ContainerProcessIdentity,
    expected_selinux_labels: Option<&crate::python::selinux::SelinuxLabels>,
) -> Result<ContainerProcessIdentity, String> {
    if identity.selinux_labels.as_ref() != expected_selinux_labels {
        return Err("retained container SELinux labels no longer match its manifest".to_string());
    }
    Ok(identity)
}

pub(crate) async fn attest_frozen_schema_v2_container(
    config: &RetainedPodmanConfig,
    container_id: &str,
    expected_selinux_labels: Option<&crate::python::selinux::SelinuxLabels>,
) -> Result<ContainerProcessIdentity, String> {
    if !matches!(
        config.layout_profile,
        RuntimeLayoutProfile::LegacyRuntimeLocalV2 | RuntimeLayoutProfile::ShortSiblingV2Imported
    ) {
        return Err("schema-v2 classification requires a frozen historical layout".to_string());
    }
    config.validate_static()?;
    if config.runtime_abi != LEGACY_RUNTIME_ABI_V2 {
        return Err("schema-v2 classification requires the legacy runtime ABI".to_string());
    }
    validate_container_id(container_id)?;
    let inspection = load_container_inspection(&config.podman, container_id).await?;
    let identity = validate_retained_inspection(
        config,
        container_id,
        &inspection,
        podman_selinux_enabled(&config.podman).await?,
    )?;
    if let Some(expected) = expected_selinux_labels
        && identity.selinux_labels.as_ref() != Some(expected)
    {
        return Err("schema-v2 container SELinux labels do not match its manifest".to_string());
    }
    Ok(identity)
}

async fn inspect_attested_container(
    config: &RetainedPodmanConfig,
    container_id: &str,
    workspace: &crate::python::runtime_store::WorkspaceIdentity,
    phase: RetainedAttestationPhase,
) -> Result<ContainerProcessIdentity, String> {
    validate_container_id(container_id)?;
    validate_attestation_workspace(config, workspace)?;
    config.validate()?;
    let inspection = load_container_inspection(&config.podman, container_id).await?;
    validate_retained_inspection_for_phase(
        config,
        container_id,
        &inspection,
        podman_selinux_enabled(&config.podman).await?,
        phase,
    )
}

pub(super) fn validate_attestation_workspace(
    config: &RetainedPodmanConfig,
    workspace: &crate::python::runtime_store::WorkspaceIdentity,
) -> Result<(), String> {
    workspace.verify_current()?;
    if workspace.canonical_path != config.workspace {
        return Err(
            "retained container config does not match the manifest workspace identity".to_string(),
        );
    }
    Ok(())
}

struct RetainedInspectionAttestation<'a> {
    config: &'a RetainedPodmanConfig,
    container_id: &'a str,
    inspection: &'a ContainerInspection,
}

impl RetainedInspectionAttestation<'_> {
    fn validate(
        self,
        selinux_enabled: bool,
        phase: RetainedAttestationPhase,
    ) -> Result<ContainerProcessIdentity, String> {
        let identity = inspected_process_identity(
            self.config,
            self.container_id,
            self.inspection,
            selinux_enabled,
            phase,
        )?;
        let expected_args = self.validate_runtime_identity()?;
        self.validate_container_config(&expected_args)?;
        self.validate_host_config()?;
        Ok(identity)
    }

    fn validate_runtime_identity(&self) -> Result<Vec<String>, String> {
        let expected_args = expected_supervisor_args(self.config);
        if self.inspection.name != self.config.container_name
            || self.inspection.path != CONTAINER_ENTRYPOINT
            || self.inspection.args != expected_args
        {
            return Err(
                "retained container name, entrypoint, or supervisor arguments changed".to_string(),
            );
        }
        if normalize_image_id(&self.inspection.image)? != self.config.image_id
            || normalize_image_id(&self.inspection.config.image)? != self.config.image_id
        {
            return Err("retained container image ID no longer matches its manifest".to_string());
        }
        Ok(expected_args)
    }

    fn validate_container_config(&self, expected_args: &[String]) -> Result<(), String> {
        let expected_create = expected_create_command(self.config)?;
        let container = &self.inspection.config;
        if container.create_command != expected_create
            || container.entrypoint != [CONTAINER_ENTRYPOINT]
            || container.cmd != expected_args
            || container.user != "0:0"
            || !container.open_stdin
            || container.stdin_once
            || container.tty
            || container.working_dir != self.config.workspace_destination.to_string_lossy()
            || container.stop_signal != "SIGTERM"
            || container.stop_timeout != 15
        {
            return Err("retained container immutable create configuration changed".to_string());
        }
        validate_container_labels(self.config, &container.labels)
    }

    fn validate_host_config(&self) -> Result<(), String> {
        let expected_label_annotation = match self.config.workspace_ownership {
            RuntimeWorkspaceOwnership::Managed => {
                "type:container_t,label=filetype:container_file_t"
            }
            RuntimeWorkspaceOwnership::ExternalLaunchCwd => "disable",
        };
        let expected_annotations = BTreeMap::from([
            (
                "io.podman.annotations.label".to_string(),
                expected_label_annotation.to_string(),
            ),
            (
                "io.podman.annotations.pids-limit".to_string(),
                "512".to_string(),
            ),
            (
                "io.podman.annotations.userns".to_string(),
                "keep-id".to_string(),
            ),
        ]);
        validate_podman_annotations(&self.inspection.config.annotations, &expected_annotations)?;
        validate_podman_annotations(
            &self.inspection.host_config.annotations,
            &expected_annotations,
        )?;

        let host = &self.inspection.host_config;
        if host.auto_remove
            || host.privileged
            || host.publish_all_ports
            || host.readonly_rootfs
            || host.network_mode != "none"
            || host.ipc_mode != "private"
            || host.pid_mode != "private"
            || host.uts_mode != "private"
            || host.cgroup_mode != "private"
            || host.userns_mode != "private"
            || host.pids_limit != 512
            || host.log_config.log_type != "none"
            || host.restart_policy.name != "no"
            || host.restart_policy.maximum_retry_count != 0
            || !host.devices.is_empty()
            || !host.group_add.is_empty()
            || !host.port_bindings.is_empty()
        {
            return Err("retained container host isolation configuration changed".to_string());
        }
        validate_security_options(self.config, &host.security_opt)?;
        validate_effective_capabilities(&self.inspection.effective_caps)?;
        validate_id_map(&host.id_mappings.uid_map, self.config.worker_uid, "UID")?;
        validate_id_map(&host.id_mappings.gid_map, self.config.worker_gid, "GID")?;
        validate_tmpfs(self.config, &host.tmpfs)?;
        validate_binds(self.config, &host.binds)?;
        validate_mounts(self.config, &self.inspection.mounts)?;
        validate_none_network(&self.inspection.network_settings)
    }
}

fn validate_retained_inspection_for_phase(
    config: &RetainedPodmanConfig,
    container_id: &str,
    inspection: &ContainerInspection,
    selinux_enabled: bool,
    phase: RetainedAttestationPhase,
) -> Result<ContainerProcessIdentity, String> {
    RetainedInspectionAttestation {
        config,
        container_id,
        inspection,
    }
    .validate(selinux_enabled, phase)
}

pub(super) fn validate_retained_inspection(
    config: &RetainedPodmanConfig,
    container_id: &str,
    inspection: &ContainerInspection,
    selinux_enabled: bool,
) -> Result<ContainerProcessIdentity, String> {
    validate_retained_inspection_for_phase(
        config,
        container_id,
        inspection,
        selinux_enabled,
        RetainedAttestationPhase::Existing,
    )
}

#[cfg(test)]
pub(super) fn validate_new_retained_inspection(
    config: &RetainedPodmanConfig,
    container_id: &str,
    inspection: &ContainerInspection,
    selinux_enabled: bool,
) -> Result<ContainerProcessIdentity, String> {
    validate_retained_inspection_for_phase(
        config,
        container_id,
        inspection,
        selinux_enabled,
        RetainedAttestationPhase::New,
    )
}

pub(super) fn expected_supervisor_args(config: &RetainedPodmanConfig) -> Vec<String> {
    vec![
        "supervisor".to_string(),
        config.runtime_abi.clone(),
        config.runtime_id.clone(),
        CONTAINER_CAPABILITY_PATH.to_string(),
        config.container_broker_socket().to_string(),
        config.workspace_destination.to_string_lossy().into_owned(),
        config.worker_uid.to_string(),
        config.worker_gid.to_string(),
    ]
}

pub(super) fn expected_create_command(
    config: &RetainedPodmanConfig,
) -> Result<Vec<String>, String> {
    let spec = build_create_spec_without_live_path_validation(config)?;
    let mut command = vec!["podman".to_string()];
    for argument in spec.args {
        command.push(
            argument
                .into_string()
                .map_err(|_| "retained Podman create argument is not UTF-8".to_string())?,
        );
    }
    Ok(command)
}

fn validate_podman_annotations(
    observed: &BTreeMap<String, String>,
    expected: &BTreeMap<String, String>,
) -> Result<(), String> {
    for (key, value) in expected {
        if observed.get(key) != Some(value) {
            return Err(format!(
                "retained container Podman annotation {key} changed"
            ));
        }
    }
    if observed
        .keys()
        .any(|key| key.starts_with("io.podman.annotations.") && !expected.contains_key(key))
    {
        return Err(
            "retained container has an unexpected security-relevant Podman annotation".to_string(),
        );
    }
    Ok(())
}

fn validate_container_labels(
    config: &RetainedPodmanConfig,
    observed: &BTreeMap<String, String>,
) -> Result<(), String> {
    let expected = config.expected_labels();
    for (key, value) in &expected {
        if observed.get(key) != Some(value) {
            return Err(format!(
                "retained container label {key} is missing or changed"
            ));
        }
    }
    if observed.get(RUNTIME_IMAGE_LABEL).map(String::as_str) != Some(RUNTIME_IMAGE_LABEL_VALUE) {
        return Err("retained container lost its trusted runtime image label".to_string());
    }
    let mut permitted = expected.keys().map(String::as_str).collect::<BTreeSet<_>>();
    permitted.insert(RUNTIME_IMAGE_LABEL);
    if observed
        .keys()
        .any(|key| key.starts_with("org.lethetic.") && !permitted.contains(key.as_str()))
    {
        return Err("retained container has an unexpected Lethetic-owned label".to_string());
    }
    Ok(())
}

fn validate_security_options(
    config: &RetainedPodmanConfig,
    options: &[String],
) -> Result<(), String> {
    let observed = options.iter().map(String::as_str).collect::<BTreeSet<_>>();
    let expected = match config.workspace_ownership {
        RuntimeWorkspaceOwnership::Managed => BTreeSet::from([
            "label=type:container_t,label=filetype:container_file_t",
            "no-new-privileges",
        ]),
        RuntimeWorkspaceOwnership::ExternalLaunchCwd => {
            BTreeSet::from(["label=disable", "no-new-privileges"])
        }
    };
    if observed != expected || observed.len() != options.len() {
        return Err("retained container security options changed".to_string());
    }
    Ok(())
}

fn validate_effective_capabilities(capabilities: &[String]) -> Result<(), String> {
    let observed = capabilities
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let expected = REQUIRED_CAPABILITIES
        .iter()
        .map(|capability| format!("CAP_{capability}"))
        .collect::<BTreeSet<_>>();
    if observed.len() != capabilities.len()
        || observed != expected.iter().map(String::as_str).collect::<BTreeSet<_>>()
    {
        return Err("retained container effective capabilities changed".to_string());
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct IdMapRange {
    container: u32,
    host: u32,
    size: u32,
}

fn validate_id_map(mappings: &[String], worker_id: u32, label: &str) -> Result<(), String> {
    let ranges = mappings
        .iter()
        .map(|mapping| parse_id_map(mapping, label))
        .collect::<Result<Vec<_>, _>>()?;
    if ranges.is_empty() {
        return Err(format!("retained container {label} mapping is empty"));
    }
    for (index, range) in ranges.iter().enumerate() {
        let container_end = range
            .container
            .checked_add(range.size)
            .ok_or_else(|| format!("retained container {label} mapping overflows"))?;
        let host_end = range
            .host
            .checked_add(range.size)
            .ok_or_else(|| format!("retained container {label} mapping overflows"))?;
        for other in &ranges[index + 1..] {
            let other_container_end = other
                .container
                .checked_add(other.size)
                .ok_or_else(|| format!("retained container {label} mapping overflows"))?;
            let other_host_end = other
                .host
                .checked_add(other.size)
                .ok_or_else(|| format!("retained container {label} mapping overflows"))?;
            if range.container < other_container_end && other.container < container_end
                || range.host < other_host_end && other.host < host_end
            {
                return Err(format!("retained container {label} mappings overlap"));
            }
        }
    }
    if !ranges
        .iter()
        .any(|range| range.container == worker_id && range.host == 0 && range.size == 1)
    {
        return Err(format!(
            "retained container {label} mapping does not keep the invoking identity"
        ));
    }
    let root = ranges
        .iter()
        .find(|range| range.container == 0)
        .ok_or_else(|| format!("retained container {label} mapping omits container root"))?;
    if root.host == 0 || root.size == 0 {
        return Err(format!(
            "retained container root maps to the invoking host {label}"
        ));
    }
    Ok(())
}

fn parse_id_map(value: &str, label: &str) -> Result<IdMapRange, String> {
    let values = value
        .split(':')
        .map(|part| {
            part.parse::<u32>()
                .map_err(|_| format!("retained container {label} mapping is malformed"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if values.len() != 3 || values[2] == 0 {
        return Err(format!("retained container {label} mapping is malformed"));
    }
    Ok(IdMapRange {
        container: values[0],
        host: values[1],
        size: values[2],
    })
}

fn validate_tmpfs(
    config: &RetainedPodmanConfig,
    tmpfs: &BTreeMap<String, String>,
) -> Result<(), String> {
    let expected_len = if config.mask_lethetic { 3 } else { 2 };
    if tmpfs.len() != expected_len {
        return Err("retained container tmpfs set changed".to_string());
    }
    validate_option_set(
        tmpfs
            .get("/tmp")
            .ok_or_else(|| "retained container /tmp tmpfs is missing".to_string())?,
        &[
            "rw",
            "nosuid",
            "nodev",
            "noexec",
            "mode=1777",
            "rprivate",
            "tmpcopyup",
        ],
        "retained container /tmp tmpfs",
    )?;
    validate_option_set(
        tmpfs
            .get("/run")
            .ok_or_else(|| "retained container /run tmpfs is missing".to_string())?,
        &[
            "rw",
            "nosuid",
            "nodev",
            "noexec",
            "mode=0755",
            "rprivate",
            "tmpcopyup",
        ],
        "retained container /run tmpfs",
    )?;
    if config.mask_lethetic {
        let mask = config
            .workspace_destination
            .join(".lethetic")
            .to_string_lossy()
            .into_owned();
        validate_option_set(
            tmpfs
                .get(&mask)
                .ok_or_else(|| "retained container .lethetic mask tmpfs is missing".to_string())?,
            &[
                "ro",
                "nosuid",
                "nodev",
                "noexec",
                "mode=000",
                "size=1048576",
                "rprivate",
            ],
            "retained container .lethetic mask tmpfs",
        )?;
    }
    Ok(())
}

fn validate_binds(config: &RetainedPodmanConfig, binds: &[String]) -> Result<(), String> {
    // Podman may retain :Z immediately after create and omit it from normalized
    // Binds/Mounts after start. The exact create command and paired private
    // SELinux labels are checked independently.
    if binds.len() != 2 {
        return Err("retained container bind set changed".to_string());
    }
    let destination = config
        .workspace_destination
        .to_str()
        .ok_or_else(|| "retained workspace destination is not UTF-8".to_string())?;
    validate_bind(
        binds,
        &config.workspace,
        destination,
        "rw",
        config.workspace_ownership == RuntimeWorkspaceOwnership::Managed,
    )?;
    validate_bind(
        binds,
        &config.broker_directory,
        CONTAINER_HOST_BRIDGE,
        "ro",
        config.workspace_ownership == RuntimeWorkspaceOwnership::Managed,
    )
}

fn validate_bind(
    binds: &[String],
    source: &Path,
    destination: &str,
    access: &str,
    relabeled: bool,
) -> Result<(), String> {
    let source = source
        .to_str()
        .ok_or_else(|| "retained bind source is not UTF-8".to_string())?;
    let prefix = format!("{source}:{destination}:");
    let value = binds
        .iter()
        .find_map(|bind| bind.strip_prefix(&prefix))
        .ok_or_else(|| format!("retained bind for {destination} is missing"))?;
    validate_bind_option_set(
        value,
        access,
        relabeled,
        &format!("retained bind for {destination}"),
    )
}

fn validate_mounts(config: &RetainedPodmanConfig, mounts: &[InspectedMount]) -> Result<(), String> {
    if mounts.len() != 2 {
        return Err("retained container mount set changed".to_string());
    }
    let destination = config
        .workspace_destination
        .to_str()
        .ok_or_else(|| "retained workspace destination is not UTF-8".to_string())?;
    validate_mount(
        mounts,
        &config.workspace,
        destination,
        true,
        config.workspace_ownership == RuntimeWorkspaceOwnership::Managed,
    )?;
    validate_mount(
        mounts,
        &config.broker_directory,
        CONTAINER_HOST_BRIDGE,
        false,
        config.workspace_ownership == RuntimeWorkspaceOwnership::Managed,
    )
}

fn validate_mount(
    mounts: &[InspectedMount],
    source: &Path,
    destination: &str,
    read_write: bool,
    _relabeled: bool,
) -> Result<(), String> {
    let source = source
        .to_str()
        .ok_or_else(|| "retained mount source is not UTF-8".to_string())?;
    let mount = mounts
        .iter()
        .find(|mount| mount.destination == destination)
        .ok_or_else(|| format!("retained mount for {destination} is missing"))?;
    let mode_matches = mount.mode.is_empty() || _relabeled && mount.mode == "Z";
    if mount.mount_type != "bind"
        || mount.source != source
        || !mode_matches
        || mount.read_write != read_write
        || mount.propagation != "rprivate"
    {
        return Err(format!(
            "retained mount for {destination} changed (observed {mount:?})"
        ));
    }
    let observed = mount
        .options
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let required = BTreeSet::from(["rbind"]);
    let permitted = BTreeSet::from(["rbind", "nosuid", "nodev"]);
    if !required.is_subset(&observed)
        || !observed.is_subset(&permitted)
        || observed.len() != mount.options.len()
    {
        return Err(format!(
            "retained mount for {destination} changed (observed {mount:?})"
        ));
    }
    Ok(())
}

fn validate_none_network(network: &InspectedNetworkSettings) -> Result<(), String> {
    if network.networks.len() != 1 {
        return Err("retained container network attachment set changed".to_string());
    }
    let none = network
        .networks
        .get("none")
        .ok_or_else(|| "retained container is not attached only to the none network".to_string())?;
    if none.network_id != "none"
        || !none.gateway.is_empty()
        || !none.ip_address.is_empty()
        || !none.ipv6_gateway.is_empty()
        || !none.global_ipv6_address.is_empty()
    {
        return Err("retained container none network unexpectedly has addressing".to_string());
    }
    Ok(())
}

fn validate_bind_option_set(
    value: &str,
    access: &str,
    relabeled: bool,
    label: &str,
) -> Result<(), String> {
    let observed = value.split(',').collect::<BTreeSet<_>>();
    let required = BTreeSet::from([access, "rprivate", "rbind"]);
    let mut permitted = BTreeSet::from([access, "rprivate", "rbind", "nosuid", "nodev"]);
    if relabeled {
        permitted.insert("Z");
    }
    if !required.is_subset(&observed)
        || !observed.is_subset(&permitted)
        || observed.len() != value.split(',').count()
    {
        return Err(format!("{label} options changed (observed {value:?})"));
    }
    Ok(())
}

fn validate_option_set(value: &str, expected: &[&str], label: &str) -> Result<(), String> {
    let observed = value.split(',').collect::<BTreeSet<_>>();
    let expected = expected.iter().copied().collect::<BTreeSet<_>>();
    if observed != expected || observed.len() != value.split(',').count() {
        return Err(format!("{label} options changed"));
    }
    Ok(())
}

pub(super) fn validate_inspected_selinux_labels(
    process_label: &str,
    mount_label: &str,
    selinux_enabled: bool,
    labels_required: bool,
) -> Result<Option<crate::python::selinux::SelinuxLabels>, String> {
    if !labels_required {
        if !process_label.is_empty() {
            return Err(
                "label-disabled retained container unexpectedly has a process label".to_string(),
            );
        }
        if !mount_label.is_empty() {
            crate::python::selinux::validate_mount_label_only(mount_label)?;
        }
        return Ok(None);
    }
    match (
        selinux_enabled,
        process_label.is_empty(),
        mount_label.is_empty(),
    ) {
        (false, true, true) => Ok(None),
        (false, _, _) => Err(
            "SELinux-disabled Podman unexpectedly returned container security labels".to_string(),
        ),
        (true, false, false) => Ok(Some(crate::python::selinux::SelinuxLabels::new(
            process_label.to_string(),
            mount_label.to_string(),
        )?)),
        (true, true, true) => {
            Err("SELinux-enabled Podman returned no ProcessLabel or MountLabel".to_string())
        }
        (true, _, _) => {
            Err("Podman container inspect returned incomplete SELinux labels".to_string())
        }
    }
}

pub(crate) async fn verify_exact_container_stopped(
    podman: &Path,
    container_id: &str,
) -> Result<(), String> {
    let inspection = load_container_inspection(podman, container_id).await?;
    let inspected_id = normalize_container_id(&inspection.id)?;
    if inspected_id != container_id {
        return Err("Podman inspected a different container ID during teardown".to_string());
    }
    if inspection.state.running
        || inspection.state.pid != 0
        || inspection.state.paused
        || inspection.state.restarting
        || !matches!(
            inspection.state.status.as_str(),
            "configured" | "created" | "exited" | "stopped"
        )
    {
        return Err("exact manifest-bound container did not reach a stopped state".to_string());
    }
    Ok(())
}
