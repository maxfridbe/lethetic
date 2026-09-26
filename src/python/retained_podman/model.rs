use crate::config::{AccessMode, NetworkAccess};
use crate::python::runtime_store::{RuntimeLayoutProfile, RuntimeWorkspaceOwnership};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

pub const RUNTIME_IMAGE_LABEL: &str = "org.lethetic.runtime";
pub const RUNTIME_IMAGE_LABEL_VALUE: &str = "python-nonlocal-v1";
pub const MANAGED_LABEL: &str = "org.lethetic.managed";
pub const RUNTIME_ID_LABEL: &str = "org.lethetic.runtime-id";
pub const SESSION_ID_LABEL: &str = "org.lethetic.session-id";
pub const FINGERPRINT_LABEL: &str = "org.lethetic.security-fingerprint";
pub const ABI_LABEL: &str = "org.lethetic.runtime.abi";
pub const LAYOUT_LABEL: &str = "org.lethetic.runtime.layout";
pub const CONTAINER_ENTRYPOINT: &str = "/usr/local/libexec/lethetic/lethetic-runtime";
pub const CONTAINER_WORKSPACE: &str = "/workspace";
pub const CONTAINER_HOST_BRIDGE: &str = "/run/lethetic-host";
pub const CONTAINER_CAPABILITY_PATH: &str = "/run/lethetic-host/capability";
pub const CONTAINER_BROKER_SOCKET: &str = "/run/lethetic-host/s";
pub const LEGACY_CONTAINER_BROKER_SOCKET: &str = "/run/lethetic-host/broker.sock";
pub const TRANSIENT_LABEL: &str = "org.lethetic.transient-python";
pub const TRANSIENT_FINGERPRINT_LABEL: &str = "org.lethetic.transient-fingerprint";

pub(super) const REQUIRED_CAPABILITIES: &[&str] = &[
    "CHOWN",
    "DAC_OVERRIDE",
    "FOWNER",
    "FSETID",
    "SETFCAP",
    "SETGID",
    "SETPCAP",
    "SETUID",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRuntimeImage {
    pub requested: String,
    pub image_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TransientPodmanMount {
    pub path: PathBuf,
    pub access: AccessMode,
    pub is_directory: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TransientPodmanConfig {
    pub podman: PathBuf,
    pub image_id: String,
    pub container_name: String,
    pub security_fingerprint: String,
    pub workspace: PathBuf,
    pub launch_cwd: PathBuf,
    pub mounts: Vec<TransientPodmanMount>,
    pub network: NetworkAccess,
    pub mask_lethetic: bool,
    pub worker_uid: u32,
    pub worker_gid: u32,
}

impl TransientPodmanConfig {
    pub(super) fn expected_labels(&self) -> BTreeMap<String, String> {
        BTreeMap::from([
            (TRANSIENT_LABEL.to_string(), "true".to_string()),
            (
                TRANSIENT_FINGERPRINT_LABEL.to_string(),
                self.security_fingerprint.clone(),
            ),
        ])
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetainedPodmanConfig {
    pub podman: PathBuf,
    pub image_id: String,
    pub container_name: String,
    pub runtime_id: String,
    pub session_id: String,
    pub security_fingerprint: String,
    pub runtime_abi: String,
    pub layout_profile: RuntimeLayoutProfile,
    pub workspace: PathBuf,
    pub workspace_destination: PathBuf,
    pub workspace_ownership: RuntimeWorkspaceOwnership,
    pub mask_lethetic: bool,
    pub broker_directory: PathBuf,
    pub worker_uid: u32,
    pub worker_gid: u32,
}

impl RetainedPodmanConfig {
    pub fn expected_labels(&self) -> BTreeMap<String, String> {
        let mut labels = BTreeMap::from([
            (MANAGED_LABEL.to_string(), "true".to_string()),
            (RUNTIME_ID_LABEL.to_string(), self.runtime_id.clone()),
            (SESSION_ID_LABEL.to_string(), self.session_id.clone()),
            (
                FINGERPRINT_LABEL.to_string(),
                self.security_fingerprint.clone(),
            ),
            (ABI_LABEL.to_string(), self.runtime_abi.clone()),
        ]);
        match self.layout_profile {
            RuntimeLayoutProfile::ShortSiblingV1 => {
                labels.insert(LAYOUT_LABEL.to_string(), "short-sibling-v1".to_string());
            }
            RuntimeLayoutProfile::ShortSiblingSharedCwdV1 => {
                labels.insert(
                    LAYOUT_LABEL.to_string(),
                    "short-sibling-shared-cwd-v1".to_string(),
                );
            }
            _ => {}
        }
        labels
    }

    pub(super) fn container_broker_socket(&self) -> &'static str {
        if self.layout_profile == RuntimeLayoutProfile::LegacyRuntimeLocalV2 {
            LEGACY_CONTAINER_BROKER_SOCKET
        } else {
            CONTAINER_BROKER_SOCKET
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PodmanCommandSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerProcessIdentity {
    pub container_id: String,
    pub running: bool,
    pub host_pid: Option<u32>,
    pub selinux_labels: Option<crate::python::selinux::SelinuxLabels>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContainerPresence {
    Present,
    Absent,
}

fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Option::<T>::deserialize(deserializer).map(Option::unwrap_or_default)
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct ImageInspection {
    pub(super) id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub(super) labels: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct ContainerInspection {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) path: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub(super) args: Vec<String>,
    pub(super) image: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub(super) effective_caps: Vec<String>,
    pub(super) config: InspectedContainerConfig,
    pub(super) host_config: InspectedHostConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub(super) mounts: Vec<InspectedMount>,
    pub(super) network_settings: InspectedNetworkSettings,
    pub(super) state: ContainerState,
    pub(super) process_label: String,
    pub(super) mount_label: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct InspectedContainerConfig {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub(super) annotations: BTreeMap<String, String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub(super) create_command: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub(super) entrypoint: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub(super) cmd: Vec<String>,
    pub(super) image: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub(super) labels: BTreeMap<String, String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub(super) env: Vec<String>,
    pub(super) open_stdin: bool,
    pub(super) stdin_once: bool,
    pub(super) tty: bool,
    pub(super) stop_signal: String,
    pub(super) stop_timeout: u64,
    pub(super) user: String,
    pub(super) working_dir: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct InspectedHostConfig {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub(super) annotations: BTreeMap<String, String>,
    pub(super) auto_remove: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub(super) binds: Vec<String>,
    pub(super) cgroup_mode: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub(super) devices: Vec<serde_json::Value>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub(super) group_add: Vec<String>,
    #[serde(rename = "IDMappings")]
    pub(super) id_mappings: InspectedIdMappings,
    pub(super) ipc_mode: String,
    pub(super) log_config: InspectedLogConfig,
    pub(super) network_mode: String,
    pub(super) pid_mode: String,
    pub(super) pids_limit: i64,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub(super) port_bindings: BTreeMap<String, serde_json::Value>,
    pub(super) privileged: bool,
    pub(super) publish_all_ports: bool,
    pub(super) readonly_rootfs: bool,
    pub(super) restart_policy: InspectedRestartPolicy,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub(super) security_opt: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub(super) tmpfs: BTreeMap<String, String>,
    #[serde(rename = "UTSMode")]
    pub(super) uts_mode: String,
    pub(super) userns_mode: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct InspectedIdMappings {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub(super) uid_map: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub(super) gid_map: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct InspectedLogConfig {
    #[serde(rename = "Type")]
    pub(super) log_type: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct InspectedRestartPolicy {
    pub(super) name: String,
    pub(super) maximum_retry_count: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct InspectedMount {
    #[serde(rename = "Type")]
    pub(super) mount_type: String,
    pub(super) source: String,
    pub(super) destination: String,
    pub(super) mode: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub(super) options: Vec<String>,
    #[serde(rename = "RW")]
    pub(super) read_write: bool,
    pub(super) propagation: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct InspectedNetworkSettings {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub(super) networks: BTreeMap<String, InspectedNetwork>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct InspectedNetwork {
    #[serde(rename = "NetworkID")]
    pub(super) network_id: String,
    pub(super) gateway: String,
    #[serde(rename = "IPAddress")]
    pub(super) ip_address: String,
    #[serde(rename = "IPv6Gateway")]
    pub(super) ipv6_gateway: String,
    #[serde(rename = "GlobalIPv6Address")]
    pub(super) global_ipv6_address: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct ContainerState {
    pub(super) status: String,
    pub(super) running: bool,
    pub(super) paused: bool,
    pub(super) restarting: bool,
    #[serde(rename = "OOMKilled")]
    pub(super) oom_killed: bool,
    pub(super) dead: bool,
    #[serde(default)]
    pub(super) pid: u64,
}
