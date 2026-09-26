use super::attestation::attest_new_transient_container;
use super::command::{
    PODMAN_COMMAND_TIMEOUT, container_reference_presence, exact_container_presence,
    load_container_inspection, load_container_inspection_reference, podman_failure,
    run_podman_output,
};
use super::model::{ContainerPresence, TransientPodmanConfig};
use super::spec::{
    build_force_remove_spec, build_transient_create_spec, normalize_container_id,
    normalize_image_id, validate_container_id, validate_transient_container_name,
};
use std::path::Path;
use std::time::Duration;

enum TransientCreateOutcome {
    Exact(String),
    Ambiguous(String),
}

fn classify_transient_create_output(output: std::process::Output) -> TransientCreateOutcome {
    if !output.status.success() {
        return TransientCreateOutcome::Ambiguous(podman_failure(
            "Podman transient-container create",
            &output,
        ));
    }
    let container_id = match String::from_utf8(output.stdout) {
        Ok(container_id) => container_id.trim().to_string(),
        Err(_) => {
            return TransientCreateOutcome::Ambiguous(
                "Podman create returned a non-UTF-8 container ID".to_string(),
            );
        }
    };
    if let Err(error) = validate_container_id(&container_id) {
        return TransientCreateOutcome::Ambiguous(format!(
            "Podman create returned an invalid container ID: {error}"
        ));
    }
    TransientCreateOutcome::Exact(container_id)
}

async fn recover_ambiguous_transient_create(
    config: &TransientPodmanConfig,
    create_error: String,
) -> Result<String, String> {
    validate_transient_container_name(&config.container_name)?;
    if container_reference_presence(&config.podman, &config.container_name).await?
        == ContainerPresence::Absent
    {
        return Err(create_error);
    }
    let inspection = load_container_inspection_reference(&config.podman, &config.container_name)
        .await
        .map_err(|error| {
            format!(
                "{create_error}; ambiguous transient create exact-name inspection failed: {error}"
            )
        })?;
    if inspection.name != config.container_name {
        return Err(format!(
            "{create_error}; ambiguous transient create resolved a different container name"
        ));
    }
    let container_id = normalize_container_id(&inspection.id).map_err(|error| {
        format!("{create_error}; ambiguous transient create returned an invalid exact ID: {error}")
    })?;
    attest_new_transient_container(config, &container_id)
        .await
        .map_err(|error| {
            format!(
                "{create_error}; exact-name recovery found container {container_id}, but immutable attestation failed and it was not removed: {error}"
            )
        })?;
    Ok(container_id)
}

pub(crate) async fn create_transient_container(
    config: &TransientPodmanConfig,
) -> Result<String, String> {
    let spec = build_transient_create_spec(config)?;
    config.prepare_workspace_for_create()?;
    let outcome = match run_podman_output(&spec.program, &spec.args, PODMAN_COMMAND_TIMEOUT).await {
        Ok(output) => classify_transient_create_output(output),
        Err(error) => TransientCreateOutcome::Ambiguous(format!(
            "Podman transient-container create was ambiguous: {error}"
        )),
    };
    match outcome {
        TransientCreateOutcome::Exact(container_id) => Ok(container_id),
        TransientCreateOutcome::Ambiguous(error) => {
            recover_ambiguous_transient_create(config, error).await
        }
    }
}

pub(crate) async fn cleanup_failed_transient_create(
    config: &TransientPodmanConfig,
    container_id: &str,
) -> Result<(), String> {
    config.validate_cleanup_identity()?;
    validate_container_id(container_id)?;
    let inspection = load_container_inspection(&config.podman, container_id)
        .await
        .map_err(|error| {
            format!(
                "failed transient container could not be ownership-attested and was not removed: {error}"
            )
        })?;
    if normalize_container_id(&inspection.id)? != container_id
        || inspection.name != config.container_name
        || normalize_image_id(&inspection.image)? != config.image_id
        || normalize_image_id(&inspection.config.image)? != config.image_id
    {
        return Err(
            "failed transient container identity or image changed; it was not removed".to_string(),
        );
    }
    let expected_labels = config.expected_labels();
    for (key, value) in &expected_labels {
        if inspection.config.labels.get(key) != Some(value) {
            return Err(format!(
                "failed transient container label {key} is missing or changed; it was not removed"
            ));
        }
    }
    if inspection
        .config
        .labels
        .keys()
        .any(|key| key.starts_with("org.lethetic.transient-") && !expected_labels.contains_key(key))
    {
        return Err(
            "failed transient container has an unexpected Lethetic-owned label; it was not removed"
                .to_string(),
        );
    }
    cleanup_transient_container(&config.podman, container_id).await
}

enum TransientRecoveryTarget {
    Absent,
    Container(String),
}

async fn resolve_known_recovery_target(
    config: &TransientPodmanConfig,
    container_id: &str,
) -> Result<TransientRecoveryTarget, String> {
    validate_container_id(container_id)?;
    let presence = exact_container_presence(&config.podman, container_id).await?;
    Ok(match presence {
        ContainerPresence::Present => TransientRecoveryTarget::Container(container_id.to_string()),
        ContainerPresence::Absent => TransientRecoveryTarget::Absent,
    })
}

async fn inspect_recovered_transient_name(
    config: &TransientPodmanConfig,
) -> Result<TransientRecoveryTarget, String> {
    let inspection =
        load_container_inspection_reference(&config.podman, &config.container_name).await?;
    if inspection.name != config.container_name {
        return Err(
            "interrupted transient create resolved a different container name; it was not removed"
                .to_string(),
        );
    }
    Ok(TransientRecoveryTarget::Container(normalize_container_id(
        &inspection.id,
    )?))
}

async fn resolve_named_recovery_target(
    config: &TransientPodmanConfig,
) -> Result<TransientRecoveryTarget, String> {
    validate_transient_container_name(&config.container_name)?;
    let mut last_presence_error = None;
    for attempt in 1..=5 {
        match container_reference_presence(&config.podman, &config.container_name).await {
            Ok(ContainerPresence::Present) => {
                return inspect_recovered_transient_name(config).await;
            }
            Ok(ContainerPresence::Absent) => last_presence_error = None,
            Err(error) => last_presence_error = Some(error),
        }
        if attempt < 5 {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    match last_presence_error {
        Some(error) => Err(format!(
            "could not verify interrupted transient create absence by its exact name: {error}"
        )),
        None => Ok(TransientRecoveryTarget::Absent),
    }
}

async fn resolve_recovery_target(
    config: &TransientPodmanConfig,
    known_container_id: Option<&str>,
) -> Result<TransientRecoveryTarget, String> {
    match known_container_id {
        Some(container_id) => resolve_known_recovery_target(config, container_id).await,
        None => resolve_named_recovery_target(config).await,
    }
}

async fn cleanup_recovered_transient(
    config: &TransientPodmanConfig,
    container_id: &str,
) -> Result<(), String> {
    match attest_new_transient_container(config, container_id).await {
        Ok(()) => cleanup_transient_container(&config.podman, container_id).await,
        Err(attestation) => cleanup_failed_transient_create(config, container_id)
            .await
            .map_err(|cleanup| {
                format!(
                    "interrupted transient create failed full attestation ({attestation}); ownership-safe cleanup failed: {cleanup}"
                )
            }),
    }
}

pub(crate) async fn cleanup_interrupted_transient_create(
    config: &TransientPodmanConfig,
    known_container_id: Option<&str>,
) -> Result<(), String> {
    config.validate_cleanup_identity()?;
    match resolve_recovery_target(config, known_container_id).await? {
        TransientRecoveryTarget::Absent => Ok(()),
        TransientRecoveryTarget::Container(container_id) => {
            cleanup_recovered_transient(config, &container_id).await
        }
    }
}

pub(crate) async fn cleanup_transient_container(
    podman: &Path,
    container_id: &str,
) -> Result<(), String> {
    validate_container_id(container_id)?;
    let initial_presence = exact_container_presence(podman, container_id).await;
    if initial_presence == Ok(ContainerPresence::Absent) {
        return Ok(());
    }
    let initial_presence_error = initial_presence.err();
    let spec = build_force_remove_spec(podman, container_id)?;
    let removal = run_podman_output(&spec.program, &spec.args, PODMAN_COMMAND_TIMEOUT).await;
    let presence = exact_container_presence(podman, container_id).await;
    let initial_context = initial_presence_error
        .map(|error| format!("; initial exact-presence check failed: {error}"))
        .unwrap_or_default();
    match (removal, presence) {
        (_, Ok(ContainerPresence::Absent)) => Ok(()),
        (Ok(output), Ok(ContainerPresence::Present)) if !output.status.success() => Err(format!(
            "{}{}",
            podman_failure("Podman transient-container cleanup", &output),
            initial_context
        )),
        (Ok(_), Ok(ContainerPresence::Present)) => Err(format!(
            "exact transient Podman container still exists after cleanup{initial_context}"
        )),
        (Err(removal), Ok(ContainerPresence::Present)) => Err(format!(
            "Podman transient-container cleanup command failed and the exact container still exists: {removal}{initial_context}"
        )),
        (Ok(output), Err(presence)) => Err(format!(
            "could not verify exact transient-container absence after cleanup (command status {}): {presence}{initial_context}",
            output.status
        )),
        (Err(removal), Err(presence)) => Err(format!(
            "Podman transient-container cleanup command failed: {removal}; absence verification also failed: {presence}{initial_context}"
        )),
    }
}
