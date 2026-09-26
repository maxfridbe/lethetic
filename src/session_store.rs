use crate::app::{SessionDirectoryBinding, SessionWorkspaceBinding};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

const SESSION_LOCKS_DIRECTORY: &str = "session-locks";
const SESSION_LIFECYCLE_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum SessionLifecycleState {
    Creating,
    Active,
    Deleting,
    Deleted,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct SessionLifecycleRecord {
    schema_version: u32,
    session_id: String,
    directory_name: String,
    binding: Option<SessionDirectoryBinding>,
    #[serde(default)]
    python_runtime_id: Option<String>,
    #[serde(default)]
    managed_python_workspace: Option<SessionWorkspaceBinding>,
    state: SessionLifecycleState,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionDeletionResources {
    pub python_runtime_id: Option<String>,
    pub managed_python_workspace: Option<SessionWorkspaceBinding>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionRemovalOutcome {
    pub durability_warnings: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionPathDisposition {
    Resumable,
    Creating,
    CleanupOnly,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionPathRegistration {
    pub session_id: String,
    pub disposition: SessionPathDisposition,
    pub binding: Option<SessionDirectoryBinding>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionPathClassification {
    pub canonical_path: PathBuf,
    pub registration: Option<SessionPathRegistration>,
}

pub struct SessionStore {
    sessions_root: PathBuf,
    locks_root: PathBuf,
}

pub struct SessionPathLock {
    canonical_path: PathBuf,
    device: u64,
    inode: u64,
    locks_root: PathBuf,
    _path_lock: File,
}

pub struct SessionLease {
    canonical_path: PathBuf,
    session_id: String,
    binding: SessionDirectoryBinding,
    _path_lock: File,
    _identity_lock: File,
}

impl std::fmt::Debug for SessionLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SessionLease")
            .field("canonical_path", &self.canonical_path)
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

impl SessionStore {
    pub fn open(project_root: &Path) -> Result<Self, String> {
        Self::open_at(project_root, &crate::platform::lethetic_state_dir())
    }

    fn open_at(project_root: &Path, state_root: &Path) -> Result<Self, String> {
        let project_root = project_root
            .canonicalize()
            .map_err(|error| format!("could not canonicalize project root: {error}"))?;
        validate_owned_directory(&project_root, false)?;
        let lethetic = project_root.join(".lethetic");
        ensure_owned_private_directory(&lethetic)?;
        let sessions_root = lethetic.join("sessions");
        ensure_owned_private_directory(&sessions_root)?;

        ensure_owned_private_directory(state_root)?;
        let locks_root = state_root.join(SESSION_LOCKS_DIRECTORY);
        ensure_owned_private_directory(&locks_root)?;
        Ok(Self {
            sessions_root,
            locks_root,
        })
    }

    pub fn sessions_root(&self) -> &Path {
        &self.sessions_root
    }

    pub fn classify_session_path(&self, path: &Path) -> Result<SessionPathClassification, String> {
        let link = std::fs::symlink_metadata(path)
            .map_err(|error| format!("could not inspect registered session path: {error}"))?;
        if link.file_type().is_symlink() || !link.is_dir() {
            return Err("registered session path must be a real directory".to_string());
        }
        let canonical = path
            .canonicalize()
            .map_err(|error| format!("could not canonicalize registered session path: {error}"))?;
        if canonical.parent() != Some(self.sessions_root.as_path()) {
            return Err("registered session path belongs to a different project root".to_string());
        }
        let directory_name = canonical
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "registered session directory name must be UTF-8".to_string())?;
        validate_session_directory_name(directory_name)?;
        validate_owned_directory(&canonical, false)?;

        let lifecycle_records = list_lifecycle_records(&self.locks_root)?;
        let identity_bindings = list_identity_bindings(&self.locks_root)?;
        let mut matching_ids = std::collections::BTreeSet::new();
        for record in &lifecycle_records {
            let record_path = record
                .binding
                .as_ref()
                .map(|binding| binding.canonical_path.clone())
                .unwrap_or_else(|| self.sessions_root.join(&record.directory_name));
            if record_path == canonical {
                matching_ids.insert(record.session_id.clone());
            }
        }
        for (session_id, binding) in &identity_bindings {
            if binding.canonical_path == canonical {
                binding.validate_stored(session_id)?;
                matching_ids.insert(session_id.clone());
            }
        }
        if matching_ids.len() > 1 {
            return Err("registered session path is bound to multiple session IDs".to_string());
        }
        let Some(session_id) = matching_ids.into_iter().next() else {
            return Ok(SessionPathClassification {
                canonical_path: canonical,
                registration: None,
            });
        };

        let lifecycle = lifecycle_records
            .iter()
            .find(|record| record.session_id == session_id);
        let identity_binding = identity_bindings
            .iter()
            .find(|(registered_id, _)| registered_id == &session_id)
            .map(|(_, binding)| binding);
        let lifecycle_binding = lifecycle.and_then(|record| record.binding.as_ref());

        if let Some(record) = lifecycle {
            let record_path = record
                .binding
                .as_ref()
                .map(|binding| binding.canonical_path.clone())
                .unwrap_or_else(|| self.sessions_root.join(&record.directory_name));
            if record_path != canonical {
                return Err(
                    "session lifecycle and identity registries target different paths".to_string(),
                );
            }
        }
        if let Some(binding) = identity_binding {
            if binding.canonical_path != canonical {
                return Err(
                    "session lifecycle and identity registries target different paths".to_string(),
                );
            }
            binding.verify_current(&session_id)?;
        }
        if let Some(binding) = lifecycle_binding {
            binding.verify_current(&session_id)?;
        }
        if let (Some(identity), Some(lifecycle)) = (identity_binding, lifecycle_binding)
            && identity != lifecycle
        {
            return Err("session lifecycle and identity bindings disagree".to_string());
        }

        let disposition = match lifecycle.map(|record| record.state) {
            Some(SessionLifecycleState::Creating) => SessionPathDisposition::Creating,
            Some(SessionLifecycleState::Deleting | SessionLifecycleState::Deleted) => {
                SessionPathDisposition::CleanupOnly
            }
            Some(SessionLifecycleState::Active) | None => SessionPathDisposition::Resumable,
        };
        Ok(SessionPathClassification {
            canonical_path: canonical,
            registration: Some(SessionPathRegistration {
                session_id,
                disposition,
                binding: identity_binding.or(lifecycle_binding).cloned(),
            }),
        })
    }

    pub fn registered_session_id_for_path(&self, path: &Path) -> Result<Option<String>, String> {
        let classification = self.classify_session_path(path)?;
        Ok(classification.registration.and_then(|registration| {
            (registration.disposition != SessionPathDisposition::CleanupOnly)
                .then_some(registration.session_id)
        }))
    }

    pub fn create_locked_session(
        &self,
        directory_name: &str,
        session_id: &str,
    ) -> Result<(PathBuf, SessionLease), String> {
        validate_session_id(session_id)?;
        validate_session_directory_name(directory_name)?;
        let identity_lock = try_lock_file(
            &self.locks_root,
            &format!("identity-{session_id}.lock"),
        )?
        .ok_or_else(|| {
            "chat session identity is already active in another Lethetic process".to_string()
        })?;
        if read_identity_binding(&self.locks_root, session_id)?.is_some()
            || read_lifecycle_record(&self.locks_root, session_id)?.is_some()
        {
            return Err("chat session identity is already durably registered".to_string());
        }
        let path = self.sessions_root.join(directory_name);
        match std::fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => return Err("new session directory already exists".to_string()),
            Err(error) => return Err(format!("could not inspect new session path: {error}")),
        }
        let mut lifecycle = SessionLifecycleRecord {
            schema_version: SESSION_LIFECYCLE_SCHEMA_VERSION,
            session_id: session_id.to_string(),
            directory_name: directory_name.to_string(),
            binding: None,
            python_runtime_id: None,
            managed_python_workspace: None,
            state: SessionLifecycleState::Creating,
        };
        save_lifecycle_record(&self.locks_root, &lifecycle)?;

        let mut builder = std::fs::DirBuilder::new();
        builder.mode(0o700);
        builder
            .create(&path)
            .map_err(|error| format!("could not create session directory: {error}"))?;
        File::open(&self.sessions_root)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("could not sync new session directory: {error}"))?;
        let path_lock = self
            .try_lock_session_path(&path)?
            .ok_or_else(|| "new session path is unexpectedly locked".to_string())?;
        let lease = path_lock.bind_identity_with_lock(session_id, identity_lock)?;
        lifecycle.binding = Some(lease.binding().clone());
        save_lifecycle_record(&self.locks_root, &lifecycle)?;
        Ok((path, lease))
    }

    pub fn lock_registered_session(&self, session_id: &str) -> Result<SessionLease, String> {
        validate_session_id(session_id)?;
        let lifecycle = read_lifecycle_record(&self.locks_root, session_id)?;
        if lifecycle.as_ref().is_some_and(|record| {
            matches!(
                record.state,
                SessionLifecycleState::Deleting | SessionLifecycleState::Deleted
            )
        }) {
            return Err("session is durably marked for deletion and cannot be resumed".to_string());
        }
        let registered = read_identity_binding(&self.locks_root, session_id)?;
        let path = registered
            .as_ref()
            .map(|binding| binding.canonical_path.clone())
            .or_else(|| {
                lifecycle
                    .as_ref()
                    .and_then(|record| record.binding.as_ref())
                    .map(|binding| binding.canonical_path.clone())
            })
            .or_else(|| {
                lifecycle
                    .as_ref()
                    .map(|record| self.sessions_root.join(&record.directory_name))
            })
            .ok_or_else(|| "session ID has no durable directory registry entry".to_string())?;
        if path.parent() != Some(self.sessions_root.as_path()) {
            return Err("registered session belongs to a different project root".to_string());
        }
        if let Some(binding) = registered.as_ref().or_else(|| {
            lifecycle
                .as_ref()
                .and_then(|record| record.binding.as_ref())
        }) {
            binding.verify_current(session_id)?;
        }
        let path_lock = self
            .try_lock_session_path(&path)?
            .ok_or_else(|| "session is already active in another Lethetic process".to_string())?;
        let lease = path_lock.bind_identity(session_id)?;
        if let Some(binding) = registered.as_ref().or_else(|| {
            lifecycle
                .as_ref()
                .and_then(|record| record.binding.as_ref())
        }) && lease.binding() != binding
        {
            return Err("session identity registry changed while locking".to_string());
        }

        if lifecycle
            .as_ref()
            .is_some_and(|record| record.state == SessionLifecycleState::Creating)
            && crate::platform::read_file_nofollow(
                lease.canonical_path(),
                &[],
                "session_state.json",
            )
            .map_err(|error| format!("could not inspect creating session state: {error}"))?
            .is_none()
        {
            let outcome = self.remove_locked_session(&lease)?;
            let warning = if outcome.durability_warnings.is_empty() {
                String::new()
            } else {
                format!(" ({})", outcome.durability_warnings.join("; "))
            };
            return Err(format!(
                "incomplete session creation was cleaned up{warning}"
            ));
        }
        self.commit_locked_session_creation(&lease)?;
        Ok(lease)
    }

    pub fn try_lock_session_path(&self, path: &Path) -> Result<Option<SessionPathLock>, String> {
        let link_metadata = std::fs::symlink_metadata(path)
            .map_err(|error| format!("could not inspect session directory: {error}"))?;
        if link_metadata.file_type().is_symlink() || !link_metadata.is_dir() {
            return Err("session path must be a real directory".to_string());
        }
        let canonical_path = path
            .canonicalize()
            .map_err(|error| format!("could not canonicalize session directory: {error}"))?;
        if canonical_path.parent() != Some(self.sessions_root.as_path()) {
            return Err(
                "session directory is outside the current project session root".to_string(),
            );
        }
        let name = canonical_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "session directory name must be UTF-8".to_string())?;
        validate_session_directory_name(name)?;
        let metadata = validate_owned_directory(&canonical_path, false)?;
        let key = path_lock_key(&canonical_path);
        let Some(path_lock) = try_lock_file(&self.locks_root, &format!("path-{key}.lock"))? else {
            return Ok(None);
        };
        let current = validate_owned_directory(&canonical_path, false)?;
        if current.dev() != metadata.dev() || current.ino() != metadata.ino() {
            return Err("session directory changed while its path lock was acquired".to_string());
        }
        std::fs::set_permissions(&canonical_path, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("could not secure session directory: {error}"))?;
        Ok(Some(SessionPathLock {
            canonical_path,
            device: metadata.dev(),
            inode: metadata.ino(),
            locks_root: self.locks_root.clone(),
            _path_lock: path_lock,
        }))
    }

    pub fn lock_session_for_deletion(
        &self,
        path: &Path,
        session_id: &str,
    ) -> Result<SessionLease, String> {
        validate_session_id(session_id)?;
        let lifecycle = read_lifecycle_record(&self.locks_root, session_id)?;
        if let Some(record) = &lifecycle {
            if record.directory_name
                != path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
                || record
                    .binding
                    .as_ref()
                    .is_some_and(|binding| binding.canonical_path != path)
            {
                return Err("session deletion record targets a different path".to_string());
            }
        }
        let path_lock = self
            .try_lock_session_path(path)?
            .ok_or_else(|| "session is already active in another Lethetic process".to_string())?;
        let identity_lock =
            try_lock_file(&self.locks_root, &format!("identity-{session_id}.lock"))?.ok_or_else(
                || "session identity is already active in another Lethetic process".to_string(),
            )?;
        let lease = path_lock.bind_identity_with_lock(session_id, identity_lock)?;
        if lifecycle
            .as_ref()
            .and_then(|record| record.binding.as_ref())
            .is_some_and(|binding| binding != lease.binding())
        {
            return Err("session deletion binding changed".to_string());
        }
        Ok(lease)
    }

    pub fn begin_locked_session_deletion(
        &self,
        lease: &SessionLease,
        python_runtime_id: Option<&str>,
        managed_python_workspace: Option<&SessionWorkspaceBinding>,
    ) -> Result<(), String> {
        if let Some(runtime_id) = python_runtime_id {
            validate_session_id(runtime_id)?;
            if managed_python_workspace.is_none() {
                return Err("session deletion runtime is missing its workspace binding".to_string());
            }
        }
        if let Some(workspace) = managed_python_workspace {
            workspace.validate_stored()?;
        }
        lease.verify(lease.canonical_path(), lease.session_id(), lease.binding())?;
        let directory_name = lease
            .canonical_path()
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "session directory name must be UTF-8".to_string())?;
        let mut lifecycle = read_lifecycle_record(&self.locks_root, lease.session_id())?.unwrap_or(
            SessionLifecycleRecord {
                schema_version: SESSION_LIFECYCLE_SCHEMA_VERSION,
                session_id: lease.session_id().to_string(),
                directory_name: directory_name.to_string(),
                binding: Some(lease.binding().clone()),
                python_runtime_id: None,
                managed_python_workspace: None,
                state: SessionLifecycleState::Active,
            },
        );
        if lifecycle.directory_name != directory_name
            || lifecycle
                .binding
                .as_ref()
                .is_some_and(|binding| binding != lease.binding())
        {
            return Err("session deletion record does not match its exact lease".to_string());
        }
        if lifecycle
            .python_runtime_id
            .as_deref()
            .is_some_and(|stored| Some(stored) != python_runtime_id)
            || lifecycle
                .managed_python_workspace
                .as_ref()
                .is_some_and(|stored| Some(stored) != managed_python_workspace)
        {
            return Err("session deletion resources changed after tombstoning".to_string());
        }
        lifecycle.binding = Some(lease.binding().clone());
        if lifecycle.python_runtime_id.is_none() {
            lifecycle.python_runtime_id = python_runtime_id.map(str::to_string);
        }
        if lifecycle.managed_python_workspace.is_none() {
            lifecycle.managed_python_workspace = managed_python_workspace.cloned();
        }
        lifecycle.state = SessionLifecycleState::Deleting;
        save_lifecycle_record(&self.locks_root, &lifecycle)
    }

    pub fn deletion_resources(
        &self,
        lease: &SessionLease,
    ) -> Result<SessionDeletionResources, String> {
        let lifecycle = read_lifecycle_record(&self.locks_root, lease.session_id())?
            .ok_or_else(|| "session has no deletion tombstone".to_string())?;
        if !matches!(
            lifecycle.state,
            SessionLifecycleState::Deleting | SessionLifecycleState::Deleted
        ) || lifecycle.binding.as_ref() != Some(lease.binding())
        {
            return Err("session deletion tombstone does not match its lease".to_string());
        }
        Ok(SessionDeletionResources {
            python_runtime_id: lifecycle.python_runtime_id,
            managed_python_workspace: lifecycle.managed_python_workspace,
        })
    }

    pub fn commit_locked_session_creation(&self, lease: &SessionLease) -> Result<(), String> {
        lease.verify(&lease.canonical_path, &lease.session_id, &lease.binding)?;
        let path = lease
            .canonical_path
            .to_str()
            .ok_or_else(|| "session path must be UTF-8".to_string())?;
        let state = crate::app::SessionState::load_checked(path)?;
        if state.session_id.as_deref() != Some(lease.session_id())
            || state.session_directory_binding.as_ref() != Some(lease.binding())
        {
            return Err("first session state does not match its durable lease".to_string());
        }
        let directory_name = lease
            .canonical_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "session directory name must be UTF-8".to_string())?;
        let mut lifecycle = read_lifecycle_record(&self.locks_root, lease.session_id())?.unwrap_or(
            SessionLifecycleRecord {
                schema_version: SESSION_LIFECYCLE_SCHEMA_VERSION,
                session_id: lease.session_id().to_string(),
                directory_name: directory_name.to_string(),
                binding: Some(lease.binding().clone()),
                python_runtime_id: None,
                managed_python_workspace: None,
                state: SessionLifecycleState::Creating,
            },
        );
        validate_lifecycle_record(&lifecycle)?;
        if lifecycle.directory_name != directory_name
            || lifecycle
                .binding
                .as_ref()
                .is_some_and(|binding| binding != lease.binding())
        {
            return Err("session lifecycle record does not match its active lease".to_string());
        }
        match lifecycle.state {
            SessionLifecycleState::Creating => {
                lifecycle.binding = Some(lease.binding().clone());
                lifecycle.state = SessionLifecycleState::Active;
                save_lifecycle_record(&self.locks_root, &lifecycle)
            }
            SessionLifecycleState::Active => Ok(()),
            SessionLifecycleState::Deleting | SessionLifecycleState::Deleted => {
                Err("session creation cannot commit after deletion began".to_string())
            }
        }
    }

    pub fn lock_recoverable_session_path(&self, path: &Path) -> Result<SessionLease, String> {
        let canonical = path
            .canonicalize()
            .map_err(|error| format!("could not canonicalize recoverable session: {error}"))?;
        if canonical.parent() != Some(self.sessions_root.as_path()) {
            return Err("recoverable session is outside the current session root".to_string());
        }
        let name = canonical
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "recoverable session directory name must be UTF-8".to_string())?;
        validate_session_directory_name(name)?;
        let records = list_lifecycle_records(&self.locks_root)?
            .into_iter()
            .filter(|record| {
                record.directory_name == name
                    && matches!(
                        record.state,
                        SessionLifecycleState::Creating
                            | SessionLifecycleState::Deleting
                            | SessionLifecycleState::Deleted
                    )
            })
            .collect::<Vec<_>>();
        if records.len() != 1 {
            return Err(
                "session path has no unique recoverable creation/deletion tombstone".to_string(),
            );
        }
        let mut lifecycle = records.into_iter().next().expect("one record was checked");
        if lifecycle
            .binding
            .as_ref()
            .is_some_and(|binding| binding.canonical_path != canonical)
        {
            return Err("recoverable session tombstone targets a different path".to_string());
        }
        let path_lock = self.try_lock_session_path(&canonical)?.ok_or_else(|| {
            "recoverable session is active in another Lethetic process".to_string()
        })?;
        let identity_lock = try_lock_file(
            &self.locks_root,
            &format!("identity-{}.lock", lifecycle.session_id),
        )?
        .ok_or_else(|| "recoverable session identity is active elsewhere".to_string())?;
        let lease = path_lock.bind_identity_with_lock(&lifecycle.session_id, identity_lock)?;
        if lifecycle
            .binding
            .as_ref()
            .is_some_and(|binding| binding != lease.binding())
        {
            return Err("recoverable session binding changed".to_string());
        }
        if lifecycle.binding.is_none() {
            lifecycle.binding = Some(lease.binding().clone());
            save_lifecycle_record(&self.locks_root, &lifecycle)?;
        }
        Ok(lease)
    }

    pub fn remove_locked_session(
        &self,
        lease: &SessionLease,
    ) -> Result<SessionRemovalOutcome, String> {
        if lease.canonical_path.parent() != Some(self.sessions_root.as_path()) {
            return Err("locked session deletion target is outside the session root".to_string());
        }
        lease.verify(&lease.canonical_path, &lease.session_id, &lease.binding)?;
        let directory_name = lease
            .canonical_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "session directory name must be UTF-8".to_string())?;
        let mut lifecycle = read_lifecycle_record(&self.locks_root, lease.session_id())?.unwrap_or(
            SessionLifecycleRecord {
                schema_version: SESSION_LIFECYCLE_SCHEMA_VERSION,
                session_id: lease.session_id().to_string(),
                directory_name: directory_name.to_string(),
                binding: Some(lease.binding().clone()),
                python_runtime_id: None,
                managed_python_workspace: None,
                state: SessionLifecycleState::Active,
            },
        );
        validate_lifecycle_record(&lifecycle)?;
        if lifecycle.directory_name != directory_name
            || lifecycle
                .binding
                .as_ref()
                .is_some_and(|binding| binding != lease.binding())
        {
            return Err("session deletion tombstone does not match its exact lease".to_string());
        }
        lifecycle.binding = Some(lease.binding().clone());
        lifecycle.state = SessionLifecycleState::Deleting;
        save_lifecycle_record(&self.locks_root, &lifecycle)?;

        std::fs::set_permissions(
            &lease.canonical_path,
            std::fs::Permissions::from_mode(0o700),
        )
        .map_err(|error| format!("could not normalize locked session permissions: {error}"))?;
        let state_name = std::ffi::OsStr::new("session_state.json");
        let entries = std::fs::read_dir(&lease.canonical_path)
            .map_err(|error| format!("could not enumerate locked session directory: {error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("could not inspect locked session entry: {error}"))?;
        for entry in entries {
            if entry.file_name() == state_name {
                continue;
            }
            remove_session_entry(&entry.path())?;
        }
        let state_path = lease.canonical_path.join(state_name);
        match std::fs::symlink_metadata(&state_path) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                return Err("session state deletion marker is unexpectedly a directory".to_string());
            }
            Ok(_) => remove_session_entry(&state_path)
                .map_err(|error| format!("could not remove session state last: {error}"))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("could not inspect session state marker: {error}")),
        }
        if std::fs::read_dir(&lease.canonical_path)
            .map_err(|error| format!("could not verify emptied session directory: {error}"))?
            .next()
            .is_some()
        {
            return Err("session directory changed while it was being deleted".to_string());
        }
        std::fs::remove_dir(&lease.canonical_path)
            .map_err(|error| format!("could not remove locked session directory: {error}"))?;

        let mut outcome = SessionRemovalOutcome::default();
        lifecycle.state = SessionLifecycleState::Deleted;
        if let Err(error) = save_lifecycle_record(&self.locks_root, &lifecycle) {
            outcome.durability_warnings.push(format!(
                "could not commit deleted session tombstone: {error}"
            ));
        }
        if let Err(error) =
            File::open(&self.sessions_root).and_then(|directory| directory.sync_all())
        {
            outcome.durability_warnings.push(format!(
                "could not sync session directory deletion: {error}"
            ));
        }
        Ok(outcome)
    }
}

impl SessionPathLock {
    pub fn bind_identity(self, session_id: &str) -> Result<SessionLease, String> {
        validate_session_id(session_id)?;
        let directory_name = self
            .canonical_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "session directory name must be UTF-8".to_string())?;
        validate_session_directory_name(directory_name)?;
        if list_identity_bindings(&self.locks_root)?
            .into_iter()
            .any(|(registered_id, binding)| {
                registered_id != session_id && binding.canonical_path == self.canonical_path
            })
        {
            return Err(
                "session path is durably registered to a different chat session identity"
                    .to_string(),
            );
        }
        if list_lifecycle_records(&self.locks_root)?
            .into_iter()
            .any(|record| {
                record.session_id != session_id
                    && (record.directory_name == directory_name
                        || record
                            .binding
                            .as_ref()
                            .is_some_and(|binding| binding.canonical_path == self.canonical_path))
            })
        {
            return Err(
                "session path is durably registered to a different chat session identity"
                    .to_string(),
            );
        }
        if let Some(lifecycle) = read_lifecycle_record(&self.locks_root, session_id)? {
            if matches!(
                lifecycle.state,
                SessionLifecycleState::Deleting | SessionLifecycleState::Deleted
            ) {
                return Err("session lifecycle forbids resuming a deleted session".to_string());
            }
            if lifecycle.directory_name != directory_name
                || lifecycle
                    .binding
                    .as_ref()
                    .is_some_and(|binding| binding.canonical_path != self.canonical_path)
            {
                return Err("session lifecycle record targets a different directory".to_string());
            }
        }
        let identity_lock = try_lock_file(
            &self.locks_root,
            &format!("identity-{session_id}.lock"),
        )?
        .ok_or_else(|| {
            "chat session identity is already active in another Lethetic process".to_string()
        })?;
        self.bind_identity_with_lock(session_id, identity_lock)
    }

    fn bind_identity_with_lock(
        self,
        session_id: &str,
        identity_lock: File,
    ) -> Result<SessionLease, String> {
        validate_session_id(session_id)?;
        let binding = SessionDirectoryBinding::capture_parts(
            &self.canonical_path,
            self.device,
            self.inode,
            session_id,
        )?;
        binding.verify_current(session_id)?;
        let binding = load_or_create_identity_binding(&self.locks_root, session_id, &binding)?;
        binding.verify_current(session_id)?;
        Ok(SessionLease {
            canonical_path: self.canonical_path,
            session_id: session_id.to_string(),
            binding,
            _path_lock: self._path_lock,
            _identity_lock: identity_lock,
        })
    }
}

impl SessionLease {
    pub fn canonical_path(&self) -> &Path {
        &self.canonical_path
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn binding(&self) -> &SessionDirectoryBinding {
        &self.binding
    }

    pub fn verify_current(&self) -> Result<(), String> {
        self.verify(&self.canonical_path, &self.session_id, &self.binding)
    }

    pub fn verify(
        &self,
        session_path: &Path,
        session_id: &str,
        binding: &SessionDirectoryBinding,
    ) -> Result<(), String> {
        if session_id != self.session_id || binding != &self.binding {
            return Err("active session lease does not match the session state".to_string());
        }
        let canonical = session_path
            .canonicalize()
            .map_err(|error| format!("could not canonicalize active session: {error}"))?;
        if canonical != self.canonical_path {
            return Err("active session path changed after locking".to_string());
        }
        binding.verify_current(session_id)
    }
}

fn remove_session_entry(path: &Path) -> Result<(), String> {
    crate::platform::remove_tree_nofollow_same_mount(path)
        .map_err(|error| format!("could not safely remove session entry: {error}"))
}

fn lifecycle_file_name(session_id: &str) -> String {
    format!("lifecycle-{session_id}.json")
}

fn validate_lifecycle_record(record: &SessionLifecycleRecord) -> Result<(), String> {
    if record.schema_version != SESSION_LIFECYCLE_SCHEMA_VERSION {
        return Err(format!(
            "unsupported session lifecycle schema {}",
            record.schema_version
        ));
    }
    validate_session_id(&record.session_id)?;
    validate_session_directory_name(&record.directory_name)?;
    if let Some(binding) = &record.binding {
        binding.validate_stored(&record.session_id)?;
        if binding
            .canonical_path
            .file_name()
            .and_then(|name| name.to_str())
            != Some(record.directory_name.as_str())
        {
            return Err("session lifecycle binding has a different directory name".to_string());
        }
    } else if record.state != SessionLifecycleState::Creating {
        return Err("committed session lifecycle is missing its directory binding".to_string());
    }
    if let Some(runtime_id) = &record.python_runtime_id {
        validate_session_id(runtime_id)?;
        if record.managed_python_workspace.is_none() {
            return Err("session deletion runtime is missing its workspace binding".to_string());
        }
    }
    if let Some(workspace) = &record.managed_python_workspace {
        workspace.validate_stored()?;
    }
    Ok(())
}

fn read_lifecycle_record(
    locks_root: &Path,
    session_id: &str,
) -> Result<Option<SessionLifecycleRecord>, String> {
    validate_session_id(session_id)?;
    let file_name = lifecycle_file_name(session_id);
    let Some(bytes) = crate::platform::read_file_nofollow(locks_root, &[], &file_name)
        .map_err(|error| format!("could not read session lifecycle record: {error}"))?
    else {
        return Ok(None);
    };
    if bytes.len() > 32 * 1024 {
        return Err("session lifecycle record exceeds its safety limit".to_string());
    }
    let record: SessionLifecycleRecord = serde_json::from_slice(&bytes)
        .map_err(|error| format!("session lifecycle record is invalid: {error}"))?;
    validate_lifecycle_record(&record)?;
    if record.session_id != session_id {
        return Err("session lifecycle filename does not match its identity".to_string());
    }
    Ok(Some(record))
}

fn save_lifecycle_record(locks_root: &Path, record: &SessionLifecycleRecord) -> Result<(), String> {
    validate_lifecycle_record(record)?;
    let bytes = serde_json::to_vec(record)
        .map_err(|error| format!("could not encode session lifecycle record: {error}"))?;
    crate::platform::atomic_write_nofollow(
        locks_root,
        &[],
        &lifecycle_file_name(&record.session_id),
        &bytes,
        0o600,
    )
    .map_err(|error| format!("could not persist session lifecycle record: {error}"))?;
    Ok(())
}

fn list_lifecycle_records(locks_root: &Path) -> Result<Vec<SessionLifecycleRecord>, String> {
    let mut records = Vec::new();
    for entry in std::fs::read_dir(locks_root)
        .map_err(|error| format!("could not list session lifecycle records: {error}"))?
    {
        let entry =
            entry.map_err(|error| format!("could not inspect session lifecycle entry: {error}"))?;
        let name = match entry.file_name().into_string() {
            Ok(name) => name,
            Err(_) => continue,
        };
        let Some(session_id) = name
            .strip_prefix("lifecycle-")
            .and_then(|name| name.strip_suffix(".json"))
        else {
            continue;
        };
        validate_session_id(session_id)?;
        let record = read_lifecycle_record(locks_root, session_id)?
            .ok_or_else(|| "session lifecycle record disappeared while listing".to_string())?;
        records.push(record);
    }
    Ok(records)
}

fn list_identity_bindings(
    locks_root: &Path,
) -> Result<Vec<(String, SessionDirectoryBinding)>, String> {
    let mut bindings = Vec::new();
    for entry in std::fs::read_dir(locks_root)
        .map_err(|error| format!("could not list session identity bindings: {error}"))?
    {
        let entry =
            entry.map_err(|error| format!("could not inspect session identity entry: {error}"))?;
        let name = match entry.file_name().into_string() {
            Ok(name) => name,
            Err(_) => continue,
        };
        let Some(session_id) = name
            .strip_prefix("identity-")
            .and_then(|name| name.strip_suffix(".json"))
        else {
            continue;
        };
        validate_session_id(session_id)?;
        let binding = read_identity_binding(locks_root, session_id)?
            .ok_or_else(|| "session identity binding disappeared while listing".to_string())?;
        bindings.push((session_id.to_string(), binding));
    }
    Ok(bindings)
}

fn read_identity_binding(
    locks_root: &Path,
    session_id: &str,
) -> Result<Option<SessionDirectoryBinding>, String> {
    let file_name = format!("identity-{session_id}.json");
    let Some(bytes) = crate::platform::read_file_nofollow(locks_root, &[], &file_name)
        .map_err(|error| format!("could not read session identity registry: {error}"))?
    else {
        return Ok(None);
    };
    if bytes.len() > 16 * 1024 {
        return Err("session identity registry entry exceeds its safety limit".to_string());
    }
    serde_json::from_slice::<SessionDirectoryBinding>(&bytes)
        .map(Some)
        .map_err(|error| format!("session identity registry entry is invalid: {error}"))
}

fn load_or_create_identity_binding(
    locks_root: &Path,
    session_id: &str,
    candidate: &SessionDirectoryBinding,
) -> Result<SessionDirectoryBinding, String> {
    let file_name = format!("identity-{session_id}.json");
    let binding = match read_identity_binding(locks_root, session_id)? {
        Some(binding) => binding,
        None => {
            let bytes = serde_json::to_vec(candidate)
                .map_err(|error| format!("could not encode session identity registry: {error}"))?;
            crate::platform::atomic_write_nofollow(locks_root, &[], &file_name, &bytes, 0o600)
                .map_err(|error| format!("could not persist session identity registry: {error}"))?;
            candidate.clone()
        }
    };
    if &binding != candidate {
        return Err(
            "chat session identity is permanently bound to a different session directory"
                .to_string(),
        );
    }
    Ok(binding)
}

fn try_lock_file(directory: &Path, name: &str) -> Result<Option<File>, String> {
    let path = directory.join(name);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)
        .map_err(|error| format!("could not open session lock: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("could not inspect session lock: {error}"))?;
    if !metadata.is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.nlink() != 1
    {
        return Err("session lock is not a trusted singly linked regular file".to_string());
    }
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("could not secure session lock: {error}"))?;
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
            return Ok(None);
        }
        return Err(format!("could not acquire session lock: {error}"));
    }
    Ok(Some(file))
}

fn ensure_owned_private_directory(path: &Path) -> Result<(), String> {
    crate::platform::ensure_private_directory_durable(path, 0o700)
        .map_err(|error| format!("could not create durable private directory: {error}"))?;
    validate_owned_directory(path, true)?;
    Ok(())
}

fn validate_owned_directory(
    path: &Path,
    require_private: bool,
) -> Result<std::fs::Metadata, String> {
    let link_metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect directory {}: {error}", path.display()))?;
    if link_metadata.file_type().is_symlink() || !link_metadata.is_dir() {
        return Err(format!("untrusted directory {}", path.display()));
    }
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("could not canonicalize {}: {error}", path.display()))?;
    if canonical != path {
        return Err(format!("directory is not canonical: {}", path.display()));
    }
    let metadata = canonical
        .metadata()
        .map_err(|error| format!("could not inspect canonical directory: {error}"))?;
    if metadata.uid() != rustix::process::geteuid().as_raw()
        || require_private && metadata.permissions().mode() & 0o077 != 0
    {
        return Err(format!(
            "directory is not private and owner-controlled: {}",
            path.display()
        ));
    }
    Ok(metadata)
}

fn path_lock_key(path: &Path) -> String {
    let mut hash = Sha256::new();
    hash.update(b"lethetic-session-path-lock-v1\0");
    hash.update(path.as_os_str().as_encoded_bytes());
    format!("{:x}", hash.finalize())
}

fn validate_session_id(value: &str) -> Result<(), String> {
    let parsed = uuid::Uuid::parse_str(value)
        .map_err(|_| "session ID is not a canonical lowercase UUID".to_string())?;
    if parsed.to_string() != value {
        return Err("session ID is not a canonical lowercase UUID".to_string());
    }
    Ok(())
}

fn validate_session_directory_name(value: &str) -> Result<(), String> {
    if !value.starts_with("session_")
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err("invalid session directory name".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION_ID: &str = "11111111-2222-4333-8444-555555555555";
    const OTHER_SESSION_ID: &str = "66666666-7777-4888-8999-aaaaaaaaaaaa";

    fn store() -> (tempfile::TempDir, tempfile::TempDir, SessionStore) {
        let project = tempfile::TempDir::new().unwrap();
        let state = tempfile::TempDir::new().unwrap();
        let store = SessionStore::open_at(project.path(), state.path()).unwrap();
        (project, state, store)
    }

    #[test]
    fn open_durably_creates_private_store_ancestry() {
        let project = tempfile::TempDir::new().unwrap();
        let state_parent = tempfile::TempDir::new().unwrap();
        let state_root = state_parent.path().join("missing/state/lethetic");

        let store = SessionStore::open_at(project.path(), &state_root).unwrap();

        assert_eq!(store.locks_root, state_root.join(SESSION_LOCKS_DIRECTORY));
        for directory in [
            project.path().join(".lethetic"),
            project.path().join(".lethetic/sessions"),
            state_parent.path().join("missing"),
            state_parent.path().join("missing/state"),
            state_root,
            store.locks_root.clone(),
        ] {
            let metadata = std::fs::symlink_metadata(directory).unwrap();
            assert!(metadata.is_dir());
            assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
        }
    }

    #[test]
    fn path_and_identity_locks_exclude_concurrent_resume() {
        let (_project, _state, store) = store();
        let (path, lease) = store
            .create_locked_session("session_20260825_lock", SESSION_ID)
            .unwrap();
        assert!(store.try_lock_session_path(&path).unwrap().is_none());

        drop(lease);
        let path_lock = (0..100)
            .find_map(|_| {
                let lock = store.try_lock_session_path(&path).unwrap();
                if lock.is_none() {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
                lock
            })
            .expect("path lock should be released after inherited CLOEXEC descriptors close");
        let resumed = path_lock.bind_identity(SESSION_ID).unwrap();
        assert_eq!(resumed.session_id(), SESSION_ID);
    }

    #[test]
    fn registry_rejects_sequentially_resumed_copy() {
        let (_project, _state, store) = store();
        let (_original, lease) = store
            .create_locked_session("session_20260825_original", SESSION_ID)
            .unwrap();
        drop(lease);

        let copied = store.sessions_root().join("session_20260825_copy");
        std::fs::create_dir(&copied).unwrap();
        std::fs::set_permissions(&copied, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path_lock = store.try_lock_session_path(&copied).unwrap().unwrap();
        let error = path_lock.bind_identity(SESSION_ID).unwrap_err();
        assert!(error.contains("different directory"), "{error}");
    }

    #[test]
    fn registry_rejects_second_identity_for_same_legacy_path() {
        let (_project, _state, store) = store();
        let path = store.sessions_root().join("session_20260825_legacy");
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();

        let first = store
            .try_lock_session_path(&path)
            .unwrap()
            .unwrap()
            .bind_identity(SESSION_ID)
            .unwrap();
        drop(first);
        let second = store.try_lock_session_path(&path).unwrap().unwrap();
        let error = second.bind_identity(OTHER_SESSION_ID).unwrap_err();
        assert!(error.contains("different chat session identity"), "{error}");
    }

    #[test]
    fn classification_rejects_ambiguous_and_replaced_bindings() {
        let (_project, _state, store) = store();
        let (path, lease) = store
            .create_locked_session("session_20260825_classify", SESSION_ID)
            .unwrap();
        let metadata = path.metadata().unwrap();
        let conflicting_binding = SessionDirectoryBinding::capture_parts(
            &path,
            metadata.dev(),
            metadata.ino(),
            OTHER_SESSION_ID,
        )
        .unwrap();
        save_lifecycle_record(
            &store.locks_root,
            &SessionLifecycleRecord {
                schema_version: SESSION_LIFECYCLE_SCHEMA_VERSION,
                session_id: OTHER_SESSION_ID.to_string(),
                directory_name: "session_20260825_classify".to_string(),
                binding: Some(conflicting_binding),
                python_runtime_id: None,
                managed_python_workspace: None,
                state: SessionLifecycleState::Active,
            },
        )
        .unwrap();
        let ambiguous = store.classify_session_path(&path).unwrap_err();
        assert!(ambiguous.contains("multiple session IDs"), "{ambiguous}");

        std::fs::remove_file(store.locks_root.join(lifecycle_file_name(OTHER_SESSION_ID))).unwrap();
        drop(lease);
        let moved = store.sessions_root().join("session_20260825_moved");
        std::fs::rename(&path, &moved).unwrap();
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();

        let replaced = store.classify_session_path(&path).unwrap_err();
        assert!(replaced.contains("identity changed"), "{replaced}");
    }

    #[test]
    fn interrupted_legacy_migration_reuses_registered_identity() {
        let (_project, _state, store) = store();
        let path = store.sessions_root().join("session_20260825_legacy_retry");
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(path.join("ui_state.json"), "[]").unwrap();

        let first_state = crate::app::SessionState::load_checked(path.to_str().unwrap()).unwrap();
        let session_id = first_state.session_id.clone().unwrap();
        let first = store
            .try_lock_session_path(&path)
            .unwrap()
            .unwrap()
            .bind_identity(&session_id)
            .unwrap();
        drop(first);

        let retried_state = crate::app::SessionState::load_checked(path.to_str().unwrap()).unwrap();
        assert_eq!(
            retried_state.session_id.as_deref(),
            Some(session_id.as_str())
        );
        let retried = (0..100)
            .find_map(|_| {
                let lock = store.try_lock_session_path(&path).unwrap();
                if lock.is_none() {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
                lock
            })
            .expect("path lock should be released after inherited CLOEXEC descriptors close")
            .bind_identity(&session_id)
            .unwrap();
        assert_eq!(retried.session_id(), session_id);
    }

    #[test]
    fn committed_creation_resumes_and_incomplete_creation_cleans_up() {
        let (_project, _state, store) = store();
        let (path, lease) = store
            .create_locked_session("session_20260825_commit", SESSION_ID)
            .unwrap();
        let mut state = crate::app::SessionState::default();
        state.session_id = Some(SESSION_ID.to_string());
        state.session_directory_binding = Some(lease.binding().clone());
        state.needs_migration_save = false;
        state
            .save_to_directory_checked(path.to_str().unwrap())
            .unwrap();
        store.commit_locked_session_creation(&lease).unwrap();
        drop(lease);

        let resumed = store.lock_registered_session(SESSION_ID).unwrap();
        assert_eq!(resumed.canonical_path(), path);
        drop(resumed);

        const INCOMPLETE_ID: &str = "99999999-aaaa-4bbb-8ccc-dddddddddddd";
        let (incomplete_path, incomplete) = store
            .create_locked_session("session_20260825_incomplete", INCOMPLETE_ID)
            .unwrap();
        drop(incomplete);
        let load_error =
            crate::app::SessionState::load_checked(incomplete_path.to_str().unwrap()).unwrap_err();
        assert!(
            load_error.contains("no durable or legacy state"),
            "{load_error}"
        );
        let path_lock = store
            .try_lock_session_path(&incomplete_path)
            .unwrap()
            .unwrap();
        let bind_error = path_lock.bind_identity(OTHER_SESSION_ID).unwrap_err();
        assert!(
            bind_error.contains("different chat session identity"),
            "{bind_error}"
        );
        let error = (0..100)
            .find_map(|_| match store.lock_registered_session(INCOMPLETE_ID) {
                Err(error) if error.contains("already active in another Lethetic process") => {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                    None
                }
                result => Some(result),
            })
            .expect("identity lock should be released after inherited CLOEXEC descriptors close")
            .unwrap_err();
        assert!(
            error.contains("incomplete session creation was cleaned up"),
            "{error}"
        );
        assert!(!incomplete_path.exists());
        assert_eq!(
            read_lifecycle_record(&store.locks_root, INCOMPLETE_ID)
                .unwrap()
                .unwrap()
                .state,
            SessionLifecycleState::Deleted
        );
    }

    #[test]
    fn deletion_tombstone_recovers_after_state_was_removed() {
        let (_project, _state, store) = store();
        let (path, lease) = store
            .create_locked_session("session_20260825_retry", SESSION_ID)
            .unwrap();
        std::fs::write(path.join("session_state.json"), b"state").unwrap();
        const RUNTIME_ID: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
        let workspace = SessionWorkspaceBinding {
            canonical_path: path.join("managed-workspace"),
            device: 1,
            inode: 2,
            binding_hash: "a".repeat(64),
        };
        store
            .begin_locked_session_deletion(&lease, Some(RUNTIME_ID), Some(&workspace))
            .unwrap();
        std::fs::remove_file(path.join("session_state.json")).unwrap();
        std::fs::write(path.join("late-file"), b"late").unwrap();
        drop(lease);

        let resume_lock = store.try_lock_session_path(&path).unwrap().unwrap();
        let resume_error = resume_lock.bind_identity(SESSION_ID).unwrap_err();
        assert!(resume_error.contains("forbids resuming"), "{resume_error}");
        let cross_identity_lock = (0..100)
            .find_map(|_| match store.try_lock_session_path(&path) {
                Ok(Some(lock)) => Some(lock),
                Ok(None) => {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                    None
                }
                Err(error) => panic!("could not lock session path: {error}"),
            })
            .expect("path lock should be released after inherited CLOEXEC descriptors close");
        let cross_identity_error = cross_identity_lock
            .bind_identity(OTHER_SESSION_ID)
            .unwrap_err();
        assert!(
            cross_identity_error.contains("different chat session identity"),
            "{cross_identity_error}"
        );
        let recovered = (0..100)
            .find_map(|_| match store.lock_recoverable_session_path(&path) {
                Ok(lock) => Some(lock),
                Err(error) if error.contains("active") => {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                    None
                }
                Err(error) => panic!("could not recover deleting session: {error}"),
            })
            .expect("session lock should be released after inherited CLOEXEC descriptors close");
        let resources = store.deletion_resources(&recovered).unwrap();
        assert_eq!(resources.python_runtime_id.as_deref(), Some(RUNTIME_ID));
        assert_eq!(
            resources.managed_python_workspace.as_ref(),
            Some(&workspace)
        );
        let outcome = store.remove_locked_session(&recovered).unwrap();
        assert!(outcome.durability_warnings.is_empty());
        assert!(!path.exists());
        assert_eq!(
            read_lifecycle_record(&store.locks_root, SESSION_ID)
                .unwrap()
                .unwrap()
                .state,
            SessionLifecycleState::Deleted
        );
    }

    #[test]
    fn restored_directory_can_retry_after_deleted_tombstone() {
        let (_project, _state, store) = store();
        let (path, lease) = store
            .create_locked_session("session_20260825_restored", SESSION_ID)
            .unwrap();
        let mut state = crate::app::SessionState::default();
        state.session_id = Some(SESSION_ID.to_string());
        state.session_directory_binding = Some(lease.binding().clone());
        state
            .save_to_directory_checked(path.to_str().unwrap())
            .unwrap();
        store.commit_locked_session_creation(&lease).unwrap();
        store
            .begin_locked_session_deletion(&lease, None, None)
            .unwrap();
        assert!(crate::app::SessionState::load_checked(path.to_str().unwrap()).is_ok());
        let deleting = store.classify_session_path(&path).unwrap();
        assert_eq!(
            deleting.registration.unwrap().disposition,
            SessionPathDisposition::CleanupOnly
        );
        assert_eq!(store.registered_session_id_for_path(&path).unwrap(), None);
        let mut lifecycle = read_lifecycle_record(&store.locks_root, SESSION_ID)
            .unwrap()
            .unwrap();
        lifecycle.state = SessionLifecycleState::Deleted;
        save_lifecycle_record(&store.locks_root, &lifecycle).unwrap();
        let deleted = store.classify_session_path(&path).unwrap();
        let registration = deleted.registration.unwrap();
        assert_eq!(registration.session_id, SESSION_ID);
        assert_eq!(
            registration.disposition,
            SessionPathDisposition::CleanupOnly
        );
        assert_eq!(store.registered_session_id_for_path(&path).unwrap(), None);
        drop(lease);

        let recovered = (0..100)
            .find_map(|_| match store.lock_recoverable_session_path(&path) {
                Ok(lock) => Some(lock),
                Err(error) if error.contains("active") => {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                    None
                }
                Err(error) => panic!("could not recover restored session: {error}"),
            })
            .expect("session lock should be released after inherited CLOEXEC descriptors close");
        store
            .begin_locked_session_deletion(&recovered, None, None)
            .unwrap();
        assert_eq!(
            read_lifecycle_record(&store.locks_root, SESSION_ID)
                .unwrap()
                .unwrap()
                .state,
            SessionLifecycleState::Deleting
        );
        store.remove_locked_session(&recovered).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn deletion_keeps_external_lock_and_does_not_follow_symlinks() {
        use std::os::unix::fs::symlink;

        let (_project, _state, store) = store();
        let (path, lease) = store
            .create_locked_session("session_20260825_delete", SESSION_ID)
            .unwrap();
        std::fs::write(path.join("session_state.json"), b"state").unwrap();
        std::fs::create_dir(path.join("nested")).unwrap();
        std::fs::write(path.join("nested/file"), b"data").unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        symlink(outside.path(), path.join("outside-link")).unwrap();

        store.remove_locked_session(&lease).unwrap();
        assert!(!path.exists());
        assert!(outside.path().exists());

        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(store.try_lock_session_path(&path).unwrap().is_none());
        drop(lease);
        let replacement = (0..100)
            .find_map(|_| {
                let lock = store.try_lock_session_path(&path).unwrap();
                if lock.is_none() {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
                lock
            })
            .expect("path lock should be released after inherited CLOEXEC descriptors close");
        let error = replacement.bind_identity(SESSION_ID).unwrap_err();
        assert!(error.contains("forbids resuming"), "{error}");
    }
}
