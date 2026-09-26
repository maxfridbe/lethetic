use super::attestation::{
    expected_create_command, expected_supervisor_args, expected_transient_create_command,
    validate_attestation_workspace, validate_inspected_selinux_labels,
    validate_new_retained_inspection, validate_retained_inspection, validate_transient_inspection,
};
use super::cleanup_queue::{
    MAX_TRANSIENT_CLEANUP_RECORD_BYTES, MAX_TRANSIENT_CLEANUP_RECORDS_PER_RETRY,
    TRANSIENT_CLEANUP_DIRECTORY, persist_transient_cleanup_record_at,
    retry_pending_transient_cleanups_at,
};
use super::command::{podman_failure, run_podman_output, terminate_podman_child_for_test};
use super::model::{
    ContainerInspection, ContainerState, InspectedContainerConfig, InspectedHostConfig,
    InspectedIdMappings, InspectedLogConfig, InspectedMount, InspectedNetwork,
    InspectedNetworkSettings, InspectedRestartPolicy, REQUIRED_CAPABILITIES,
};
use super::spec::{
    normalize_image_id, validate_image_id, validate_image_reference, validate_uuid,
    validate_workspace_destination,
};
use super::workspace::{classify_external_workspace_against, validate_external_workspace_against};
use super::*;
use crate::config::{AccessMode, NetworkAccess};
use crate::python::backend::{require_rootless_podman, resolve_real_podman};
use crate::python::runtime_store::{
    LEGACY_RUNTIME_ABI_V2, LEGACY_RUNTIME_ABI_V3, RuntimeLayoutProfile, RuntimeWorkspaceOwnership,
};
use crate::python::supervisor::RUNTIME_ABI;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

mod mutation_regressions;

fn test_config(root: &Path) -> RetainedPodmanConfig {
    let workspace = root.join("workspace");
    let broker = root.join("broker");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::create_dir(&broker).unwrap();
    std::fs::set_permissions(&workspace, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::set_permissions(&broker, std::fs::Permissions::from_mode(0o700)).unwrap();
    let runtime_id = "01234567-89ab-cdef-0123-456789abcdef".to_string();
    RetainedPodmanConfig {
        podman: PathBuf::from("/bin/true"),
        image_id: format!("sha256:{}", "a".repeat(64)),
        container_name: format!("lethetic-python-{runtime_id}"),
        runtime_id,
        session_id: "11111111-2222-3333-4444-555555555555".to_string(),
        security_fingerprint: "b".repeat(64),
        runtime_abi: RUNTIME_ABI.to_string(),
        layout_profile: RuntimeLayoutProfile::ShortSiblingV1,
        workspace,
        workspace_destination: PathBuf::from(CONTAINER_WORKSPACE),
        workspace_ownership: RuntimeWorkspaceOwnership::Managed,
        mask_lethetic: false,
        broker_directory: broker,
        worker_uid: rustix::process::geteuid().as_raw(),
        worker_gid: rustix::process::getegid().as_raw(),
    }
}

fn shared_test_config(root: &Path) -> RetainedPodmanConfig {
    let mut config = test_config(root);
    std::fs::create_dir(config.workspace.join(".lethetic")).unwrap();
    std::fs::set_permissions(
        config.workspace.join(".lethetic"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    std::fs::set_permissions(&config.workspace, std::fs::Permissions::from_mode(0o755)).unwrap();
    config.layout_profile = RuntimeLayoutProfile::ShortSiblingSharedCwdV1;
    config.workspace_destination = config.workspace.clone();
    config.workspace_ownership = RuntimeWorkspaceOwnership::ExternalLaunchCwd;
    config.mask_lethetic = true;
    config
}

fn transient_test_config(
    root: &Path,
    network: NetworkAccess,
    mask_lethetic: bool,
) -> TransientPodmanConfig {
    let workspace = root.join("transient-workspace");
    std::fs::create_dir(&workspace).unwrap();
    if mask_lethetic {
        std::fs::create_dir(workspace.join(".lethetic")).unwrap();
    }
    let worker_uid = rustix::process::geteuid().as_raw();
    let worker_gid = rustix::process::getegid().as_raw();
    TransientPodmanConfig {
        podman: PathBuf::from("/bin/true"),
        image_id: format!("sha256:{}", "e".repeat(64)),
        container_name: "lethetic-python-transient-123-1".to_string(),
        security_fingerprint: "f".repeat(64),
        workspace: workspace.clone(),
        launch_cwd: workspace.clone(),
        mounts: vec![TransientPodmanMount {
            path: workspace,
            access: AccessMode::ReadWrite,
            is_directory: true,
        }],
        network,
        mask_lethetic,
        worker_uid,
        worker_gid,
    }
}

fn write_absent_cleanup_podman(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    std::fs::write(
        &path,
        "#!/bin/sh\nif [ \"$1\" = container ] && [ \"$2\" = exists ]; then exit 1; fi\nexit 2\n",
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    path
}

fn matching_transient_inspection(
    config: &TransientPodmanConfig,
    container_id: &str,
) -> ContainerInspection {
    let annotations = BTreeMap::from([
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
    let uid = config.worker_uid;
    let gid = config.worker_gid;
    let mut tmpfs = BTreeMap::from([
        (
            "/tmp".to_string(),
            "rw,nosuid,nodev,noexec,mode=1777,rprivate,tmpcopyup".to_string(),
        ),
        (
            "/home/lethetic".to_string(),
            "rw,nosuid,nodev,mode=1777,rprivate,tmpcopyup".to_string(),
        ),
    ]);
    if config.mask_lethetic {
        tmpfs.insert(
            config.workspace.join(".lethetic").display().to_string(),
            "ro,nosuid,nodev,noexec,mode=000,size=1048576,rprivate".to_string(),
        );
    }
    let binds = config
        .mounts
        .iter()
        .map(|mount| {
            let access = if mount.access == AccessMode::ReadWrite {
                "rw"
            } else {
                "ro"
            };
            format!(
                "{}:{}:{access},rprivate,rbind",
                mount.path.display(),
                mount.path.display()
            )
        })
        .collect();
    let mounts = config
        .mounts
        .iter()
        .map(|mount| InspectedMount {
            mount_type: "bind".to_string(),
            source: mount.path.display().to_string(),
            destination: mount.path.display().to_string(),
            mode: String::new(),
            options: vec!["rbind".to_string()],
            read_write: mount.access == AccessMode::ReadWrite,
            propagation: "rprivate".to_string(),
        })
        .collect();
    let network_name = match config.network {
        NetworkAccess::None => "none",
        NetworkAccess::Full => "host",
        NetworkAccess::Nonlocal => unreachable!(),
    };
    let worker_args = vec![
        "-u".to_string(),
        "-B".to_string(),
        "-c".to_string(),
        crate::python::WORKER_SOURCE.to_string(),
    ];
    ContainerInspection {
        id: container_id.to_string(),
        name: config.container_name.clone(),
        path: "python3".to_string(),
        args: worker_args.clone(),
        image: config.image_id.trim_start_matches("sha256:").to_string(),
        effective_caps: Vec::new(),
        config: InspectedContainerConfig {
            annotations: annotations.clone(),
            create_command: expected_transient_create_command(config).unwrap(),
            entrypoint: vec!["python3".to_string()],
            cmd: worker_args,
            image: config.image_id.clone(),
            labels: config.expected_labels(),
            env: vec![
                "HOME=/home/lethetic".to_string(),
                "LANG=C.UTF-8".to_string(),
                "PYTHONNOUSERSITE=1".to_string(),
                "PYTHONDONTWRITEBYTECODE=1".to_string(),
            ],
            open_stdin: true,
            stdin_once: false,
            tty: false,
            stop_signal: "SIGTERM".to_string(),
            stop_timeout: 10,
            user: format!("{uid}:{gid}"),
            working_dir: config.launch_cwd.display().to_string(),
        },
        host_config: InspectedHostConfig {
            annotations,
            auto_remove: true,
            binds,
            cgroup_mode: "private".to_string(),
            devices: Vec::new(),
            group_add: Vec::new(),
            id_mappings: InspectedIdMappings {
                uid_map: vec![
                    format!("0:1:{uid}"),
                    format!("{uid}:0:1"),
                    format!("{}:{}:64536", uid + 1, uid + 1),
                ],
                gid_map: vec![
                    format!("0:1:{gid}"),
                    format!("{gid}:0:1"),
                    format!("{}:{}:64536", gid + 1, gid + 1),
                ],
            },
            ipc_mode: "private".to_string(),
            log_config: InspectedLogConfig {
                log_type: "none".to_string(),
            },
            network_mode: network_name.to_string(),
            pid_mode: "private".to_string(),
            pids_limit: 256,
            port_bindings: BTreeMap::new(),
            privileged: false,
            publish_all_ports: false,
            readonly_rootfs: true,
            restart_policy: InspectedRestartPolicy {
                name: "no".to_string(),
                maximum_retry_count: 0,
            },
            security_opt: vec!["no-new-privileges".to_string(), "label=disable".to_string()],
            tmpfs,
            uts_mode: "private".to_string(),
            userns_mode: "private".to_string(),
        },
        mounts,
        network_settings: InspectedNetworkSettings {
            networks: BTreeMap::from([(
                network_name.to_string(),
                InspectedNetwork {
                    network_id: network_name.to_string(),
                    gateway: String::new(),
                    ip_address: String::new(),
                    ipv6_gateway: String::new(),
                    global_ipv6_address: String::new(),
                },
            )]),
        },
        state: ContainerState {
            status: "configured".to_string(),
            running: false,
            paused: false,
            restarting: false,
            oom_killed: false,
            dead: false,
            pid: 0,
        },
        process_label: String::new(),
        mount_label: String::new(),
    }
}

fn text_args(spec: &PodmanCommandSpec) -> Vec<String> {
    spec.args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

struct TestContainerGuard {
    podman: PathBuf,
    container_id: String,
    armed: bool,
}

impl TestContainerGuard {
    fn new(podman: PathBuf, container_id: String) -> Self {
        validate_container_id(&container_id).unwrap();
        Self {
            podman,
            container_id,
            armed: true,
        }
    }

    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for TestContainerGuard {
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
                "real Podman test cleanup could not verify exact container {} absence (remove={removal:?}, presence={presence:?})",
                self.container_id
            );
        }
    }
}

fn matching_inspection(config: &RetainedPodmanConfig, container_id: &str) -> ContainerInspection {
    let label_annotation = match config.workspace_ownership {
        RuntimeWorkspaceOwnership::Managed => "type:container_t,label=filetype:container_file_t",
        RuntimeWorkspaceOwnership::ExternalLaunchCwd => "disable",
    };
    let annotations = BTreeMap::from([
        (
            "io.podman.annotations.label".to_string(),
            label_annotation.to_string(),
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
    let mut labels = config.expected_labels();
    labels.insert(
        RUNTIME_IMAGE_LABEL.to_string(),
        RUNTIME_IMAGE_LABEL_VALUE.to_string(),
    );
    let (process_label, mount_label) = match config.workspace_ownership {
        RuntimeWorkspaceOwnership::Managed => (
            "system_u:system_r:container_t:s0:c42,c100".to_string(),
            "system_u:object_r:container_file_t:s0:c42,c100".to_string(),
        ),
        RuntimeWorkspaceOwnership::ExternalLaunchCwd => (String::new(), String::new()),
    };
    let uid = config.worker_uid;
    let gid = config.worker_gid;
    let destination = config.workspace_destination.to_string_lossy().into_owned();
    let relabel = "";
    let label_security = if config.workspace_ownership == RuntimeWorkspaceOwnership::Managed {
        "label=type:container_t,label=filetype:container_file_t"
    } else {
        "label=disable"
    };
    let mut tmpfs = BTreeMap::from([
        (
            "/run".to_string(),
            "rw,nosuid,nodev,noexec,mode=0755,rprivate,tmpcopyup".to_string(),
        ),
        (
            "/tmp".to_string(),
            "rw,nosuid,nodev,noexec,mode=1777,rprivate,tmpcopyup".to_string(),
        ),
    ]);
    if config.mask_lethetic {
        tmpfs.insert(
            format!("{destination}/.lethetic"),
            "ro,nosuid,nodev,noexec,mode=000,size=1048576,rprivate".to_string(),
        );
    }
    ContainerInspection {
        id: container_id.to_string(),
        name: config.container_name.clone(),
        path: CONTAINER_ENTRYPOINT.to_string(),
        args: expected_supervisor_args(config),
        image: config.image_id.trim_start_matches("sha256:").to_string(),
        effective_caps: REQUIRED_CAPABILITIES
            .iter()
            .map(|capability| format!("CAP_{capability}"))
            .collect(),
        config: InspectedContainerConfig {
            annotations: annotations.clone(),
            create_command: expected_create_command(config).unwrap(),
            entrypoint: vec![CONTAINER_ENTRYPOINT.to_string()],
            cmd: expected_supervisor_args(config),
            image: config.image_id.clone(),
            labels,
            env: Vec::new(),
            open_stdin: true,
            stdin_once: false,
            tty: false,
            stop_signal: "SIGTERM".to_string(),
            stop_timeout: 15,
            user: "0:0".to_string(),
            working_dir: destination.clone(),
        },
        host_config: InspectedHostConfig {
            annotations,
            auto_remove: false,
            binds: vec![
                format!(
                    "{}:{destination}:rw{relabel},rprivate,rbind",
                    config.workspace.display()
                ),
                format!(
                    "{}:{CONTAINER_HOST_BRIDGE}:ro{relabel},rprivate,rbind",
                    config.broker_directory.display()
                ),
            ],
            cgroup_mode: "private".to_string(),
            devices: Vec::new(),
            group_add: Vec::new(),
            id_mappings: InspectedIdMappings {
                uid_map: vec![
                    format!("0:1:{uid}"),
                    format!("{uid}:0:1"),
                    format!("{}:{}:64536", uid + 1, uid + 1),
                ],
                gid_map: vec![
                    format!("0:1:{gid}"),
                    format!("{gid}:0:1"),
                    format!("{}:{}:64536", gid + 1, gid + 1),
                ],
            },
            ipc_mode: "private".to_string(),
            log_config: InspectedLogConfig {
                log_type: "none".to_string(),
            },
            network_mode: "none".to_string(),
            pid_mode: "private".to_string(),
            pids_limit: 512,
            port_bindings: BTreeMap::new(),
            privileged: false,
            publish_all_ports: false,
            readonly_rootfs: false,
            restart_policy: InspectedRestartPolicy {
                name: "no".to_string(),
                maximum_retry_count: 0,
            },
            security_opt: vec!["no-new-privileges".to_string(), label_security.to_string()],
            tmpfs,
            uts_mode: "private".to_string(),
            userns_mode: "private".to_string(),
        },
        mounts: vec![
            InspectedMount {
                mount_type: "bind".to_string(),
                source: config.workspace.display().to_string(),
                destination: destination.clone(),
                mode: relabel.trim_start_matches(',').to_string(),
                options: vec!["rbind".to_string()],
                read_write: true,
                propagation: "rprivate".to_string(),
            },
            InspectedMount {
                mount_type: "bind".to_string(),
                source: config.broker_directory.display().to_string(),
                destination: CONTAINER_HOST_BRIDGE.to_string(),
                mode: relabel.trim_start_matches(',').to_string(),
                options: vec!["rbind".to_string()],
                read_write: false,
                propagation: "rprivate".to_string(),
            },
        ],
        network_settings: InspectedNetworkSettings {
            networks: BTreeMap::from([(
                "none".to_string(),
                InspectedNetwork {
                    network_id: "none".to_string(),
                    gateway: String::new(),
                    ip_address: String::new(),
                    ipv6_gateway: String::new(),
                    global_ipv6_address: String::new(),
                },
            )]),
        },
        state: ContainerState {
            status: "configured".to_string(),
            running: false,
            paused: false,
            restarting: false,
            oom_killed: false,
            dead: false,
            pid: 0,
        },
        process_label,
        mount_label,
    }
}

#[test]
fn transient_create_and_attestation_bind_exact_security_profile() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let config = transient_test_config(&root, NetworkAccess::None, true);
    let args = text_args(&build_transient_create_spec(&config).unwrap());
    for required in [
        "create",
        "--interactive",
        "--rm",
        "--pull=never",
        "--read-only",
        "--cap-drop=ALL",
        "--security-opt=no-new-privileges",
        "--security-opt=label=disable",
        "--userns=keep-id",
        "--network=none",
        "--entrypoint=python3",
    ] {
        assert!(args.iter().any(|argument| argument == required));
    }
    assert!(args.iter().any(|argument| argument == &config.image_id));
    assert!(!args.iter().any(|argument| argument == "run"));
    assert!(!args.iter().any(|argument| argument == "pull"));
    assert!(args.iter().any(|argument| {
        argument.starts_with(&format!(
            "--tmpfs={}/.lethetic:",
            config.workspace.display()
        )) && argument.contains("notmpcopyup")
    }));

    let container_id = "9".repeat(64);
    let inspection = matching_transient_inspection(&config, &container_id);
    validate_transient_inspection(&config, &container_id, &inspection).unwrap();

    let mut generated_mount_label = matching_transient_inspection(&config, &container_id);
    generated_mount_label.mount_label =
        "system_u:object_r:container_file_t:s0:c42,c100".to_string();
    validate_transient_inspection(&config, &container_id, &generated_mount_label).unwrap();
    generated_mount_label.process_label = "system_u:system_r:container_t:s0:c42,c100".to_string();
    assert!(validate_transient_inspection(&config, &container_id, &generated_mount_label).is_err());

    let mut changed = matching_transient_inspection(&config, &container_id);
    changed.host_config.network_mode = "host".to_string();
    assert!(validate_transient_inspection(&config, &container_id, &changed).is_err());

    let mut changed = matching_transient_inspection(&config, &container_id);
    changed
        .config
        .create_command
        .push("--privileged".to_string());
    assert!(validate_transient_inspection(&config, &container_id, &changed).is_err());

    let mut changed = matching_transient_inspection(&config, &container_id);
    changed.mounts[0].source = root.join("replacement").display().to_string();
    assert!(validate_transient_inspection(&config, &container_id, &changed).is_err());
}

#[test]
fn transient_full_network_is_exact_host_networking() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let config = transient_test_config(&root, NetworkAccess::Full, false);
    let args = text_args(&build_transient_create_spec(&config).unwrap());
    assert!(args.iter().any(|argument| argument == "--network=host"));
    let container_id = "8".repeat(64);
    let inspection = matching_transient_inspection(&config, &container_id);
    validate_transient_inspection(&config, &container_id, &inspection).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn transient_worker_orders_create_inspect_start_and_exact_cleanup() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let fake = root.join("podman");
    let state = root.join("state");
    let inspection_path = root.join("inspection.json");
    let log = root.join("commands.jsonl");
    let container_id = "7".repeat(64);
    let mut config = transient_test_config(&root, NetworkAccess::None, true);
    config.podman = fake.clone();
    std::fs::write(&fake, "#!/bin/sh\nexit 1\n").unwrap();
    let mut permissions = std::fs::metadata(&fake).unwrap().permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&fake, permissions).unwrap();
    let inspection = matching_transient_inspection(&config, &container_id);
    std::fs::write(
        &inspection_path,
        serde_json::to_vec(&vec![inspection]).unwrap(),
    )
    .unwrap();
    let script = format!(
        r#"#!/usr/bin/python3
import json, pathlib, sys
root = pathlib.Path(__file__).resolve().parent
state = root / "state"
retry_cleanup = root / "retry-cleanup"
remove_attempts = root / "remove-attempts"
log = root / "commands.jsonl"
inspection = root / "inspection.json"
container_id = {container_id:?}
version = {version}
with log.open("a", encoding="utf-8") as output:
    output.write(json.dumps(sys.argv[1:]) + "\n")
args = sys.argv[1:]
def frame(value):
    body = json.dumps(value, separators=(",", ":")).encode()
    sys.stdout.buffer.write(f"LETHETIC_PYTHON {{version}} {{len(body)}}\n".encode() + body)
    sys.stdout.buffer.flush()
if args and args[0] == "create":
    state.write_text("present", encoding="utf-8")
    print(container_id)
elif args[:2] == ["container", "inspect"]:
    sys.stdout.buffer.write(inspection.read_bytes())
elif args[:2] == ["container", "exists"]:
    raise SystemExit(0 if state.exists() else 1)
elif args and args[0] == "start":
    frame({{"type":"hello", "protocol":version,
           "worker_abi":"lethetic-python-worker-v4",
           "capabilities":["lethetic-output-v2"],
           "python":"test", "cwd":{cwd:?}}})
    request = json.loads(sys.stdin.buffer.readline())
    frame({{"type":"result", "id":request["id"], "ok":True, "cell":1,
           "stdout":"", "stderr":"", "repr":"42", "traceback":"",
           "cwd":{cwd:?}, "output_metadata":{{
               "cell":1, "artifact_id":request["artifact_id"], "retained":True,
               "sections":[
                   {{"section":"stdout", "captured_bytes":0, "original_bytes":0, "excerpt_bytes":0, "truncated":False}},
                   {{"section":"stderr", "captured_bytes":0, "original_bytes":0, "excerpt_bytes":0, "truncated":False}},
                   {{"section":"repr", "captured_bytes":2, "original_bytes":2, "excerpt_bytes":2, "truncated":False}},
                   {{"section":"traceback", "captured_bytes":0, "original_bytes":0, "excerpt_bytes":0, "truncated":False}}
               ]
           }}}})
    while sys.stdin.buffer.read(4096):
        pass
elif args and args[0] == "rm":
    if retry_cleanup.exists():
        attempts = int(remove_attempts.read_text(encoding="utf-8")) + 1 if remove_attempts.exists() else 1
        remove_attempts.write_text(str(attempts), encoding="utf-8")
        if attempts < 3:
            raise SystemExit(7)
    state.unlink(missing_ok=True)
else:
    raise SystemExit(2)
"#,
        container_id = container_id,
        version = crate::python::PROTOCOL_VERSION,
        cwd = config.launch_cwd.display().to_string(),
    );
    std::fs::write(&fake, script).unwrap();
    let mut permissions = std::fs::metadata(&fake).unwrap().permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&fake, permissions).unwrap();

    let create = build_transient_create_spec(&config).unwrap();
    let launch = crate::python::LaunchSpec {
        kind: crate::python::LaunchKind::TransientPodman(config.clone()),
        container_identity: Some(
            crate::python::PythonContainerIdentity::transient(&config.container_name).unwrap(),
        ),
        program: create.program.into_os_string(),
        args: create.args,
        cwd: None,
        clear_env: false,
        env: Vec::new(),
        cleanup: None,
        startup_notice: None,
        startup_timeout: Duration::from_secs(5),
        graceful_shutdown: None,
    };
    let runspace = crate::python::PythonRunspace::new();
    let notice = runspace
        .ensure_ready(
            launch.clone(),
            "fake-transient".to_string(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap()
        .expect("attested worker hello must emit its runtime notice");
    assert_eq!(notice.container_id, container_id);
    assert_eq!(notice.action, crate::python::RuntimeLaunchAction::Created);
    assert_eq!(
        runspace.operational_identity(),
        Some(crate::python::PythonContainerIdentity {
            kind: crate::python::PythonContainerKind::Transient,
            name: config.container_name.clone(),
            active: true,
        })
    );
    assert!(state.exists());
    assert!(
        runspace
            .ensure_ready(
                launch.clone(),
                "fake-transient".to_string(),
                tokio_util::sync::CancellationToken::new(),
            )
            .await
            .unwrap()
            .is_none(),
        "one worker must announce its runtime only once"
    );
    let result = runspace
        .execute(
            launch.clone(),
            "fake-transient".to_string(),
            "6 * 7",
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(result.value_repr, "42");
    assert_eq!(result.runtime_notice.unwrap().container_id, container_id);
    runspace.reset_checked().await.unwrap();
    assert_eq!(runspace.operational_identity(), None);
    assert!(!state.exists());

    std::fs::write(root.join("retry-cleanup"), "retry").unwrap();
    let terminating_runspace = crate::python::PythonRunspace::new();
    terminating_runspace
        .ensure_ready(
            launch.clone(),
            "fake-transient-terminate".to_string(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap()
        .expect("terminate retry worker must announce after hello");
    let error = terminating_runspace.reset_checked().await.unwrap_err();
    assert!(error.contains("cleanup"), "{error}");
    assert_eq!(terminating_runspace.operational_identity(), None);
    assert!(
        !state.exists(),
        "failed async cleanup must remain owned for Worker::drop retries"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("remove-attempts")).unwrap(),
        "3"
    );
    std::fs::remove_file(root.join("remove-attempts")).unwrap();

    let dropped_runspace = crate::python::PythonRunspace::new();
    dropped_runspace
        .ensure_ready(
            launch,
            "fake-transient-drop".to_string(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap()
        .expect("drop test worker must announce after hello");
    assert!(state.exists());
    drop(dropped_runspace);
    assert!(
        !state.exists(),
        "Worker::drop must synchronously verify exact transient cleanup"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("remove-attempts")).unwrap(),
        "3",
        "Worker::drop must retain and retry the exact cleanup action"
    );

    let commands = std::fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Vec<String>>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(commands[0][0], "create");
    assert_eq!(&commands[1][..2], ["container", "inspect"]);
    assert_eq!(commands[1].last(), Some(&container_id));
    assert_eq!(commands[2][0], "start");
    assert_eq!(commands[2].last(), Some(&container_id));
    let remove = commands
        .iter()
        .find(|command| command.first().map(String::as_str) == Some("rm"))
        .unwrap();
    assert_eq!(
        remove.iter().map(String::as_str).collect::<Vec<_>>(),
        vec!["rm", "--force", "--time=10", "--", container_id.as_str()]
    );
    assert_eq!(
        commands
            .iter()
            .filter(|command| command.first().map(String::as_str) == Some("rm"))
            .count(),
        7,
        "reset plus retained-action and direct Drop retries must all use exact removal"
    );
    assert!(commands.iter().all(|command| {
        command.first().map(String::as_str) != Some("rm") || command.last() == Some(&container_id)
    }));
}

#[cfg(unix)]
#[tokio::test]
async fn pending_cleanup_queue_is_bounded_per_pass_without_wedging() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let state_root = root.join("state");
    let fake = write_absent_cleanup_podman(&root, "podman");
    let mut config = transient_test_config(&root, NetworkAccess::None, true);
    config.podman = fake.clone();

    for index in 0..=MAX_TRANSIENT_CLEANUP_RECORDS_PER_RETRY {
        let container_id = format!("{:064x}", index + 1);
        persist_transient_cleanup_record_at(&state_root, &config, Some(&container_id)).unwrap();
    }

    let first = retry_pending_transient_cleanups_at(&state_root, &fake)
        .await
        .unwrap_err();
    assert!(
        first.contains("deferred to the next bounded retry pass"),
        "{first}"
    );
    assert_eq!(
        std::fs::read_dir(state_root.join(TRANSIENT_CLEANUP_DIRECTORY))
            .unwrap()
            .count(),
        1
    );

    retry_pending_transient_cleanups_at(&state_root, &fake)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_dir(state_root.join(TRANSIENT_CLEANUP_DIRECTORY))
            .unwrap()
            .count(),
        0
    );
}

#[cfg(unix)]
#[tokio::test]
async fn stale_cleanup_executable_does_not_block_another_podman_path() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let state_root = root.join("state");
    let stale = write_absent_cleanup_podman(&root, "podman-stale");
    let active = write_absent_cleanup_podman(&root, "podman-active");
    let mut config = transient_test_config(&root, NetworkAccess::None, true);
    config.podman = stale.clone();
    let container_id = "7".repeat(64);
    persist_transient_cleanup_record_at(&state_root, &config, Some(&container_id)).unwrap();
    std::fs::remove_file(&stale).unwrap();

    retry_pending_transient_cleanups_at(&state_root, &active)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_dir(state_root.join(TRANSIENT_CLEANUP_DIRECTORY))
            .unwrap()
            .count(),
        1
    );

    let restored = write_absent_cleanup_podman(&root, "podman-stale");
    assert_eq!(restored, stale);
    retry_pending_transient_cleanups_at(&state_root, &stale)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_dir(state_root.join(TRANSIENT_CLEANUP_DIRECTORY))
            .unwrap()
            .count(),
        0
    );
}

#[cfg(unix)]
#[tokio::test]
async fn cleanup_queue_rejects_malformed_symlinked_and_oversized_records() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let fake = write_absent_cleanup_podman(&root, "podman");
    let file_name = format!("{}.json", "8".repeat(64));

    let malformed_root = root.join("malformed-state");
    let malformed_directory = malformed_root.join(TRANSIENT_CLEANUP_DIRECTORY);
    std::fs::create_dir_all(&malformed_directory).unwrap();
    std::fs::write(malformed_directory.join(&file_name), b"{").unwrap();
    let malformed = retry_pending_transient_cleanups_at(&malformed_root, &fake)
        .await
        .unwrap_err();
    assert!(
        malformed.contains("invalid transient cleanup record"),
        "{malformed}"
    );

    let symlink_root = root.join("symlink-state");
    let symlink_directory = symlink_root.join(TRANSIENT_CLEANUP_DIRECTORY);
    std::fs::create_dir_all(&symlink_directory).unwrap();
    let outside = root.join("outside-record");
    std::fs::write(&outside, b"outside").unwrap();
    symlink(&outside, symlink_directory.join(&file_name)).unwrap();
    let linked = retry_pending_transient_cleanups_at(&symlink_root, &fake)
        .await
        .unwrap_err();
    assert!(
        linked.contains("could not read transient cleanup record"),
        "{linked}"
    );
    assert_eq!(std::fs::read(&outside).unwrap(), b"outside");

    let oversized_root = root.join("oversized-state");
    let oversized_directory = oversized_root.join(TRANSIENT_CLEANUP_DIRECTORY);
    std::fs::create_dir_all(&oversized_directory).unwrap();
    std::fs::write(
        oversized_directory.join(file_name),
        vec![b'x'; MAX_TRANSIENT_CLEANUP_RECORD_BYTES + 1],
    )
    .unwrap();
    let oversized = retry_pending_transient_cleanups_at(&oversized_root, &fake)
        .await
        .unwrap_err();
    assert!(oversized.contains("is oversized"), "{oversized}");
}

#[cfg(unix)]
#[tokio::test]
async fn ambiguous_transient_create_recovers_by_exact_name_and_cleanup_verifies_absence() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let fake = root.join("podman");
    let state = root.join("state");
    let keep_on_remove = root.join("keep-on-remove");
    let inspection_path = root.join("inspection.json");
    let log = root.join("commands.jsonl");
    let container_id = "6".repeat(64);
    let mut config = transient_test_config(&root, NetworkAccess::None, true);
    config.podman = fake.clone();
    std::fs::write(&fake, "#!/bin/sh\nexit 1\n").unwrap();
    let mut permissions = std::fs::metadata(&fake).unwrap().permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&fake, permissions).unwrap();
    let inspection = matching_transient_inspection(&config, &container_id);
    std::fs::write(
        &inspection_path,
        serde_json::to_vec(&vec![inspection]).unwrap(),
    )
    .unwrap();
    let script = format!(
        r#"#!/usr/bin/python3
import json, pathlib, sys
root = pathlib.Path(__file__).resolve().parent
state = root / "state"
keep_on_remove = root / "keep-on-remove"
log = root / "commands.jsonl"
inspection = root / "inspection.json"
container_id = {container_id:?}
with log.open("a", encoding="utf-8") as output:
    output.write(json.dumps(sys.argv[1:]) + "\n")
args = sys.argv[1:]
if args and args[0] == "create":
    state.write_text("present", encoding="utf-8")
    print("ambiguous-output")
elif args[:2] == ["container", "exists"]:
    raise SystemExit(0 if state.exists() else 1)
elif args[:2] == ["container", "inspect"]:
    sys.stdout.buffer.write(inspection.read_bytes())
elif args and args[0] == "rm":
    if not keep_on_remove.exists():
        state.unlink(missing_ok=True)
    raise SystemExit(7)
else:
    raise SystemExit(2)
"#,
        container_id = container_id,
    );
    std::fs::write(&fake, script).unwrap();
    let mut permissions = std::fs::metadata(&fake).unwrap().permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&fake, permissions).unwrap();

    let recovered = create_transient_container(&config).await.unwrap();
    assert_eq!(recovered, container_id);
    assert!(state.exists());
    cleanup_transient_container(&fake, &container_id)
        .await
        .expect("nonzero removal is successful only after exact absence is verified");
    assert!(!state.exists());

    std::fs::write(&state, "present").unwrap();
    std::fs::write(&keep_on_remove, "keep").unwrap();
    let error = cleanup_transient_container(&fake, &container_id)
        .await
        .unwrap_err();
    assert!(error.contains("cleanup"));
    assert!(state.exists());
    let cleanup_record = persist_transient_cleanup_record(&config, Some(&container_id)).unwrap();
    let retry_error = retry_pending_transient_cleanups(&fake).await.unwrap_err();
    assert!(retry_error.contains("remains unresolved"), "{retry_error}");
    assert!(state.exists());
    std::fs::remove_file(&keep_on_remove).unwrap();
    retry_pending_transient_cleanups(&fake).await.unwrap();
    assert!(!state.exists());
    assert!(
        crate::platform::read_file_nofollow(
            &crate::platform::lethetic_state_dir(),
            &[TRANSIENT_CLEANUP_DIRECTORY],
            &cleanup_record,
        )
        .unwrap()
        .is_none()
    );

    let commands = std::fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Vec<String>>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(commands[0][0], "create");
    assert_eq!(&commands[1][..2], ["container", "exists"]);
    assert_eq!(commands[1].last(), Some(&config.container_name));
    assert_eq!(&commands[2][..2], ["container", "inspect"]);
    assert_eq!(commands[2].last(), Some(&config.container_name));
    assert_eq!(&commands[3][..2], ["container", "inspect"]);
    assert_eq!(commands[3].last(), Some(&container_id));
    assert!(commands.iter().all(|command| {
        command.first().map(String::as_str) != Some("rm") || command.last() == Some(&container_id)
    }));
}

#[cfg(unix)]
#[tokio::test]
async fn transient_startup_cancellation_and_bad_hello_remove_the_exact_container() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let fake = root.join("podman");
    let state = root.join("state");
    let mode = root.join("mode");
    let inspection_path = root.join("inspection.json");
    let log = root.join("commands.jsonl");
    let container_id = "5".repeat(64);
    let mut config = transient_test_config(&root, NetworkAccess::None, true);
    config.podman = fake.clone();
    std::fs::write(&fake, "#!/bin/sh\nexit 1\n").unwrap();
    let mut permissions = std::fs::metadata(&fake).unwrap().permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&fake, permissions).unwrap();
    let inspection = matching_transient_inspection(&config, &container_id);
    std::fs::write(
        &inspection_path,
        serde_json::to_vec(&vec![inspection]).unwrap(),
    )
    .unwrap();
    let script = format!(
        r#"#!/usr/bin/python3
import json, pathlib, sys, time
root = pathlib.Path(__file__).resolve().parent
state = root / "state"
mode = (root / "mode").read_text(encoding="utf-8").strip()
log = root / "commands.jsonl"
inspection = root / "inspection.json"
container_id = {container_id:?}
header_version = {header_version}
with log.open("a", encoding="utf-8") as output:
    output.write(json.dumps(sys.argv[1:]) + "\n")
args = sys.argv[1:]
def frame(value):
    body = json.dumps(value, separators=(",", ":")).encode()
    sys.stdout.buffer.write(f"LETHETIC_PYTHON {{header_version}} {{len(body)}}\n".encode() + body)
    sys.stdout.buffer.flush()
if args and args[0] == "create":
    if mode == "commit-then-hang":
        state.write_text("present", encoding="utf-8")
        time.sleep(30)
    if mode == "delay-create":
        time.sleep(0.2)
    state.write_text("present", encoding="utf-8")
    print(container_id)
elif args[:2] == ["container", "inspect"]:
    sys.stdout.buffer.write(inspection.read_bytes())
elif args[:2] == ["container", "exists"]:
    raise SystemExit(0 if state.exists() else 1)
elif args and args[0] == "start":
    frame({{"type":"hello", "protocol":999,
           "worker_abi":"lethetic-python-worker-v4",
           "capabilities":["lethetic-output-v2"],
           "python":"test", "cwd":{cwd:?}}})
elif args and args[0] == "rm":
    state.unlink(missing_ok=True)
else:
    raise SystemExit(2)
"#,
        container_id = container_id,
        header_version = crate::python::PROTOCOL_VERSION,
        cwd = config.launch_cwd.display().to_string(),
    );
    std::fs::write(&fake, script).unwrap();
    let mut permissions = std::fs::metadata(&fake).unwrap().permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&fake, permissions).unwrap();
    let launch = || {
        let create = build_transient_create_spec(&config).unwrap();
        crate::python::LaunchSpec {
            kind: crate::python::LaunchKind::TransientPodman(config.clone()),
            container_identity: Some(
                crate::python::PythonContainerIdentity::transient(&config.container_name).unwrap(),
            ),
            program: create.program.into_os_string(),
            args: create.args,
            cwd: None,
            clear_env: false,
            env: Vec::new(),
            cleanup: None,
            startup_notice: None,
            startup_timeout: Duration::from_secs(5),
            graceful_shutdown: None,
        }
    };

    std::fs::write(&mode, "delay-create").unwrap();
    let cancellation = tokio_util::sync::CancellationToken::new();
    let cancel_later = cancellation.clone();
    let cancellation_task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        cancel_later.cancel();
    });
    let runspace = crate::python::PythonRunspace::new();
    let error = runspace
        .ensure_ready(launch(), "cancel-startup".to_string(), cancellation)
        .await
        .unwrap_err();
    cancellation_task.await.unwrap();
    assert!(error.contains("cancelled"), "{error}");
    assert!(!state.exists());
    let first_commands = std::fs::read_to_string(&log).unwrap();
    assert!(!first_commands.lines().any(|line| {
        serde_json::from_str::<Vec<String>>(line)
            .unwrap()
            .first()
            .map(String::as_str)
            == Some("start")
    }));

    std::fs::write(&mode, "commit-then-hang").unwrap();
    let runspace = std::sync::Arc::new(crate::python::PythonRunspace::new());
    let aborted_runspace = runspace.clone();
    let aborted_launch = launch();
    let aborted = tokio::spawn(async move {
        aborted_runspace
            .ensure_ready(
                aborted_launch,
                "aborted-create".to_string(),
                tokio_util::sync::CancellationToken::new(),
            )
            .await
    });
    for _ in 0..100 {
        if state.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        state.exists(),
        "fake create never reached its committed state"
    );
    aborted.abort();
    assert!(aborted.await.unwrap_err().is_cancelled());
    assert!(
        !state.exists(),
        "dropping a post-create launch future must recover, attest, and remove its exact ID"
    );
    drop(runspace);

    std::fs::write(&mode, "bad-hello").unwrap();
    let runspace = crate::python::PythonRunspace::new();
    let error = runspace
        .ensure_ready(
            launch(),
            "bad-hello".to_string(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(error.contains("protocol mismatch"), "{error}");
    assert!(!state.exists());

    let commands = std::fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Vec<String>>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        commands
            .iter()
            .filter(|command| command.first().map(String::as_str) == Some("rm"))
            .count(),
        3
    );
    assert!(commands.iter().all(|command| {
        command.first().map(String::as_str) != Some("rm") || command.last() == Some(&container_id)
    }));
}

#[test]
fn retained_create_is_cow_networkless_and_never_pulls() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let config = test_config(&root);
    let args = text_args(&build_create_spec(&config).unwrap());
    for required in [
        "create",
        "--interactive",
        "--pull=never",
        "--userns=keep-id",
        "--user=0:0",
        "--network=none",
        "--image-volume=ignore",
        "--log-driver=none",
        "--cap-drop=ALL",
        "--security-opt=label=type:container_t",
        "--security-opt=label=filetype:container_file_t",
        "--security-opt=no-new-privileges",
        "--entrypoint=/usr/local/libexec/lethetic/lethetic-runtime",
        "supervisor",
        RUNTIME_ABI,
    ] {
        assert!(args.iter().any(|arg| arg == required), "missing {required}");
    }
    assert!(!args.iter().any(|arg| arg == "--rm"));
    assert!(!args.iter().any(|arg| arg == "--read-only"));
    assert!(!args.iter().any(|arg| arg == "pull"));
    assert!(!args.iter().any(|arg| arg.contains("label=disable")));
    assert!(!args.iter().any(|arg| arg.contains("spc_t")));
    assert!(args.iter().any(|arg| arg.ends_with(":/workspace:rw,Z")));
    assert!(
        args.iter()
            .any(|arg| arg.ends_with(":/run/lethetic-host:ro,Z"))
    );
    assert_eq!(
        args.iter()
            .filter(|arg| arg.starts_with("--cap-add="))
            .count(),
        REQUIRED_CAPABILITIES.len()
    );
}

#[test]
fn retained_workspace_destination_cannot_overlap_trusted_paths() {
    for collision in [
        CONTAINER_ENTRYPOINT,
        "/usr/local/libexec/lethetic",
        "/usr/local/libexec",
        CONTAINER_HOST_BRIDGE,
        "/tmp",
        "/run",
        "/run/lethetic-host/subdirectory",
    ] {
        assert!(
            validate_workspace_destination(Path::new(collision)).is_err(),
            "accepted protected overlap {collision}"
        );
    }
    assert!(validate_workspace_destination(Path::new("/run/user/1000/project")).is_ok());
    assert!(validate_workspace_destination(Path::new("/tmp/project")).is_ok());
}

#[test]
fn rejected_late_retained_validation_does_not_create_control_state() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let mut config = test_config(&root);
    std::fs::set_permissions(&config.workspace, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::set_permissions(
        &config.broker_directory,
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    config.layout_profile = RuntimeLayoutProfile::ShortSiblingSharedCwdV1;
    config.workspace_destination = config.workspace.clone();
    config.workspace_ownership = RuntimeWorkspaceOwnership::ExternalLaunchCwd;
    config.mask_lethetic = true;
    assert!(build_create_spec(&config).is_err());
    assert!(!config.workspace.join(".lethetic").exists());
}

#[test]
fn external_workspace_classifies_before_committing_control_state() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let invalid = root.join("invalid");
    std::fs::create_dir(&invalid).unwrap();
    let nested = invalid.join("nested");
    std::fs::create_dir(&nested).unwrap();
    std::fs::create_dir(nested.join(".lethetic")).unwrap();
    let error = validate_external_workspace_against(&invalid, &[]).unwrap_err();
    assert!(
        error.contains("nested Lethetic control directory"),
        "{error}"
    );
    assert!(!invalid.join(".lethetic").exists());

    let workspace = root.join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    assert_eq!(
        validate_external_workspace_against(&workspace, &[]).unwrap(),
        workspace
    );
    let control = workspace.join(".lethetic");
    assert!(!control.exists());
    classify_external_workspace_against(&workspace, &[])
        .unwrap()
        .commit()
        .unwrap();
    let metadata = std::fs::symlink_metadata(control).unwrap();
    assert!(metadata.is_dir());
    assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
}

#[test]
fn external_workspace_rejects_control_root_overlap_in_both_directions() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let control = root.join("control");
    let descendant = control.join("project");
    std::fs::create_dir_all(&descendant).unwrap();

    let ancestor_error =
        validate_external_workspace_against(&root, std::slice::from_ref(&control)).unwrap_err();
    assert!(ancestor_error.contains("overlaps Lethetic control root"));

    let descendant_error =
        validate_external_workspace_against(&descendant, std::slice::from_ref(&control))
            .unwrap_err();
    assert!(descendant_error.contains("overlaps Lethetic control root"));

    let future_control = root.join("future/lethetic-sessions");
    let future_error = validate_external_workspace_against(&root, &[future_control]).unwrap_err();
    assert!(future_error.contains("overlaps Lethetic control root"));
}

#[test]
fn shared_cwd_create_uses_same_path_mask_and_no_recursive_relabel() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let config = shared_test_config(&root);
    let args = text_args(&build_create_spec(&config).unwrap());
    let workspace = config.workspace.to_string_lossy();
    assert!(args.iter().any(|arg| arg == "--security-opt=label=disable"));
    assert!(
        !args
            .iter()
            .any(|arg| arg.contains("filetype:container_file_t"))
    );
    assert!(
        args.iter()
            .any(|arg| { arg == &format!("--volume={workspace}:{workspace}:rw") })
    );
    assert!(
        args.iter()
            .any(|arg| { arg == &format!("--workdir={workspace}") })
    );
    assert!(args.iter().any(|arg| {
        arg.starts_with(&format!("--tmpfs={workspace}/.lethetic:ro,"))
            && arg.contains("notmpcopyup")
            && arg.contains("mode=000")
    }));
    assert!(
        args.iter().any(|arg| {
            arg == "--label=org.lethetic.runtime.layout=short-sibling-shared-cwd-v1"
        })
    );
    assert_eq!(expected_supervisor_args(&config)[5], workspace);
    let container_id = "d".repeat(64);
    let inspection = matching_inspection(&config, &container_id);
    assert!(validate_retained_inspection(&config, &container_id, &inspection, true).is_ok());
}

#[test]
fn frozen_layout_profiles_are_complete_and_mutually_exclusive() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let current = test_config(&root);
    let current_args = text_args(&build_create_spec(&current).unwrap());
    assert!(
        current_args
            .iter()
            .any(|argument| { argument == "--label=org.lethetic.runtime.layout=short-sibling-v1" })
    );
    assert!(
        current_args
            .iter()
            .any(|argument| argument == CONTAINER_BROKER_SOCKET)
    );

    let mut imported = current.clone();
    imported.layout_profile = RuntimeLayoutProfile::ShortSiblingV2Imported;
    let imported_args = text_args(&build_create_spec(&imported).unwrap());
    assert!(
        !imported_args
            .iter()
            .any(|argument| argument.contains(LAYOUT_LABEL))
    );
    assert!(
        imported_args
            .iter()
            .any(|argument| argument == CONTAINER_BROKER_SOCKET)
    );

    let mut legacy = current.clone();
    legacy.layout_profile = RuntimeLayoutProfile::LegacyRuntimeLocalV2;
    let legacy_args = text_args(&build_create_spec(&legacy).unwrap());
    assert!(
        !legacy_args
            .iter()
            .any(|argument| argument.contains(LAYOUT_LABEL))
    );
    assert!(
        legacy_args
            .iter()
            .any(|argument| argument == LEGACY_CONTAINER_BROKER_SOCKET)
    );

    let container_id = "c".repeat(64);
    let imported_inspection = matching_inspection(&imported, &container_id);
    assert!(
        validate_retained_inspection(&imported, &container_id, &imported_inspection, true,).is_ok()
    );
    assert!(
        validate_retained_inspection(&legacy, &container_id, &imported_inspection, true).is_err()
    );
    let legacy_inspection = matching_inspection(&legacy, &container_id);
    assert!(validate_retained_inspection(&legacy, &container_id, &legacy_inspection, true).is_ok());
    assert!(
        validate_retained_inspection(&imported, &container_id, &legacy_inspection, true).is_err()
    );

    std::fs::set_permissions(&imported.workspace, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::remove_dir(&imported.broker_directory).unwrap();
    assert!(imported.validate().is_err());
    imported.validate_static().unwrap();
    assert!(
        validate_retained_inspection(&imported, &container_id, &imported_inspection, true,).is_ok()
    );
}

#[test]
fn new_retained_attestation_requires_pristine_created_state() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let config = test_config(&root);
    let container_id = "a".repeat(64);
    let created = matching_inspection(&config, &container_id);
    validate_new_retained_inspection(&config, &container_id, &created, true).unwrap();

    for status in ["exited", "stopped"] {
        let mut stopped = matching_inspection(&config, &container_id);
        stopped.state.status = status.to_string();
        validate_retained_inspection(&config, &container_id, &stopped, true).unwrap();
        assert!(validate_new_retained_inspection(&config, &container_id, &stopped, true).is_err());
    }
    let mut running = matching_inspection(&config, &container_id);
    running.state.status = "running".to_string();
    running.state.running = true;
    running.state.pid = 42;
    validate_retained_inspection(&config, &container_id, &running, true).unwrap();
    assert!(validate_new_retained_inspection(&config, &container_id, &running, true).is_err());
}

#[test]
fn synthetic_previous_v3_attestation_is_maintenance_only() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let mut config = test_config(&root);
    config.runtime_abi = LEGACY_RUNTIME_ABI_V3.to_string();
    let container_id = "b".repeat(64);
    let inspection = matching_inspection(&config, &container_id);

    config.validate_static().unwrap();
    validate_retained_inspection(&config, &container_id, &inspection, true).unwrap();
    assert_eq!(inspection.args[1], LEGACY_RUNTIME_ABI_V3);
    assert!(build_create_spec(&config).is_err());
}

#[test]
fn synthetic_legacy_v2_attestation_binds_every_abi_axis() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let mut config = test_config(&root);
    config.runtime_abi = LEGACY_RUNTIME_ABI_V2.to_string();
    config.layout_profile = RuntimeLayoutProfile::LegacyRuntimeLocalV2;
    let container_id = "b".repeat(64);
    let inspection = matching_inspection(&config, &container_id);
    validate_retained_inspection(&config, &container_id, &inspection, true).unwrap();
    assert_eq!(inspection.args[1], LEGACY_RUNTIME_ABI_V2);
    assert!(build_create_spec(&config).is_err());

    let mut argv_drift = matching_inspection(&config, &container_id);
    argv_drift.args[1] = RUNTIME_ABI.to_string();
    assert!(validate_retained_inspection(&config, &container_id, &argv_drift, true).is_err());
    let mut command_drift = matching_inspection(&config, &container_id);
    let abi = command_drift
        .config
        .create_command
        .iter_mut()
        .find(|argument| argument.as_str() == LEGACY_RUNTIME_ABI_V2)
        .unwrap();
    *abi = RUNTIME_ABI.to_string();
    assert!(validate_retained_inspection(&config, &container_id, &command_drift, true).is_err());
    let mut label_drift = matching_inspection(&config, &container_id);
    label_drift
        .config
        .labels
        .insert(ABI_LABEL.to_string(), RUNTIME_ABI.to_string());
    assert!(validate_retained_inspection(&config, &container_id, &label_drift, true).is_err());

    config.runtime_abi = "lethetic-python-runtime-unknown".to_string();
    assert!(config.validate_static().is_err());
}

#[test]
fn exact_attestation_rejects_security_and_mount_drift() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let config = test_config(&root);
    let container_id = "c".repeat(64);
    let valid = matching_inspection(&config, &container_id);
    let identity = validate_retained_inspection(&config, &container_id, &valid, true).unwrap();
    assert_eq!(identity.container_id, container_id);
    assert!(identity.selinux_labels.is_some());

    let mut changed = matching_inspection(&config, &container_id);
    changed.host_config.network_mode = "host".to_string();
    assert!(validate_retained_inspection(&config, &container_id, &changed, true).is_err());

    let mut changed = matching_inspection(&config, &container_id);
    changed.effective_caps.push("CAP_NET_RAW".to_string());
    assert!(validate_retained_inspection(&config, &container_id, &changed, true).is_err());

    let mut changed = matching_inspection(&config, &container_id);
    changed.mounts[0].source = root.join("replacement").display().to_string();
    assert!(validate_retained_inspection(&config, &container_id, &changed, true).is_err());

    let mut changed = matching_inspection(&config, &container_id);
    changed
        .config
        .create_command
        .push("--privileged".to_string());
    assert!(validate_retained_inspection(&config, &container_id, &changed, true).is_err());

    let mut changed = matching_inspection(&config, &container_id);
    changed.config.labels.insert(
        RUNTIME_ID_LABEL.to_string(),
        "11111111-2222-4333-8444-555555555555".to_string(),
    );
    assert!(validate_retained_inspection(&config, &container_id, &changed, true).is_err());
}

#[test]
fn attestation_rejects_replaced_manifest_workspace() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let config = test_config(&root);
    let identity =
        crate::python::runtime_store::WorkspaceIdentity::capture(&config.workspace).unwrap();
    std::fs::rename(&config.workspace, root.join("original-workspace")).unwrap();
    std::fs::create_dir(&config.workspace).unwrap();
    std::fs::set_permissions(&config.workspace, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(validate_attestation_workspace(&config, &identity).is_err());
}

#[test]
fn retained_create_binds_exact_labels_and_ids() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let config = test_config(&root);
    let args = text_args(&build_create_spec(&config).unwrap());
    for (key, value) in config.expected_labels() {
        assert!(
            args.iter()
                .any(|arg| arg == &format!("--label={key}={value}"))
        );
    }
    assert!(args.iter().any(|arg| arg == &config.image_id));
    assert!(args.iter().any(|arg| arg == &config.worker_uid.to_string()));
    assert!(args.iter().any(|arg| arg == &config.worker_gid.to_string()));
}

#[test]
fn create_rejects_noncanonical_or_nonprivate_mounts() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let mut config = test_config(&root);
    std::fs::set_permissions(&config.workspace, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(build_create_spec(&config).is_err());

    std::fs::set_permissions(&config.workspace, std::fs::Permissions::from_mode(0o700)).unwrap();
    config.broker_directory = config.workspace.clone();
    assert!(build_create_spec(&config).is_err());
}

mod cancellation;

#[test]
fn exact_container_ids_are_required_for_lifecycle_commands() {
    assert!(validate_container_id(&"a".repeat(64)).is_ok());
    for invalid in ["abc", &"A".repeat(64), &format!("{}z", "a".repeat(63))] {
        assert!(validate_container_id(invalid).is_err());
    }
    let id = "c".repeat(64);
    let start = text_args(&build_start_attach_spec(Path::new("/bin/true"), &id).unwrap());
    assert_eq!(start, ["start", "--attach", "--interactive", "--", &id]);
    let stop = text_args(&build_stop_spec(Path::new("/bin/true"), &id).unwrap());
    assert_eq!(stop, ["stop", "--time=15", "--", &id]);
    let remove = text_args(&build_remove_spec(Path::new("/bin/true"), &id).unwrap());
    assert_eq!(remove, ["rm", "--", &id]);
    let force = text_args(&build_force_remove_spec(Path::new("/bin/true"), &id).unwrap());
    assert_eq!(force, ["rm", "--force", "--time=10", "--", &id]);
}

#[test]
fn selinux_status_and_container_labels_must_be_consistent() {
    assert!(
        validate_inspected_selinux_labels("", "", false, true)
            .unwrap()
            .is_none()
    );
    assert!(validate_inspected_selinux_labels("", "", true, true).is_err());
    let process = "system_u:system_r:container_t:s0:c42,c100";
    let mount = "system_u:object_r:container_file_t:s0:c42,c100";
    assert!(
        validate_inspected_selinux_labels(process, mount, true, true)
            .unwrap()
            .is_some()
    );
    assert!(validate_inspected_selinux_labels(process, mount, false, true).is_err());
    assert!(validate_inspected_selinux_labels(process, "", true, true).is_err());
}

#[test]
fn image_and_identity_values_are_strict() {
    assert_eq!(
        normalize_image_id(&format!("sha256:{}", "a".repeat(64))).unwrap(),
        format!("sha256:{}", "a".repeat(64))
    );
    assert!(validate_image_id(&"a".repeat(64)).is_err());
    assert!(validate_uuid("01234567-89AB-cdef-0123-456789abcdef", "id").is_err());
    assert!(validate_image_reference("--pull=always").is_err());
    assert!(validate_image_reference("image\nother").is_err());
}

#[tokio::test]
#[ignore = "requires an explicitly selected local Lethetic runtime image"]
async fn real_local_runtime_image_resolves_to_exact_id() {
    let requested = std::env::var("LETHETIC_RUNTIME_IMAGE")
        .expect("set LETHETIC_RUNTIME_IMAGE to an already-local image");
    let podman = resolve_real_podman().unwrap();
    require_rootless_podman(&podman).await.unwrap();
    let resolved = resolve_local_runtime_image(&podman, &requested)
        .await
        .unwrap();
    validate_image_id(&resolved.image_id).unwrap();
}

#[tokio::test]
#[ignore = "creates and removes a real rootless Podman container from an explicitly local image"]
async fn real_retained_container_starts_networkless_and_stops() {
    let requested = std::env::var("LETHETIC_RUNTIME_IMAGE")
        .expect("set LETHETIC_RUNTIME_IMAGE to an already-local image");
    let (podman, image) = prepare_retained_podman(&requested).await.unwrap();
    let current = std::env::current_dir().unwrap().canonicalize().unwrap();
    let temp = tempfile::Builder::new()
        .prefix(".lethetic-runtime-smoke-")
        .tempdir_in(current)
        .unwrap();
    let root = temp.path().canonicalize().unwrap();
    let runtime_id = crate::python::runtime_store::generate_uuid_v4().unwrap();
    let session_id = crate::python::runtime_store::generate_uuid_v4().unwrap();
    let mut config = test_config(&root);
    config.podman = podman.clone();
    config.image_id = image.image_id;
    config.container_name = format!("lethetic-python-{runtime_id}");
    config.runtime_id = runtime_id;
    config.session_id = session_id;
    std::fs::write(config.broker_directory.join("capability"), "a".repeat(64)).unwrap();
    let workspace_identity =
        crate::python::runtime_store::WorkspaceIdentity::capture(&config.workspace).unwrap();

    let container_id = create_retained_container(&config).await.unwrap();
    let cleanup_guard = TestContainerGuard::new(podman.clone(), container_id.clone());
    let created = attest_new_retained_container(&config, &container_id, &workspace_identity)
        .await
        .unwrap();
    assert!(!created.running);
    let expected_selinux_labels = created.selinux_labels.clone();
    let exercise = async {
        let start = run_podman_output(
            &podman,
            &[
                OsString::from("start"),
                OsString::from("--"),
                OsString::from(&container_id),
            ],
            Duration::from_secs(30),
        )
        .await?;
        if !start.status.success() {
            return Err(podman_failure("Podman retained-container start", &start));
        }
        let mut identity = None;
        for _ in 0..50 {
            let current = attest_retained_container(
                &config,
                &container_id,
                &workspace_identity,
                expected_selinux_labels.as_ref(),
            )
            .await?;
            if current.running {
                identity = Some(current);
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let identity =
            identity.ok_or_else(|| "retained container did not enter running state".to_string())?;
        if identity.host_pid.is_none() {
            return Err("running retained container did not expose a host PID".to_string());
        }
        run_stop(&podman, &container_id).await?;
        let stopped = attest_retained_container(
            &config,
            &container_id,
            &workspace_identity,
            expected_selinux_labels.as_ref(),
        )
        .await?;
        if stopped.running {
            return Err("retained container remained running after stop".to_string());
        }
        Ok::<(), String>(())
    }
    .await;

    exercise.unwrap();
    let removable = attest_retained_container(
        &config,
        &container_id,
        &workspace_identity,
        expected_selinux_labels.as_ref(),
    )
    .await
    .unwrap();
    assert!(!removable.running);
    cleanup_transient_container(&podman, &container_id)
        .await
        .unwrap();
    assert_eq!(
        exact_container_presence(&podman, &container_id)
            .await
            .unwrap(),
        ContainerPresence::Absent
    );
    cleanup_guard.disarm();
}
