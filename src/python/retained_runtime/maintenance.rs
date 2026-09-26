use super::launch::{config_from_manifest, import_legacy_manifest};
use crate::python::retained_podman::{
    ContainerPresence, RetainedPodmanConfig, attest_new_retained_container,
    attest_retained_container, attest_retained_container_with_unbound_labels,
    exact_container_presence, podman_command_cancellation_requested, resolve_exact_container_name,
    resolve_retained_podman, run_remove, run_stop, verify_exact_container_stopped,
    with_podman_command_cancellation,
};
use crate::python::runtime_store::{
    RuntimeLifecycleState, RuntimeLock, RuntimeManifest, RuntimeStore,
};
use chrono::{DateTime, Utc};
use std::path::Path;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeMaintenanceReport {
    pub scanned: usize,
    pub skipped_locked: usize,
    pub reconciled: usize,
    pub removed: usize,
    pub quarantined: usize,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RuntimeMaintenanceOutcome {
    Unchanged,
    Reconciled,
    Removed,
    Quarantined,
}

const MAINTENANCE_CANCELLED: &str = "retained runtime maintenance was cancelled";

pub async fn sweep_retained_runtimes() -> Result<RuntimeMaintenanceReport, String> {
    sweep_retained_runtimes_at(Utc::now()).await
}

pub async fn sweep_retained_runtimes_with_cancellation(
    cancellation: CancellationToken,
) -> Result<RuntimeMaintenanceReport, String> {
    let operation_cancellation = cancellation.clone();
    with_podman_command_cancellation(cancellation, async move {
        sweep_retained_runtimes_at_inner(Utc::now(), &operation_cancellation).await
    })
    .await
}

pub async fn sweep_retained_runtimes_at(
    now: DateTime<Utc>,
) -> Result<RuntimeMaintenanceReport, String> {
    let cancellation = CancellationToken::new();
    sweep_retained_runtimes_at_inner(now, &cancellation).await
}

async fn sweep_retained_runtimes_at_inner(
    now: DateTime<Utc>,
    cancellation: &CancellationToken,
) -> Result<RuntimeMaintenanceReport, String> {
    require_maintenance_active(cancellation)?;
    let store = RuntimeStore::open()?;
    let scan = store.scan_runtime_ids_for_maintenance()?;
    require_maintenance_active(cancellation)?;
    let mut report = RuntimeMaintenanceReport {
        errors: scan.diagnostics,
        ..Default::default()
    };
    if scan.runtime_ids.is_empty() {
        return Ok(report);
    }
    let podman_result = resolve_retained_podman().await;
    require_maintenance_active(cancellation)?;
    let podman = match podman_result {
        Ok(podman) => podman,
        Err(error) => {
            report.errors.push(format!(
                "retained runtime maintenance preflight failed: {error}"
            ));
            return Ok(report);
        }
    };
    for runtime_id in scan.runtime_ids {
        require_maintenance_active(cancellation)?;
        report.scanned += 1;
        let lock = match store.try_lock_runtime(&runtime_id) {
            Ok(Some(lock)) => lock,
            Ok(None) => {
                report.skipped_locked += 1;
                continue;
            }
            Err(error) => {
                report.errors.push(format!("runtime {runtime_id}: {error}"));
                continue;
            }
        };
        let outcome = Box::pin(maintain_locked_runtime(
            &store, lock, &podman, now, false, None,
        ))
        .await;
        require_maintenance_active(cancellation)?;
        match outcome {
            Ok(RuntimeMaintenanceOutcome::Unchanged) => {}
            Ok(RuntimeMaintenanceOutcome::Reconciled) => report.reconciled += 1,
            Ok(RuntimeMaintenanceOutcome::Removed) => report.removed += 1,
            Ok(RuntimeMaintenanceOutcome::Quarantined) => report.quarantined += 1,
            Err(error) => report.errors.push(format!("runtime {runtime_id}: {error}")),
        }
    }
    Ok(report)
}

fn require_maintenance_active(cancellation: &CancellationToken) -> Result<(), String> {
    if cancellation.is_cancelled() || podman_command_cancellation_requested() {
        Err(MAINTENANCE_CANCELLED.to_string())
    } else {
        Ok(())
    }
}

pub async fn reconcile_retained_runtime(
    runtime_id: &str,
    expected_session_id: &str,
) -> Result<(), String> {
    validate_canonical_uuid(runtime_id, "runtime ID")?;
    validate_canonical_uuid(expected_session_id, "session ID")?;
    let store = RuntimeStore::open()?;
    if !store.runtime_state_exists(runtime_id)? {
        let lock = store
            .try_lock_runtime_identity_only(runtime_id)?
            .ok_or_else(|| "retained runtime deletion is still being finalized".to_string())?;
        if store.runtime_state_exists(runtime_id)? {
            return Err("retained runtime state reappeared while reconciling detach".to_string());
        }
        let receipt = store.load_deletion_receipt(&lock)?;
        let intent = store.load_creation_intent(&lock)?;
        if let (Some(receipt), Some(intent)) = (&receipt, &intent)
            && (receipt.session_id != intent.session_id
                || receipt.container_name != intent.container_name)
        {
            return Err(
                "runtime creation intent and deletion receipt disagree on ownership".to_string(),
            );
        }
        let recorded_session_id = receipt
            .as_ref()
            .map(|receipt| receipt.session_id.as_str())
            .or_else(|| intent.as_ref().map(|intent| intent.session_id.as_str()))
            .ok_or_else(|| {
                "retained runtime state is absent without a trusted creation intent or deletion receipt"
                    .to_string()
            })?;
        if recorded_session_id != expected_session_id {
            return Err(
                "retained runtime ownership record belongs to a different session".to_string(),
            );
        }
        let podman = resolve_retained_podman().await?;
        if let Some(container_id) = receipt
            .as_ref()
            .and_then(|receipt| receipt.container_id.as_deref())
            && exact_container_presence(&podman, container_id).await? != ContainerPresence::Absent
        {
            return Err("deleted runtime receipt container ID is still present".to_string());
        }
        let container_name = receipt
            .as_ref()
            .map(|receipt| receipt.container_name.as_str())
            .or_else(|| intent.as_ref().map(|intent| intent.container_name.as_str()))
            .expect("ownership record was checked");
        if resolve_exact_container_name(&podman, runtime_id, container_name)
            .await?
            .is_some()
        {
            return Err("absent runtime state still has its exact container name".to_string());
        }
        store.sync_runtime_parent()?;
        return Ok(());
    }
    let lock = store.try_lock_runtime(runtime_id)?.ok_or_else(|| {
        "retained runtime remained locked after its attach helper exited".to_string()
    })?;
    let podman = resolve_retained_podman().await?;
    match Box::pin(maintain_locked_runtime(
        &store,
        lock,
        &podman,
        Utc::now(),
        false,
        Some(expected_session_id),
    ))
    .await?
    {
        RuntimeMaintenanceOutcome::Unchanged | RuntimeMaintenanceOutcome::Reconciled => Ok(()),
        RuntimeMaintenanceOutcome::Removed => {
            Err("retained runtime was removed instead of reaching stopped detach state".to_string())
        }
        RuntimeMaintenanceOutcome::Quarantined => {
            Err("retained runtime was quarantined during detach reconciliation".to_string())
        }
    }
}

pub async fn delete_retained_runtime(
    runtime_id: &str,
    expected_session_id: &str,
) -> Result<(), String> {
    let cancellation = CancellationToken::new();
    delete_retained_runtime_inner(runtime_id, expected_session_id, &cancellation).await
}

pub async fn delete_retained_runtime_with_cancellation(
    runtime_id: &str,
    expected_session_id: &str,
    cancellation: CancellationToken,
) -> Result<(), String> {
    let operation_cancellation = cancellation.clone();
    with_podman_command_cancellation(cancellation, async move {
        delete_retained_runtime_inner(runtime_id, expected_session_id, &operation_cancellation)
            .await
    })
    .await
}

async fn delete_retained_runtime_inner(
    runtime_id: &str,
    expected_session_id: &str,
    cancellation: &CancellationToken,
) -> Result<(), String> {
    require_maintenance_active(cancellation)?;
    validate_canonical_uuid(runtime_id, "runtime ID")?;
    validate_canonical_uuid(expected_session_id, "session ID")?;
    let store = RuntimeStore::open()?;
    let podman_result = resolve_retained_podman().await;
    require_maintenance_active(cancellation)?;
    let podman = podman_result?;
    if !store.runtime_state_exists(runtime_id)? {
        let container_name = format!("lethetic-python-{runtime_id}");
        let named_container =
            resolve_exact_container_name(&podman, runtime_id, &container_name).await;
        match named_container {
            Ok(None) => {
                store.sync_runtime_parent()?;
                return Ok(());
            }
            Ok(Some(_)) => {
                return Err(
                    "retained runtime state is absent but its exact container name still exists"
                        .to_string(),
                );
            }
            Err(error) => {
                require_maintenance_active(cancellation)?;
                return Err(error);
            }
        }
    }
    let lock = store
        .try_lock_runtime(runtime_id)?
        .ok_or_else(|| "retained Python runtime is active or being maintained".to_string())?;
    let outcome = Box::pin(maintain_locked_runtime(
        &store,
        lock,
        &podman,
        Utc::now(),
        true,
        Some(expected_session_id),
    ))
    .await;
    match outcome {
        Ok(RuntimeMaintenanceOutcome::Removed) => Ok(()),
        outcome => {
            require_maintenance_active(cancellation)?;
            match outcome? {
                RuntimeMaintenanceOutcome::Quarantined => {
                    Err("quarantined retained runtime requires manual inspection".to_string())
                }
                _ => Err("retained runtime deletion did not reach a durable result".to_string()),
            }
        }
    }
}

pub(super) async fn maintain_locked_runtime(
    store: &RuntimeStore,
    lock: RuntimeLock,
    podman: &Path,
    now: DateTime<Utc>,
    delete_requested: bool,
    expected_session_id: Option<&str>,
) -> Result<RuntimeMaintenanceOutcome, String> {
    let Some(mut manifest) = store.load_manifest_optional(&lock)? else {
        let receipt = store.load_deletion_receipt(&lock)?;
        let intent = store.load_creation_intent(&lock)?;
        if let (Some(receipt), Some(intent)) = (&receipt, &intent)
            && (receipt.session_id != intent.session_id
                || receipt.container_name != intent.container_name)
        {
            return Err(
                "runtime creation intent and deletion receipt disagree on ownership".to_string(),
            );
        }
        if let Some(expected_session_id) = expected_session_id {
            let recorded_session_id = receipt
                .as_ref()
                .map(|receipt| receipt.session_id.as_str())
                .or_else(|| intent.as_ref().map(|intent| intent.session_id.as_str()))
                .ok_or_else(|| {
                    "runtime manifest is missing without a trusted creation intent or deletion receipt"
                        .to_string()
                })?;
            if recorded_session_id != expected_session_id {
                return Err(
                    "runtime ownership record belongs to a different chat session".to_string(),
                );
            }
        }
        if let Some(container_id) = receipt
            .as_ref()
            .and_then(|receipt| receipt.container_id.as_deref())
            && exact_container_presence(podman, container_id).await? != ContainerPresence::Absent
        {
            return Err("receipt-bound runtime container ID is still present".to_string());
        }
        let container_name = receipt
            .as_ref()
            .map(|receipt| receipt.container_name.clone())
            .or_else(|| intent.as_ref().map(|intent| intent.container_name.clone()))
            .unwrap_or_else(|| format!("lethetic-python-{}", lock.runtime_id()));
        if resolve_exact_container_name(podman, lock.runtime_id(), &container_name)
            .await?
            .is_some()
        {
            return Err(
                "uninitialized runtime state has an exact-name container but no trusted manifest"
                    .to_string(),
            );
        }
        store.remove_uninitialized_runtime_state(lock)?;
        return Ok(RuntimeMaintenanceOutcome::Removed);
    };
    import_legacy_manifest(store, &lock, &mut manifest, podman).await?;
    if let Some(intent) = store.load_creation_intent(&lock)?
        && (intent.runtime_id != manifest.runtime_id
            || intent.session_id != manifest.session_id
            || intent.container_name != manifest.container_name
            || intent.workspace != manifest.workspace
            || intent.workspace_ownership != manifest.workspace_ownership
            || intent.managed_workspace != manifest.managed_workspace)
    {
        return Err("runtime manifest does not match its durable creation intent".to_string());
    }
    if expected_session_id.is_some_and(|expected| manifest.session_id != expected) {
        return Err("retained runtime belongs to a different chat session".to_string());
    }
    if manifest.lifecycle == RuntimeLifecycleState::Quarantined {
        if delete_requested
            && Box::pin(deleting_runtime_container_is_absent(podman, &manifest)).await?
        {
            manifest.mark_deleting_after_verified_container_absence()?;
            store.save_manifest(&lock, &manifest)?;
            store.save_deletion_receipt(&lock, &manifest)?;
            store.remove_runtime_state(lock, &manifest)?;
            return Ok(RuntimeMaintenanceOutcome::Removed);
        }
        if delete_requested {
            return Err(
                "quarantined retained runtime still has an exact ID or exact-name container; refusing deletion"
                    .to_string(),
            );
        }
        return Ok(RuntimeMaintenanceOutcome::Quarantined);
    }
    if manifest.lifecycle == RuntimeLifecycleState::Deleting
        && Box::pin(deleting_runtime_container_is_absent(podman, &manifest)).await?
    {
        store.save_deletion_receipt(&lock, &manifest)?;
        store.remove_runtime_state(lock, &manifest)?;
        return Ok(RuntimeMaintenanceOutcome::Removed);
    }
    let config = config_from_manifest(store, &lock, &manifest, podman.to_path_buf())?;
    match manifest.lifecycle {
        RuntimeLifecycleState::Creating => {
            Box::pin(prepare_creating_runtime_for_deletion(
                store,
                &lock,
                &mut manifest,
                &config,
            ))
            .await?;
            Box::pin(delete_manifest_bound_runtime(
                store, lock, manifest, &config,
            ))
            .await?;
            Ok(RuntimeMaintenanceOutcome::Removed)
        }
        RuntimeLifecycleState::Attached | RuntimeLifecycleState::Stopping => {
            Box::pin(reconcile_interrupted_attach(
                store,
                &lock,
                &mut manifest,
                &config,
            ))
            .await?;
            if delete_requested || manifest.is_expired_at(now)? {
                manifest.transition(RuntimeLifecycleState::Deleting)?;
                store.save_manifest(&lock, &manifest)?;
                Box::pin(delete_manifest_bound_runtime(
                    store, lock, manifest, &config,
                ))
                .await?;
                Ok(RuntimeMaintenanceOutcome::Removed)
            } else {
                Ok(RuntimeMaintenanceOutcome::Reconciled)
            }
        }
        RuntimeLifecycleState::Stopped => {
            Box::pin(validate_stopped_runtime_for_maintenance(
                store,
                &lock,
                &mut manifest,
                &config,
            ))
            .await?;
            if delete_requested || manifest.is_expired_at(now)? {
                manifest.transition(RuntimeLifecycleState::Deleting)?;
                store.save_manifest(&lock, &manifest)?;
                Box::pin(delete_manifest_bound_runtime(
                    store, lock, manifest, &config,
                ))
                .await?;
                Ok(RuntimeMaintenanceOutcome::Removed)
            } else {
                Ok(RuntimeMaintenanceOutcome::Unchanged)
            }
        }
        RuntimeLifecycleState::Deleting => {
            Box::pin(delete_manifest_bound_runtime(
                store, lock, manifest, &config,
            ))
            .await?;
            Ok(RuntimeMaintenanceOutcome::Removed)
        }
        RuntimeLifecycleState::Quarantined => unreachable!("quarantine handled above"),
    }
}

async fn deleting_runtime_container_is_absent(
    podman: &Path,
    manifest: &RuntimeManifest,
) -> Result<bool, String> {
    if let Some(container_id) = manifest.container_id.as_deref()
        && exact_container_presence(podman, container_id).await? == ContainerPresence::Present
    {
        return Ok(false);
    }
    Ok(
        resolve_exact_container_name(podman, &manifest.runtime_id, &manifest.container_name)
            .await?
            .is_none(),
    )
}

async fn prepare_creating_runtime_for_deletion(
    store: &RuntimeStore,
    lock: &RuntimeLock,
    manifest: &mut RuntimeManifest,
    config: &RetainedPodmanConfig,
) -> Result<(), String> {
    if manifest.container_id.is_none() {
        if let Some(container_id) = resolve_exact_container_name(
            &config.podman,
            &manifest.runtime_id,
            &manifest.container_name,
        )
        .await?
        {
            let identity =
                match attest_new_retained_container(config, &container_id, &manifest.workspace)
                    .await
                {
                    Ok(identity) => identity,
                    Err(error) => {
                        return Err(quarantine_reconciliation_failure(
                            store,
                            lock,
                            manifest,
                            format!("exact-name creating-container attestation failed: {error}"),
                            None,
                        )
                        .await);
                    }
                };
            if !manifest.create_command_is_known_finished() {
                manifest.mark_create_command_finished()?;
                store.save_manifest(lock, manifest)?;
            }
            if identity.running || identity.host_pid.is_some() {
                return Err(quarantine_reconciliation_failure(
                    store,
                    lock,
                    manifest,
                    "creating runtime unexpectedly reached a running state".to_string(),
                    Some((config, &container_id)),
                )
                .await);
            }
            manifest.bind_created_container_with_security(container_id, identity.selinux_labels)?;
            store.save_manifest(lock, manifest)?;
            manifest.transition(RuntimeLifecycleState::Deleting)?;
            store.save_manifest(lock, manifest)?;
            return Ok(());
        } else {
            if !manifest.create_command_is_known_finished() {
                return Err(
                    "creating runtime has no container yet and its Podman create command did not durably finish; retaining state for future reconciliation"
                        .to_string(),
                );
            }
            manifest.transition(RuntimeLifecycleState::Deleting)?;
            store.save_manifest(lock, manifest)?;
            return Ok(());
        }
    }
    let container_id = manifest
        .container_id
        .clone()
        .ok_or_else(|| "creating runtime did not persist its recovered container ID".to_string())?;
    if exact_container_presence(&config.podman, &container_id).await? == ContainerPresence::Absent {
        if let Some(named_id) = resolve_exact_container_name(
            &config.podman,
            &manifest.runtime_id,
            &manifest.container_name,
        )
        .await?
        {
            let error = format!(
                "creating runtime container {container_id} is absent but its exact name resolves to {named_id}"
            );
            return Err(
                quarantine_reconciliation_failure(store, lock, manifest, error, None).await,
            );
        }
        manifest.transition(RuntimeLifecycleState::Deleting)?;
        store.save_manifest(lock, manifest)?;
        return Ok(());
    }
    let identity = match attest_retained_container_with_unbound_labels(
        config,
        &container_id,
        &manifest.workspace,
    )
    .await
    {
        Ok(identity) => identity,
        Err(error) => {
            return Err(quarantine_reconciliation_failure(
                store,
                lock,
                manifest,
                format!("creating-container attestation failed: {error}"),
                Some((config, &container_id)),
            )
            .await);
        }
    };
    if identity.running || identity.host_pid.is_some() {
        return Err(quarantine_reconciliation_failure(
            store,
            lock,
            manifest,
            "creating runtime unexpectedly reached a running state".to_string(),
            Some((config, &container_id)),
        )
        .await);
    }
    manifest.complete_created_container_attestation(identity.selinux_labels)?;
    store.save_manifest(lock, manifest)?;
    manifest.transition(RuntimeLifecycleState::Deleting)?;
    store.save_manifest(lock, manifest)
}

async fn reconcile_interrupted_attach(
    store: &RuntimeStore,
    lock: &RuntimeLock,
    manifest: &mut RuntimeManifest,
    config: &RetainedPodmanConfig,
) -> Result<(), String> {
    if manifest.lifecycle == RuntimeLifecycleState::Attached {
        manifest.transition(RuntimeLifecycleState::Stopping)?;
        store.save_manifest(lock, manifest)?;
    }
    let container_id = manifest
        .container_id
        .clone()
        .ok_or_else(|| "interrupted runtime has no container ID".to_string())?;
    let identity = match attest_retained_container(
        config,
        &container_id,
        &manifest.workspace,
        manifest.selinux_labels.as_ref(),
    )
    .await
    {
        Ok(identity) => identity,
        Err(error) => {
            return Err(quarantine_reconciliation_failure(
                store,
                lock,
                manifest,
                format!("interrupted attach attestation failed: {error}"),
                Some((config, &container_id)),
            )
            .await);
        }
    };
    let contained = if identity.running {
        stop_manifest_bound_container(config, &container_id).await
    } else {
        verify_exact_container_stopped(&config.podman, &container_id).await
    };
    if let Err(error) = contained {
        return Err(quarantine_reconciliation_failure(
            store,
            lock,
            manifest,
            format!("interrupted attach containment failed: {error}"),
            Some((config, &container_id)),
        )
        .await);
    }
    let post_reconciliation = match attest_retained_container(
        config,
        &container_id,
        &manifest.workspace,
        manifest.selinux_labels.as_ref(),
    )
    .await
    {
        Ok(identity) => identity,
        Err(error) => {
            return Err(quarantine_reconciliation_failure(
                store,
                lock,
                manifest,
                format!("post-reconciliation attestation failed: {error}"),
                None,
            )
            .await);
        }
    };
    if post_reconciliation.running || post_reconciliation.host_pid.is_some() {
        return Err(quarantine_reconciliation_failure(
            store,
            lock,
            manifest,
            "container restarted during attach reconciliation".to_string(),
            Some((config, &container_id)),
        )
        .await);
    }
    if let Err(error) = store.clear_stale_broker_artifacts_for_layout(lock, manifest.layout_profile)
    {
        return Err(quarantine_reconciliation_failure(
            store,
            lock,
            manifest,
            format!("stale broker cleanup failed: {error}"),
            None,
        )
        .await);
    }
    manifest.transition(RuntimeLifecycleState::Stopped)?;
    store.save_manifest(lock, manifest)
}

async fn validate_stopped_runtime_for_maintenance(
    store: &RuntimeStore,
    lock: &RuntimeLock,
    manifest: &mut RuntimeManifest,
    config: &RetainedPodmanConfig,
) -> Result<(), String> {
    let container_id = manifest
        .container_id
        .clone()
        .ok_or_else(|| "stopped runtime has no container ID".to_string())?;
    let identity = match attest_retained_container(
        config,
        &container_id,
        &manifest.workspace,
        manifest.selinux_labels.as_ref(),
    )
    .await
    {
        Ok(identity) => identity,
        Err(error) => {
            return Err(quarantine_reconciliation_failure(
                store,
                lock,
                manifest,
                format!("stopped runtime attestation failed: {error}"),
                Some((config, &container_id)),
            )
            .await);
        }
    };
    if identity.running || identity.host_pid.is_some() {
        return Err(quarantine_reconciliation_failure(
            store,
            lock,
            manifest,
            "stopped manifest pointed to a running container".to_string(),
            Some((config, &container_id)),
        )
        .await);
    }
    verify_exact_container_stopped(&config.podman, &container_id).await?;
    if let Err(error) = store.clear_stale_broker_artifacts_for_layout(lock, manifest.layout_profile)
    {
        return Err(quarantine_reconciliation_failure(
            store,
            lock,
            manifest,
            format!("stale broker cleanup failed: {error}"),
            None,
        )
        .await);
    }
    Ok(())
}

async fn delete_manifest_bound_runtime(
    store: &RuntimeStore,
    lock: RuntimeLock,
    mut manifest: RuntimeManifest,
    config: &RetainedPodmanConfig,
) -> Result<(), String> {
    if manifest.lifecycle != RuntimeLifecycleState::Deleting {
        return Err("runtime deletion requires a persisted deleting tombstone".to_string());
    }
    let mut preattested = None;
    if manifest.container_id.is_none() {
        match Box::pin(resolve_exact_container_name(
            &config.podman,
            &manifest.runtime_id,
            &manifest.container_name,
        ))
        .await
        {
            Ok(Some(container_id)) => {
                let identity = match Box::pin(attest_new_retained_container(
                    config,
                    &container_id,
                    &manifest.workspace,
                ))
                .await
                {
                    Ok(identity) => identity,
                    Err(error) => {
                        return Err(persist_cleanup_failure(
                            store,
                            &lock,
                            &mut manifest,
                            format!("exact-name deleting-container attestation failed: {error}"),
                        ));
                    }
                };
                manifest.bind_deleting_container_with_security(
                    container_id,
                    identity.selinux_labels.clone(),
                )?;
                store.save_manifest(&lock, &manifest)?;
                preattested = Some(identity);
            }
            Ok(None) => {
                store.save_deletion_receipt(&lock, &manifest)?;
                store.remove_runtime_state(lock, &manifest)?;
                return Ok(());
            }
            Err(error) => {
                return Err(persist_cleanup_failure(store, &lock, &mut manifest, error));
            }
        }
    }
    let container_id = manifest
        .container_id
        .clone()
        .ok_or_else(|| "deleting runtime did not retain its exact container ID".to_string())?;
    match Box::pin(exact_container_presence(&config.podman, &container_id)).await {
        Ok(ContainerPresence::Absent) => {
            if let Err(error) = Box::pin(ensure_exact_container_name_absent(
                config,
                Some(&container_id),
            ))
            .await
            {
                return Err(persist_cleanup_failure(store, &lock, &mut manifest, error));
            }
            store.save_deletion_receipt(&lock, &manifest)?;
            store.remove_runtime_state(lock, &manifest)?;
            return Ok(());
        }
        Ok(ContainerPresence::Present) => {}
        Err(error) => {
            return Err(persist_cleanup_failure(store, &lock, &mut manifest, error));
        }
    }
    let identity = match preattested {
        Some(identity) => Ok(identity),
        None => {
            Box::pin(attest_retained_container(
                config,
                &container_id,
                &manifest.workspace,
                manifest.selinux_labels.as_ref(),
            ))
            .await
        }
    };
    let identity = match identity {
        Ok(identity) => identity,
        Err(error) => {
            let containment = Box::pin(stop_manifest_bound_container(config, &container_id)).await;
            let mut detail = format!("pre-delete attestation failed: {error}");
            if let Err(containment_error) = containment {
                detail.push_str(&format!(
                    "; exact-ID containment failed: {containment_error}"
                ));
            }
            return Err(persist_cleanup_failure(store, &lock, &mut manifest, detail));
        }
    };
    let stopped = if identity.running {
        Box::pin(stop_manifest_bound_container(config, &container_id)).await
    } else {
        Box::pin(verify_exact_container_stopped(
            &config.podman,
            &container_id,
        ))
        .await
    };
    if let Err(error) = stopped {
        return Err(persist_cleanup_failure(
            store,
            &lock,
            &mut manifest,
            format!("pre-delete containment failed: {error}"),
        ));
    }
    let final_attestation = Box::pin(attest_retained_container(
        config,
        &container_id,
        &manifest.workspace,
        manifest.selinux_labels.as_ref(),
    ))
    .await;
    if let Err(error) = final_attestation {
        return Err(persist_cleanup_failure(
            store,
            &lock,
            &mut manifest,
            format!("final pre-delete attestation failed: {error}"),
        ));
    }
    if let Err(error) = Box::pin(run_remove(&config.podman, &container_id)).await {
        return Err(persist_cleanup_failure(store, &lock, &mut manifest, error));
    }
    match Box::pin(exact_container_presence(&config.podman, &container_id)).await {
        Ok(ContainerPresence::Absent) => {}
        Ok(ContainerPresence::Present) => {
            return Err(persist_cleanup_failure(
                store,
                &lock,
                &mut manifest,
                "exact container remained present after Podman removal".to_string(),
            ));
        }
        Err(error) => {
            return Err(persist_cleanup_failure(store, &lock, &mut manifest, error));
        }
    }
    if let Err(error) = Box::pin(ensure_exact_container_name_absent(
        config,
        Some(&container_id),
    ))
    .await
    {
        return Err(persist_cleanup_failure(store, &lock, &mut manifest, error));
    }
    store.save_deletion_receipt(&lock, &manifest)?;
    store.remove_runtime_state(lock, &manifest)
}

async fn ensure_exact_container_name_absent(
    config: &RetainedPodmanConfig,
    removed_container_id: Option<&str>,
) -> Result<(), String> {
    if let Some(found) = Box::pin(resolve_exact_container_name(
        &config.podman,
        &config.runtime_id,
        &config.container_name,
    ))
    .await?
    {
        if removed_container_id == Some(found.as_str()) {
            return Err("exact container name still resolves after removal".to_string());
        }
        return Err(format!(
            "exact retained container name was replaced by different container {found}"
        ));
    }
    Ok(())
}

async fn quarantine_reconciliation_failure(
    store: &RuntimeStore,
    lock: &RuntimeLock,
    manifest: &mut RuntimeManifest,
    mut detail: String,
    container: Option<(&RetainedPodmanConfig, &str)>,
) -> String {
    if podman_command_cancellation_requested() {
        return format!("{MAINTENANCE_CANCELLED}: {detail}");
    }
    if let Some((config, container_id)) = container
        && let Err(error) = stop_manifest_bound_container(config, container_id).await
    {
        detail.push_str(&format!("; exact-ID containment failed: {error}"));
    }
    if podman_command_cancellation_requested() {
        return format!("{MAINTENANCE_CANCELLED}: {detail}");
    }
    if let Err(error) = manifest.quarantine(detail.clone()) {
        detail.push_str(&format!("; quarantine transition failed: {error}"));
        return detail;
    }
    if let Err(error) = store.save_manifest(lock, manifest) {
        detail.push_str(&format!("; quarantine persistence failed: {error}"));
    }
    detail
}

fn persist_cleanup_failure(
    store: &RuntimeStore,
    lock: &RuntimeLock,
    manifest: &mut RuntimeManifest,
    mut detail: String,
) -> String {
    if podman_command_cancellation_requested() {
        return format!("{MAINTENANCE_CANCELLED}: {detail}");
    }
    if let Err(error) = manifest.mark_cleanup_error(detail.clone()) {
        detail.push_str(&format!("; cleanup tombstone update failed: {error}"));
        return detail;
    }
    if let Err(error) = store.save_manifest(lock, manifest) {
        detail.push_str(&format!("; cleanup tombstone persistence failed: {error}"));
    }
    detail
}

pub(super) async fn stop_manifest_bound_container(
    config: &RetainedPodmanConfig,
    container_id: &str,
) -> Result<(), String> {
    let stop = run_stop(&config.podman, container_id).await;
    let verification = verify_exact_container_stopped(&config.podman, container_id).await;
    match (stop, verification) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(stop), Ok(())) => Err(stop),
        (Ok(()), Err(verification)) => Err(verification),
        (Err(stop), Err(verification)) => Err(format!(
            "{stop}; exact stopped-state verification also failed: {verification}"
        )),
    }
}

fn validate_canonical_uuid(value: &str, label: &str) -> Result<(), String> {
    let parsed = uuid::Uuid::parse_str(value)
        .map_err(|_| format!("{label} is not a canonical lowercase UUID"))?;
    if parsed.to_string() != value {
        return Err(format!("{label} is not a canonical lowercase UUID"));
    }
    Ok(())
}
