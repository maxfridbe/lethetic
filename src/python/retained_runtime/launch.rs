use crate::config::{AccessMode, Config, NetworkAccess};
use crate::python::backend::{PythonBackendChoice, ResolvedLaunch};
use crate::python::retained_podman::{
    ContainerPresence, RetainedPodmanConfig, attest_frozen_schema_v2_container,
    attest_new_retained_container, attest_retained_container, create_retained_container,
    exact_container_presence, prepare_retained_podman, resolve_exact_container_name,
};
use crate::python::runtime_store::{
    RuntimeCreationIntent, RuntimeLayoutProfile, RuntimeLifecycleState, RuntimeLock,
    RuntimeManifest, RuntimeStore, RuntimeWorkspaceOwnership, WorkspaceIdentity,
    legacy_retained_security_fingerprint, retained_security_fingerprint,
    retained_shared_security_fingerprint,
};
use crate::python::{
    LaunchKind, LaunchSpec, PythonContainerIdentity, PythonRuntimeNotice, RuntimeLaunchAction,
};
use crate::tool_runtime::SessionBinding;
use chrono::Utc;
use std::ffi::OsString;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

pub const RETAINED_ATTACH_ABI: &str = "lethetic-retained-attach-v1";
pub const DEFAULT_RETAINED_RUNTIME_IMAGE: &str = crate::config::DEFAULT_RETAINED_PODMAN_IMAGE;
pub(super) const STARTUP_TIMEOUT: Duration = Duration::from_secs(75);
pub(super) const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(210);

pub async fn prepare_retained_launch(
    config: &Config,
    binding: &SessionBinding,
) -> Result<ResolvedLaunch, String> {
    prepare_retained_launch_with_cancellation(config, binding, CancellationToken::new()).await
}

pub async fn prepare_retained_launch_with_cancellation(
    config: &Config,
    binding: &SessionBinding,
    cancellation: CancellationToken,
) -> Result<ResolvedLaunch, String> {
    if cancellation.is_cancelled() {
        return Err("retained Python launch preparation was cancelled".to_string());
    }
    let operation_cancellation = cancellation.clone();
    crate::python::retained_podman::with_podman_command_cancellation(
        cancellation,
        prepare_retained_launch_inner(config, binding, &operation_cancellation),
    )
    .await
}

async fn prepare_retained_launch_inner(
    config: &Config,
    binding: &SessionBinding,
    cancellation: &CancellationToken,
) -> Result<ResolvedLaunch, String> {
    validate_nonlocal_config(config)?;
    let runtime_id = binding
        .runtime_id
        .as_ref()
        .ok_or_else(|| "Nonlocal Python session has no retained runtime ID".to_string())?
        .clone();
    let managed_workspace = WorkspaceIdentity {
        canonical_path: binding.managed_workspace.clone(),
        device: binding.workspace_device,
        inode: binding.workspace_inode,
        binding_hash: binding.workspace_binding_hash.clone(),
    };
    managed_workspace.verify_current()?;
    let (workspace, workspace_ownership) = match &binding.shared_workspace {
        Some(shared) => {
            let workspace = shared.to_runtime_identity();
            workspace.verify_current()?;
            if workspace == managed_workspace {
                return Err("shared launch cwd must differ from the managed workspace".to_string());
            }
            (workspace, RuntimeWorkspaceOwnership::ExternalLaunchCwd)
        }
        None => (
            managed_workspace.clone(),
            RuntimeWorkspaceOwnership::Managed,
        ),
    };

    let store = RuntimeStore::open()?;
    let state_root = store.root().to_path_buf();
    let (lock, created) = store.lock_or_create_runtime(&runtime_id)?;
    let creation_intent = RuntimeCreationIntent::new_with_workspace_binding(
        runtime_id.clone(),
        binding.session_id.clone(),
        workspace.clone(),
        workspace_ownership,
        (workspace_ownership == RuntimeWorkspaceOwnership::ExternalLaunchCwd)
            .then_some(managed_workspace.clone()),
    )?;
    if created {
        if store.load_deletion_receipt(&lock)?.is_some() {
            return Err("new runtime ID already has a durable deletion receipt".to_string());
        }
    } else if store.load_creation_intent(&lock)?.is_none() {
        let existing = store.load_manifest(&lock).map_err(|error| {
            format!("existing runtime has neither a creation intent nor a valid manifest: {error}")
        })?;
        if existing.session_id != binding.session_id
            || existing.workspace != workspace
            || existing.workspace_ownership != workspace_ownership
            || existing.managed_workspace
                != (workspace_ownership == RuntimeWorkspaceOwnership::ExternalLaunchCwd)
                    .then_some(managed_workspace.clone())
            || existing.container_name != creation_intent.container_name
        {
            return Err(
                "existing runtime manifest cannot be bound to this creation intent".to_string(),
            );
        }
    }
    store.bind_creation_intent(&lock, &creation_intent)?;
    if cancellation.is_cancelled() {
        return Err("retained Python launch preparation was cancelled".to_string());
    }
    if !created {
        // Gate the persisted worker ABI before resolving a replacement image.
        // An older retained runtime is delete-only and must not be made to look
        // recoverable by silently selecting or preparing new runtime resources.
        let existing = store.load_manifest(&lock)?;
        require_current_attach_abi(&existing)?;
    }

    let (podman, image) =
        prepare_retained_podman(&config.python_runtime.sandbox.podman_image).await?;
    let worker_uid = rustix::process::geteuid().as_raw();
    let worker_gid = rustix::process::getegid().as_raw();
    let policy_security_fingerprint = config.python_security_fingerprint();
    let security_fingerprint = match workspace_ownership {
        RuntimeWorkspaceOwnership::Managed => retained_security_fingerprint(
            &policy_security_fingerprint,
            &image.image_id,
            &workspace,
            worker_uid,
            worker_gid,
        )?,
        RuntimeWorkspaceOwnership::ExternalLaunchCwd => retained_shared_security_fingerprint(
            &policy_security_fingerprint,
            &image.image_id,
            &workspace,
            &managed_workspace,
            worker_uid,
            worker_gid,
        )?,
    };
    let legacy_security_fingerprint = legacy_retained_security_fingerprint(
        &policy_security_fingerprint,
        &image.image_id,
        &workspace,
        worker_uid,
        worker_gid,
    )?;
    let broker_directory = if created {
        store.broker_directory(&lock)?
    } else {
        store.root().join("b").join(&runtime_id)
    };
    let podman_config = RetainedPodmanConfig {
        podman,
        image_id: image.image_id,
        container_name: format!("lethetic-python-{runtime_id}"),
        runtime_id: runtime_id.clone(),
        session_id: binding.session_id.clone(),
        security_fingerprint: security_fingerprint.clone(),
        runtime_abi: crate::python::supervisor::RUNTIME_ABI.to_string(),
        layout_profile: match workspace_ownership {
            RuntimeWorkspaceOwnership::Managed => RuntimeLayoutProfile::ShortSiblingV1,
            RuntimeWorkspaceOwnership::ExternalLaunchCwd => {
                RuntimeLayoutProfile::ShortSiblingSharedCwdV1
            }
        },
        workspace: workspace.canonical_path.clone(),
        workspace_destination: match workspace_ownership {
            RuntimeWorkspaceOwnership::Managed => {
                PathBuf::from(crate::python::retained_podman::CONTAINER_WORKSPACE)
            }
            RuntimeWorkspaceOwnership::ExternalLaunchCwd => workspace.canonical_path.clone(),
        },
        workspace_ownership,
        mask_lethetic: workspace_ownership == RuntimeWorkspaceOwnership::ExternalLaunchCwd,
        broker_directory,
        worker_uid,
        worker_gid,
    };

    if created {
        create_runtime(
            &store,
            &lock,
            &podman_config,
            &workspace,
            &managed_workspace,
            workspace_ownership,
            security_fingerprint.clone(),
        )
        .await?;
    } else {
        validate_existing_runtime(
            &store,
            &lock,
            &podman_config,
            &workspace,
            &managed_workspace,
            workspace_ownership,
            &security_fingerprint,
            &legacy_security_fingerprint,
        )
        .await?;
    }
    if cancellation.is_cancelled() {
        return Err("retained Python launch preparation was cancelled".to_string());
    }
    let manifest = store.load_manifest(&lock)?;
    let container_id = manifest
        .container_id
        .clone()
        .ok_or_else(|| "attested retained runtime has no container ID".to_string())?;
    drop(lock);

    let executable = trusted_current_executable()?;
    let state_root_text = state_root
        .to_str()
        .ok_or_else(|| "Lethetic runtime state path is not UTF-8".to_string())?;
    let policy_fingerprint = config.python_policy_fingerprint();
    let container_identity = PythonContainerIdentity::retained(&runtime_id, true)
        .ok_or_else(|| "retained Python runtime ID is not canonical".to_string())?;
    let container_name = container_identity.name.clone();
    Ok(ResolvedLaunch {
        spec: LaunchSpec {
            kind: LaunchKind::Direct,
            container_identity: Some(container_identity),
            program: executable.into_os_string(),
            args: vec![
                OsString::from("--internal-retained-attach"),
                OsString::from(RETAINED_ATTACH_ABI),
                OsString::from(runtime_id.clone()),
                OsString::from(state_root_text),
            ],
            cwd: Some(workspace.canonical_path.clone()),
            clear_env: false,
            env: Vec::new(),
            cleanup: None,
            startup_notice: Some(PythonRuntimeNotice {
                container_id,
                container_name,
                action: if created {
                    RuntimeLaunchAction::Created
                } else {
                    RuntimeLaunchAction::Resumed
                },
                network: NetworkAccess::Nonlocal,
                mounted_cwd: workspace.canonical_path.clone(),
            }),
            startup_timeout: STARTUP_TIMEOUT,
            graceful_shutdown: Some(GRACEFUL_SHUTDOWN_TIMEOUT),
        },
        fingerprint: format!("{policy_fingerprint}:retained:{runtime_id}:{security_fingerprint}"),
        choice: PythonBackendChoice::Podman,
        network: Some(NetworkAccess::Nonlocal),
        workspace_access: Some(AccessMode::ReadWrite),
        host_visible_roots: vec![workspace.canonical_path.clone()],
        launch_cwd: workspace.canonical_path,
    })
}

async fn create_runtime(
    store: &RuntimeStore,
    lock: &RuntimeLock,
    config: &RetainedPodmanConfig,
    workspace: &WorkspaceIdentity,
    managed_workspace: &WorkspaceIdentity,
    workspace_ownership: RuntimeWorkspaceOwnership,
    security_fingerprint: String,
) -> Result<(), String> {
    let mut manifest = RuntimeManifest::new_with_workspace_binding(
        config.runtime_id.clone(),
        config.session_id.clone(),
        config.image_id.clone(),
        security_fingerprint,
        workspace.clone(),
        workspace_ownership,
        (workspace_ownership == RuntimeWorkspaceOwnership::ExternalLaunchCwd)
            .then_some(managed_workspace.clone()),
        Utc::now(),
    )?;
    store.save_manifest(lock, &manifest)?;

    let create_result = create_retained_container(config).await;
    manifest.mark_create_command_finished()?;
    store.save_manifest(lock, &manifest)?;
    let container_id = create_result?;
    manifest.record_created_container_id(container_id.clone())?;
    store.save_manifest(lock, &manifest)?;

    let identity = attest_new_retained_container(config, &container_id, workspace).await?;
    if identity.running || identity.host_pid.is_some() {
        return Err("new retained container unexpectedly started during creation".to_string());
    }
    manifest.complete_created_container_attestation(identity.selinux_labels)?;
    store.save_manifest(lock, &manifest)
}

pub(super) async fn import_legacy_manifest(
    store: &RuntimeStore,
    lock: &RuntimeLock,
    manifest: &mut RuntimeManifest,
    podman: &Path,
) -> Result<(), String> {
    if manifest.layout_profile != RuntimeLayoutProfile::LegacyV2Unclassified {
        return manifest.validate_for_maintenance();
    }

    let container_id = if let Some(container_id) = manifest.container_id.as_deref() {
        match exact_container_presence(podman, container_id).await? {
            ContainerPresence::Present => Some(container_id.to_string()),
            ContainerPresence::Absent => {
                if let Some(named_id) = resolve_exact_container_name(
                    podman,
                    &manifest.runtime_id,
                    &manifest.container_name,
                )
                .await?
                {
                    return Err(format!(
                        "schema-v2 manifest container {container_id} is absent but its exact name resolves to {named_id}"
                    ));
                }
                None
            }
        }
    } else {
        resolve_exact_container_name(podman, &manifest.runtime_id, &manifest.container_name).await?
    };

    let layout_profile = if let Some(container_id) = container_id {
        let mut matched = Vec::new();
        let mut failures = Vec::new();
        for profile in [
            RuntimeLayoutProfile::LegacyRuntimeLocalV2,
            RuntimeLayoutProfile::ShortSiblingV2Imported,
        ] {
            let candidate = config_from_manifest_for_layout(
                store,
                lock,
                manifest,
                podman.to_path_buf(),
                profile,
            );
            let result = match candidate {
                Ok(candidate) => {
                    attest_frozen_schema_v2_container(
                        &candidate,
                        &container_id,
                        manifest.selinux_labels.as_ref(),
                    )
                    .await
                }
                Err(error) => Err(error),
            };
            match result {
                Ok(_) => matched.push(profile),
                Err(error) => failures.push(format!("{profile:?}: {error}")),
            }
        }
        if matched.len() != 1 {
            return Err(format!(
                "schema-v2 runtime matched {} complete retained layouts; refusing ambiguous import ({})",
                matched.len(),
                failures.join("; ")
            ));
        }
        matched[0]
    } else {
        store
            .classify_absent_schema_v2_layout(lock)?
            .unwrap_or(RuntimeLayoutProfile::LegacyRuntimeLocalV2)
    };

    manifest.import_legacy_layout(layout_profile)?;
    store.save_manifest(lock, manifest)
}

pub(super) fn require_current_attach_abi(manifest: &RuntimeManifest) -> Result<(), String> {
    if manifest.runtime_abi == crate::python::supervisor::RUNTIME_ABI {
        return Ok(());
    }
    Err(format!(
        "retained Python runtime uses worker ABI {}; this build requires {} with worker-local output recovery. The incompatible runtime remains stopped and delete-only; Lethetic will not rebuild, pull, or delete it automatically",
        manifest.runtime_abi,
        crate::python::supervisor::RUNTIME_ABI
    ))
}

async fn validate_existing_runtime(
    store: &RuntimeStore,
    lock: &RuntimeLock,
    config: &RetainedPodmanConfig,
    workspace: &WorkspaceIdentity,
    managed_workspace: &WorkspaceIdentity,
    workspace_ownership: RuntimeWorkspaceOwnership,
    security_fingerprint: &str,
    legacy_security_fingerprint: &str,
) -> Result<(), String> {
    let mut manifest = store.load_manifest(lock)?;
    import_legacy_manifest(store, lock, &mut manifest, &config.podman).await?;
    require_current_attach_abi(&manifest)?;
    if !manifest.layout_profile.is_attachable() {
        return Err(
            "legacy runtime-local broker layouts are delete-only and cannot be resumed".to_string(),
        );
    }
    let expected_security_fingerprint =
        match manifest.layout_profile {
            RuntimeLayoutProfile::ShortSiblingV1
            | RuntimeLayoutProfile::ShortSiblingSharedCwdV1 => security_fingerprint,
            RuntimeLayoutProfile::ShortSiblingV2Imported => legacy_security_fingerprint,
            RuntimeLayoutProfile::LegacyRuntimeLocalV2
            | RuntimeLayoutProfile::LegacyV2Unclassified => unreachable!("attachability checked"),
        };
    if manifest.runtime_id != config.runtime_id
        || manifest.session_id != config.session_id
        || manifest.image_id != config.image_id
        || manifest.security_fingerprint != expected_security_fingerprint
        || manifest.workspace != *workspace
        || manifest.workspace_ownership != workspace_ownership
        || manifest.managed_workspace
            != (workspace_ownership == RuntimeWorkspaceOwnership::ExternalLaunchCwd)
                .then_some(managed_workspace.clone())
        || manifest.owner_uid != config.worker_uid
        || manifest.owner_gid != config.worker_gid
        || manifest.container_name != config.container_name
    {
        return Err(
            "retained runtime manifest is incompatible with this chat and policy".to_string(),
        );
    }
    if manifest.is_expired_at(Utc::now())? {
        return Err(
            "retained Python runtime expired and must be cleaned up before reuse".to_string(),
        );
    }
    if manifest.lifecycle != RuntimeLifecycleState::Stopped {
        return Err(format!(
            "retained Python runtime requires reconciliation from state {:?}",
            manifest.lifecycle
        ));
    }
    let container_id = manifest
        .container_id
        .as_deref()
        .ok_or_else(|| "stopped runtime manifest has no container ID".to_string())?;
    let attestation_config = config_from_manifest(store, lock, &manifest, config.podman.clone())?;
    let identity = attest_retained_container(
        &attestation_config,
        container_id,
        workspace,
        manifest.selinux_labels.as_ref(),
    )
    .await?;
    if identity.running || identity.host_pid.is_some() {
        return Err("stopped runtime manifest points to a running container".to_string());
    }
    manifest.touch(Utc::now())?;
    store.save_manifest(lock, &manifest)
}

pub(super) fn config_from_manifest(
    store: &RuntimeStore,
    lock: &RuntimeLock,
    manifest: &RuntimeManifest,
    podman: PathBuf,
) -> Result<RetainedPodmanConfig, String> {
    config_from_manifest_for_layout(store, lock, manifest, podman, manifest.layout_profile)
}

fn config_from_manifest_for_layout(
    store: &RuntimeStore,
    lock: &RuntimeLock,
    manifest: &RuntimeManifest,
    podman: PathBuf,
    layout_profile: RuntimeLayoutProfile,
) -> Result<RetainedPodmanConfig, String> {
    Ok(RetainedPodmanConfig {
        podman,
        image_id: manifest.image_id.clone(),
        container_name: manifest.container_name.clone(),
        runtime_id: manifest.runtime_id.clone(),
        session_id: manifest.session_id.clone(),
        security_fingerprint: manifest.security_fingerprint.clone(),
        runtime_abi: manifest.runtime_abi.clone(),
        layout_profile,
        workspace: manifest.workspace.canonical_path.clone(),
        workspace_destination: match manifest.workspace_ownership {
            RuntimeWorkspaceOwnership::Managed => {
                PathBuf::from(crate::python::retained_podman::CONTAINER_WORKSPACE)
            }
            RuntimeWorkspaceOwnership::ExternalLaunchCwd => {
                manifest.workspace.canonical_path.clone()
            }
        },
        workspace_ownership: manifest.workspace_ownership,
        mask_lethetic: manifest.workspace_ownership == RuntimeWorkspaceOwnership::ExternalLaunchCwd,
        broker_directory: store.broker_attestation_path_for_layout(lock, layout_profile)?,
        worker_uid: manifest.owner_uid,
        worker_gid: manifest.owner_gid,
    })
}

pub(super) fn validate_nonlocal_config(config: &Config) -> Result<(), String> {
    if !crate::config::is_exact_retained_nonlocal_python_policy(
        config.tool_profile,
        &config.python_runtime,
    ) {
        return Err(
            "retained runtime requires the exact fail-closed Nonlocal Python policy".to_string(),
        );
    }
    Ok(())
}

pub(super) fn trusted_current_executable() -> Result<PathBuf, String> {
    let path = std::env::current_exe()
        .map_err(|error| format!("could not resolve Lethetic executable: {error}"))?
        .canonicalize()
        .map_err(|error| format!("could not canonicalize Lethetic executable: {error}"))?;
    let metadata = path
        .metadata()
        .map_err(|error| format!("could not inspect Lethetic executable: {error}"))?;
    if !metadata.is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o022 != 0
        || metadata.permissions().mode() & 0o111 == 0
    {
        return Err("Lethetic executable is not a trusted owner-controlled executable".to_string());
    }
    Ok(path)
}
