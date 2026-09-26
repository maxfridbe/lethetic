use super::model::TransientPodmanConfig;
use super::spec::{validate_container_id, validate_transient_container_name};
use super::transient::cleanup_interrupted_transient_create;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

pub(super) const TRANSIENT_CLEANUP_DIRECTORY: &str = "transient-python-cleanups";
const TRANSIENT_CLEANUP_RECORD_VERSION: u32 = 1;
pub(super) const MAX_TRANSIENT_CLEANUP_RECORD_BYTES: usize = 256 * 1024;
pub(super) const MAX_TRANSIENT_CLEANUP_RECORDS_PER_RETRY: usize = 128;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct TransientCleanupRecord {
    version: u32,
    config: TransientPodmanConfig,
    container_id: Option<String>,
    #[serde(default)]
    attempts: u64,
}
fn transient_cleanup_file_name(
    config: &TransientPodmanConfig,
    container_id: Option<&str>,
) -> Result<String, String> {
    let key = match container_id {
        Some(container_id) => {
            validate_container_id(container_id)?;
            container_id
        }
        None => {
            validate_transient_container_name(&config.container_name)?;
            config.container_name.as_str()
        }
    };
    Ok(format!("{key}.json"))
}

pub(crate) fn persist_transient_cleanup_record(
    config: &TransientPodmanConfig,
    container_id: Option<&str>,
) -> Result<String, String> {
    persist_transient_cleanup_record_at(
        &crate::platform::lethetic_state_dir(),
        config,
        container_id,
    )
}

pub(super) fn persist_transient_cleanup_record_at(
    root: &Path,
    config: &TransientPodmanConfig,
    container_id: Option<&str>,
) -> Result<String, String> {
    config.validate_cleanup_record_identity()?;
    write_transient_cleanup_record(
        root,
        &TransientCleanupRecord {
            version: TRANSIENT_CLEANUP_RECORD_VERSION,
            config: config.clone(),
            container_id: container_id.map(str::to_string),
            attempts: 0,
        },
    )
}

fn write_transient_cleanup_record(
    root: &Path,
    record: &TransientCleanupRecord,
) -> Result<String, String> {
    record.config.validate_cleanup_record_identity()?;
    let file_name = transient_cleanup_file_name(&record.config, record.container_id.as_deref())?;
    let encoded = serde_json::to_vec(record)
        .map_err(|error| format!("could not encode transient cleanup record: {error}"))?;
    if encoded.len() > MAX_TRANSIENT_CLEANUP_RECORD_BYTES {
        return Err("transient cleanup record exceeds its storage limit".to_string());
    }
    crate::platform::ensure_private_directory_durable(root, 0o700)
        .map_err(|error| format!("could not secure transient cleanup state root: {error}"))?;
    crate::platform::atomic_write_nofollow(
        root,
        &[TRANSIENT_CLEANUP_DIRECTORY],
        &file_name,
        &encoded,
        0o600,
    )
    .map_err(|error| format!("could not persist transient cleanup record: {error}"))?;
    Ok(file_name)
}

pub(crate) fn clear_transient_cleanup_record(file_name: &str) -> Result<(), String> {
    clear_transient_cleanup_record_at(&crate::platform::lethetic_state_dir(), file_name)
}

fn clear_transient_cleanup_record_at(root: &Path, file_name: &str) -> Result<(), String> {
    crate::platform::remove_file_nofollow(root, &[TRANSIENT_CLEANUP_DIRECTORY], file_name)
        .map_err(|error| format!("could not remove transient cleanup record: {error}"))?;
    Ok(())
}

pub(crate) async fn retry_pending_transient_cleanups(podman: &Path) -> Result<(), String> {
    retry_pending_transient_cleanups_at(&crate::platform::lethetic_state_dir(), podman).await
}

pub(super) async fn retry_pending_transient_cleanups_at(
    root: &Path,
    podman: &Path,
) -> Result<(), String> {
    crate::python::backend::validate_podman_executable(podman)?;
    let directory = root.join(TRANSIENT_CLEANUP_DIRECTORY);
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "could not scan pending transient cleanup records: {error}"
            ));
        }
    };
    let mut pending = BTreeMap::new();
    let mut matching_count = 0_usize;
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!("could not inspect pending transient cleanup record: {error}")
        })?;
        let file_name = entry
            .file_name()
            .into_string()
            .map_err(|_| "transient cleanup record name is not UTF-8".to_string())?;
        if !file_name.ends_with(".json") {
            return Err(format!(
                "unexpected entry in transient cleanup state: {file_name:?}"
            ));
        }
        let bytes =
            crate::platform::read_file_nofollow(root, &[TRANSIENT_CLEANUP_DIRECTORY], &file_name)
                .map_err(|error| format!("could not read transient cleanup record: {error}"))?
                .ok_or_else(|| format!("transient cleanup record disappeared: {file_name}"))?;
        if bytes.len() > MAX_TRANSIENT_CLEANUP_RECORD_BYTES {
            return Err(format!(
                "transient cleanup record is oversized: {file_name}"
            ));
        }
        let record: TransientCleanupRecord = serde_json::from_slice(&bytes)
            .map_err(|error| format!("invalid transient cleanup record {file_name}: {error}"))?;
        if record.version != TRANSIENT_CLEANUP_RECORD_VERSION {
            return Err(format!(
                "unsupported transient cleanup record version in {file_name}"
            ));
        }
        record.config.validate_cleanup_record_identity()?;
        if transient_cleanup_file_name(&record.config, record.container_id.as_deref())? != file_name
        {
            return Err(format!(
                "transient cleanup record filename does not match its identity: {file_name}"
            ));
        }
        if record.config.podman != podman {
            continue;
        }
        record.config.validate_cleanup_identity()?;
        matching_count = matching_count
            .checked_add(1)
            .ok_or_else(|| "transient cleanup record count overflowed".to_string())?;
        pending.insert((record.attempts, file_name), record);
        if pending.len() > MAX_TRANSIENT_CLEANUP_RECORDS_PER_RETRY {
            let largest = pending
                .keys()
                .next_back()
                .cloned()
                .expect("an over-limit cleanup selection is nonempty");
            pending.remove(&largest);
        }
    }

    let deferred = matching_count.saturating_sub(pending.len());
    let mut failures = Vec::new();
    for ((_, file_name), mut record) in pending {
        match Box::pin(cleanup_interrupted_transient_create(
            &record.config,
            record.container_id.as_deref(),
        ))
        .await
        {
            Ok(()) => clear_transient_cleanup_record_at(root, &file_name)?,
            Err(error) => {
                record.attempts = record.attempts.saturating_add(1);
                match write_transient_cleanup_record(root, &record) {
                    Ok(rewritten) if rewritten == file_name => {}
                    Ok(rewritten) => failures.push(format!(
                        "{file_name}: retry record changed identity to {rewritten}"
                    )),
                    Err(write_error) => failures.push(format!(
                        "{file_name}: {error}; could not persist retry state: {write_error}"
                    )),
                }
                failures.push(format!("{file_name}: {error}"));
            }
        }
    }
    if deferred > 0 {
        failures.push(format!(
            "{deferred} pending transient cleanup record(s) were deferred to the next bounded retry pass"
        ));
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "pending transient cleanup remains unresolved: {}",
            failures.join("; ")
        ))
    }
}
