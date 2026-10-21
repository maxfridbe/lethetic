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

#[test]
fn podman_entrypoint_wire_formats_preserve_exact_attestation() {
    use crate::python::retained_podman::model::ContainerInspection;
    use serde_json::json;

    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let config = transient_test_config(&root, NetworkAccess::None, false);
    let id = "d".repeat(64);
    let fixture = serde_json::to_value(matching_transient_inspection(&config, &id)).unwrap();
    for entrypoint in [json!("python3"), json!(["python3"])] {
        let mut value = fixture.clone();
        value["Config"]["Entrypoint"] = entrypoint;
        let inspection: ContainerInspection = serde_json::from_value(value).unwrap();
        validate_transient_inspection(&config, &id, &inspection).unwrap();
    }
    // A legacy string is one executable, never a shell command to split, trim,
    // or unquote. Empty/missing entrypoints and extra arguments still fail.
    for entrypoint in [
        json!("python3 -u"),
        json!(" python3"),
        json!("\"python3\""),
        json!("sh"),
        json!(""),
        json!([]),
        json!(null),
        json!(["python3", "-u"]),
    ] {
        let mut value = fixture.clone();
        value["Config"]["Entrypoint"] = entrypoint;
        let inspection: ContainerInspection = serde_json::from_value(value).unwrap();
        assert!(validate_transient_inspection(&config, &id, &inspection).is_err());
    }
    for entrypoint in [
        json!(42),
        json!({"command": "python3"}),
        json!(["python3", 42]),
    ] {
        let mut value = fixture.clone();
        value["Config"]["Entrypoint"] = entrypoint;
        assert!(serde_json::from_value::<ContainerInspection>(value).is_err());
    }
    let mut missing = fixture;
    missing["Config"]
        .as_object_mut()
        .unwrap()
        .remove("Entrypoint");
    let inspection: ContainerInspection = serde_json::from_value(missing).unwrap();
    assert!(validate_transient_inspection(&config, &id, &inspection).is_err());
}

#[test]
fn podman_legacy_retained_supervisor_is_attested() {
    use crate::python::retained_podman::model::{CONTAINER_ENTRYPOINT, ContainerInspection};

    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let config = test_config(&root);
    let id = "e".repeat(64);
    let mut fixture = serde_json::to_value(matching_inspection(&config, &id)).unwrap();
    fixture["Config"]["Entrypoint"] = CONTAINER_ENTRYPOINT.into();
    fixture["Config"]["StopSignal"] = 15.into();
    fixture["HostConfig"]["RestartPolicy"]["Name"] = "".into();
    fixture["Config"]["CreateCommand"][0] = config.podman.to_str().unwrap().into();
    fixture["Config"]["Annotations"]
        .as_object_mut()
        .unwrap()
        .remove("io.podman.annotations.pids-limit");
    fixture["Config"]["Annotations"]
        .as_object_mut()
        .unwrap()
        .remove("io.podman.annotations.userns");
    fixture["HostConfig"]
        .as_object_mut()
        .unwrap()
        .remove("Annotations");
    let inspection: ContainerInspection = serde_json::from_value(fixture).unwrap();
    validate_retained_inspection(&config, &id, &inspection, true).unwrap();
}

#[test]
fn podman_legacy_inspection_keeps_signal_and_annotation_checks() {
    use crate::python::retained_podman::model::ContainerInspection;
    use serde_json::json;

    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let config = transient_test_config(&root, NetworkAccess::None, false);
    let id = "d".repeat(64);
    let mut fixture = serde_json::to_value(matching_transient_inspection(&config, &id)).unwrap();
    // Podman 4.9's complete relevant schema, not just individual field changes:
    // https://github.com/containers/podman/blob/v4.9.3/libpod/define/container_inspect.go
    fixture["Config"]["Entrypoint"] = json!("python3");
    fixture["Config"]["StopSignal"] = json!(15);
    fixture["HostConfig"]["RestartPolicy"]["Name"] = json!("");
    fixture["Config"]["CreateCommand"][0] = config.podman.to_str().unwrap().into();
    fixture["Config"]["Annotations"]
        .as_object_mut()
        .unwrap()
        .remove("io.podman.annotations.pids-limit");
    fixture["Config"]["Annotations"]
        .as_object_mut()
        .unwrap()
        .remove("io.podman.annotations.userns");
    fixture["HostConfig"]
        .as_object_mut()
        .unwrap()
        .remove("Annotations");
    let inspection: ContainerInspection = serde_json::from_value(fixture.clone()).unwrap();
    validate_transient_inspection(&config, &id, &inspection).unwrap();

    // Missing legacy metadata never excuses a changed enforced limit or
    // namespace identity, or conflicting metadata that is actually present.
    for (field, value) in [("PidsLimit", json!(0)), ("UsernsMode", json!("host"))] {
        let mut changed = fixture.clone();
        changed["HostConfig"][field] = value;
        let inspection: ContainerInspection = serde_json::from_value(changed).unwrap();
        assert!(validate_transient_inspection(&config, &id, &inspection).is_err());
    }
    for name in ["always", "unless-stopped", "on-failure", " no"] {
        let mut changed = fixture.clone();
        changed["HostConfig"]["RestartPolicy"]["Name"] = json!(name);
        let inspection: ContainerInspection = serde_json::from_value(changed).unwrap();
        assert!(validate_transient_inspection(&config, &id, &inspection).is_err());
    }
    let mut retries = fixture.clone();
    retries["HostConfig"]["RestartPolicy"]["MaximumRetryCount"] = json!(1);
    let inspection: ContainerInspection = serde_json::from_value(retries).unwrap();
    assert!(validate_transient_inspection(&config, &id, &inspection).is_err());
    let mut changed = fixture.clone();
    changed["HostConfig"]["IDMappings"]["UidMap"] = json!(["0:0:65536"]);
    let inspection: ContainerInspection = serde_json::from_value(changed).unwrap();
    assert!(validate_transient_inspection(&config, &id, &inspection).is_err());
    for (key, value) in [("pids-limit", "0"), ("userns", "host")] {
        let mut changed = fixture.clone();
        changed["Config"]["Annotations"][format!("io.podman.annotations.{key}")] = json!(value);
        let inspection: ContainerInspection = serde_json::from_value(changed).unwrap();
        assert!(validate_transient_inspection(&config, &id, &inspection).is_err());
    }
    let mut modern = serde_json::to_value(matching_transient_inspection(&config, &id)).unwrap();
    modern["Config"]["Annotations"]
        .as_object_mut()
        .unwrap()
        .remove("io.podman.annotations.pids-limit");
    let inspection: ContainerInspection = serde_json::from_value(modern).unwrap();
    assert!(validate_transient_inspection(&config, &id, &inspection).is_err());

    for program in ["/tmp/podman", "/untrusted/bin/podman", "", "sh"] {
        let mut value = fixture.clone();
        value["Config"]["CreateCommand"][0] = json!(program);
        let inspection: ContainerInspection = serde_json::from_value(value).unwrap();
        assert!(validate_transient_inspection(&config, &id, &inspection).is_err());
    }
    let mut changed_args = fixture.clone();
    changed_args["Config"]["CreateCommand"][1] = json!("run");
    let inspection: ContainerInspection = serde_json::from_value(changed_args).unwrap();
    assert!(validate_transient_inspection(&config, &id, &inspection).is_err());

    for signal in [json!(9), json!(0), json!("SIGKILL"), json!(" SIGTERM")] {
        let mut value = fixture.clone();
        value["Config"]["StopSignal"] = signal;
        let inspection: ContainerInspection = serde_json::from_value(value).unwrap();
        assert!(validate_transient_inspection(&config, &id, &inspection).is_err());
    }
    for signal in [
        json!(null),
        json!(-1),
        json!(15.5),
        json!(true),
        json!([15]),
    ] {
        let mut value = fixture.clone();
        value["Config"]["StopSignal"] = signal;
        assert!(serde_json::from_value::<ContainerInspection>(value).is_err());
    }
    // The Config annotations remain mandatory with or without the host copy.
    let mut missing = fixture.clone();
    missing["Config"]
        .as_object_mut()
        .unwrap()
        .remove("Annotations");
    let inspection: ContainerInspection = serde_json::from_value(missing).unwrap();
    assert!(validate_transient_inspection(&config, &id, &inspection).is_err());

    let mut empty = fixture.clone();
    empty["HostConfig"]["Annotations"] = json!({});
    let inspection: ContainerInspection = serde_json::from_value(empty).unwrap();
    assert!(validate_transient_inspection(&config, &id, &inspection).is_err());
    fixture["HostConfig"]["Annotations"] = json!(null);
    assert!(serde_json::from_value::<ContainerInspection>(fixture).is_err());
}

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
