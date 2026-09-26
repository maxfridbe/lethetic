use super::common::{
    format_timestamp, parse_timestamp, ttl_deadline, validate_image_id, validate_lower_hex,
    validate_uuid,
};
use super::workspace::WorkspaceIdentity;
use crate::python::egress_broker::{BROKER_PROTOCOL_ABI, BROKER_PROTOCOL_VERSION};
use crate::python::retained_podman::{
    ABI_LABEL, FINGERPRINT_LABEL, LAYOUT_LABEL, MANAGED_LABEL, RUNTIME_ID_LABEL, SESSION_ID_LABEL,
};
use crate::python::supervisor::RUNTIME_ABI;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const RUNTIME_MANIFEST_SCHEMA: u32 = 4;
pub(super) const PREVIOUS_RUNTIME_MANIFEST_SCHEMA: u32 = 3;
pub(super) const LEGACY_RUNTIME_MANIFEST_SCHEMA: u32 = 2;
pub const LEGACY_RUNTIME_ABI_V2: &str = "lethetic-python-runtime-v2";
pub const LEGACY_RUNTIME_ABI_V3: &str = "lethetic-python-runtime-v3";
pub(super) const DELETION_RECEIPT_SCHEMA: u32 = 1;
pub(super) const CREATION_INTENT_SCHEMA: u32 = 2;
pub(super) const PREVIOUS_CREATION_INTENT_SCHEMA: u32 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeLifecycleState {
    Creating,
    Stopped,
    Attached,
    Stopping,
    Deleting,
    Quarantined,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeLayoutProfile {
    LegacyRuntimeLocalV2,
    ShortSiblingV2Imported,
    ShortSiblingV1,
    ShortSiblingSharedCwdV1,
    LegacyV2Unclassified,
}

impl RuntimeLayoutProfile {
    pub fn is_attachable(self) -> bool {
        matches!(
            self,
            Self::ShortSiblingV2Imported | Self::ShortSiblingV1 | Self::ShortSiblingSharedCwdV1
        )
    }

    pub fn uses_short_sibling_bridge(self) -> bool {
        matches!(
            self,
            Self::ShortSiblingV2Imported | Self::ShortSiblingV1 | Self::ShortSiblingSharedCwdV1
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeDeletionReceipt {
    pub schema_version: u32,
    pub runtime_id: String,
    pub session_id: String,
    pub container_id: Option<String>,
    pub container_name: String,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeWorkspaceOwnership {
    #[default]
    Managed,
    ExternalLaunchCwd,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeCreationIntent {
    pub schema_version: u32,
    pub runtime_id: String,
    pub session_id: String,
    pub container_name: String,
    pub owner_uid: u32,
    pub owner_gid: u32,
    pub workspace: WorkspaceIdentity,
    #[serde(default)]
    pub workspace_ownership: RuntimeWorkspaceOwnership,
    #[serde(default)]
    pub managed_workspace: Option<WorkspaceIdentity>,
}

impl RuntimeCreationIntent {
    pub fn new(
        runtime_id: String,
        session_id: String,
        workspace: WorkspaceIdentity,
    ) -> Result<Self, String> {
        Self::new_with_workspace_binding(
            runtime_id,
            session_id,
            workspace,
            RuntimeWorkspaceOwnership::Managed,
            None,
        )
    }

    pub fn new_with_workspace_binding(
        runtime_id: String,
        session_id: String,
        workspace: WorkspaceIdentity,
        workspace_ownership: RuntimeWorkspaceOwnership,
        managed_workspace: Option<WorkspaceIdentity>,
    ) -> Result<Self, String> {
        let intent = Self {
            schema_version: CREATION_INTENT_SCHEMA,
            container_name: format!("lethetic-python-{runtime_id}"),
            runtime_id,
            session_id,
            owner_uid: rustix::process::geteuid().as_raw(),
            owner_gid: rustix::process::getegid().as_raw(),
            workspace,
            workspace_ownership,
            managed_workspace,
        };
        validate_creation_intent(&intent)?;
        Ok(intent)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeManifest {
    pub schema_version: u32,
    pub runtime_id: String,
    pub session_id: String,
    pub container_id: Option<String>,
    pub container_name: String,
    pub owner_uid: u32,
    pub owner_gid: u32,
    pub image_id: String,
    pub security_fingerprint: String,
    pub runtime_abi: String,
    pub layout_profile: RuntimeLayoutProfile,
    pub broker_protocol_version: u32,
    pub broker_protocol_abi: String,
    pub labels: BTreeMap<String, String>,
    pub workspace: WorkspaceIdentity,
    #[serde(default)]
    pub workspace_ownership: RuntimeWorkspaceOwnership,
    #[serde(default)]
    pub managed_workspace: Option<WorkspaceIdentity>,
    #[serde(default)]
    pub create_command_finished: Option<bool>,
    #[serde(default)]
    pub selinux_labels: Option<crate::python::selinux::SelinuxLabels>,
    pub lifecycle: RuntimeLifecycleState,
    pub created_at: String,
    pub last_used_at: String,
    pub expires_at: String,
    #[serde(default)]
    pub quarantine_reason: Option<String>,
    #[serde(default)]
    pub cleanup_error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LegacyRuntimeManifestV2 {
    schema_version: u32,
    runtime_id: String,
    session_id: String,
    container_id: Option<String>,
    container_name: String,
    owner_uid: u32,
    owner_gid: u32,
    image_id: String,
    security_fingerprint: String,
    runtime_abi: String,
    broker_protocol_version: u32,
    broker_protocol_abi: String,
    labels: BTreeMap<String, String>,
    workspace: WorkspaceIdentity,
    #[serde(default)]
    create_command_finished: Option<bool>,
    #[serde(default)]
    selinux_labels: Option<crate::python::selinux::SelinuxLabels>,
    lifecycle: RuntimeLifecycleState,
    created_at: String,
    last_used_at: String,
    expires_at: String,
    #[serde(default)]
    quarantine_reason: Option<String>,
    #[serde(default)]
    cleanup_error: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct RuntimeManifestSchemaProbe {
    pub(super) schema_version: u32,
}

impl From<LegacyRuntimeManifestV2> for RuntimeManifest {
    fn from(legacy: LegacyRuntimeManifestV2) -> Self {
        Self {
            schema_version: legacy.schema_version,
            runtime_id: legacy.runtime_id,
            session_id: legacy.session_id,
            container_id: legacy.container_id,
            container_name: legacy.container_name,
            owner_uid: legacy.owner_uid,
            owner_gid: legacy.owner_gid,
            image_id: legacy.image_id,
            security_fingerprint: legacy.security_fingerprint,
            runtime_abi: legacy.runtime_abi,
            layout_profile: RuntimeLayoutProfile::LegacyV2Unclassified,
            broker_protocol_version: legacy.broker_protocol_version,
            broker_protocol_abi: legacy.broker_protocol_abi,
            labels: legacy.labels,
            workspace: legacy.workspace,
            workspace_ownership: RuntimeWorkspaceOwnership::Managed,
            managed_workspace: None,
            create_command_finished: legacy.create_command_finished,
            selinux_labels: legacy.selinux_labels,
            lifecycle: legacy.lifecycle,
            created_at: legacy.created_at,
            last_used_at: legacy.last_used_at,
            expires_at: legacy.expires_at,
            quarantine_reason: legacy.quarantine_reason,
            cleanup_error: legacy.cleanup_error,
        }
    }
}

impl RuntimeManifest {
    pub fn new(
        runtime_id: String,
        session_id: String,
        image_id: String,
        security_fingerprint: String,
        workspace: WorkspaceIdentity,
        now: DateTime<Utc>,
    ) -> Result<Self, String> {
        Self::new_with_workspace_binding(
            runtime_id,
            session_id,
            image_id,
            security_fingerprint,
            workspace,
            RuntimeWorkspaceOwnership::Managed,
            None,
            now,
        )
    }

    pub fn new_with_workspace_binding(
        runtime_id: String,
        session_id: String,
        image_id: String,
        security_fingerprint: String,
        workspace: WorkspaceIdentity,
        workspace_ownership: RuntimeWorkspaceOwnership,
        managed_workspace: Option<WorkspaceIdentity>,
        now: DateTime<Utc>,
    ) -> Result<Self, String> {
        validate_uuid(&runtime_id, "runtime ID")?;
        validate_uuid(&session_id, "session ID")?;
        validate_image_id(&image_id)?;
        validate_lower_hex(&security_fingerprint, 64, "security fingerprint")?;
        let owner_uid = rustix::process::geteuid().as_raw();
        let owner_gid = rustix::process::getegid().as_raw();
        let container_name = format!("lethetic-python-{runtime_id}");
        let layout_profile = match workspace_ownership {
            RuntimeWorkspaceOwnership::Managed => RuntimeLayoutProfile::ShortSiblingV1,
            RuntimeWorkspaceOwnership::ExternalLaunchCwd => {
                RuntimeLayoutProfile::ShortSiblingSharedCwdV1
            }
        };
        let labels = expected_labels(
            &runtime_id,
            &session_id,
            &security_fingerprint,
            layout_profile,
        );
        let created_at = format_timestamp(now);
        let expires_at = format_timestamp(ttl_deadline(now)?);
        let manifest = Self {
            schema_version: RUNTIME_MANIFEST_SCHEMA,
            runtime_id,
            session_id,
            container_id: None,
            container_name,
            owner_uid,
            owner_gid,
            image_id,
            security_fingerprint,
            runtime_abi: RUNTIME_ABI.to_string(),
            layout_profile,
            broker_protocol_version: BROKER_PROTOCOL_VERSION,
            broker_protocol_abi: BROKER_PROTOCOL_ABI.to_string(),
            labels,
            workspace,
            workspace_ownership,
            managed_workspace,
            create_command_finished: Some(false),
            selinux_labels: None,
            lifecycle: RuntimeLifecycleState::Creating,
            created_at: created_at.clone(),
            last_used_at: created_at,
            expires_at,
            quarantine_reason: None,
            cleanup_error: None,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), String> {
        self.validate_classified(false)
    }

    pub(crate) fn validate_for_maintenance(&self) -> Result<(), String> {
        self.validate_classified(true)
    }

    fn validate_classified(&self, allow_legacy_runtime_abi: bool) -> Result<(), String> {
        if self.schema_version != RUNTIME_MANIFEST_SCHEMA
            || self.layout_profile == RuntimeLayoutProfile::LegacyV2Unclassified
        {
            return Err(format!(
                "unsupported Python runtime manifest schema/layout {}:{:?}",
                self.schema_version, self.layout_profile
            ));
        }
        self.validate_common(allow_legacy_runtime_abi)
    }

    pub(super) fn validate_loaded(&self) -> Result<(), String> {
        if !matches!(
            (self.schema_version, self.layout_profile),
            (
                RUNTIME_MANIFEST_SCHEMA,
                RuntimeLayoutProfile::LegacyRuntimeLocalV2
            ) | (
                RUNTIME_MANIFEST_SCHEMA,
                RuntimeLayoutProfile::ShortSiblingV2Imported
            ) | (
                RUNTIME_MANIFEST_SCHEMA,
                RuntimeLayoutProfile::ShortSiblingV1
            ) | (
                RUNTIME_MANIFEST_SCHEMA,
                RuntimeLayoutProfile::ShortSiblingSharedCwdV1
            ) | (
                PREVIOUS_RUNTIME_MANIFEST_SCHEMA,
                RuntimeLayoutProfile::LegacyRuntimeLocalV2
            ) | (
                PREVIOUS_RUNTIME_MANIFEST_SCHEMA,
                RuntimeLayoutProfile::ShortSiblingV2Imported
            ) | (
                PREVIOUS_RUNTIME_MANIFEST_SCHEMA,
                RuntimeLayoutProfile::ShortSiblingV1
            ) | (
                LEGACY_RUNTIME_MANIFEST_SCHEMA,
                RuntimeLayoutProfile::LegacyV2Unclassified
            )
        ) {
            return Err(format!(
                "unsupported Python runtime manifest schema/layout {}:{:?}",
                self.schema_version, self.layout_profile
            ));
        }
        self.validate_common(true)
    }

    fn validate_common(&self, allow_legacy_runtime_abi: bool) -> Result<(), String> {
        validate_uuid(&self.runtime_id, "runtime ID")?;
        validate_uuid(&self.session_id, "session ID")?;
        validate_image_id(&self.image_id)?;
        validate_lower_hex(&self.security_fingerprint, 64, "security fingerprint")?;
        if let Some(container_id) = &self.container_id {
            validate_lower_hex(container_id, 64, "container ID")?;
        }
        if self.container_name != format!("lethetic-python-{}", self.runtime_id) {
            return Err("runtime manifest container name is not exact".to_string());
        }
        if self.owner_uid != rustix::process::geteuid().as_raw()
            || self.owner_gid != rustix::process::getegid().as_raw()
        {
            return Err("runtime manifest belongs to a different host identity".to_string());
        }
        let runtime_abi_compatible = self.runtime_abi == RUNTIME_ABI
            || (allow_legacy_runtime_abi
                && matches!(
                    self.runtime_abi.as_str(),
                    LEGACY_RUNTIME_ABI_V2 | LEGACY_RUNTIME_ABI_V3
                ));
        if !runtime_abi_compatible
            || self.broker_protocol_version != BROKER_PROTOCOL_VERSION
            || self.broker_protocol_abi != BROKER_PROTOCOL_ABI
        {
            return Err("runtime manifest protocol identity is incompatible".to_string());
        }
        if self.labels
            != expected_labels_for_abi(
                &self.runtime_id,
                &self.session_id,
                &self.security_fingerprint,
                self.layout_profile,
                &self.runtime_abi,
            )
        {
            return Err("runtime manifest labels are incomplete or inconsistent".to_string());
        }
        self.workspace.validate_stored()?;
        match self.workspace_ownership {
            RuntimeWorkspaceOwnership::Managed => {
                if self.managed_workspace.is_some() {
                    return Err(
                        "managed runtime manifest unexpectedly has a second managed workspace"
                            .to_string(),
                    );
                }
                if self.layout_profile == RuntimeLayoutProfile::ShortSiblingSharedCwdV1 {
                    return Err("managed runtime uses the shared-cwd layout".to_string());
                }
            }
            RuntimeWorkspaceOwnership::ExternalLaunchCwd => {
                let managed = self.managed_workspace.as_ref().ok_or_else(|| {
                    "external runtime manifest is missing its owned managed workspace".to_string()
                })?;
                managed.validate_stored()?;
                if managed == &self.workspace {
                    return Err(
                        "external and managed runtime workspaces must be distinct".to_string()
                    );
                }
                if self.layout_profile != RuntimeLayoutProfile::ShortSiblingSharedCwdV1 {
                    return Err("external runtime does not use the shared-cwd layout".to_string());
                }
            }
        }
        if let Some(labels) = &self.selinux_labels {
            labels.validate()?;
        }
        let created = parse_timestamp(&self.created_at, "created_at")?;
        let last_used = parse_timestamp(&self.last_used_at, "last_used_at")?;
        let expires = parse_timestamp(&self.expires_at, "expires_at")?;
        if last_used < created || expires != ttl_deadline(last_used)? {
            return Err("runtime manifest timestamps are inconsistent".to_string());
        }
        if self
            .cleanup_error
            .as_ref()
            .is_some_and(|error| error.len() > 4096)
        {
            return Err("runtime cleanup error exceeds its safety limit".to_string());
        }
        if self.cleanup_error.is_some() && self.lifecycle != RuntimeLifecycleState::Deleting {
            return Err("cleanup error is present outside the deleting state".to_string());
        }
        if self.lifecycle == RuntimeLifecycleState::Quarantined {
            if self
                .quarantine_reason
                .as_deref()
                .is_none_or(|reason| reason.is_empty() || reason.len() > 2048)
            {
                return Err("quarantined runtime is missing a reason".to_string());
            }
        } else if self.quarantine_reason.is_some() {
            return Err("non-quarantined runtime contains a quarantine reason".to_string());
        }
        if self.create_command_finished == Some(false)
            && (self.container_id.is_some() || self.lifecycle != RuntimeLifecycleState::Creating)
        {
            return Err("runtime advanced before its create command durably finished".to_string());
        }
        match self.lifecycle {
            RuntimeLifecycleState::Creating if self.selinux_labels.is_some() => {
                return Err(
                    "creating runtime unexpectedly has completed container attestation".to_string(),
                );
            }
            RuntimeLifecycleState::Stopped
            | RuntimeLifecycleState::Attached
            | RuntimeLifecycleState::Stopping
                if self.container_id.is_none() =>
            {
                return Err("runtime lifecycle state requires a container ID".to_string());
            }
            _ => {}
        }
        Ok(())
    }

    pub fn import_legacy_layout(
        &mut self,
        layout_profile: RuntimeLayoutProfile,
    ) -> Result<(), String> {
        if self.schema_version != LEGACY_RUNTIME_MANIFEST_SCHEMA
            || self.layout_profile != RuntimeLayoutProfile::LegacyV2Unclassified
            || !matches!(
                layout_profile,
                RuntimeLayoutProfile::LegacyRuntimeLocalV2
                    | RuntimeLayoutProfile::ShortSiblingV2Imported
            )
        {
            return Err("runtime manifest is not an importable schema-v2 layout".to_string());
        }
        self.schema_version = RUNTIME_MANIFEST_SCHEMA;
        self.layout_profile = layout_profile;
        self.validate_for_maintenance()
    }

    pub fn mark_create_command_finished(&mut self) -> Result<(), String> {
        if self.lifecycle != RuntimeLifecycleState::Creating {
            return Err("create command can finish only during creating lifecycle".to_string());
        }
        self.create_command_finished = Some(true);
        self.validate_for_maintenance()
    }

    pub fn create_command_is_known_finished(&self) -> bool {
        self.create_command_finished == Some(true) || self.container_id.is_some()
    }

    pub fn bind_created_container(&mut self, container_id: String) -> Result<(), String> {
        self.bind_created_container_with_security(container_id, None)
    }

    pub fn bind_created_container_with_security(
        &mut self,
        container_id: String,
        selinux_labels: Option<crate::python::selinux::SelinuxLabels>,
    ) -> Result<(), String> {
        if let Some(labels) = &selinux_labels {
            labels.validate()?;
        }
        self.record_created_container_id(container_id)?;
        self.complete_created_container_attestation(selinux_labels)
    }

    pub fn record_created_container_id(&mut self, container_id: String) -> Result<(), String> {
        if self.lifecycle != RuntimeLifecycleState::Creating || self.container_id.is_some() {
            return Err("container ID can be recorded only once while creating".to_string());
        }
        validate_lower_hex(&container_id, 64, "container ID")?;
        self.container_id = Some(container_id);
        self.validate_for_maintenance()
    }

    pub(crate) fn bind_deleting_container_with_security(
        &mut self,
        container_id: String,
        selinux_labels: Option<crate::python::selinux::SelinuxLabels>,
    ) -> Result<(), String> {
        if self.lifecycle != RuntimeLifecycleState::Deleting || self.container_id.is_some() {
            return Err(
                "container ID and security can be recovered only once while deleting an incomplete runtime"
                    .to_string(),
            );
        }
        validate_lower_hex(&container_id, 64, "container ID")?;
        if let Some(labels) = &selinux_labels {
            labels.validate()?;
        }
        self.container_id = Some(container_id);
        self.selinux_labels = selinux_labels;
        self.validate_for_maintenance()
    }

    pub fn bind_deleting_container_id(&mut self, container_id: String) -> Result<(), String> {
        if self.lifecycle != RuntimeLifecycleState::Deleting || self.container_id.is_some() {
            return Err(
                "container ID can be recovered only once while deleting an incomplete runtime"
                    .to_string(),
            );
        }
        validate_lower_hex(&container_id, 64, "container ID")?;
        self.container_id = Some(container_id);
        self.validate_for_maintenance()
    }

    pub fn complete_created_container_attestation(
        &mut self,
        selinux_labels: Option<crate::python::selinux::SelinuxLabels>,
    ) -> Result<(), String> {
        if self.lifecycle != RuntimeLifecycleState::Creating || self.container_id.is_none() {
            return Err("container attestation requires a recorded creating container".to_string());
        }
        if let Some(labels) = &selinux_labels {
            labels.validate()?;
        }
        self.selinux_labels = selinux_labels;
        self.lifecycle = RuntimeLifecycleState::Stopped;
        self.validate_for_maintenance()
    }

    pub fn transition(&mut self, next: RuntimeLifecycleState) -> Result<(), String> {
        let previous = self.lifecycle;
        let allowed = matches!(
            (self.lifecycle, next),
            (
                RuntimeLifecycleState::Stopped,
                RuntimeLifecycleState::Attached
            ) | (
                RuntimeLifecycleState::Stopped,
                RuntimeLifecycleState::Deleting
            ) | (
                RuntimeLifecycleState::Attached,
                RuntimeLifecycleState::Stopping
            ) | (
                RuntimeLifecycleState::Stopping,
                RuntimeLifecycleState::Stopped
            ) | (
                RuntimeLifecycleState::Stopping,
                RuntimeLifecycleState::Deleting
            ) | (
                RuntimeLifecycleState::Creating,
                RuntimeLifecycleState::Deleting
            ) | (_, RuntimeLifecycleState::Quarantined)
        );
        if !allowed || self.lifecycle == RuntimeLifecycleState::Quarantined {
            return Err(format!(
                "invalid runtime lifecycle transition {:?} -> {next:?}",
                self.lifecycle
            ));
        }
        if next == RuntimeLifecycleState::Quarantined {
            return Err("use quarantine() so a reason is recorded".to_string());
        }
        self.lifecycle = next;
        self.cleanup_error = None;
        if previous == RuntimeLifecycleState::Stopped && next == RuntimeLifecycleState::Attached {
            self.validate()
        } else {
            self.validate_for_maintenance()
        }
    }

    pub fn quarantine(&mut self, reason: impl Into<String>) -> Result<(), String> {
        let reason = reason.into();
        if reason.trim().is_empty() || reason.len() > 2048 {
            return Err("runtime quarantine reason is empty or too long".to_string());
        }
        self.lifecycle = RuntimeLifecycleState::Quarantined;
        self.quarantine_reason = Some(reason);
        self.cleanup_error = None;
        self.validate_for_maintenance()
    }

    pub(crate) fn mark_deleting_after_verified_container_absence(&mut self) -> Result<(), String> {
        if self.lifecycle != RuntimeLifecycleState::Quarantined {
            return Err("absence-only recovery requires a quarantined runtime".to_string());
        }
        self.lifecycle = RuntimeLifecycleState::Deleting;
        self.quarantine_reason = None;
        self.cleanup_error = None;
        self.validate_for_maintenance()
    }

    pub fn mark_cleanup_error(&mut self, error: impl Into<String>) -> Result<(), String> {
        if self.lifecycle != RuntimeLifecycleState::Deleting {
            return Err("cleanup errors may be recorded only while deleting".to_string());
        }
        let mut error = error.into();
        if error.len() > 4096 {
            let mut boundary = 4096;
            while !error.is_char_boundary(boundary) {
                boundary -= 1;
            }
            error.truncate(boundary);
        }
        self.cleanup_error = Some(error);
        self.validate_for_maintenance()
    }

    pub fn is_expired_at(&self, now: DateTime<Utc>) -> Result<bool, String> {
        Ok(now >= parse_timestamp(&self.expires_at, "expires_at")?)
    }

    pub fn touch(&mut self, now: DateTime<Utc>) -> Result<(), String> {
        if !matches!(
            self.lifecycle,
            RuntimeLifecycleState::Stopped | RuntimeLifecycleState::Attached
        ) {
            return Err(
                "runtime TTL can be refreshed only after a compatible attach or use".to_string(),
            );
        }
        if self.is_expired_at(now)? {
            return Err("expired runtime cannot be resurrected by touching it".to_string());
        }
        let last_used = parse_timestamp(&self.last_used_at, "last_used_at")?;
        if now < last_used {
            return Err("runtime last-used timestamp cannot move backwards".to_string());
        }
        self.last_used_at = format_timestamp(now);
        self.expires_at = format_timestamp(ttl_deadline(now)?);
        self.validate()
    }
}

pub(super) fn validate_creation_intent(intent: &RuntimeCreationIntent) -> Result<(), String> {
    if !matches!(
        intent.schema_version,
        PREVIOUS_CREATION_INTENT_SCHEMA | CREATION_INTENT_SCHEMA
    ) {
        return Err("unsupported runtime creation intent schema".to_string());
    }
    validate_uuid(&intent.runtime_id, "runtime creation intent ID")?;
    validate_uuid(&intent.session_id, "runtime creation intent session ID")?;
    if intent.container_name != format!("lethetic-python-{}", intent.runtime_id) {
        return Err("runtime creation intent container name is not exact".to_string());
    }
    if intent.owner_uid != rustix::process::geteuid().as_raw()
        || intent.owner_gid != rustix::process::getegid().as_raw()
    {
        return Err("runtime creation intent belongs to a different host identity".to_string());
    }
    intent.workspace.validate_stored()?;
    match intent.workspace_ownership {
        RuntimeWorkspaceOwnership::Managed => {
            if intent.managed_workspace.is_some() {
                return Err(
                    "managed creation intent unexpectedly has a second managed workspace"
                        .to_string(),
                );
            }
        }
        RuntimeWorkspaceOwnership::ExternalLaunchCwd => {
            let managed = intent.managed_workspace.as_ref().ok_or_else(|| {
                "external creation intent is missing its owned managed workspace".to_string()
            })?;
            managed.validate_stored()?;
            if managed == &intent.workspace {
                return Err("creation intent workspaces must be distinct".to_string());
            }
        }
    }
    Ok(())
}

pub(super) fn validate_deletion_receipt(receipt: &RuntimeDeletionReceipt) -> Result<(), String> {
    if receipt.schema_version != DELETION_RECEIPT_SCHEMA {
        return Err("unsupported runtime deletion receipt schema".to_string());
    }
    validate_uuid(&receipt.runtime_id, "runtime deletion receipt ID")?;
    validate_uuid(&receipt.session_id, "runtime deletion receipt session ID")?;
    if let Some(container_id) = &receipt.container_id {
        validate_lower_hex(container_id, 64, "runtime deletion receipt container ID")?;
    }
    if receipt.container_name != format!("lethetic-python-{}", receipt.runtime_id) {
        return Err("runtime deletion receipt container name is not exact".to_string());
    }
    Ok(())
}

pub(super) fn expected_labels(
    runtime_id: &str,
    session_id: &str,
    security_fingerprint: &str,
    layout_profile: RuntimeLayoutProfile,
) -> BTreeMap<String, String> {
    expected_labels_for_abi(
        runtime_id,
        session_id,
        security_fingerprint,
        layout_profile,
        RUNTIME_ABI,
    )
}

pub(super) fn expected_labels_for_abi(
    runtime_id: &str,
    session_id: &str,
    security_fingerprint: &str,
    layout_profile: RuntimeLayoutProfile,
    runtime_abi: &str,
) -> BTreeMap<String, String> {
    let mut labels = BTreeMap::from([
        (MANAGED_LABEL.to_string(), "true".to_string()),
        (RUNTIME_ID_LABEL.to_string(), runtime_id.to_string()),
        (SESSION_ID_LABEL.to_string(), session_id.to_string()),
        (
            FINGERPRINT_LABEL.to_string(),
            security_fingerprint.to_string(),
        ),
        (ABI_LABEL.to_string(), runtime_abi.to_string()),
    ]);
    match layout_profile {
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
