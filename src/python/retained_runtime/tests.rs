use super::attach::finish_attach_child_then;
use super::launch::{
    GRACEFUL_SHUTDOWN_TIMEOUT, STARTUP_TIMEOUT, config_from_manifest, import_legacy_manifest,
    require_current_attach_abi, validate_nonlocal_config,
};
use super::maintenance::{RuntimeMaintenanceOutcome, maintain_locked_runtime};
use super::*;
use crate::config::{
    AccessMode, Config, NetworkAccess, PackageAccess, PythonExecutionTarget, SandboxBackend,
    ToolProfile,
};
use crate::config::{PythonRuntimeConfig, SandboxConfig};
use crate::python::retained_podman::{
    ABI_LABEL, CONTAINER_BROKER_SOCKET, CONTAINER_CAPABILITY_PATH, CONTAINER_ENTRYPOINT,
    CONTAINER_HOST_BRIDGE, ContainerPresence, LAYOUT_LABEL, LEGACY_CONTAINER_BROKER_SOCKET,
    RUNTIME_IMAGE_LABEL, RUNTIME_IMAGE_LABEL_VALUE, RetainedPodmanConfig,
    attest_new_retained_container, attest_retained_container, create_retained_container,
    exact_container_presence, prepare_retained_podman, resolve_retained_podman, run_remove,
    verify_exact_container_stopped,
};
use crate::python::runtime_store::{
    RuntimeLayoutProfile, RuntimeLifecycleState, RuntimeLock, RuntimeManifest, RuntimeStore,
    RuntimeWorkspaceOwnership, WorkspaceIdentity, retained_security_fingerprint,
    retained_shared_security_fingerprint,
};
use crate::python::{LaunchKind, LaunchSpec, PythonRuntimeNotice, RuntimeLaunchAction};
use chrono::Utc;
use serde_json::Value;
use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

fn nonlocal_config() -> Config {
    Config {
        tool_profile: ToolProfile::PythonOnly,
        python_runtime: PythonRuntimeConfig {
            target: Some(PythonExecutionTarget::Sandbox),
            sandbox: SandboxConfig {
                backend: Some(SandboxBackend::Podman),
                network: Some(NetworkAccess::Nonlocal),
                workspace_access: Some(AccessMode::ReadWrite),
                grants: Vec::new(),
                package_access: PackageAccess::Session,
                podman_image:
                    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                        .to_string(),
            },
            ..Default::default()
        },
        ..Default::default()
    }
}

#[tokio::test]
async fn cancelled_sweep_stops_before_runtime_store_or_podman_work() {
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    let error = sweep_retained_runtimes_with_cancellation(cancellation)
        .await
        .unwrap_err();

    assert_eq!(error, "retained runtime maintenance was cancelled");
}

#[tokio::test]
async fn cancelled_delete_stops_before_validation_or_runtime_store_work() {
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    let error = delete_retained_runtime_with_cancellation(
        "not-a-runtime-id",
        "not-a-session-id",
        cancellation,
    )
    .await
    .unwrap_err();

    assert_eq!(error, "retained runtime maintenance was cancelled");
}

fn expected_retained_create_command(config: &RetainedPodmanConfig) -> Vec<String> {
    let expected_abi = config.runtime_abi.clone();
    let mut current = config.clone();
    current.runtime_abi = crate::python::supervisor::RUNTIME_ABI.to_string();
    let spec = crate::python::retained_podman::build_create_spec(&current).unwrap();
    let current_label = format!(
        "--label={ABI_LABEL}={}",
        crate::python::supervisor::RUNTIME_ABI
    );
    let expected_label = format!("--label={ABI_LABEL}={expected_abi}");
    std::iter::once("podman".to_string())
        .chain(spec.args.into_iter().map(|argument| {
            let argument = argument.into_string().unwrap();
            if argument == crate::python::supervisor::RUNTIME_ABI {
                expected_abi.clone()
            } else if argument == current_label {
                expected_label.clone()
            } else {
                argument
            }
        }))
        .collect()
}

fn test_selinux_labels(category: u32) -> crate::python::selinux::SelinuxLabels {
    crate::python::selinux::SelinuxLabels::new(
        format!(
            "system_u:system_r:container_t:s0:c{category},c{}",
            category + 1
        ),
        format!(
            "system_u:object_r:container_file_t:s0:c{category},c{}",
            category + 1
        ),
    )
    .unwrap()
}

struct PresentRuntimeCase {
    runtime_abi: &'static str,
    layout_profile: RuntimeLayoutProfile,
    stopped_status: &'static str,
    container_id_bound: bool,
    creating_lifecycle: bool,
    simulate_unsaved_none_completion: bool,
    manifest_labels: Option<crate::python::selinux::SelinuxLabels>,
    observed_labels: Option<crate::python::selinux::SelinuxLabels>,
    expect_removed: bool,
}

fn stopped_retained_inspection(
    config: &RetainedPodmanConfig,
    container_id: &str,
    status: &str,
    observed_labels: Option<&crate::python::selinux::SelinuxLabels>,
) -> Value {
    let annotations = serde_json::json!({
        "io.podman.annotations.label": "type:container_t,label=filetype:container_file_t",
        "io.podman.annotations.pids-limit": "512",
        "io.podman.annotations.userns": "keep-id",
    });
    let mut labels = config.expected_labels();
    labels.insert(
        RUNTIME_IMAGE_LABEL.to_string(),
        RUNTIME_IMAGE_LABEL_VALUE.to_string(),
    );
    let destination = config.workspace_destination.to_string_lossy().into_owned();
    let broker_socket = if config.layout_profile == RuntimeLayoutProfile::LegacyRuntimeLocalV2 {
        LEGACY_CONTAINER_BROKER_SOCKET
    } else {
        CONTAINER_BROKER_SOCKET
    };
    let supervisor_args = vec![
        "supervisor".to_string(),
        config.runtime_abi.clone(),
        config.runtime_id.clone(),
        CONTAINER_CAPABILITY_PATH.to_string(),
        broker_socket.to_string(),
        destination.clone(),
        config.worker_uid.to_string(),
        config.worker_gid.to_string(),
    ];
    let uid = config.worker_uid;
    let gid = config.worker_gid;
    let (process_label, mount_label) = observed_labels
        .map(|labels| (labels.process_label.clone(), labels.mount_label.clone()))
        .unwrap_or_default();
    serde_json::json!({
        "Id": container_id,
        "Name": config.container_name,
        "Path": CONTAINER_ENTRYPOINT,
        "Args": supervisor_args,
        "Image": config.image_id.trim_start_matches("sha256:"),
        "EffectiveCaps": [
            "CAP_CHOWN", "CAP_DAC_OVERRIDE", "CAP_FOWNER", "CAP_FSETID",
            "CAP_SETFCAP", "CAP_SETGID", "CAP_SETPCAP", "CAP_SETUID"
        ],
        "Config": {
            "Annotations": annotations.clone(),
            "CreateCommand": expected_retained_create_command(config),
            "Entrypoint": [CONTAINER_ENTRYPOINT],
            "Cmd": supervisor_args,
            "Image": config.image_id,
            "Labels": labels,
            "Env": [],
            "OpenStdin": true,
            "StdinOnce": false,
            "Tty": false,
            "StopSignal": "SIGTERM",
            "StopTimeout": 15,
            "User": "0:0",
            "WorkingDir": destination,
        },
        "HostConfig": {
            "Annotations": annotations,
            "AutoRemove": false,
            "Binds": [
                format!("{}:{}:rw,rprivate,rbind", config.workspace.display(), config.workspace_destination.display()),
                format!("{}:{CONTAINER_HOST_BRIDGE}:ro,rprivate,rbind", config.broker_directory.display()),
            ],
            "CgroupMode": "private",
            "Devices": [],
            "GroupAdd": [],
            "IDMappings": {
                "UidMap": [format!("0:1:{uid}"), format!("{uid}:0:1"), format!("{}:{}:64536", uid + 1, uid + 1)],
                "GidMap": [format!("0:1:{gid}"), format!("{gid}:0:1"), format!("{}:{}:64536", gid + 1, gid + 1)],
            },
            "IpcMode": "private",
            "LogConfig": {"Type": "none"},
            "NetworkMode": "none",
            "PidMode": "private",
            "PidsLimit": 512,
            "PortBindings": {},
            "Privileged": false,
            "PublishAllPorts": false,
            "ReadonlyRootfs": false,
            "RestartPolicy": {"Name": "no", "MaximumRetryCount": 0},
            "SecurityOpt": [
                "no-new-privileges",
                "label=type:container_t,label=filetype:container_file_t"
            ],
            "Tmpfs": {
                "/run": "rw,nosuid,nodev,noexec,mode=0755,rprivate,tmpcopyup",
                "/tmp": "rw,nosuid,nodev,noexec,mode=1777,rprivate,tmpcopyup"
            },
            "UTSMode": "private",
            "UsernsMode": "private",
        },
        "Mounts": [
            {
                "Type": "bind",
                "Source": config.workspace,
                "Destination": config.workspace_destination,
                "Mode": "",
                "Options": ["rbind"],
                "RW": true,
                "Propagation": "rprivate",
            },
            {
                "Type": "bind",
                "Source": config.broker_directory,
                "Destination": CONTAINER_HOST_BRIDGE,
                "Mode": "",
                "Options": ["rbind"],
                "RW": false,
                "Propagation": "rprivate",
            }
        ],
        "NetworkSettings": {
            "Networks": {
                "none": {
                    "NetworkID": "none",
                    "Gateway": "",
                    "IPAddress": "",
                    "IPv6Gateway": "",
                    "GlobalIPv6Address": "",
                }
            }
        },
        "State": {
            "Status": status,
            "Running": false,
            "Paused": false,
            "Restarting": false,
            "OOMKilled": false,
            "Dead": false,
            "Pid": 0,
        },
        "ProcessLabel": process_label,
        "MountLabel": mount_label,
    })
}

fn install_fake_present_container(
    root: &std::path::Path,
    inspection: Value,
    selinux_enabled: bool,
) -> PathBuf {
    let executable = root.join("podman");
    std::fs::write(
        root.join("inspection.json"),
        serde_json::to_vec(&vec![inspection]).unwrap(),
    )
    .unwrap();
    std::fs::write(root.join("present"), b"present").unwrap();
    std::fs::write(
        root.join("selinux-enabled"),
        if selinux_enabled { "true" } else { "false" },
    )
    .unwrap();
    std::fs::write(
        &executable,
        r#"#!/usr/bin/python3
import json, pathlib, sys
root = pathlib.Path(__file__).resolve().parent
state = root / "present"
inspection = root / "inspection.json"
log = root / "commands.jsonl"
args = sys.argv[1:]
with log.open("a", encoding="utf-8") as output:
    output.write(json.dumps(args) + "\n")
if args and args[0] == "info":
    print((root / "selinux-enabled").read_text(encoding="utf-8"))
elif args[:2] == ["container", "exists"]:
    raise SystemExit(0 if state.exists() else 1)
elif args[:2] == ["container", "inspect"]:
    if not state.exists():
        raise SystemExit(1)
    sys.stdout.buffer.write(inspection.read_bytes())
elif args and args[0] == "stop":
    pass
elif args and args[0] == "rm":
    state.unlink(missing_ok=True)
else:
    raise SystemExit(2)
"#,
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    executable
}

async fn assert_present_runtime_maintenance(case: PresentRuntimeCase) {
    let temp = tempfile::tempdir().unwrap();
    let workspace_path = temp.path().join("workspace");
    std::fs::create_dir(&workspace_path).unwrap();
    std::fs::set_permissions(&workspace_path, std::fs::Permissions::from_mode(0o700)).unwrap();
    let workspace = WorkspaceIdentity::capture(&workspace_path).unwrap();
    let store = RuntimeStore::open_at(temp.path().join("state")).unwrap();
    let runtime_id = store.generate_runtime_id().unwrap();
    let session_id = crate::python::runtime_store::generate_uuid_v4().unwrap();
    let container_id = "d".repeat(64);
    let mut manifest = RuntimeManifest::new(
        runtime_id.clone(),
        session_id.clone(),
        format!("sha256:{}", "a".repeat(64)),
        "b".repeat(64),
        workspace,
        Utc::now(),
    )
    .unwrap();
    manifest.mark_create_command_finished().unwrap();
    if case.container_id_bound {
        manifest
            .bind_created_container(container_id.clone())
            .unwrap();
    } else if !case.creating_lifecycle {
        manifest
            .transition(RuntimeLifecycleState::Deleting)
            .unwrap();
    }
    manifest.runtime_abi = case.runtime_abi.to_string();
    manifest.layout_profile = case.layout_profile;
    manifest
        .labels
        .insert(ABI_LABEL.to_string(), case.runtime_abi.to_string());
    if case.layout_profile == RuntimeLayoutProfile::LegacyRuntimeLocalV2 {
        manifest.labels.remove(LAYOUT_LABEL);
    }
    manifest.selinux_labels = case.manifest_labels.clone();
    if case.creating_lifecycle {
        manifest.lifecycle = RuntimeLifecycleState::Creating;
    }
    manifest.validate_for_maintenance().unwrap();
    if case.simulate_unsaved_none_completion {
        assert!(case.creating_lifecycle);
        let mut unsaved_candidate = manifest.clone();
        unsaved_candidate
            .complete_created_container_attestation(None)
            .unwrap();
        assert_eq!(unsaved_candidate.lifecycle, RuntimeLifecycleState::Stopped);
        assert!(unsaved_candidate.selinux_labels.is_none());
    }

    let lock = store.create_locked_runtime(&runtime_id).unwrap();
    if case.layout_profile == RuntimeLayoutProfile::LegacyRuntimeLocalV2 {
        let legacy_broker = store
            .root()
            .join("python-runtimes")
            .join(&runtime_id)
            .join("broker");
        std::fs::create_dir(&legacy_broker).unwrap();
        std::fs::set_permissions(&legacy_broker, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    store.save_manifest(&lock, &manifest).unwrap();

    let fake_root = temp.path().join("fake-podman");
    std::fs::create_dir(&fake_root).unwrap();
    let fake = fake_root.join("podman");
    std::fs::write(&fake, "#!/bin/sh\nexit 2\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
    let config = config_from_manifest(&store, &lock, &manifest, fake.clone()).unwrap();
    let inspection = stopped_retained_inspection(
        &config,
        &container_id,
        case.stopped_status,
        case.observed_labels.as_ref(),
    );
    assert_eq!(
        install_fake_present_container(&fake_root, inspection, case.observed_labels.is_some(),),
        fake
    );

    let result =
        maintain_locked_runtime(&store, lock, &fake, Utc::now(), true, Some(&session_id)).await;
    let commands = std::fs::read_to_string(fake_root.join("commands.jsonl")).unwrap();
    assert!(
        commands.contains("\"container\", \"inspect\""),
        "{commands}"
    );
    assert!(!commands.contains("\"create\""), "{commands}");
    if case.expect_removed {
        assert_eq!(result.unwrap(), RuntimeMaintenanceOutcome::Removed);
        assert!(!store.runtime_state_exists(&runtime_id).unwrap());
        assert!(!fake_root.join("present").exists());
        assert!(commands.contains("\"rm\""), "{commands}");
    } else {
        let error = result.unwrap_err();
        assert!(error.contains("SELinux"), "{error}");
        assert!(store.runtime_state_exists(&runtime_id).unwrap());
        assert!(fake_root.join("present").exists());
        assert!(!commands.contains("\"rm\""), "{commands}");
        let lock = (0..100)
            .find_map(|_| {
                let lock = store.try_lock_runtime(&runtime_id).unwrap();
                if lock.is_none() {
                    std::thread::sleep(Duration::from_millis(2));
                }
                lock
            })
            .expect("maintenance should release the quarantined runtime lock");
        let quarantined = store.load_manifest(&lock).unwrap();
        assert_eq!(quarantined.lifecycle, RuntimeLifecycleState::Quarantined);
        assert_eq!(quarantined.selinux_labels, case.manifest_labels);
    }
}

#[tokio::test]
async fn stopped_bound_containers_without_selinux_labels_delete_as_existing() {
    assert_present_runtime_maintenance(PresentRuntimeCase {
        runtime_abi: crate::python::supervisor::RUNTIME_ABI,
        layout_profile: RuntimeLayoutProfile::ShortSiblingV1,
        stopped_status: "stopped",
        container_id_bound: true,
        creating_lifecycle: false,
        simulate_unsaved_none_completion: false,
        manifest_labels: None,
        observed_labels: None,
        expect_removed: true,
    })
    .await;
    assert_present_runtime_maintenance(PresentRuntimeCase {
        runtime_abi: crate::python::runtime_store::LEGACY_RUNTIME_ABI_V2,
        layout_profile: RuntimeLayoutProfile::LegacyRuntimeLocalV2,
        stopped_status: "exited",
        container_id_bound: true,
        creating_lifecycle: false,
        simulate_unsaved_none_completion: false,
        manifest_labels: None,
        observed_labels: None,
        expect_removed: true,
    })
    .await;
}

#[tokio::test]
async fn creating_bound_id_retry_after_unsaved_none_completion_binds_labels() {
    assert_present_runtime_maintenance(PresentRuntimeCase {
        runtime_abi: crate::python::supervisor::RUNTIME_ABI,
        layout_profile: RuntimeLayoutProfile::ShortSiblingV1,
        stopped_status: "exited",
        container_id_bound: true,
        creating_lifecycle: true,
        simulate_unsaved_none_completion: true,
        manifest_labels: None,
        observed_labels: Some(test_selinux_labels(42)),
        expect_removed: true,
    })
    .await;
}

#[tokio::test]
async fn creating_exact_name_recovery_atomically_completes_none_binding() {
    assert_present_runtime_maintenance(PresentRuntimeCase {
        runtime_abi: crate::python::supervisor::RUNTIME_ABI,
        layout_profile: RuntimeLayoutProfile::ShortSiblingV1,
        stopped_status: "configured",
        container_id_bound: false,
        creating_lifecycle: true,
        simulate_unsaved_none_completion: false,
        manifest_labels: None,
        observed_labels: None,
        expect_removed: true,
    })
    .await;
}

#[tokio::test]
async fn deleting_exact_name_recovery_binds_observed_selinux_labels_with_id() {
    assert_present_runtime_maintenance(PresentRuntimeCase {
        runtime_abi: crate::python::supervisor::RUNTIME_ABI,
        layout_profile: RuntimeLayoutProfile::ShortSiblingV1,
        stopped_status: "configured",
        container_id_bound: false,
        creating_lifecycle: false,
        simulate_unsaved_none_completion: false,
        manifest_labels: None,
        observed_labels: Some(test_selinux_labels(50)),
        expect_removed: true,
    })
    .await;
}

#[tokio::test]
async fn durably_attested_none_rejects_some_label_drift_on_retry() {
    assert_present_runtime_maintenance(PresentRuntimeCase {
        runtime_abi: crate::python::supervisor::RUNTIME_ABI,
        layout_profile: RuntimeLayoutProfile::ShortSiblingV1,
        stopped_status: "stopped",
        container_id_bound: true,
        creating_lifecycle: false,
        simulate_unsaved_none_completion: false,
        manifest_labels: None,
        observed_labels: Some(test_selinux_labels(55)),
        expect_removed: false,
    })
    .await;
}

#[tokio::test]
async fn already_bound_selinux_label_drift_is_quarantined_without_removal() {
    assert_present_runtime_maintenance(PresentRuntimeCase {
        runtime_abi: crate::python::supervisor::RUNTIME_ABI,
        layout_profile: RuntimeLayoutProfile::ShortSiblingV1,
        stopped_status: "stopped",
        container_id_bound: true,
        creating_lifecycle: false,
        simulate_unsaved_none_completion: false,
        manifest_labels: Some(test_selinux_labels(60)),
        observed_labels: Some(test_selinux_labels(70)),
        expect_removed: false,
    })
    .await;
}

#[tokio::test]
async fn final_stopped_verification_runs_after_attach_child_is_reaped() {
    let mut command = tokio::process::Command::new("/bin/sh");
    command
        .arg("-c")
        .arg("trap '' TERM; while :; do sleep 1; done")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .process_group(0);
    let mut child = command.spawn().unwrap();
    let pid = i32::try_from(child.id().unwrap()).unwrap();
    let verified = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let verified_after_reap = verified.clone();

    finish_attach_child_then(
        &mut child,
        Duration::from_millis(50),
        Duration::from_secs(1),
        move || async move {
            assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ESRCH)
            );
            verified_after_reap.store(true, std::sync::atomic::Ordering::Release);
            Ok(())
        },
    )
    .await
    .unwrap();
    assert!(verified.load(std::sync::atomic::Ordering::Acquire));
}

#[test]
fn only_exact_nonlocal_policy_reaches_retained_manager() {
    let config = nonlocal_config();
    assert!(validate_nonlocal_config(&config).is_ok());
    let mut changed = config.clone();
    changed.python_runtime.sandbox.network = Some(NetworkAccess::Full);
    assert!(validate_nonlocal_config(&changed).is_err());
    let mut changed = config;
    changed.python_runtime.sandbox.package_access = PackageAccess::Disabled;
    assert!(validate_nonlocal_config(&changed).is_err());
}

#[test]
fn previous_v3_worker_layers_fail_closed_without_automatic_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace-v3");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::set_permissions(&workspace, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut manifest = RuntimeManifest::new(
        crate::python::runtime_store::generate_uuid_v4().unwrap(),
        crate::python::runtime_store::generate_uuid_v4().unwrap(),
        format!("sha256:{}", "a".repeat(64)),
        "b".repeat(64),
        WorkspaceIdentity::capture(&workspace).unwrap(),
        Utc::now(),
    )
    .unwrap();
    manifest.mark_create_command_finished().unwrap();
    manifest.runtime_abi = crate::python::runtime_store::LEGACY_RUNTIME_ABI_V3.to_string();
    manifest.labels.insert(
        ABI_LABEL.to_string(),
        crate::python::runtime_store::LEGACY_RUNTIME_ABI_V3.to_string(),
    );

    assert!(manifest.validate_for_maintenance().is_ok());
    assert!(manifest.validate().is_err());
    let error = require_current_attach_abi(&manifest).unwrap_err();
    assert!(error.contains("worker-local output recovery"), "{error}");
    assert!(error.contains("stopped and delete-only"), "{error}");
    assert!(
        error.contains("will not rebuild, pull, or delete"),
        "{error}"
    );
}

#[tokio::test]
async fn v2_worker_layers_are_explicitly_delete_only() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace-v2");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::set_permissions(&workspace, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut manifest = RuntimeManifest::new(
        crate::python::runtime_store::generate_uuid_v4().unwrap(),
        crate::python::runtime_store::generate_uuid_v4().unwrap(),
        format!("sha256:{}", "a".repeat(64)),
        "b".repeat(64),
        WorkspaceIdentity::capture(&workspace).unwrap(),
        Utc::now(),
    )
    .unwrap();
    manifest.mark_create_command_finished().unwrap();
    manifest.runtime_abi = crate::python::runtime_store::LEGACY_RUNTIME_ABI_V2.to_string();
    manifest.labels.insert(
        ABI_LABEL.to_string(),
        crate::python::runtime_store::LEGACY_RUNTIME_ABI_V2.to_string(),
    );
    assert!(manifest.validate_for_maintenance().is_ok());
    let error = require_current_attach_abi(&manifest).unwrap_err();
    assert!(error.contains("stopped and delete-only"), "{error}");
    assert!(error.contains("worker-local output recovery"), "{error}");
    assert!(
        error.contains("will not rebuild, pull, or delete"),
        "{error}"
    );

    let store = RuntimeStore::open_at(temp.path().join("state")).unwrap();
    let lock = store.create_locked_runtime(&manifest.runtime_id).unwrap();
    store.save_manifest(&lock, &manifest).unwrap();
    import_legacy_manifest(
        &store,
        &lock,
        &mut manifest,
        std::path::Path::new("/bin/false"),
    )
    .await
    .unwrap();
    let maintenance_config =
        config_from_manifest(&store, &lock, &manifest, PathBuf::from("/bin/false")).unwrap();
    assert_eq!(
        maintenance_config.runtime_abi,
        crate::python::runtime_store::LEGACY_RUNTIME_ABI_V2
    );
    manifest
        .transition(RuntimeLifecycleState::Deleting)
        .unwrap();
    store.save_manifest(&lock, &manifest).unwrap();
    assert_eq!(
        maintain_locked_runtime(
            &store,
            lock,
            std::path::Path::new("/bin/false"),
            Utc::now(),
            true,
            Some(&manifest.session_id),
        )
        .await
        .unwrap(),
        RuntimeMaintenanceOutcome::Removed
    );
    assert!(!store.runtime_state_exists(&manifest.runtime_id).unwrap());
}

#[test]
fn maintenance_config_defers_live_broker_validation_to_containment_path() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::set_permissions(&workspace, std::fs::Permissions::from_mode(0o700)).unwrap();
    let workspace = WorkspaceIdentity::capture(&workspace).unwrap();
    let store = RuntimeStore::open_at(temp.path().join("state")).unwrap();
    let runtime_id = store.generate_runtime_id().unwrap();
    let session_id = crate::python::runtime_store::generate_uuid_v4().unwrap();
    let lock = store.create_locked_runtime(&runtime_id).unwrap();
    let manifest = RuntimeManifest::new(
        runtime_id.clone(),
        session_id,
        format!("sha256:{}", "a".repeat(64)),
        "b".repeat(64),
        workspace,
        Utc::now(),
    )
    .unwrap();
    std::fs::remove_dir(store.broker_directory(&lock).unwrap()).unwrap();

    let config = config_from_manifest(&store, &lock, &manifest, PathBuf::from("/bin/true"))
        .expect("immutable maintenance config must survive missing live broker state");
    assert!(config.validate().is_err());
}

struct RealContainerGuard {
    podman: PathBuf,
    container_id: String,
    armed: bool,
}

impl RealContainerGuard {
    fn new(podman: PathBuf, container_id: String) -> Self {
        Self {
            podman,
            container_id,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for RealContainerGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let removal = std::process::Command::new(&self.podman)
            .args(["rm", "--force", "--time=10", "--", &self.container_id])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let presence = std::process::Command::new(&self.podman)
            .args(["container", "exists", "--", &self.container_id])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if !matches!(presence, Ok(status) if status.code() == Some(1)) {
            eprintln!(
                "real retained test cleanup could not verify exact container {} absence (remove={removal:?}, presence={presence:?})",
                self.container_id
            );
        }
    }
}

struct RealRuntimeFixture {
    _state: tempfile::TempDir,
    _workspace_root: tempfile::TempDir,
    store: RuntimeStore,
    workspace: WorkspaceIdentity,
    config: RetainedPodmanConfig,
    container_id: String,
    launch: LaunchSpec,
    fingerprint: String,
    guard: RealContainerGuard,
}

impl RealRuntimeFixture {
    async fn create() -> Self {
        Self::create_with_shared_workspace(false).await
    }

    async fn create_shared() -> Self {
        Self::create_with_shared_workspace(true).await
    }

    async fn create_with_shared_workspace(shared: bool) -> Self {
        let requested = std::env::var("LETHETIC_RUNTIME_IMAGE")
            .expect("set LETHETIC_RUNTIME_IMAGE to an already-local Lethetic runtime image");
        let binary = std::env::var_os("LETHETIC_TEST_BINARY")
            .map(PathBuf::from)
            .expect("set LETHETIC_TEST_BINARY to the already-built Lethetic binary")
            .canonicalize()
            .unwrap();
        let state = tempfile::tempdir().unwrap();
        let workspace_root = tempfile::tempdir().unwrap();
        let managed_workspace_path = workspace_root.path().join("managed-workspace");
        std::fs::create_dir(&managed_workspace_path).unwrap();
        std::fs::set_permissions(
            &managed_workspace_path,
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        let managed_workspace = WorkspaceIdentity::capture(&managed_workspace_path).unwrap();
        let workspace = if shared {
            let shared_path = workspace_root.path().join("shared launch cwd with spaces");
            std::fs::create_dir(&shared_path).unwrap();
            std::fs::set_permissions(&shared_path, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::create_dir(shared_path.join(".lethetic")).unwrap();
            std::fs::set_permissions(
                shared_path.join(".lethetic"),
                std::fs::Permissions::from_mode(0o700),
            )
            .unwrap();
            std::fs::write(
                shared_path.join(".lethetic").join("host-marker"),
                "host-only",
            )
            .unwrap();
            WorkspaceIdentity::capture(&shared_path).unwrap()
        } else {
            managed_workspace.clone()
        };
        let state_root = state
            .path()
            .join("production-length-state-root-padding-1234567890");
        let store = RuntimeStore::open_at(state_root).unwrap();
        let runtime_id = store.generate_runtime_id().unwrap();
        let session_id = crate::python::runtime_store::generate_uuid_v4().unwrap();
        let (podman, image) = prepare_retained_podman(&requested).await.unwrap();
        let worker_uid = rustix::process::geteuid().as_raw();
        let worker_gid = rustix::process::getegid().as_raw();
        let security_fingerprint = if shared {
            retained_shared_security_fingerprint(
                &"a".repeat(64),
                &image.image_id,
                &workspace,
                &managed_workspace,
                worker_uid,
                worker_gid,
            )
        } else {
            retained_security_fingerprint(
                &"a".repeat(64),
                &image.image_id,
                &workspace,
                worker_uid,
                worker_gid,
            )
        }
        .unwrap();
        let lock = store.create_locked_runtime(&runtime_id).unwrap();
        let broker_directory = store.broker_directory(&lock).unwrap();
        let legacy_socket = store
            .root()
            .join("python-runtimes")
            .join(&runtime_id)
            .join("broker")
            .join("broker.sock");
        assert!(
            crate::python::egress_broker::validate_unix_socket_path_length(&legacy_socket).is_err(),
            "real fixture no longer exercises the production socket-length boundary: {}",
            legacy_socket.display()
        );
        crate::python::egress_broker::validate_unix_socket_path_length(
            &broker_directory.join(crate::python::runtime_store::BROKER_SOCKET_FILE),
        )
        .unwrap();
        let config = RetainedPodmanConfig {
            podman: podman.clone(),
            image_id: image.image_id.clone(),
            container_name: format!("lethetic-python-{runtime_id}"),
            runtime_id: runtime_id.clone(),
            session_id: session_id.clone(),
            security_fingerprint: security_fingerprint.clone(),
            runtime_abi: crate::python::supervisor::RUNTIME_ABI.to_string(),
            layout_profile: if shared {
                RuntimeLayoutProfile::ShortSiblingSharedCwdV1
            } else {
                RuntimeLayoutProfile::ShortSiblingV1
            },
            workspace: workspace.canonical_path.clone(),
            workspace_destination: if shared {
                workspace.canonical_path.clone()
            } else {
                PathBuf::from(crate::python::retained_podman::CONTAINER_WORKSPACE)
            },
            workspace_ownership: if shared {
                RuntimeWorkspaceOwnership::ExternalLaunchCwd
            } else {
                RuntimeWorkspaceOwnership::Managed
            },
            mask_lethetic: shared,
            broker_directory,
            worker_uid,
            worker_gid,
        };
        let mut manifest = RuntimeManifest::new_with_workspace_binding(
            runtime_id.clone(),
            session_id,
            image.image_id,
            security_fingerprint.clone(),
            workspace.clone(),
            if shared {
                RuntimeWorkspaceOwnership::ExternalLaunchCwd
            } else {
                RuntimeWorkspaceOwnership::Managed
            },
            shared.then_some(managed_workspace),
            Utc::now(),
        )
        .unwrap();
        store.save_manifest(&lock, &manifest).unwrap();
        let container_id = create_retained_container(&config).await.unwrap();
        let guard = RealContainerGuard::new(podman, container_id.clone());
        manifest.mark_create_command_finished().unwrap();
        manifest
            .record_created_container_id(container_id.clone())
            .unwrap();
        store.save_manifest(&lock, &manifest).unwrap();
        let identity = attest_new_retained_container(&config, &container_id, &workspace)
            .await
            .unwrap();
        manifest
            .complete_created_container_attestation(identity.selinux_labels)
            .unwrap();
        store.save_manifest(&lock, &manifest).unwrap();
        drop(lock);

        let launch = LaunchSpec {
            kind: LaunchKind::Direct,
            container_identity: Some(
                crate::python::PythonContainerIdentity::retained(&runtime_id, true).unwrap(),
            ),
            program: binary.into_os_string(),
            args: vec![
                OsString::from("--internal-retained-attach"),
                OsString::from(RETAINED_ATTACH_ABI),
                OsString::from(&runtime_id),
                store.root().as_os_str().to_os_string(),
            ],
            cwd: Some(workspace.canonical_path.clone()),
            clear_env: false,
            env: Vec::new(),
            cleanup: None,
            startup_notice: Some(PythonRuntimeNotice {
                container_id: container_id.clone(),
                container_name: format!("lethetic-python-{runtime_id}"),
                action: RuntimeLaunchAction::Created,
                network: NetworkAccess::Nonlocal,
                mounted_cwd: workspace.canonical_path.clone(),
            }),
            startup_timeout: STARTUP_TIMEOUT,
            graceful_shutdown: Some(GRACEFUL_SHUTDOWN_TIMEOUT),
        };
        Self {
            _state: state,
            _workspace_root: workspace_root,
            store,
            workspace,
            config,
            container_id,
            launch,
            fingerprint: format!("real-retained-smoke:{runtime_id}:{security_fingerprint}"),
            guard,
        }
    }

    async fn lock(&self) -> RuntimeLock {
        timeout(Duration::from_secs(5), async {
            loop {
                if let Some(lock) = self
                    .store
                    .try_lock_runtime(&self.config.runtime_id)
                    .unwrap()
                {
                    break lock;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("retained attach did not release its runtime lock")
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the configured rootless Podman forwarder"]
async fn real_uninitialized_runtime_maintenance_uses_bounded_stack() {
    let state = tempfile::tempdir().unwrap();
    let store = RuntimeStore::open_at(state.path().join("state")).unwrap();
    let runtime_id = store.generate_runtime_id().unwrap();
    let lock = store.create_locked_runtime(&runtime_id).unwrap();
    let podman = resolve_retained_podman().await.unwrap();
    assert_eq!(
        Box::pin(maintain_locked_runtime(
            &store,
            lock,
            &podman,
            Utc::now(),
            false,
            None,
        ))
        .await
        .unwrap(),
        RuntimeMaintenanceOutcome::Removed
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "recovers a real post-create crash before the container ID was persisted"]
async fn real_creating_reconciliation_recovers_exact_name_then_deletes() {
    let mut fixture = RealRuntimeFixture::create().await;
    let lock = fixture.lock().await;
    let mut manifest = fixture.store.load_manifest(&lock).unwrap();
    manifest.lifecycle = RuntimeLifecycleState::Creating;
    manifest.container_id = None;
    manifest.selinux_labels = None;
    manifest.create_command_finished = Some(false);
    fixture.store.save_manifest(&lock, &manifest).unwrap();
    assert_eq!(
        Box::pin(maintain_locked_runtime(
            &fixture.store,
            lock,
            &fixture.config.podman,
            Utc::now(),
            false,
            Some(&fixture.config.session_id),
        ))
        .await
        .unwrap(),
        RuntimeMaintenanceOutcome::Removed
    );
    fixture.guard.disarm();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "creates, attaches, stops, and removes an exact real retained Podman container"]
async fn real_manifest_bound_attach_executes_a_python_cell_and_stops() {
    let mut fixture = RealRuntimeFixture::create().await;
    let runspace = crate::python::PythonRunspace::new();
    let result = runspace
        .execute(
            fixture.launch.clone(),
            fixture.fingerprint.clone(),
            "retained_smoke_value = 41\nretained_smoke_value + 1",
            CancellationToken::new(),
        )
        .await;
    runspace.reset().await;
    let result = result.unwrap();
    assert!(!result.is_error, "{}", result.render());
    assert_eq!(result.value_repr, "42");

    let lock = fixture.lock().await;
    let manifest = fixture.store.load_manifest(&lock).unwrap();
    assert_eq!(manifest.lifecycle, RuntimeLifecycleState::Stopped);
    let identity = attest_retained_container(
        &fixture.config,
        &fixture.container_id,
        &fixture.workspace,
        manifest.selinux_labels.as_ref(),
    )
    .await
    .unwrap();
    assert!(!identity.running);
    assert_eq!(
        Box::pin(maintain_locked_runtime(
            &fixture.store,
            lock,
            &fixture.config.podman,
            Utc::now() + chrono::Duration::days(crate::python::runtime_store::RUNTIME_TTL_DAYS,),
            false,
            Some(&fixture.config.session_id),
        ))
        .await
        .unwrap(),
        RuntimeMaintenanceOutcome::Removed
    );
    fixture.guard.disarm();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "creates and removes a shared-cwd retained Podman runtime from an already-local image"]
async fn real_shared_cwd_retained_runtime_masks_control_state_and_stops() {
    let mut fixture = RealRuntimeFixture::create_shared().await;
    let runspace = crate::python::PythonRunspace::new();
    let notice = runspace
        .ensure_ready(
            fixture.launch.clone(),
            fixture.fingerprint.clone(),
            CancellationToken::new(),
        )
        .await
        .unwrap()
        .expect("retained worker hello did not emit its runtime notice");
    assert_eq!(notice.container_id, fixture.container_id);
    assert_eq!(notice.action, RuntimeLaunchAction::Created);
    assert_eq!(notice.network, NetworkAccess::Nonlocal);
    assert_eq!(notice.mounted_cwd, fixture.workspace.canonical_path);

    let root = serde_json::to_string(fixture.workspace.canonical_path.to_str().unwrap()).unwrap();
    let code = format!(
        "import pathlib, socket\nroot = pathlib.Path({root})\nassert pathlib.Path.cwd() == root\ntry:\n    marker_visible = (root / '.lethetic' / 'host-marker').exists()\nexcept PermissionError:\n    pass\nelse:\n    assert not marker_visible\n(root / 'retained write.txt').write_text('ok', encoding='utf-8')\ns = socket.socket()\ns.settimeout(2)\nassert s.connect_ex(('127.0.0.1', 9)) != 0\n42"
    );
    let result = runspace
        .execute(
            fixture.launch.clone(),
            fixture.fingerprint.clone(),
            &code,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(!result.is_error, "{}", result.render());
    assert_eq!(result.value_repr, "42");
    assert_eq!(
        result.runtime_notice.unwrap().container_id,
        fixture.container_id
    );
    runspace.reset_checked().await.unwrap();
    assert_eq!(
        std::fs::read_to_string(fixture.workspace.canonical_path.join("retained write.txt"),)
            .unwrap(),
        "ok"
    );
    assert_eq!(
        std::fs::read_to_string(
            fixture
                .workspace
                .canonical_path
                .join(".lethetic")
                .join("host-marker"),
        )
        .unwrap(),
        "host-only"
    );

    let lock = fixture.lock().await;
    let manifest = fixture.store.load_manifest(&lock).unwrap();
    assert_eq!(manifest.lifecycle, RuntimeLifecycleState::Stopped);
    let identity = attest_retained_container(
        &fixture.config,
        &fixture.container_id,
        &fixture.workspace,
        manifest.selinux_labels.as_ref(),
    )
    .await
    .unwrap();
    assert!(!identity.running);
    assert_eq!(
        Box::pin(maintain_locked_runtime(
            &fixture.store,
            lock,
            &fixture.config.podman,
            Utc::now() + chrono::Duration::days(crate::python::runtime_store::RUNTIME_TTL_DAYS,),
            false,
            Some(&fixture.config.session_id),
        ))
        .await
        .unwrap(),
        RuntimeMaintenanceOutcome::Removed
    );
    fixture.guard.disarm();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires public HTTP/HTTPS and the configured rootless Podman forwarder"]
async fn real_nonlocal_broker_allows_public_web_and_denies_direct_local_routes() {
    let mut fixture = RealRuntimeFixture::create().await;
    let runspace = crate::python::PythonRunspace::new();
    let code = r#"import socket
import urllib.error
import urllib.request

for url in (
    "http://deb.debian.org/debian/README",
    "https://pypi.org/simple/",
):
    try:
        with urllib.request.urlopen(url, timeout=20) as response:
            response.read(1)
    except Exception as error:
        raise RuntimeError(
            f"public proxy request failed: {error!r}; proxies={urllib.request.getproxies()!r}"
        ) from error

for url in (
    "http://127.0.0.1/",
    "http://10.0.0.1/",
    "http://169.254.169.254/latest/meta-data/",
):
    try:
        urllib.request.urlopen(url, timeout=3)
    except urllib.error.HTTPError as error:
        if error.code != 403:
            raise
    else:
        raise AssertionError(f"blocked destination unexpectedly succeeded: {url}")

try:
    socket.create_connection(("1.1.1.1", 443), timeout=3)
except OSError:
    pass
else:
    raise AssertionError("direct public networking unexpectedly succeeded")

non_loopback = [name for _, name in socket.if_nameindex() if name != "lo"]
if non_loopback:
    raise AssertionError(f"unexpected direct network interfaces: {non_loopback}")

True"#;
    let result = runspace
        .execute(
            fixture.launch.clone(),
            fixture.fingerprint.clone(),
            code,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let worker_diagnostics = runspace.diagnostics_text().await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    runspace.reset().await;
    let audit_path = fixture
        .store
        .root()
        .join("python-runtimes")
        .join(&fixture.config.runtime_id)
        .join("egress-audit.jsonl");
    let audit = std::fs::read_to_string(audit_path).unwrap_or_default();
    assert!(
        !result.is_error,
        "{}\nWorker diagnostics:\n{}\nBroker audit:\n{audit}",
        result.render(),
        worker_diagnostics,
    );
    assert_eq!(result.value_repr, "True");
    let records = audit
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    for (destination, port) in [("deb.debian.org", 80), ("pypi.org", 443)] {
        assert!(
            records.iter().any(|record| {
                record["destination"] == destination
                    && record["port"] == port
                    && record["decision"] == "allow"
                    && record["code"] == "connected"
            }),
            "missing public allow record for {destination}:{port}:\n{audit}"
        );
    }
    assert_eq!(
        records
            .iter()
            .filter(|record| { record["decision"] == "deny" && record["code"] == "proxy_request" })
            .count(),
        3,
        "local/metadata requests were not rejected by broker policy:\n{audit}"
    );

    let lock = fixture.lock().await;
    assert_eq!(
        Box::pin(maintain_locked_runtime(
            &fixture.store,
            lock,
            &fixture.config.podman,
            Utc::now() + chrono::Duration::days(crate::python::runtime_store::RUNTIME_TTL_DAYS,),
            false,
            Some(&fixture.config.session_id),
        ))
        .await
        .unwrap(),
        RuntimeMaintenanceOutcome::Removed
    );
    fixture.guard.disarm();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "refreshes package indexes through the configured retained runtime broker"]
async fn real_package_refresh_uses_only_the_constrained_broker() {
    let mut fixture = RealRuntimeFixture::create().await;
    let runspace = crate::python::PythonRunspace::new();
    let code = r#"import subprocess
result = subprocess.run(
    ["lethetic-pkg", "refresh"],
    capture_output=True,
    text=True,
    timeout=300,
)
if result.returncode != 0:
    raise RuntimeError(result.stdout + "\n" + result.stderr)
True"#;
    let result = runspace
        .execute(
            fixture.launch.clone(),
            fixture.fingerprint.clone(),
            code,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    runspace.reset().await;
    let audit_path = fixture
        .store
        .root()
        .join("python-runtimes")
        .join(&fixture.config.runtime_id)
        .join("egress-audit.jsonl");
    let audit = std::fs::read_to_string(audit_path).unwrap_or_default();
    assert!(
        !result.is_error,
        "{}\nBroker audit:\n{audit}",
        result.render()
    );
    assert_eq!(result.value_repr, "True");
    assert!(audit.contains("\"decision\":\"allow\""));
    assert!(audit.contains("\"code\":\"connected\""));

    let lock = fixture.lock().await;
    assert_eq!(
        Box::pin(maintain_locked_runtime(
            &fixture.store,
            lock,
            &fixture.config.podman,
            Utc::now() + chrono::Duration::days(crate::python::runtime_store::RUNTIME_TTL_DAYS,),
            false,
            Some(&fixture.config.session_id),
        ))
        .await
        .unwrap(),
        RuntimeMaintenanceOutcome::Removed
    );
    fixture.guard.disarm();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "uses a real container to prove teardown survives model-controlled workspace chmod"]
async fn real_teardown_contains_workspace_attestation_drift() {
    let mut fixture = RealRuntimeFixture::create().await;
    let runspace = crate::python::PythonRunspace::new();
    let code = r#"import os, subprocess, sys
subprocess.Popen([
    sys.executable,
    "-c",
    "import pathlib,time\np=pathlib.Path('/workspace/drift-writer.log')\nwhile True:\n p.open('ab').write(b'x')\n time.sleep(0.02)",
], start_new_session=True)
os.chmod('/workspace', 0o755)"#;
    let result = runspace
        .execute(
            fixture.launch.clone(),
            fixture.fingerprint.clone(),
            code,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(!result.is_error, "{}", result.render());
    tokio::time::sleep(Duration::from_millis(200)).await;
    runspace.reset().await;

    let lock = fixture.lock().await;
    let manifest = fixture.store.load_manifest(&lock).unwrap();
    assert_eq!(manifest.lifecycle, RuntimeLifecycleState::Quarantined);
    verify_exact_container_stopped(&fixture.config.podman, &fixture.container_id)
        .await
        .unwrap();
    let writer = fixture.workspace.canonical_path.join("drift-writer.log");
    let before = std::fs::metadata(&writer).unwrap().len();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(std::fs::metadata(&writer).unwrap().len(), before);

    std::fs::set_permissions(
        &fixture.workspace.canonical_path,
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    attest_retained_container(
        &fixture.config,
        &fixture.container_id,
        &fixture.workspace,
        manifest.selinux_labels.as_ref(),
    )
    .await
    .unwrap();
    run_remove(&fixture.config.podman, &fixture.container_id)
        .await
        .unwrap();
    assert_eq!(
        exact_container_presence(&fixture.config.podman, &fixture.container_id)
            .await
            .unwrap(),
        ContainerPresence::Absent
    );
    drop(lock);
    fixture.guard.disarm();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "kills a real attach helper to verify parent-death containment and reconciliation"]
async fn real_attach_helper_crash_stops_and_reconciles_container() {
    let mut fixture = RealRuntimeFixture::create().await;
    let mut worker = crate::python::Worker::spawn(fixture.launch.clone(), CancellationToken::new())
        .await
        .unwrap();
    let result = worker
            .execute(
                "import subprocess,sys\nsubprocess.Popen([sys.executable, '-c', \"import pathlib,time\\np=pathlib.Path('/workspace/crash-writer.log')\\nwhile True:\\n p.open('ab').write(b'x')\\n time.sleep(0.02)\"], start_new_session=True)",
                None,
            )
            .await
            .unwrap();
    assert!(!result.is_error, "{}", result.render());
    tokio::time::sleep(Duration::from_millis(200)).await;
    let helper_pid = worker
        .child
        .as_ref()
        .and_then(tokio::process::Child::id)
        .unwrap();
    assert_eq!(unsafe { libc::kill(helper_pid as i32, libc::SIGKILL) }, 0);
    timeout(
        Duration::from_secs(5),
        worker.child.as_mut().unwrap().wait(),
    )
    .await
    .expect("attach helper did not die after SIGKILL")
    .unwrap();
    drop(worker);

    timeout(Duration::from_secs(30), async {
        loop {
            if verify_exact_container_stopped(&fixture.config.podman, &fixture.container_id)
                .await
                .is_ok()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("parent-death broker lease did not stop the retained container");
    let writer = fixture.workspace.canonical_path.join("crash-writer.log");
    let before = std::fs::metadata(&writer).unwrap().len();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(std::fs::metadata(&writer).unwrap().len(), before);

    let lock = fixture.lock().await;
    assert_eq!(
        Box::pin(maintain_locked_runtime(
            &fixture.store,
            lock,
            &fixture.config.podman,
            Utc::now(),
            false,
            Some(&fixture.config.session_id),
        ))
        .await
        .unwrap(),
        RuntimeMaintenanceOutcome::Reconciled
    );
    let lock = fixture.lock().await;
    assert_eq!(
        fixture.store.load_manifest(&lock).unwrap().lifecycle,
        RuntimeLifecycleState::Stopped
    );
    assert_eq!(
        Box::pin(maintain_locked_runtime(
            &fixture.store,
            lock,
            &fixture.config.podman,
            Utc::now(),
            true,
            Some(&fixture.config.session_id),
        ))
        .await
        .unwrap(),
        RuntimeMaintenanceOutcome::Removed
    );
    fixture.guard.disarm();
}
