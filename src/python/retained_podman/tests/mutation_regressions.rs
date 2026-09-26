use super::{
    matching_inspection, matching_transient_inspection, test_config, transient_test_config,
};
use crate::config::NetworkAccess;
use crate::python::retained_podman::attestation::{
    enforce_expected_selinux_labels, validate_retained_inspection, validate_transient_inspection,
};
use crate::python::retained_podman::{
    attest_new_retained_container, attest_new_transient_container, build_create_spec,
    build_transient_create_spec,
};
use crate::python::runtime_store::{
    RuntimeLayoutProfile, RuntimeWorkspaceOwnership, WorkspaceIdentity,
};
use std::os::unix::fs::PermissionsExt;

#[tokio::test]
async fn rejected_replaced_retained_workspace_never_commits_control_state() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let mut config = test_config(&root);
    std::fs::set_permissions(&config.workspace, std::fs::Permissions::from_mode(0o755)).unwrap();
    config.layout_profile = RuntimeLayoutProfile::ShortSiblingSharedCwdV1;
    config.workspace_destination = config.workspace.clone();
    config.workspace_ownership = RuntimeWorkspaceOwnership::ExternalLaunchCwd;
    config.mask_lethetic = true;
    let stored = WorkspaceIdentity::capture(&config.workspace).unwrap();

    std::fs::rename(&config.workspace, root.join("retained-original")).unwrap();
    std::fs::create_dir(&config.workspace).unwrap();
    std::fs::set_permissions(&config.workspace, std::fs::Permissions::from_mode(0o755)).unwrap();
    let control = config.workspace.join(".lethetic");

    config.validate().unwrap();
    build_create_spec(&config).unwrap();
    assert!(!control.exists());
    let error = attest_new_retained_container(&config, &"c".repeat(64), &stored)
        .await
        .unwrap_err();
    assert!(error.contains("workspace"), "{error}");
    assert!(!control.exists());
}

#[tokio::test]
async fn rejected_replaced_transient_workspace_never_commits_control_state() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let mut config = transient_test_config(&root, NetworkAccess::None, false);
    let container_id = "d".repeat(64);
    let inspection = matching_transient_inspection(&config, &container_id);

    std::fs::rename(&config.workspace, root.join("transient-original")).unwrap();
    std::fs::create_dir(&config.workspace).unwrap();
    config.mask_lethetic = true;
    let control = config.workspace.join(".lethetic");

    config.validate().unwrap();
    build_transient_create_spec(&config).unwrap();
    assert!(!control.exists());
    attest_new_transient_container(&config, "invalid")
        .await
        .unwrap_err();
    assert!(!control.exists());
    validate_transient_inspection(&config, &container_id, &inspection).unwrap_err();
    assert!(!control.exists());
}

#[test]
fn existing_unbound_phase_returns_labels_before_exact_binding() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let config = test_config(&root);
    let container_id = "e".repeat(64);
    let mut inspection = matching_inspection(&config, &container_id);
    inspection.process_label = "system_u:system_r:container_t:s0:c43,c101".to_string();
    inspection.mount_label = "system_u:object_r:container_file_t:s0:c43,c101".to_string();

    let identity = validate_retained_inspection(&config, &container_id, &inspection, true).unwrap();
    let observed = identity.selinux_labels.clone().unwrap();
    assert!(observed.process_label.ends_with("c43,c101"));
    enforce_expected_selinux_labels(identity.clone(), Some(&observed)).unwrap();
    enforce_expected_selinux_labels(identity.clone(), None).unwrap_err();

    let different = crate::python::selinux::SelinuxLabels::new(
        "system_u:system_r:container_t:s0:c44,c102".to_string(),
        "system_u:object_r:container_file_t:s0:c44,c102".to_string(),
    )
    .unwrap();
    enforce_expected_selinux_labels(identity.clone(), Some(&different)).unwrap_err();
    let mut unlabeled = identity;
    unlabeled.selinux_labels = None;
    enforce_expected_selinux_labels(unlabeled.clone(), None).unwrap();
    enforce_expected_selinux_labels(unlabeled, Some(&observed)).unwrap_err();
}
