use super::common::{validate_image_id, validate_lower_hex};
use super::manifest::{RuntimeLayoutProfile, RuntimeWorkspaceOwnership};
use super::workspace::WorkspaceIdentity;
use crate::python::egress_broker::{BROKER_PROTOCOL_ABI, BROKER_PROTOCOL_VERSION};
use crate::python::supervisor::RUNTIME_ABI;
use sha2::{Digest, Sha256};

pub const RETAINED_SECURITY_PROFILE: &str = "lethetic-retained-podman-v1";

pub fn retained_security_fingerprint(
    policy_fingerprint: &str,
    image_id: &str,
    workspace: &WorkspaceIdentity,
    worker_uid: u32,
    worker_gid: u32,
) -> Result<String, String> {
    validate_lower_hex(policy_fingerprint, 64, "Python policy fingerprint")?;
    validate_image_id(image_id)?;
    workspace.validate_stored()?;
    if worker_uid == 0 || worker_gid == 0 {
        return Err("retained runtime worker identity must be unprivileged".to_string());
    }
    let encoded = serde_json::to_vec(&(
        RETAINED_SECURITY_PROFILE,
        policy_fingerprint,
        image_id,
        workspace,
        worker_uid,
        worker_gid,
        RUNTIME_ABI,
        BROKER_PROTOCOL_VERSION,
        BROKER_PROTOCOL_ABI,
        RuntimeLayoutProfile::ShortSiblingV1,
    ))
    .map_err(|error| format!("could not encode retained security fingerprint: {error}"))?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

pub fn retained_shared_security_fingerprint(
    policy_fingerprint: &str,
    image_id: &str,
    workspace: &WorkspaceIdentity,
    managed_workspace: &WorkspaceIdentity,
    worker_uid: u32,
    worker_gid: u32,
) -> Result<String, String> {
    validate_lower_hex(policy_fingerprint, 64, "Python policy fingerprint")?;
    validate_image_id(image_id)?;
    workspace.validate_stored()?;
    managed_workspace.validate_stored()?;
    if workspace == managed_workspace {
        return Err("shared and managed workspace identities must differ".to_string());
    }
    if worker_uid == 0 || worker_gid == 0 {
        return Err("retained runtime worker identity must be unprivileged".to_string());
    }
    let encoded = serde_json::to_vec(&(
        RETAINED_SECURITY_PROFILE,
        policy_fingerprint,
        image_id,
        RuntimeWorkspaceOwnership::ExternalLaunchCwd,
        workspace,
        managed_workspace,
        worker_uid,
        worker_gid,
        RUNTIME_ABI,
        BROKER_PROTOCOL_VERSION,
        BROKER_PROTOCOL_ABI,
        RuntimeLayoutProfile::ShortSiblingSharedCwdV1,
    ))
    .map_err(|error| format!("could not encode retained security fingerprint: {error}"))?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

pub fn legacy_retained_security_fingerprint(
    policy_fingerprint: &str,
    image_id: &str,
    workspace: &WorkspaceIdentity,
    worker_uid: u32,
    worker_gid: u32,
) -> Result<String, String> {
    validate_lower_hex(policy_fingerprint, 64, "Python policy fingerprint")?;
    validate_image_id(image_id)?;
    workspace.validate_stored()?;
    if worker_uid == 0 || worker_gid == 0 {
        return Err("retained runtime worker identity must be unprivileged".to_string());
    }
    let encoded = serde_json::to_vec(&(
        RETAINED_SECURITY_PROFILE,
        policy_fingerprint,
        image_id,
        workspace,
        worker_uid,
        worker_gid,
        RUNTIME_ABI,
        BROKER_PROTOCOL_VERSION,
        BROKER_PROTOCOL_ABI,
    ))
    .map_err(|error| format!("could not encode legacy retained security fingerprint: {error}"))?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}
