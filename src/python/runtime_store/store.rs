use super::common::{
    create_private_directory, ensure_private_directory, generate_broker_capability,
    generate_uuid_v4, repair_existing_private_directory, validate_existing_private_directory,
    validate_owner_controlled_directory, validate_uuid,
};
use super::manifest::{
    CREATION_INTENT_SCHEMA, DELETION_RECEIPT_SCHEMA, LEGACY_RUNTIME_MANIFEST_SCHEMA,
    LegacyRuntimeManifestV2, PREVIOUS_CREATION_INTENT_SCHEMA, PREVIOUS_RUNTIME_MANIFEST_SCHEMA,
    RUNTIME_MANIFEST_SCHEMA, RuntimeCreationIntent, RuntimeDeletionReceipt, RuntimeLayoutProfile,
    RuntimeLifecycleState, RuntimeManifest, RuntimeManifestSchemaProbe, validate_creation_intent,
    validate_deletion_receipt,
};
use crate::platform::{atomic_write_nofollow, lethetic_state_dir, read_file_nofollow};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub(super) const RUNTIMES_DIRECTORY: &str = "python-runtimes";
pub(super) const RUNTIME_LOCKS_DIRECTORY: &str = "python-runtime-locks";
pub(super) const BROKER_BRIDGES_DIRECTORY: &str = "b";
const LEGACY_BROKER_DIRECTORY: &str = "broker";
pub(super) const BROKER_CAPABILITY_FILE: &str = "capability";
const BROKER_CAPABILITY_TEMP_PREFIX: &str = ".capability.tmp-";
const MANIFEST_TEMP_PREFIX: &str = ".manifest.json.tmp-";
pub(crate) const BROKER_SOCKET_FILE: &str = "s";
pub(crate) const LEGACY_BROKER_SOCKET_FILE: &str = "broker.sock";
pub(super) const MANIFEST_FILE: &str = "manifest.json";
const LOCK_FILE: &str = "runtime.lock";

pub struct RuntimeStore {
    root: PathBuf,
}

pub struct RuntimeLock {
    runtime_id: String,
    file: File,
}

#[derive(Debug, Clone)]
pub struct BrokerBootstrap {
    pub capability: String,
    pub capability_path: PathBuf,
    pub socket_path: PathBuf,
    pub audit_path: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeMaintenanceScan {
    pub runtime_ids: Vec<String>,
    pub diagnostics: Vec<String>,
}

impl RuntimeLock {
    pub fn runtime_id(&self) -> &str {
        &self.runtime_id
    }
}

impl std::fmt::Debug for RuntimeLock {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeLock")
            .field("runtime_id", &self.runtime_id)
            .finish_non_exhaustive()
    }
}

impl RuntimeStore {
    pub fn open() -> Result<Self, String> {
        Self::open_at(lethetic_state_dir())
    }

    pub fn open_at(root: PathBuf) -> Result<Self, String> {
        ensure_private_directory(&root, true)?;
        let runtimes = root.join(RUNTIMES_DIRECTORY);
        ensure_private_directory(&runtimes, true)?;
        let locks = root.join(RUNTIME_LOCKS_DIRECTORY);
        ensure_private_directory(&locks, true)?;
        let broker_bridges = root.join(BROKER_BRIDGES_DIRECTORY);
        ensure_private_directory(&broker_bridges, true)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn sync_runtime_parent(&self) -> Result<(), String> {
        File::open(self.root.join(RUNTIMES_DIRECTORY))
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("could not sync runtime store parent: {error}"))
    }

    pub fn generate_runtime_id(&self) -> Result<String, String> {
        for _ in 0..32 {
            let id = generate_uuid_v4()?;
            if !self.runtime_directory(&id).exists() {
                return Ok(id);
            }
        }
        Err("could not allocate a unique Python runtime ID".to_string())
    }

    pub fn runtime_state_exists(&self, runtime_id: &str) -> Result<bool, String> {
        validate_uuid(runtime_id, "runtime ID")?;
        let directory = self.runtime_directory(runtime_id);
        match std::fs::symlink_metadata(&directory) {
            Ok(_) => {
                validate_owner_controlled_directory(&directory)?;
                Ok(true)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(format!(
                "could not inspect runtime state directory: {error}"
            )),
        }
    }

    pub fn try_lock_runtime_identity_only(
        &self,
        runtime_id: &str,
    ) -> Result<Option<RuntimeLock>, String> {
        validate_uuid(runtime_id, "runtime ID")?;
        self.try_lock_runtime_identity(runtime_id)
    }

    pub fn save_deletion_receipt(
        &self,
        lock: &RuntimeLock,
        manifest: &RuntimeManifest,
    ) -> Result<(), String> {
        self.validate_lock(lock, &manifest.runtime_id)?;
        manifest.validate_for_maintenance()?;
        if manifest.lifecycle != RuntimeLifecycleState::Deleting {
            return Err("runtime deletion receipt requires deleting lifecycle".to_string());
        }
        let receipt = RuntimeDeletionReceipt {
            schema_version: DELETION_RECEIPT_SCHEMA,
            runtime_id: manifest.runtime_id.clone(),
            session_id: manifest.session_id.clone(),
            container_id: manifest.container_id.clone(),
            container_name: manifest.container_name.clone(),
        };
        validate_deletion_receipt(&receipt)?;
        let bytes = serde_json::to_vec(&receipt)
            .map_err(|error| format!("could not encode runtime deletion receipt: {error}"))?;
        atomic_write_nofollow(
            &self.root,
            &[RUNTIME_LOCKS_DIRECTORY],
            &format!("{}.deleted.json", manifest.runtime_id),
            &bytes,
            0o600,
        )
        .map_err(|error| format!("could not persist runtime deletion receipt: {error}"))?;
        Ok(())
    }

    pub fn load_deletion_receipt(
        &self,
        lock: &RuntimeLock,
    ) -> Result<Option<RuntimeDeletionReceipt>, String> {
        self.validate_lock(lock, lock.runtime_id())?;
        let file_name = format!("{}.deleted.json", lock.runtime_id());
        let Some(bytes) = read_file_nofollow(&self.root, &[RUNTIME_LOCKS_DIRECTORY], &file_name)
            .map_err(|error| format!("could not read runtime deletion receipt: {error}"))?
        else {
            return Ok(None);
        };
        if bytes.len() > 16 * 1024 {
            return Err("runtime deletion receipt exceeds its safety limit".to_string());
        }
        let receipt: RuntimeDeletionReceipt = serde_json::from_slice(&bytes)
            .map_err(|error| format!("runtime deletion receipt is invalid: {error}"))?;
        validate_deletion_receipt(&receipt)?;
        if receipt.runtime_id != lock.runtime_id() {
            return Err("runtime deletion receipt ID does not match its lock".to_string());
        }
        Ok(Some(receipt))
    }

    pub fn bind_creation_intent(
        &self,
        lock: &RuntimeLock,
        intent: &RuntimeCreationIntent,
    ) -> Result<(), String> {
        self.validate_lock(lock, &intent.runtime_id)?;
        validate_creation_intent(intent)?;
        if let Some(existing) = self.load_creation_intent(lock)? {
            if existing != *intent {
                return Err(
                    "retained runtime creation intent belongs to a different session or workspace"
                        .to_string(),
                );
            }
            return Ok(());
        }
        let bytes = serde_json::to_vec(intent)
            .map_err(|error| format!("could not encode runtime creation intent: {error}"))?;
        atomic_write_nofollow(
            &self.root,
            &[RUNTIME_LOCKS_DIRECTORY],
            &format!("{}.intent.json", intent.runtime_id),
            &bytes,
            0o600,
        )
        .map_err(|error| format!("could not persist runtime creation intent: {error}"))?;
        Ok(())
    }

    pub fn load_creation_intent(
        &self,
        lock: &RuntimeLock,
    ) -> Result<Option<RuntimeCreationIntent>, String> {
        self.validate_lock(lock, lock.runtime_id())?;
        let file_name = format!("{}.intent.json", lock.runtime_id());
        let Some(bytes) = read_file_nofollow(&self.root, &[RUNTIME_LOCKS_DIRECTORY], &file_name)
            .map_err(|error| format!("could not read runtime creation intent: {error}"))?
        else {
            return Ok(None);
        };
        if bytes.len() > 32 * 1024 {
            return Err("runtime creation intent exceeds its safety limit".to_string());
        }
        let mut intent: RuntimeCreationIntent = serde_json::from_slice(&bytes)
            .map_err(|error| format!("runtime creation intent is invalid: {error}"))?;
        validate_creation_intent(&intent)?;
        if intent.schema_version == PREVIOUS_CREATION_INTENT_SCHEMA {
            intent.schema_version = CREATION_INTENT_SCHEMA;
        }
        if intent.runtime_id != lock.runtime_id() {
            return Err("runtime creation intent ID does not match its lock".to_string());
        }
        Ok(Some(intent))
    }

    pub fn create_locked_runtime(&self, runtime_id: &str) -> Result<RuntimeLock, String> {
        let (lock, created) = self.lock_or_create_runtime(runtime_id)?;
        if !created {
            return Err("runtime directory already exists".to_string());
        }
        Ok(lock)
    }

    pub fn lock_or_create_runtime(&self, runtime_id: &str) -> Result<(RuntimeLock, bool), String> {
        validate_uuid(runtime_id, "runtime ID")?;
        let lock = self
            .try_lock_runtime_identity(runtime_id)?
            .ok_or_else(|| "runtime is already locked by another Lethetic process".to_string())?;
        let directory = self.runtime_directory(runtime_id);
        let created = match create_private_directory(&directory) {
            Ok(()) => true,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                repair_existing_private_directory(&directory)?;
                false
            }
            Err(error) => return Err(format!("could not create runtime directory: {error}")),
        };
        File::open(self.root.join(RUNTIMES_DIRECTORY))
            .and_then(|parent| parent.sync_all())
            .map_err(|error| format!("could not sync runtime directory entry: {error}"))?;
        let manifest_exists = match std::fs::symlink_metadata(directory.join(MANIFEST_FILE)) {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => {
                return Err(format!("could not inspect runtime manifest path: {error}"));
            }
        };
        if created || !manifest_exists {
            ensure_private_directory(&self.broker_directory_path(runtime_id), true)?;
        }
        Ok((lock, created))
    }

    pub fn try_lock_runtime(&self, runtime_id: &str) -> Result<Option<RuntimeLock>, String> {
        validate_uuid(runtime_id, "runtime ID")?;
        let Some(lock) = self.try_lock_runtime_identity(runtime_id)? else {
            return Ok(None);
        };
        repair_existing_private_directory(&self.runtime_directory(runtime_id))?;
        Ok(Some(lock))
    }

    fn try_lock_runtime_identity(&self, runtime_id: &str) -> Result<Option<RuntimeLock>, String> {
        let lock_directory = self.root.join(RUNTIME_LOCKS_DIRECTORY);
        validate_existing_private_directory(&lock_directory)?;
        let path = lock_directory.join(format!("{runtime_id}.lock"));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)
            .map_err(|error| format!("could not open runtime lock: {error}"))?;
        let metadata = file
            .metadata()
            .map_err(|error| format!("could not inspect runtime lock: {error}"))?;
        if !metadata.is_file()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.nlink() != 1
        {
            return Err("runtime lock is not a trusted singly linked regular file".to_string());
        }
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("could not secure runtime lock: {error}"))?;
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
                return Ok(None);
            }
            return Err(format!("could not acquire runtime lock: {error}"));
        }
        Ok(Some(RuntimeLock {
            runtime_id: runtime_id.to_string(),
            file,
        }))
    }

    pub fn save_manifest(
        &self,
        lock: &RuntimeLock,
        manifest: &RuntimeManifest,
    ) -> Result<(), String> {
        self.validate_lock(lock, &manifest.runtime_id)?;
        manifest.validate_loaded()?;
        let bytes = serde_json::to_vec_pretty(manifest)
            .map_err(|error| format!("could not encode runtime manifest: {error}"))?;
        atomic_write_nofollow(
            &self.root,
            &[RUNTIMES_DIRECTORY, &manifest.runtime_id],
            MANIFEST_FILE,
            &bytes,
            0o600,
        )
        .map_err(|error| format!("could not save runtime manifest: {error}"))?;
        Ok(())
    }

    pub fn load_manifest(&self, lock: &RuntimeLock) -> Result<RuntimeManifest, String> {
        self.load_manifest_optional(lock)?
            .ok_or_else(|| "runtime manifest is missing".to_string())
    }

    pub fn load_manifest_optional(
        &self,
        lock: &RuntimeLock,
    ) -> Result<Option<RuntimeManifest>, String> {
        self.validate_lock(lock, &lock.runtime_id)?;
        let Some(bytes) = read_file_nofollow(
            &self.root,
            &[RUNTIMES_DIRECTORY, &lock.runtime_id],
            MANIFEST_FILE,
        )
        .map_err(|error| format!("could not read runtime manifest: {error}"))?
        else {
            return Ok(None);
        };
        if bytes.len() > 1024 * 1024 {
            return Err("runtime manifest exceeds its safety limit".to_string());
        }
        let schema: RuntimeManifestSchemaProbe = serde_json::from_slice(&bytes)
            .map_err(|error| format!("runtime manifest schema probe is invalid: {error}"))?;
        let mut manifest = match schema.schema_version {
            RUNTIME_MANIFEST_SCHEMA | PREVIOUS_RUNTIME_MANIFEST_SCHEMA => {
                serde_json::from_slice::<RuntimeManifest>(&bytes)
                    .map_err(|error| format!("runtime manifest is invalid JSON: {error}"))?
            }
            LEGACY_RUNTIME_MANIFEST_SCHEMA => {
                let legacy = serde_json::from_slice::<LegacyRuntimeManifestV2>(&bytes)
                    .map_err(|error| format!("legacy runtime manifest is invalid JSON: {error}"))?;
                RuntimeManifest::from(legacy)
            }
            unsupported => {
                return Err(format!(
                    "unsupported Python runtime manifest schema {unsupported}"
                ));
            }
        };
        if manifest.runtime_id != lock.runtime_id {
            return Err("runtime manifest ID does not match its directory".to_string());
        }
        manifest.validate_loaded()?;
        if manifest.schema_version == PREVIOUS_RUNTIME_MANIFEST_SCHEMA {
            manifest.schema_version = RUNTIME_MANIFEST_SCHEMA;
        }
        Ok(Some(manifest))
    }

    pub fn broker_directory(&self, lock: &RuntimeLock) -> Result<PathBuf, String> {
        self.validate_lock(lock, &lock.runtime_id)?;
        let path = self.broker_directory_path(&lock.runtime_id);
        validate_existing_private_directory(&path)?;
        crate::python::egress_broker::validate_unix_socket_path_length(
            &path.join(BROKER_SOCKET_FILE),
        )?;
        Ok(path)
    }

    pub fn broker_directory_for_layout(
        &self,
        lock: &RuntimeLock,
        layout_profile: RuntimeLayoutProfile,
    ) -> Result<PathBuf, String> {
        let path = self.broker_attestation_path_for_layout(lock, layout_profile)?;
        validate_existing_private_directory(&path)?;
        if layout_profile.uses_short_sibling_bridge() {
            crate::python::egress_broker::validate_unix_socket_path_length(
                &path.join(BROKER_SOCKET_FILE),
            )?;
        }
        Ok(path)
    }

    pub fn broker_attestation_path_for_layout(
        &self,
        lock: &RuntimeLock,
        layout_profile: RuntimeLayoutProfile,
    ) -> Result<PathBuf, String> {
        self.validate_lock(lock, &lock.runtime_id)?;
        match layout_profile {
            RuntimeLayoutProfile::ShortSiblingV2Imported
            | RuntimeLayoutProfile::ShortSiblingV1
            | RuntimeLayoutProfile::ShortSiblingSharedCwdV1 => {
                Ok(self.broker_directory_path(&lock.runtime_id))
            }
            RuntimeLayoutProfile::LegacyRuntimeLocalV2 => {
                Ok(self.legacy_broker_directory_path(&lock.runtime_id))
            }
            RuntimeLayoutProfile::LegacyV2Unclassified => {
                Err("schema-v2 runtime layout has not been exactly classified".to_string())
            }
        }
    }

    pub fn classify_absent_schema_v2_layout(
        &self,
        lock: &RuntimeLock,
    ) -> Result<Option<RuntimeLayoutProfile>, String> {
        self.validate_lock(lock, &lock.runtime_id)?;
        let short = self.broker_directory_path(&lock.runtime_id);
        let short_exists = validate_optional_private_directory(&short)?;
        if short_exists {
            validate_broker_entries(&short, BROKER_SOCKET_FILE)?;
        }
        let legacy = self.legacy_broker_directory_path(&lock.runtime_id);
        let legacy_exists = validate_optional_private_directory(&legacy)?;
        if legacy_exists {
            validate_broker_entries(&legacy, LEGACY_BROKER_SOCKET_FILE)?;
        }
        if legacy_exists {
            Ok(Some(RuntimeLayoutProfile::LegacyRuntimeLocalV2))
        } else if short_exists {
            Ok(Some(RuntimeLayoutProfile::ShortSiblingV2Imported))
        } else {
            Ok(None)
        }
    }

    pub fn clear_stale_broker_artifacts(&self, lock: &RuntimeLock) -> Result<bool, String> {
        let directory = self.broker_directory(lock)?;
        clear_stale_broker_artifacts_in(&directory, BROKER_SOCKET_FILE)
    }

    pub fn clear_stale_broker_artifacts_for_layout(
        &self,
        lock: &RuntimeLock,
        layout_profile: RuntimeLayoutProfile,
    ) -> Result<bool, String> {
        let directory = self.broker_directory_for_layout(lock, layout_profile)?;
        let socket_file = if layout_profile.uses_short_sibling_bridge() {
            BROKER_SOCKET_FILE
        } else {
            LEGACY_BROKER_SOCKET_FILE
        };
        clear_stale_broker_artifacts_in(&directory, socket_file)
    }

    pub fn prepare_broker_bootstrap(&self, lock: &RuntimeLock) -> Result<BrokerBootstrap, String> {
        self.clear_stale_broker_artifacts(lock)?;
        let directory = self.broker_directory(lock)?;
        let capability = generate_broker_capability()?;
        let bytes = capability.as_bytes();
        let path = atomic_write_nofollow(
            &self.root,
            &[BROKER_BRIDGES_DIRECTORY, &lock.runtime_id],
            BROKER_CAPABILITY_FILE,
            bytes,
            0o000,
        )
        .map_err(|error| format!("could not provision broker capability: {error}"))?;
        let metadata = path
            .symlink_metadata()
            .map_err(|error| format!("could not inspect broker capability: {error}"))?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.nlink() != 1
            || metadata.permissions().mode() & 0o777 != 0
        {
            return Err(
                "broker capability file is not a trusted mode-000 regular file".to_string(),
            );
        }
        Ok(BrokerBootstrap {
            capability,
            capability_path: path,
            socket_path: directory.join(BROKER_SOCKET_FILE),
            audit_path: self
                .runtime_directory(&lock.runtime_id)
                .join("egress-audit.jsonl"),
        })
    }

    pub fn remove_uninitialized_runtime_state(&self, lock: RuntimeLock) -> Result<(), String> {
        self.validate_lock(&lock, &lock.runtime_id)?;
        if self.load_manifest_optional(&lock)?.is_some() {
            return Err("initialized runtime state requires manifest-bound deletion".to_string());
        }
        let runtime_directory = self.runtime_directory(&lock.runtime_id);
        validate_existing_private_directory(&runtime_directory)?;
        let short_broker_directory = self.broker_directory_path(&lock.runtime_id);
        let short_broker_exists = validate_optional_private_directory(&short_broker_directory)?;
        let short_broker_entries = if short_broker_exists {
            validate_broker_entries(&short_broker_directory, BROKER_SOCKET_FILE)?
        } else {
            Vec::new()
        };
        let legacy_broker_directory = self.legacy_broker_directory_path(&lock.runtime_id);
        let legacy_broker_exists = validate_optional_private_directory(&legacy_broker_directory)?;
        let legacy_broker_entries = if legacy_broker_exists {
            validate_broker_entries(&legacy_broker_directory, LEGACY_BROKER_SOCKET_FILE)?
        } else {
            Vec::new()
        };
        let runtime_entries = validate_known_entries(
            &runtime_directory,
            &[
                (LOCK_FILE, StoredEntryKind::Regular),
                (LEGACY_BROKER_DIRECTORY, StoredEntryKind::Directory),
            ],
        )?;

        remove_validated_entries(short_broker_entries)?;
        if short_broker_exists {
            std::fs::remove_dir(&short_broker_directory).map_err(|error| {
                format!("could not remove uninitialized broker bridge: {error}")
            })?;
        }
        File::open(self.root.join(BROKER_BRIDGES_DIRECTORY))
            .and_then(|parent| parent.sync_all())
            .map_err(|error| {
                format!("could not sync uninitialized broker bridge deletion: {error}")
            })?;

        remove_validated_entries(legacy_broker_entries)?;
        if legacy_broker_exists {
            std::fs::remove_dir(&legacy_broker_directory).map_err(|error| {
                format!("could not remove uninitialized legacy broker bridge: {error}")
            })?;
        }
        File::open(&runtime_directory)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| {
                format!("could not sync uninitialized legacy broker deletion: {error}")
            })?;

        remove_validated_manifest_temporaries(&runtime_entries)?;
        remove_validated_named_entry(&runtime_entries, LOCK_FILE)?;
        std::fs::remove_dir(&runtime_directory).map_err(|error| {
            format!("could not remove uninitialized runtime directory: {error}")
        })?;
        File::open(self.root.join(RUNTIMES_DIRECTORY))
            .and_then(|parent| parent.sync_all())
            .map_err(|error| format!("could not sync uninitialized runtime deletion: {error}"))?;
        drop(lock);
        Ok(())
    }

    pub fn remove_runtime_state(
        &self,
        lock: RuntimeLock,
        manifest: &RuntimeManifest,
    ) -> Result<(), String> {
        self.validate_lock(&lock, &manifest.runtime_id)?;
        manifest.validate_for_maintenance()?;
        if manifest.lifecycle != RuntimeLifecycleState::Deleting {
            return Err(
                "runtime state may be removed only from the deleting lifecycle".to_string(),
            );
        }
        let runtime_directory = self.runtime_directory(&manifest.runtime_id);
        validate_existing_private_directory(&runtime_directory)?;

        let short_broker_directory = self.broker_directory_path(&manifest.runtime_id);
        let short_broker_exists = validate_optional_private_directory(&short_broker_directory)?;
        let short_broker_entries = if short_broker_exists {
            validate_broker_entries(&short_broker_directory, BROKER_SOCKET_FILE)?
        } else {
            Vec::new()
        };

        let legacy_broker_directory = self.legacy_broker_directory_path(&manifest.runtime_id);
        let legacy_broker_exists = validate_optional_private_directory(&legacy_broker_directory)?;
        if legacy_broker_exists
            && manifest.layout_profile != RuntimeLayoutProfile::LegacyRuntimeLocalV2
        {
            return Err(
                "non-legacy runtime state contains a legacy broker directory; refusing deletion"
                    .to_string(),
            );
        }
        let legacy_broker_entries = if legacy_broker_exists {
            validate_broker_entries(&legacy_broker_directory, LEGACY_BROKER_SOCKET_FILE)?
        } else {
            Vec::new()
        };

        let mut allowed_runtime_entries = vec![
            (MANIFEST_FILE, StoredEntryKind::Regular),
            (LOCK_FILE, StoredEntryKind::Regular),
            ("egress-audit.jsonl", StoredEntryKind::Regular),
        ];
        if manifest.layout_profile == RuntimeLayoutProfile::LegacyRuntimeLocalV2 {
            allowed_runtime_entries.push((LEGACY_BROKER_DIRECTORY, StoredEntryKind::Directory));
        }
        let runtime_entries = validate_known_entries(&runtime_directory, &allowed_runtime_entries)?;

        remove_validated_entries(short_broker_entries)?;
        if short_broker_exists {
            std::fs::remove_dir(&short_broker_directory)
                .map_err(|error| format!("could not remove runtime broker bridge: {error}"))?;
        }
        File::open(self.root.join(BROKER_BRIDGES_DIRECTORY))
            .and_then(|parent| parent.sync_all())
            .map_err(|error| format!("could not sync runtime broker bridge deletion: {error}"))?;
        remove_validated_entries(legacy_broker_entries)?;
        if legacy_broker_exists {
            std::fs::remove_dir(&legacy_broker_directory).map_err(|error| {
                format!("could not remove legacy runtime broker bridge: {error}")
            })?;
        }
        File::open(&runtime_directory)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("could not sync legacy runtime broker deletion: {error}"))?;
        remove_validated_named_entry(&runtime_entries, "egress-audit.jsonl")?;
        remove_validated_manifest_temporaries(&runtime_entries)?;
        remove_validated_named_entry(&runtime_entries, LOCK_FILE)?;
        remove_validated_named_entry(&runtime_entries, MANIFEST_FILE)?;
        std::fs::remove_dir(&runtime_directory)
            .map_err(|error| format!("could not remove runtime state directory: {error}"))?;
        File::open(self.root.join(RUNTIMES_DIRECTORY))
            .and_then(|parent| parent.sync_all())
            .map_err(|error| format!("could not sync runtime-store deletion: {error}"))?;
        drop(lock);
        Ok(())
    }

    pub fn list_runtime_ids(&self) -> Result<Vec<String>, String> {
        let directory = self.root.join(RUNTIMES_DIRECTORY);
        validate_existing_private_directory(&directory)?;
        let mut ids = Vec::new();
        for entry in std::fs::read_dir(directory)
            .map_err(|error| format!("could not list Python runtimes: {error}"))?
        {
            let entry =
                entry.map_err(|error| format!("could not inspect runtime entry: {error}"))?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| "runtime directory name is not UTF-8".to_string())?;
            let metadata = entry
                .path()
                .symlink_metadata()
                .map_err(|error| format!("could not inspect runtime directory: {error}"))?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(format!(
                    "unexpected non-directory runtime store entry {name:?}"
                ));
            }
            validate_uuid(&name, "runtime directory name")?;
            ids.push(name);
        }
        ids.sort();
        Ok(ids)
    }

    pub fn scan_runtime_ids_for_maintenance(&self) -> Result<RuntimeMaintenanceScan, String> {
        let directory = self.root.join(RUNTIMES_DIRECTORY);
        validate_existing_private_directory(&directory)?;
        let mut scan = RuntimeMaintenanceScan::default();
        for entry in std::fs::read_dir(directory)
            .map_err(|error| format!("could not scan Python runtimes: {error}"))?
        {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    scan.diagnostics
                        .push(format!("could not inspect a runtime store entry: {error}"));
                    continue;
                }
            };
            let display_name = entry.file_name().to_string_lossy().into_owned();
            let name = match entry.file_name().into_string() {
                Ok(name) => name,
                Err(_) => {
                    scan.diagnostics.push(format!(
                        "runtime store entry {display_name:?} is not valid UTF-8"
                    ));
                    continue;
                }
            };
            let metadata = match entry.path().symlink_metadata() {
                Ok(metadata) => metadata,
                Err(error) => {
                    scan.diagnostics.push(format!(
                        "could not inspect runtime store entry {name:?}: {error}"
                    ));
                    continue;
                }
            };
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                scan.diagnostics.push(format!(
                    "runtime store entry {name:?} is not a real directory"
                ));
                continue;
            }
            if let Err(error) = validate_uuid(&name, "runtime directory name") {
                scan.diagnostics
                    .push(format!("runtime store entry {name:?} is invalid: {error}"));
                continue;
            }
            scan.runtime_ids.push(name);
        }
        scan.runtime_ids.sort();
        Ok(scan)
    }

    pub(super) fn broker_directory_path(&self, runtime_id: &str) -> PathBuf {
        self.root.join(BROKER_BRIDGES_DIRECTORY).join(runtime_id)
    }

    pub(super) fn legacy_broker_directory_path(&self, runtime_id: &str) -> PathBuf {
        self.runtime_directory(runtime_id)
            .join(LEGACY_BROKER_DIRECTORY)
    }

    pub(super) fn runtime_directory(&self, runtime_id: &str) -> PathBuf {
        self.root.join(RUNTIMES_DIRECTORY).join(runtime_id)
    }

    fn validate_lock(&self, lock: &RuntimeLock, runtime_id: &str) -> Result<(), String> {
        if lock.runtime_id != runtime_id {
            return Err("runtime lock does not cover the requested manifest".to_string());
        }
        let result = unsafe { libc::flock(lock.file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result != 0 {
            return Err(format!(
                "runtime lock is no longer held: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum StoredEntryKind {
    Regular,
    Directory,
    ManifestTemporary,
}

fn clear_stale_broker_artifacts_in(directory: &Path, socket_file: &str) -> Result<bool, String> {
    let entries = validate_broker_entries(directory, socket_file)?;
    let mut removed_socket = false;
    let mut removed_any = false;
    for (name, path) in entries {
        if name == socket_file || is_broker_capability_temporary_name(&name) {
            std::fs::remove_file(&path).map_err(|error| {
                format!("could not remove stale broker artifact {name:?}: {error}")
            })?;
            removed_socket |= name == socket_file;
            removed_any = true;
        }
    }
    if removed_any {
        File::open(directory)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("could not sync stale broker artifact removal: {error}"))?;
    }
    Ok(removed_socket)
}

fn validate_broker_entries(
    directory: &Path,
    socket_file: &str,
) -> Result<Vec<(String, PathBuf)>, String> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(directory)
        .map_err(|error| format!("could not inspect broker bridge directory: {error}"))?
    {
        let entry =
            entry.map_err(|error| format!("could not inspect broker bridge entry: {error}"))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "broker bridge entry name is not UTF-8".to_string())?;
        let path = entry.path();
        let metadata = path
            .symlink_metadata()
            .map_err(|error| format!("could not inspect broker bridge entry {name:?}: {error}"))?;
        let common_safe = !metadata.file_type().is_symlink()
            && metadata.uid() == rustix::process::geteuid().as_raw()
            && metadata.nlink() == 1;
        let safe = if name == BROKER_CAPABILITY_FILE {
            common_safe
                && metadata.is_file()
                && metadata.permissions().mode() & 0o777 == 0
                && metadata.len() == 64
        } else if name == socket_file {
            // bind(2) creates the socket inode before its mode can be tightened.
            // The exact name remains safe to remove because its validated parent
            // is private, owner-controlled, and inaccessible to the container for writes.
            common_safe && metadata.file_type().is_socket()
        } else if is_broker_capability_temporary_name(&name) {
            common_safe
                && metadata.is_file()
                && metadata.permissions().mode() & 0o777 == 0
                && metadata.len() <= 64
        } else {
            return Err(format!(
                "unexpected broker bridge entry {name:?}; refusing cleanup"
            ));
        };
        if !safe {
            return Err(format!(
                "unsafe broker bridge entry {name:?}; refusing cleanup"
            ));
        }
        entries.push((name, path));
    }
    Ok(entries)
}

fn is_broker_capability_temporary_name(name: &str) -> bool {
    is_atomic_temporary_name(name, BROKER_CAPABILITY_TEMP_PREFIX)
}

fn is_atomic_temporary_name(name: &str, prefix: &str) -> bool {
    let Some(suffix) = name.strip_prefix(prefix) else {
        return false;
    };
    let mut fields = suffix.split('-');
    let valid_field =
        |field: &str| !field.is_empty() && field.bytes().all(|byte| byte.is_ascii_digit());
    matches!(
        (fields.next(), fields.next(), fields.next(), fields.next()),
        (Some(pid), Some(nanos), Some(counter), None)
            if valid_field(pid) && valid_field(nanos) && valid_field(counter)
    )
}

fn validate_optional_private_directory(path: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {
            validate_existing_private_directory(path)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!(
            "could not inspect optional private directory {}: {error}",
            path.display()
        )),
    }
}

fn validate_known_entries(
    directory: &Path,
    allowed: &[(&str, StoredEntryKind)],
) -> Result<Vec<(String, PathBuf)>, String> {
    let allowed = allowed.iter().copied().collect::<BTreeMap<_, _>>();
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(directory)
        .map_err(|error| format!("could not inspect runtime state directory: {error}"))?
    {
        let entry =
            entry.map_err(|error| format!("could not inspect runtime state entry: {error}"))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "runtime state entry name is not UTF-8".to_string())?;
        let expected = if let Some(expected) = allowed.get(name.as_str()).copied() {
            expected
        } else if is_atomic_temporary_name(&name, MANIFEST_TEMP_PREFIX) {
            StoredEntryKind::ManifestTemporary
        } else {
            return Err(format!(
                "unexpected runtime state entry {name:?}; refusing deletion"
            ));
        };
        let path = entry.path();
        let metadata = path
            .symlink_metadata()
            .map_err(|error| format!("could not inspect runtime state entry {name:?}: {error}"))?;
        if metadata.file_type().is_symlink()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || match expected {
                StoredEntryKind::Regular => metadata.nlink() != 1 || !metadata.is_file(),
                StoredEntryKind::Directory => !metadata.is_dir(),
                StoredEntryKind::ManifestTemporary => {
                    metadata.nlink() != 1
                        || !metadata.is_file()
                        || metadata.permissions().mode() & 0o177 != 0
                        || metadata.len() > 1024 * 1024
                }
            }
        {
            return Err(format!(
                "unsafe runtime state entry {name:?}; refusing deletion"
            ));
        }
        entries.push((name, path));
    }
    Ok(entries)
}

fn remove_validated_manifest_temporaries(entries: &[(String, PathBuf)]) -> Result<(), String> {
    for (name, path) in entries
        .iter()
        .filter(|(name, _)| is_atomic_temporary_name(name, MANIFEST_TEMP_PREFIX))
    {
        std::fs::remove_file(path).map_err(|error| {
            format!("could not remove runtime manifest temporary {name:?}: {error}")
        })?;
    }
    Ok(())
}

fn remove_validated_named_entry(entries: &[(String, PathBuf)], name: &str) -> Result<(), String> {
    let Some((_, path)) = entries.iter().find(|(entry, _)| entry == name) else {
        return Ok(());
    };
    std::fs::remove_file(path)
        .map_err(|error| format!("could not remove runtime state entry {name:?}: {error}"))
}

fn remove_validated_entries(entries: Vec<(String, PathBuf)>) -> Result<(), String> {
    for (name, path) in entries {
        std::fs::remove_file(path)
            .map_err(|error| format!("could not remove runtime state entry {name:?}: {error}"))?;
    }
    Ok(())
}
