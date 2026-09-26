use super::common::{
    create_private_directory, ensure_private_directory, repair_existing_private_directory,
    validate_existing_private_directory, validate_lower_hex, validate_uuid,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

pub const MANAGED_WORKSPACE_CLEANUP_ABI: &str = "lethetic-managed-workspace-cleanup-v1";
pub(super) const MANAGED_SESSIONS_DIRECTORY: &str = "lethetic-sessions";
const MANAGED_WORKSPACE_DIRECTORY: &str = "workspace";
const MAPPED_CLEANUP_TIMEOUT: Duration = Duration::from_secs(60);
const MAPPED_CLEANUP_TERM_GRACE: Duration = Duration::from_secs(1);
const MAPPED_CLEANUP_KILL_GRACE: Duration = Duration::from_secs(2);
const CHILD_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceIdentity {
    pub canonical_path: PathBuf,
    pub device: u64,
    pub inode: u64,
    pub binding_hash: String,
}

impl WorkspaceIdentity {
    pub fn capture(path: &Path) -> Result<Self, String> {
        let link_metadata = std::fs::symlink_metadata(path)
            .map_err(|error| format!("could not inspect managed workspace: {error}"))?;
        if link_metadata.file_type().is_symlink() || !link_metadata.is_dir() {
            return Err("managed workspace must be a real directory".to_string());
        }
        let canonical = path
            .canonicalize()
            .map_err(|error| format!("could not canonicalize managed workspace: {error}"))?;
        if canonical != path || canonical.to_str().is_none() {
            return Err("managed workspace must be canonical UTF-8".to_string());
        }
        let metadata = canonical
            .metadata()
            .map_err(|error| format!("could not inspect canonical workspace: {error}"))?;
        #[cfg(unix)]
        if metadata.uid() != rustix::process::geteuid().as_raw() {
            return Err("managed workspace is not owned by the invoking user".to_string());
        }
        let mut hash = Sha256::new();
        hash.update(b"lethetic-workspace-v1\0");
        hash.update(canonical.as_os_str().as_encoded_bytes());
        hash.update(metadata.dev().to_le_bytes());
        hash.update(metadata.ino().to_le_bytes());
        Ok(Self {
            canonical_path: canonical,
            device: metadata.dev(),
            inode: metadata.ino(),
            binding_hash: format!("{:x}", hash.finalize()),
        })
    }

    pub fn validate_stored(&self) -> Result<(), String> {
        if !self.canonical_path.is_absolute() || self.canonical_path.to_str().is_none() {
            return Err("stored managed workspace path is not absolute UTF-8".to_string());
        }
        validate_lower_hex(&self.binding_hash, 64, "workspace binding hash")
    }

    pub fn verify_current(&self) -> Result<(), String> {
        self.validate_stored()?;
        let current = Self::capture(&self.canonical_path)?;
        if &current != self {
            return Err(
                "managed workspace identity no longer matches the runtime manifest".to_string(),
            );
        }
        Ok(())
    }

    fn verify_current_for_deletion(&self) -> Result<(), String> {
        self.validate_stored()?;
        let link_metadata = std::fs::symlink_metadata(&self.canonical_path).map_err(|error| {
            format!("could not inspect managed workspace for deletion: {error}")
        })?;
        if link_metadata.file_type().is_symlink() || !link_metadata.is_dir() {
            return Err("managed workspace deletion target must be a real directory".to_string());
        }
        let canonical = self.canonical_path.canonicalize().map_err(|error| {
            format!("could not canonicalize managed workspace for deletion: {error}")
        })?;
        if canonical != self.canonical_path {
            return Err("managed workspace deletion target is no longer canonical".to_string());
        }
        let metadata = canonical.metadata().map_err(|error| {
            format!("could not inspect canonical workspace for deletion: {error}")
        })?;
        if metadata.dev() != self.device || metadata.ino() != self.inode {
            return Err("managed workspace device/inode changed before deletion".to_string());
        }
        let mut hash = Sha256::new();
        hash.update(b"lethetic-workspace-v1\0");
        hash.update(canonical.as_os_str().as_encoded_bytes());
        hash.update(metadata.dev().to_le_bytes());
        hash.update(metadata.ino().to_le_bytes());
        if format!("{:x}", hash.finalize()) != self.binding_hash {
            return Err("managed workspace binding hash changed before deletion".to_string());
        }
        Ok(())
    }
}

pub struct ManagedWorkspaceStore {
    root: PathBuf,
}

impl ManagedWorkspaceStore {
    pub(crate) fn default_root() -> Result<PathBuf, String> {
        let home = dirs::home_dir().ok_or_else(|| {
            "could not resolve home directory for managed Python sessions".to_string()
        })?;
        Ok(home.join("tmp").join(MANAGED_SESSIONS_DIRECTORY))
    }

    pub fn open() -> Result<Self, String> {
        Self::open_at(Self::default_root()?)
    }

    pub fn open_at(root: PathBuf) -> Result<Self, String> {
        ensure_private_directory(&root, true)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn create(&self, session_id: &str) -> Result<WorkspaceIdentity, String> {
        validate_uuid(session_id, "session ID")?;
        let session = self.root.join(session_id);
        create_private_directory(&session)
            .map_err(|error| format!("could not create managed session directory: {error}"))?;
        if let Err(error) = validate_existing_private_directory(&session) {
            let _ = std::fs::remove_dir(&session);
            return Err(error);
        }
        File::open(&self.root)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("could not sync managed session directory: {error}"))?;
        let workspace = session.join(MANAGED_WORKSPACE_DIRECTORY);
        if let Err(error) = create_private_directory(&workspace) {
            let _ = std::fs::remove_dir(&session);
            return Err(format!(
                "could not create managed Python workspace: {error}"
            ));
        }
        File::open(&session)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("could not sync managed Python workspace: {error}"))?;
        WorkspaceIdentity::capture(&workspace)
    }

    pub fn load(&self, session_id: &str) -> Result<WorkspaceIdentity, String> {
        validate_uuid(session_id, "session ID")?;
        let session = self.root.join(session_id);
        repair_existing_private_directory(&session)?;
        let workspace = session.join(MANAGED_WORKSPACE_DIRECTORY);
        repair_existing_private_directory(&workspace)?;
        WorkspaceIdentity::capture(&workspace)
    }

    pub fn load_optional(&self, session_id: &str) -> Result<Option<WorkspaceIdentity>, String> {
        validate_uuid(session_id, "session ID")?;
        let session = self.root.join(session_id);
        match std::fs::symlink_metadata(&session) {
            Ok(_) => self.load(session_id).map(Some),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!(
                "could not inspect managed session directory: {error}"
            )),
        }
    }

    pub fn load_or_create(&self, session_id: &str) -> Result<WorkspaceIdentity, String> {
        validate_uuid(session_id, "session ID")?;
        let session = self.root.join(session_id);
        match std::fs::symlink_metadata(&session) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return self.create(session_id);
            }
            Err(error) => {
                return Err(format!(
                    "could not inspect managed session directory: {error}"
                ));
            }
            Ok(_) => repair_existing_private_directory(&session)?,
        }
        let workspace = session.join(MANAGED_WORKSPACE_DIRECTORY);
        match std::fs::symlink_metadata(&workspace) {
            Ok(_) => self.load(session_id),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut entries = std::fs::read_dir(&session).map_err(|error| {
                    format!("could not inspect managed session contents: {error}")
                })?;
                if entries.next().is_some() {
                    return Err(
                        "partially initialized managed session directory is not empty".to_string(),
                    );
                }
                create_private_directory(&workspace).map_err(|error| {
                    format!("could not complete managed workspace creation: {error}")
                })?;
                File::open(&session)
                    .and_then(|directory| directory.sync_all())
                    .map_err(|error| {
                        format!("could not sync recovered managed workspace: {error}")
                    })?;
                WorkspaceIdentity::capture(&workspace)
            }
            Err(error) => Err(format!(
                "could not inspect managed Python workspace: {error}"
            )),
        }
    }

    pub fn delete(&self, session_id: &str, expected: &WorkspaceIdentity) -> Result<(), String> {
        self.delete_in_current_namespace(session_id, expected, true)
    }

    fn delete_in_current_namespace(
        &self,
        session_id: &str,
        expected: &WorkspaceIdentity,
        allow_user_namespace_fallback: bool,
    ) -> Result<(), String> {
        validate_uuid(session_id, "session ID")?;
        expected.validate_stored()?;
        let session = self.root.join(session_id);
        let workspace = session.join(MANAGED_WORKSPACE_DIRECTORY);
        if expected.canonical_path != workspace {
            return Err(
                "managed workspace binding is outside its exact session directory".to_string(),
            );
        }
        match std::fs::symlink_metadata(&session) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                File::open(&self.root)
                    .and_then(|directory| directory.sync_all())
                    .map_err(|error| {
                        format!("could not sync already-absent managed session: {error}")
                    })?;
                return Ok(());
            }
            Err(error) => {
                return Err(format!(
                    "could not inspect managed session deletion target: {error}"
                ));
            }
            Ok(_) => repair_existing_private_directory(&session)?,
        }
        let entries = std::fs::read_dir(&session)
            .map_err(|error| format!("could not inspect managed session deletion: {error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("could not inspect managed session entry: {error}"))?;
        if entries
            .iter()
            .any(|entry| entry.file_name() != std::ffi::OsStr::new(MANAGED_WORKSPACE_DIRECTORY))
        {
            return Err("managed session directory contains an unexpected entry".to_string());
        }
        match std::fs::symlink_metadata(&workspace) {
            Ok(_) => {
                expected.verify_current_for_deletion()?;
                if let Err(error) = crate::platform::remove_tree_nofollow_same_mount(&workspace) {
                    if error.kind() != std::io::ErrorKind::PermissionDenied
                        || !allow_user_namespace_fallback
                    {
                        return Err(format!(
                            "could not remove exact managed Python workspace: {error}"
                        ));
                    }
                    delete_mapped_workspace_in_user_namespace(&self.root, session_id, expected)?;
                    match std::fs::symlink_metadata(&session) {
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            File::open(&self.root)
                                .and_then(|directory| directory.sync_all())
                                .map_err(|error| {
                                    format!(
                                        "could not sync mapped-workspace session deletion: {error}"
                                    )
                                })?;
                            return Ok(());
                        }
                        Ok(_) => {
                            return Err(
                                "user-namespace cleanup returned without removing the exact managed session"
                                    .to_string(),
                            );
                        }
                        Err(error) => {
                            return Err(format!(
                                "could not verify user-namespace workspace cleanup: {error}"
                            ));
                        }
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "could not inspect managed workspace deletion target: {error}"
                ));
            }
        }
        if std::fs::read_dir(&session)
            .map_err(|error| format!("could not verify managed session deletion: {error}"))?
            .next()
            .is_some()
        {
            return Err("managed session directory contains an unexpected entry".to_string());
        }
        std::fs::remove_dir(&session)
            .map_err(|error| format!("could not remove managed session directory: {error}"))?;
        File::open(&self.root)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("could not sync managed workspace deletion: {error}"))
    }
}

fn delete_mapped_workspace_in_user_namespace(
    root: &Path,
    session_id: &str,
    expected: &WorkspaceIdentity,
) -> Result<(), String> {
    let podman = crate::python::backend::resolve_real_podman()?;
    #[cfg(test)]
    let executable = std::env::var_os("LETHETIC_TEST_BINARY")
        .map(PathBuf::from)
        .unwrap_or(std::env::current_exe().map_err(|error| {
            format!("could not resolve mapped-workspace cleanup executable: {error}")
        })?);
    #[cfg(not(test))]
    let executable = std::env::current_exe().map_err(|error| {
        format!("could not resolve mapped-workspace cleanup executable: {error}")
    })?;
    let executable = executable.canonicalize().map_err(|error| {
        format!("could not canonicalize mapped-workspace cleanup executable: {error}")
    })?;
    let metadata = executable.metadata().map_err(|error| {
        format!("could not inspect mapped-workspace cleanup executable: {error}")
    })?;
    if !metadata.is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o022 != 0
        || metadata.permissions().mode() & 0o111 == 0
    {
        return Err(
            "mapped-workspace cleanup executable is not trusted and owner-controlled".to_string(),
        );
    }
    let mut command = Command::new(podman);
    command
        .arg("unshare")
        .arg(executable)
        .arg("--internal-managed-workspace-cleanup")
        .arg(MANAGED_WORKSPACE_CLEANUP_ABI)
        .arg(root)
        .arg(session_id)
        .arg(&expected.canonical_path)
        .arg(expected.device.to_string())
        .arg(expected.inode.to_string())
        .arg(&expected.binding_hash)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let status = run_bounded_process_group(
        command,
        MAPPED_CLEANUP_TIMEOUT,
        MAPPED_CLEANUP_TERM_GRACE,
        MAPPED_CLEANUP_KILL_GRACE,
        "mapped-workspace cleanup",
    )?;
    if !status.success() {
        return Err(format!(
            "mapped-workspace cleanup exited unsuccessfully: {status}"
        ));
    }
    Ok(())
}

fn wait_for_child(child: &mut Child, timeout: Duration) -> Result<Option<ExitStatus>, String> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| "child-process deadline overflowed".to_string())?;
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("could not poll child process: {error}"))?
        {
            return Ok(Some(status));
        }
        let now = Instant::now();
        if now >= deadline {
            return Ok(None);
        }
        std::thread::sleep(CHILD_POLL_INTERVAL.min(deadline.duration_since(now)));
    }
}

fn signal_process_group(pid: i32, signal: i32) -> Result<(), String> {
    if unsafe { libc::kill(-pid, signal) } == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        return Ok(());
    }
    Err(error.to_string())
}

fn terminate_process_group_and_reap(
    mut child: Child,
    term_grace: Duration,
    kill_grace: Duration,
) -> Vec<String> {
    let mut errors = Vec::new();
    let pid = match i32::try_from(child.id()) {
        Ok(pid) => pid,
        Err(_) => {
            errors.push("child PID does not fit in pid_t".to_string());
            let _ = child.kill();
            match wait_for_child(&mut child, kill_grace) {
                Ok(Some(_)) => return errors,
                Ok(None) => {
                    errors.push("child was not reaped before the kill deadline".to_string())
                }
                Err(error) => errors.push(error),
            }
            if let Err(error) = std::thread::Builder::new()
                .name("lethetic-workspace-cleanup-reaper".to_string())
                .spawn(move || {
                    let _ = child.kill();
                    let _ = child.wait();
                })
            {
                errors.push(format!("could not start deferred child reaper: {error}"));
            }
            return errors;
        }
    };
    if let Err(error) = signal_process_group(pid, libc::SIGTERM) {
        errors.push(format!("SIGTERM failed: {error}"));
    }
    std::thread::sleep(term_grace);
    if let Err(error) = signal_process_group(pid, libc::SIGKILL) {
        errors.push(format!("SIGKILL failed: {error}"));
    }
    if let Err(error) = child.kill()
        && !matches!(
            error.kind(),
            std::io::ErrorKind::InvalidInput | std::io::ErrorKind::NotFound
        )
    {
        errors.push(format!("direct child kill failed: {error}"));
    }
    match wait_for_child(&mut child, kill_grace) {
        Ok(Some(_)) => return errors,
        Ok(None) => errors.push("child was not reaped before the kill deadline".to_string()),
        Err(error) => errors.push(error),
    }
    if let Err(error) = std::thread::Builder::new()
        .name("lethetic-workspace-cleanup-reaper".to_string())
        .spawn(move || {
            let _ = child.kill();
            let _ = child.wait();
        })
    {
        errors.push(format!("could not start deferred child reaper: {error}"));
    }
    errors
}

pub(super) fn run_bounded_process_group(
    mut command: Command,
    timeout: Duration,
    term_grace: Duration,
    kill_grace: Duration,
    operation: &str,
) -> Result<ExitStatus, String> {
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not start {operation}: {error}"))?;
    let failure = match wait_for_child(&mut child, timeout) {
        Ok(Some(status)) => return Ok(status),
        Ok(None) => format!("{operation} exceeded its {timeout:?} deadline"),
        Err(error) => format!("could not monitor {operation}: {error}"),
    };
    let containment_errors = terminate_process_group_and_reap(child, term_grace, kill_grace);
    if containment_errors.is_empty() {
        Err(failure)
    } else {
        Err(format!(
            "{failure}; process-group containment reported: {}",
            containment_errors.join("; ")
        ))
    }
}

pub fn run_internal_managed_workspace_cleanup(
    root: PathBuf,
    session_id: &str,
    expected: WorkspaceIdentity,
) -> Result<(), String> {
    ensure_private_directory(&root, false)?;
    let store = ManagedWorkspaceStore { root };
    store.delete_in_current_namespace(session_id, &expected, false)
}
