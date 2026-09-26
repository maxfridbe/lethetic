use super::common::{create_private_directory, parse_timestamp, validate_lower_hex, validate_uuid};
use super::manifest::{LEGACY_RUNTIME_MANIFEST_SCHEMA, expected_labels, expected_labels_for_abi};
use super::store::{
    BROKER_BRIDGES_DIRECTORY, BROKER_CAPABILITY_FILE, LEGACY_BROKER_SOCKET_FILE, MANIFEST_FILE,
    RUNTIME_LOCKS_DIRECTORY, RUNTIMES_DIRECTORY,
};
use super::workspace::{MANAGED_SESSIONS_DIRECTORY, run_bounded_process_group};
use super::*;
use crate::python::retained_podman::{LAYOUT_LABEL, SESSION_ID_LABEL};
use chrono::{DateTime, Duration, Utc};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

fn manifest(root: &Path, now: DateTime<Utc>) -> RuntimeManifest {
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::set_permissions(&workspace, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut manifest = RuntimeManifest::new(
        "01234567-89ab-4def-8123-456789abcdef".to_string(),
        "11111111-2222-4333-8444-555555555555".to_string(),
        format!("sha256:{}", "a".repeat(64)),
        "b".repeat(64),
        WorkspaceIdentity::capture(&workspace).unwrap(),
        now,
    )
    .unwrap();
    manifest.mark_create_command_finished().unwrap();
    manifest
}

#[test]
fn uuid_generation_sets_version_and_variant() {
    let id = generate_uuid_v4().unwrap();
    validate_uuid(&id, "generated UUID").unwrap();
    assert_eq!(id.as_bytes()[14], b'4');
    assert!(matches!(id.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
    let capability = generate_broker_capability().unwrap();
    validate_lower_hex(&capability, 64, "broker capability").unwrap();
}

#[test]
fn bounded_process_group_times_out_reaps_leader_and_terminates_descendants() {
    let temp = tempfile::tempdir().unwrap();
    let pid_path = temp.path().join("processes.pid");
    let mut command = std::process::Command::new("/bin/sh");
    command
        .arg("-c")
        .arg(
            "trap '' TERM; /bin/sh -c 'trap \"\" TERM; while :; do sleep 1; done' & echo \"$$ $!\" > \"$1\"; wait",
        )
        .arg("lethetic-bounded-child")
        .arg(&pid_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    let started = std::time::Instant::now();
    let error = run_bounded_process_group(
        command,
        std::time::Duration::from_millis(200),
        std::time::Duration::from_millis(20),
        std::time::Duration::from_secs(1),
        "test cleanup",
    )
    .unwrap_err();
    assert!(error.contains("exceeded"), "{error}");
    assert!(started.elapsed() < std::time::Duration::from_secs(3));

    let pids = std::fs::read_to_string(&pid_path)
        .unwrap()
        .split_whitespace()
        .map(|pid| pid.parse::<i32>().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(pids.len(), 2);
    for pid in pids {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        loop {
            let result = unsafe { libc::kill(pid, 0) };
            if result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "process-group member {pid} survived bounded cleanup"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

#[test]
fn managed_workspace_is_private_stable_and_session_scoped() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join(MANAGED_SESSIONS_DIRECTORY);
    let store = ManagedWorkspaceStore::open_at(root).unwrap();
    let session = generate_uuid_v4().unwrap();
    let identity = store.create(&session).unwrap();
    assert_eq!(store.load(&session).unwrap(), identity);
    assert!(store.create(&session).is_err());
    assert_eq!(
        identity
            .canonical_path
            .metadata()
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let other = generate_uuid_v4().unwrap();
    assert_ne!(
        store.create(&other).unwrap().canonical_path,
        identity.canonical_path
    );
    let fingerprint = retained_security_fingerprint(
        &"c".repeat(64),
        &format!("sha256:{}", "d".repeat(64)),
        &identity,
        1000,
        1000,
    )
    .unwrap();
    validate_lower_hex(&fingerprint, 64, "retained fingerprint").unwrap();
    assert_ne!(
        fingerprint,
        legacy_retained_security_fingerprint(
            &"c".repeat(64),
            &format!("sha256:{}", "d".repeat(64)),
            &identity,
            1000,
            1000,
        )
        .unwrap()
    );
    assert_ne!(
        fingerprint,
        retained_security_fingerprint(
            &"c".repeat(64),
            &format!("sha256:{}", "d".repeat(64)),
            &identity,
            1001,
            1000,
        )
        .unwrap()
    );
}

#[test]
#[serial_test::serial]
#[ignore = "requires rootless Podman and LETHETIC_TEST_BINARY"]
fn managed_workspace_deletes_nonempty_mapped_subuid_tree() {
    assert!(std::env::var_os("LETHETIC_TEST_BINARY").is_some());
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join(MANAGED_SESSIONS_DIRECTORY);
    let store = ManagedWorkspaceStore::open_at(root).unwrap();
    let session = generate_uuid_v4().unwrap();
    let identity = store.create(&session).unwrap();
    let mapped = identity.canonical_path.join("mapped-root-tree");
    std::fs::create_dir(&mapped).unwrap();
    std::fs::write(mapped.join("payload"), b"data").unwrap();
    let status = std::process::Command::new("/usr/local/bin/podman")
        .args(["unshare", "chown", "-R", "1:1", "--"])
        .arg(&identity.canonical_path)
        .status()
        .unwrap();
    assert!(status.success());
    assert_ne!(
        std::fs::symlink_metadata(&identity.canonical_path)
            .unwrap()
            .uid(),
        rustix::process::geteuid().as_raw()
    );

    store.delete(&session, &identity).unwrap();
    assert!(!identity.canonical_path.exists());
}

#[test]
fn owner_created_mode_drift_is_repaired_under_runtime_identity() {
    let temp = tempfile::tempdir().unwrap();
    let workspace_root = temp.path().join(MANAGED_SESSIONS_DIRECTORY);
    let workspace_store = ManagedWorkspaceStore::open_at(workspace_root).unwrap();
    let session_id = generate_uuid_v4().unwrap();
    let identity = workspace_store.create(&session_id).unwrap();
    let session_directory = identity.canonical_path.parent().unwrap();
    std::fs::set_permissions(session_directory, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::set_permissions(
        &identity.canonical_path,
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert_eq!(workspace_store.load(&session_id).unwrap(), identity);
    assert_eq!(
        session_directory.metadata().unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        identity
            .canonical_path
            .metadata()
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );

    let runtime_root = temp.path().join("runtime-state");
    let runtime_store = RuntimeStore::open_at(runtime_root).unwrap();
    let runtime_id = generate_uuid_v4().unwrap();
    let lock = runtime_store.create_locked_runtime(&runtime_id).unwrap();
    let runtime_directory = runtime_store.runtime_directory(&runtime_id);
    drop(lock);
    std::fs::set_permissions(&runtime_directory, std::fs::Permissions::from_mode(0o755)).unwrap();
    let repaired = runtime_store
        .try_lock_runtime(&runtime_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        runtime_directory.metadata().unwrap().permissions().mode() & 0o777,
        0o700
    );
    drop(repaired);
}

#[test]
fn managed_workspace_deletion_is_exact_symlink_safe_and_retryable() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join(MANAGED_SESSIONS_DIRECTORY);
    let store = ManagedWorkspaceStore::open_at(root).unwrap();
    let session = generate_uuid_v4().unwrap();
    let identity = store.create(&session).unwrap();
    std::fs::create_dir(identity.canonical_path.join("nested")).unwrap();
    std::fs::write(identity.canonical_path.join("nested/file"), b"data").unwrap();
    let outside = tempfile::NamedTempFile::new().unwrap();
    symlink(outside.path(), identity.canonical_path.join("outside-link")).unwrap();

    store.delete(&session, &identity).unwrap();
    assert!(outside.path().exists());
    assert!(!identity.canonical_path.exists());
    store.delete(&session, &identity).unwrap();

    let other_session = generate_uuid_v4().unwrap();
    let other = store.create(&other_session).unwrap();
    let mut redirected = other.clone();
    redirected.canonical_path = temp.path().to_path_buf();
    assert!(store.delete(&other_session, &redirected).is_err());
    assert!(other.canonical_path.exists());

    let blocked_session = generate_uuid_v4().unwrap();
    let blocked = store.create(&blocked_session).unwrap();
    std::fs::write(blocked.canonical_path.join("keep"), b"source").unwrap();
    let blocked_parent = blocked.canonical_path.parent().unwrap();
    std::fs::write(blocked_parent.join("unexpected"), b"keep").unwrap();
    assert!(store.delete(&blocked_session, &blocked).is_err());
    assert!(blocked.canonical_path.exists());
    assert_eq!(
        std::fs::read(blocked.canonical_path.join("keep")).unwrap(),
        b"source"
    );
}

#[test]
fn lifecycle_and_container_binding_are_strict() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let now = Utc::now();
    let mut recovered = manifest(&root, now);
    let mut manifest = manifest(&root, now);
    assert!(
        manifest
            .transition(RuntimeLifecycleState::Attached)
            .is_err()
    );
    let selinux_labels = crate::python::selinux::SelinuxLabels::new(
        "system_u:system_r:container_t:s0:c42,c100".to_string(),
        "system_u:object_r:container_file_t:s0:c42,c100".to_string(),
    )
    .unwrap();
    manifest
        .bind_created_container_with_security("c".repeat(64), Some(selinux_labels.clone()))
        .unwrap();
    assert_eq!(manifest.lifecycle, RuntimeLifecycleState::Stopped);
    assert_eq!(manifest.selinux_labels, Some(selinux_labels));
    assert!(manifest.bind_created_container("d".repeat(64)).is_err());
    manifest
        .transition(RuntimeLifecycleState::Attached)
        .unwrap();
    assert!(manifest.transition(RuntimeLifecycleState::Stopped).is_err());
    manifest
        .transition(RuntimeLifecycleState::Stopping)
        .unwrap();
    manifest.transition(RuntimeLifecycleState::Stopped).unwrap();
    manifest
        .transition(RuntimeLifecycleState::Deleting)
        .unwrap();
    assert!(manifest.transition(RuntimeLifecycleState::Stopped).is_err());

    recovered
        .transition(RuntimeLifecycleState::Deleting)
        .unwrap();
    recovered
        .bind_deleting_container_id("d".repeat(64))
        .unwrap();
    assert_eq!(
        recovered.container_id.as_deref(),
        Some("d".repeat(64).as_str())
    );
    assert!(
        recovered
            .bind_deleting_container_id("e".repeat(64))
            .is_err()
    );

    let mut quarantined = self::manifest(&root, now);
    quarantined.bind_created_container("f".repeat(64)).unwrap();
    quarantined.quarantine("attestation failed").unwrap();
    quarantined
        .mark_deleting_after_verified_container_absence()
        .unwrap();
    assert_eq!(quarantined.lifecycle, RuntimeLifecycleState::Deleting);
    assert!(quarantined.quarantine_reason.is_none());
}

#[test]
fn ttl_expires_at_exact_boundary_and_never_resurrects() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let now = Utc::now();
    let mut manifest = manifest(&root, now);
    manifest.bind_created_container("c".repeat(64)).unwrap();
    let boundary = now + Duration::days(RUNTIME_TTL_DAYS);
    assert!(
        !manifest
            .is_expired_at(boundary - Duration::seconds(1))
            .unwrap()
    );
    assert!(manifest.is_expired_at(boundary).unwrap());
    assert!(manifest.touch(boundary).is_err());
    manifest.touch(now + Duration::days(1)).unwrap();
    assert_eq!(
        parse_timestamp(&manifest.expires_at, "expires").unwrap(),
        now + Duration::days(1 + RUNTIME_TTL_DAYS)
    );
}

#[test]
fn creation_intent_survives_uninitialized_state_removal_and_is_immutable() {
    let temp = tempfile::tempdir().unwrap();
    let store = RuntimeStore::open_at(temp.path().join("state")).unwrap();
    let root = temp.path().canonicalize().unwrap();
    let manifest = manifest(&root, Utc::now());
    let intent = RuntimeCreationIntent::new(
        manifest.runtime_id.clone(),
        manifest.session_id.clone(),
        manifest.workspace.clone(),
    )
    .unwrap();
    let lock = store.create_locked_runtime(&manifest.runtime_id).unwrap();
    store.bind_creation_intent(&lock, &intent).unwrap();
    assert_eq!(
        store.load_creation_intent(&lock).unwrap(),
        Some(intent.clone())
    );
    store.remove_uninitialized_runtime_state(lock).unwrap();

    let lock = (0..100)
        .find_map(|_| {
            let lock = store
                .try_lock_runtime_identity_only(&manifest.runtime_id)
                .unwrap();
            if lock.is_none() {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            lock
        })
        .expect("runtime lock should be released after inherited CLOEXEC descriptors close");
    assert_eq!(
        store.load_creation_intent(&lock).unwrap(),
        Some(intent.clone())
    );
    let mut conflicting = intent;
    conflicting.session_id = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee".to_string();
    assert!(store.bind_creation_intent(&lock, &conflicting).is_err());
}

#[test]
fn deletion_receipt_survives_runtime_state_removal() {
    let temp = tempfile::tempdir().unwrap();
    let store = RuntimeStore::open_at(temp.path().join("state")).unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut manifest = manifest(&root, Utc::now());
    manifest
        .transition(RuntimeLifecycleState::Deleting)
        .unwrap();
    let lock = store.create_locked_runtime(&manifest.runtime_id).unwrap();
    store.save_manifest(&lock, &manifest).unwrap();
    store.save_deletion_receipt(&lock, &manifest).unwrap();
    store.remove_runtime_state(lock, &manifest).unwrap();

    let receipt_lock = (0..100)
        .find_map(|_| {
            let lock = store
                .try_lock_runtime_identity_only(&manifest.runtime_id)
                .unwrap();
            if lock.is_none() {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            lock
        })
        .expect("runtime lock should be released after inherited CLOEXEC descriptors close");
    let receipt = store.load_deletion_receipt(&receipt_lock).unwrap().unwrap();
    assert_eq!(receipt.runtime_id, manifest.runtime_id);
    assert_eq!(receipt.session_id, manifest.session_id);
    assert_eq!(receipt.container_id, manifest.container_id);
}

#[test]
fn schema_v2_manifest_requires_explicit_layout_import() {
    let temp = tempfile::tempdir().unwrap();
    let store = RuntimeStore::open_at(temp.path().join("state")).unwrap();
    let root = temp.path().canonicalize().unwrap();
    let current = manifest(&root, Utc::now());
    let lock = store.create_locked_runtime(&current.runtime_id).unwrap();

    let mut legacy = serde_json::to_value(&current).unwrap();
    legacy["schema_version"] = serde_json::json!(LEGACY_RUNTIME_MANIFEST_SCHEMA);
    legacy.as_object_mut().unwrap().remove("layout_profile");
    legacy
        .as_object_mut()
        .unwrap()
        .remove("workspace_ownership");
    legacy.as_object_mut().unwrap().remove("managed_workspace");
    legacy["labels"]
        .as_object_mut()
        .unwrap()
        .remove(LAYOUT_LABEL);
    std::fs::write(
        store
            .runtime_directory(&current.runtime_id)
            .join(MANIFEST_FILE),
        serde_json::to_vec_pretty(&legacy).unwrap(),
    )
    .unwrap();

    let mut loaded = store.load_manifest(&lock).unwrap();
    assert_eq!(loaded.schema_version, LEGACY_RUNTIME_MANIFEST_SCHEMA);
    assert_eq!(
        loaded.layout_profile,
        RuntimeLayoutProfile::LegacyV2Unclassified
    );
    assert!(loaded.validate().is_err());
    loaded
        .import_legacy_layout(RuntimeLayoutProfile::ShortSiblingV2Imported)
        .unwrap();
    store.save_manifest(&lock, &loaded).unwrap();
    assert_eq!(
        store.load_manifest(&lock).unwrap().layout_profile,
        RuntimeLayoutProfile::ShortSiblingV2Imported
    );

    let mut missing_current_layout = legacy;
    missing_current_layout["schema_version"] = serde_json::json!(RUNTIME_MANIFEST_SCHEMA);
    std::fs::write(
        store
            .runtime_directory(&current.runtime_id)
            .join(MANIFEST_FILE),
        serde_json::to_vec_pretty(&missing_current_layout).unwrap(),
    )
    .unwrap();
    assert!(store.load_manifest(&lock).is_err());
}

#[test]
fn schema_v2_deletion_recovers_after_container_bridge_artifacts_are_absent() {
    let temp = tempfile::tempdir().unwrap();
    let store = RuntimeStore::open_at(temp.path().join("state")).unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut current = manifest(&root, Utc::now());
    current.runtime_abi = LEGACY_RUNTIME_ABI_V2.to_string();
    current.labels = expected_labels_for_abi(
        &current.runtime_id,
        &current.session_id,
        &current.security_fingerprint,
        current.layout_profile,
        LEGACY_RUNTIME_ABI_V2,
    );
    let lock = store.create_locked_runtime(&current.runtime_id).unwrap();
    let mut legacy = serde_json::to_value(&current).unwrap();
    legacy["schema_version"] = serde_json::json!(LEGACY_RUNTIME_MANIFEST_SCHEMA);
    legacy.as_object_mut().unwrap().remove("layout_profile");
    legacy
        .as_object_mut()
        .unwrap()
        .remove("workspace_ownership");
    legacy.as_object_mut().unwrap().remove("managed_workspace");
    legacy["labels"]
        .as_object_mut()
        .unwrap()
        .remove(LAYOUT_LABEL);
    std::fs::write(
        store
            .runtime_directory(&current.runtime_id)
            .join(MANIFEST_FILE),
        serde_json::to_vec_pretty(&legacy).unwrap(),
    )
    .unwrap();
    std::fs::remove_dir(store.broker_directory_path(&current.runtime_id)).unwrap();

    let mut loaded = store.load_manifest(&lock).unwrap();
    assert_eq!(loaded.runtime_abi, LEGACY_RUNTIME_ABI_V2);
    assert!(loaded.validate_for_maintenance().is_err());
    assert_eq!(store.classify_absent_schema_v2_layout(&lock).unwrap(), None);
    loaded
        .import_legacy_layout(RuntimeLayoutProfile::LegacyRuntimeLocalV2)
        .unwrap();
    assert!(loaded.validate().is_err());
    assert!(loaded.validate_for_maintenance().is_ok());
    loaded.transition(RuntimeLifecycleState::Deleting).unwrap();
    store.save_manifest(&lock, &loaded).unwrap();
    store.save_deletion_receipt(&lock, &loaded).unwrap();
    store.remove_runtime_state(lock, &loaded).unwrap();
    assert!(!store.runtime_directory(&current.runtime_id).exists());
}

#[test]
fn shared_workspace_manifest_keeps_external_and_owned_identities_distinct() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let managed_path = root.join("managed");
    let shared_path = root.join("shared");
    std::fs::create_dir(&managed_path).unwrap();
    std::fs::create_dir(&shared_path).unwrap();
    let managed = WorkspaceIdentity::capture(&managed_path).unwrap();
    let shared = WorkspaceIdentity::capture(&shared_path).unwrap();
    let runtime_id = generate_uuid_v4().unwrap();
    let session_id = generate_uuid_v4().unwrap();
    let image_id = format!("sha256:{}", "a".repeat(64));
    let fingerprint = retained_shared_security_fingerprint(
        &"b".repeat(64),
        &image_id,
        &shared,
        &managed,
        rustix::process::geteuid().as_raw(),
        rustix::process::getegid().as_raw(),
    )
    .unwrap();
    let manifest = RuntimeManifest::new_with_workspace_binding(
        runtime_id,
        session_id,
        image_id,
        fingerprint,
        shared.clone(),
        RuntimeWorkspaceOwnership::ExternalLaunchCwd,
        Some(managed.clone()),
        Utc::now(),
    )
    .unwrap();
    assert_eq!(
        manifest.layout_profile,
        RuntimeLayoutProfile::ShortSiblingSharedCwdV1
    );
    assert_eq!(manifest.workspace, shared);
    assert_eq!(manifest.managed_workspace.as_ref(), Some(&managed));

    let store = RuntimeStore::open_at(root.join("state")).unwrap();
    let lock = store.create_locked_runtime(&manifest.runtime_id).unwrap();
    store.save_manifest(&lock, &manifest).unwrap();
    assert_eq!(store.load_manifest(&lock).unwrap(), manifest);
}

#[test]
fn store_requires_lock_and_roundtrips_manifest() {
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("state");
    let store = RuntimeStore::open_at(state).unwrap();
    let now = Utc::now();
    let manifest_root = temp.path().canonicalize().unwrap();
    let manifest = manifest(&manifest_root, now);
    let lock = store.create_locked_runtime(&manifest.runtime_id).unwrap();
    store.save_manifest(&lock, &manifest).unwrap();
    assert_eq!(store.load_manifest(&lock).unwrap(), manifest);
    let broker = store.prepare_broker_bootstrap(&lock).unwrap();
    let expected_bridge = store
        .root()
        .join(BROKER_BRIDGES_DIRECTORY)
        .join(&manifest.runtime_id);
    assert_eq!(
        broker.capability_path.parent(),
        Some(expected_bridge.as_path())
    );
    assert_eq!(broker.socket_path, expected_bridge.join(BROKER_SOCKET_FILE));
    crate::python::egress_broker::validate_unix_socket_path_length(&broker.socket_path).unwrap();
    validate_lower_hex(&broker.capability, 64, "broker capability").unwrap();
    assert_eq!(
        broker
            .capability_path
            .metadata()
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0
    );
    assert!(
        store
            .try_lock_runtime(&manifest.runtime_id)
            .unwrap()
            .is_none()
    );
    drop(lock);
    let mut reacquired = None;
    for _ in 0..100 {
        if let Some(lock) = store.try_lock_runtime(&manifest.runtime_id).unwrap() {
            reacquired = Some(lock);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(reacquired.is_some());
}

#[test]
fn known_v2_worker_manifest_is_loadable_but_not_current_attach_compatible() {
    let temp = tempfile::tempdir().unwrap();
    let store = RuntimeStore::open_at(temp.path().join("state")).unwrap();
    let mut manifest = manifest(&temp.path().canonicalize().unwrap(), Utc::now());
    manifest.bind_created_container("c".repeat(64)).unwrap();
    manifest.runtime_abi = LEGACY_RUNTIME_ABI_V2.to_string();
    manifest.labels = expected_labels_for_abi(
        &manifest.runtime_id,
        &manifest.session_id,
        &manifest.security_fingerprint,
        manifest.layout_profile,
        LEGACY_RUNTIME_ABI_V2,
    );
    let lock = store.create_locked_runtime(&manifest.runtime_id).unwrap();
    store.save_manifest(&lock, &manifest).unwrap();
    let loaded = store.load_manifest(&lock).unwrap();
    assert_eq!(loaded.runtime_abi, LEGACY_RUNTIME_ABI_V2);
    assert!(loaded.validate().is_err());
    assert!(loaded.validate_for_maintenance().is_ok());
    let mut unknown_abi = loaded.clone();
    unknown_abi.runtime_abi = "lethetic-python-runtime-v999".to_string();
    unknown_abi.labels = expected_labels_for_abi(
        &unknown_abi.runtime_id,
        &unknown_abi.session_id,
        &unknown_abi.security_fingerprint,
        unknown_abi.layout_profile,
        &unknown_abi.runtime_abi,
    );
    assert!(unknown_abi.validate_for_maintenance().is_err());

    let mut attach_attempt = loaded.clone();
    assert!(
        attach_attempt
            .transition(RuntimeLifecycleState::Attached)
            .is_err()
    );
    let mut touch_attempt = loaded.clone();
    assert!(touch_attempt.touch(Utc::now()).is_err());

    let mut deleting = loaded;
    deleting
        .transition(RuntimeLifecycleState::Deleting)
        .unwrap();
    store.save_manifest(&lock, &deleting).unwrap();
    store.save_deletion_receipt(&lock, &deleting).unwrap();
    store.remove_runtime_state(lock, &deleting).unwrap();
    assert!(!store.runtime_state_exists(&deleting.runtime_id).unwrap());
}

#[test]
fn manifestless_legacy_runtime_artifacts_are_exactly_cleaned() {
    let temp = tempfile::tempdir().unwrap();
    let store = RuntimeStore::open_at(temp.path().join("state")).unwrap();
    let runtime_id = generate_uuid_v4().unwrap();
    let lock = store.create_locked_runtime(&runtime_id).unwrap();
    std::fs::remove_dir(store.broker_directory_path(&runtime_id)).unwrap();

    let legacy = store.legacy_broker_directory_path(&runtime_id);
    create_private_directory(&legacy).unwrap();
    std::fs::write(legacy.join(BROKER_CAPABILITY_FILE), "a".repeat(64)).unwrap();
    std::fs::set_permissions(
        legacy.join(BROKER_CAPABILITY_FILE),
        std::fs::Permissions::from_mode(0o000),
    )
    .unwrap();
    let manifest_temporary = store
        .runtime_directory(&runtime_id)
        .join(".manifest.json.tmp-123-456-7");
    std::fs::write(&manifest_temporary, b"partial").unwrap();
    std::fs::set_permissions(&manifest_temporary, std::fs::Permissions::from_mode(0o600)).unwrap();

    store.remove_uninitialized_runtime_state(lock).unwrap();
    assert!(!store.runtime_directory(&runtime_id).exists());
    assert!(!store.broker_directory_path(&runtime_id).exists());
}

#[test]
fn uninitialized_runtime_state_is_removed_only_when_empty() {
    let temp = tempfile::tempdir().unwrap();
    let store = RuntimeStore::open_at(temp.path().join("state")).unwrap();
    let runtime_id = generate_uuid_v4().unwrap();
    let lock = store.create_locked_runtime(&runtime_id).unwrap();
    assert!(store.load_manifest_optional(&lock).unwrap().is_none());
    store.remove_uninitialized_runtime_state(lock).unwrap();
    assert!(!store.runtime_directory(&runtime_id).exists());

    let runtime_id = generate_uuid_v4().unwrap();
    let lock = store.create_locked_runtime(&runtime_id).unwrap();
    std::fs::write(
        store.broker_directory(&lock).unwrap().join("unexpected"),
        b"keep",
    )
    .unwrap();
    assert!(store.remove_uninitialized_runtime_state(lock).is_err());
    assert!(store.runtime_directory(&runtime_id).exists());
}

#[test]
fn stale_broker_cleanup_recovers_bind_and_atomic_write_crashes() {
    let temp = tempfile::tempdir().unwrap();
    let store = RuntimeStore::open_at(temp.path().join("state")).unwrap();
    let runtime_id = generate_uuid_v4().unwrap();
    let lock = store.create_locked_runtime(&runtime_id).unwrap();
    let broker_directory = store.broker_directory(&lock).unwrap();
    let socket = broker_directory.join(BROKER_SOCKET_FILE);
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o755)).unwrap();
    let temporary = broker_directory.join(".capability.tmp-123-456-7");
    std::fs::write(&temporary, b"partial").unwrap();
    std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o000)).unwrap();

    assert!(store.clear_stale_broker_artifacts(&lock).unwrap());
    assert!(!socket.exists());
    assert!(!temporary.exists());
    drop(listener);

    std::fs::write(&socket, b"not a socket").unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(store.clear_stale_broker_artifacts(&lock).is_err());
    assert!(socket.exists());
}

#[test]
fn malformed_capability_temporary_name_blocks_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let store = RuntimeStore::open_at(temp.path().join("state")).unwrap();
    let runtime_id = generate_uuid_v4().unwrap();
    let lock = store.create_locked_runtime(&runtime_id).unwrap();
    let broker_directory = store.broker_directory(&lock).unwrap();
    let malformed = broker_directory.join(".capability.tmp-not-a-crash-artifact");
    std::fs::write(&malformed, b"").unwrap();
    std::fs::set_permissions(&malformed, std::fs::Permissions::from_mode(0o000)).unwrap();

    assert!(store.clear_stale_broker_artifacts(&lock).is_err());
    assert!(malformed.exists());
}

#[test]
fn maintenance_scan_reports_bad_entries_without_starving_valid_ids() {
    let temp = tempfile::tempdir().unwrap();
    let store = RuntimeStore::open_at(temp.path().join("state")).unwrap();
    let valid = generate_uuid_v4().unwrap();
    drop(store.create_locked_runtime(&valid).unwrap());
    std::fs::write(
        store.root().join(RUNTIMES_DIRECTORY).join("unexpected"),
        b"keep",
    )
    .unwrap();
    let scan = store.scan_runtime_ids_for_maintenance().unwrap();
    assert_eq!(scan.runtime_ids, vec![valid]);
    assert_eq!(scan.diagnostics.len(), 1);
}

#[test]
fn manifest_rejects_moved_workspace_and_tampered_labels() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let mut manifest = manifest(&root, Utc::now());
    manifest
        .labels
        .insert(SESSION_ID_LABEL.to_string(), "other".to_string());
    assert!(manifest.validate().is_err());
    manifest.labels = expected_labels(
        &manifest.runtime_id,
        &manifest.session_id,
        &manifest.security_fingerprint,
        manifest.layout_profile,
    );
    let moved = root.join("moved");
    std::fs::rename(&manifest.workspace.canonical_path, &moved).unwrap();
    assert!(manifest.workspace.verify_current().is_err());
}

#[test]
fn legacy_runtime_state_deletion_removes_only_frozen_layout_artifacts() {
    let temp = tempfile::tempdir().unwrap();
    let store = RuntimeStore::open_at(temp.path().join("state")).unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut manifest = manifest(&root, Utc::now());
    manifest.layout_profile = RuntimeLayoutProfile::LegacyRuntimeLocalV2;
    manifest.labels = expected_labels(
        &manifest.runtime_id,
        &manifest.session_id,
        &manifest.security_fingerprint,
        manifest.layout_profile,
    );
    manifest
        .transition(RuntimeLifecycleState::Deleting)
        .unwrap();
    let lock = store.create_locked_runtime(&manifest.runtime_id).unwrap();
    store.save_manifest(&lock, &manifest).unwrap();

    let legacy = store.legacy_broker_directory_path(&manifest.runtime_id);
    create_private_directory(&legacy).unwrap();
    std::fs::write(legacy.join(BROKER_CAPABILITY_FILE), "a".repeat(64)).unwrap();
    std::fs::set_permissions(
        legacy.join(BROKER_CAPABILITY_FILE),
        std::fs::Permissions::from_mode(0o000),
    )
    .unwrap();
    let socket = legacy.join(LEGACY_BROKER_SOCKET_FILE);
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o755)).unwrap();

    store.remove_runtime_state(lock, &manifest).unwrap();
    assert!(!store.runtime_directory(&manifest.runtime_id).exists());
    assert!(!store.broker_directory_path(&manifest.runtime_id).exists());
    drop(listener);
}

#[test]
fn deleting_state_removes_only_known_manifest_bound_entries() {
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("state");
    let store = RuntimeStore::open_at(state).unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut manifest = manifest(&root, Utc::now());
    let runtime_id = manifest.runtime_id.clone();
    let lock = store.create_locked_runtime(&runtime_id).unwrap();
    let external_lock = store
        .root()
        .join(RUNTIME_LOCKS_DIRECTORY)
        .join(format!("{runtime_id}.lock"));
    assert!(external_lock.exists());
    manifest
        .transition(RuntimeLifecycleState::Deleting)
        .unwrap();
    store.save_manifest(&lock, &manifest).unwrap();
    store.prepare_broker_bootstrap(&lock).unwrap();
    let manifest_temporary = store
        .runtime_directory(&runtime_id)
        .join(".manifest.json.tmp-123-456-7");
    std::fs::write(&manifest_temporary, b"partial manifest").unwrap();
    std::fs::set_permissions(&manifest_temporary, std::fs::Permissions::from_mode(0o600)).unwrap();
    store.remove_runtime_state(lock, &manifest).unwrap();
    assert!(!store.runtime_directory(&runtime_id).exists());
    assert!(external_lock.exists());
}

#[test]
fn deleting_state_recovers_after_partial_broker_removal_without_lock_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let store = RuntimeStore::open_at(temp.path().join("state")).unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut manifest = manifest(&root, Utc::now());
    let runtime_id = manifest.runtime_id.clone();
    let lock = store.create_locked_runtime(&runtime_id).unwrap();
    manifest
        .transition(RuntimeLifecycleState::Deleting)
        .unwrap();
    store.save_manifest(&lock, &manifest).unwrap();
    let bootstrap = store.prepare_broker_bootstrap(&lock).unwrap();
    std::fs::remove_file(bootstrap.capability_path).unwrap();
    std::fs::remove_dir(store.broker_directory(&lock).unwrap()).unwrap();
    assert!(store.try_lock_runtime(&runtime_id).unwrap().is_none());
    store.remove_runtime_state(lock, &manifest).unwrap();
    assert!(!store.runtime_directory(&runtime_id).exists());
}

#[test]
fn unexpected_state_entry_blocks_deletion_without_partial_removal() {
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("state");
    let store = RuntimeStore::open_at(state).unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut manifest = manifest(&root, Utc::now());
    let lock = store.create_locked_runtime(&manifest.runtime_id).unwrap();
    manifest
        .transition(RuntimeLifecycleState::Deleting)
        .unwrap();
    store.save_manifest(&lock, &manifest).unwrap();
    let bootstrap = store.prepare_broker_bootstrap(&lock).unwrap();
    std::fs::write(
        store
            .runtime_directory(&manifest.runtime_id)
            .join("unexpected"),
        b"do not remove",
    )
    .unwrap();
    assert!(store.remove_runtime_state(lock, &manifest).is_err());
    assert!(bootstrap.capability_path.exists());
    assert!(
        store
            .runtime_directory(&manifest.runtime_id)
            .join(MANIFEST_FILE)
            .exists()
    );
}

#[cfg(unix)]
#[test]
fn store_rejects_symlink_runtime_entries() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("state");
    let store = RuntimeStore::open_at(state).unwrap();
    let id = "01234567-89ab-4def-8123-456789abcdef";
    symlink(temp.path(), store.runtime_directory(id)).unwrap();
    assert!(store.try_lock_runtime(id).is_err());
    assert!(store.list_runtime_ids().is_err());
}
