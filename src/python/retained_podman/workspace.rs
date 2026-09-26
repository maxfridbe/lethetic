use crate::python::runtime_store::ManagedWorkspaceStore;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

const MAX_SHARED_WORKSPACE_ENTRIES: usize = 200_000;

#[derive(Clone, Copy)]
struct DirectoryIdentity {
    device: u64,
    inode: u64,
    uid: u32,
}

impl DirectoryIdentity {
    fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            uid: metadata.uid(),
        }
    }

    fn matches(self, metadata: &std::fs::Metadata) -> bool {
        self.device == metadata.dev() && self.inode == metadata.ino() && self.uid == metadata.uid()
    }
}

pub(super) struct ExternalWorkspacePlan {
    canonical: PathBuf,
    workspace_identity: DirectoryIdentity,
    control_identity: Option<DirectoryIdentity>,
}

impl ExternalWorkspacePlan {
    pub(super) fn path(&self) -> &Path {
        &self.canonical
    }

    pub(super) fn commit(self) -> Result<PathBuf, String> {
        self.verify_workspace_identity()?;
        let control = self.canonical.join(".lethetic");
        self.verify_control_identity(&control)?;
        crate::platform::ensure_private_directory_durable(&control, 0o700).map_err(|error| {
            format!(
                "could not establish shared-cwd Lethetic control directory {}: {error}",
                control.display()
            )
        })?;
        let control_metadata = validate_control_directory(&control)?;
        self.verify_workspace_identity()?;
        if self
            .control_identity
            .is_some_and(|identity| !identity.matches(&control_metadata))
        {
            return Err(
                "shared-cwd .lethetic changed while its permissions were secured".to_string(),
            );
        }
        Ok(self.canonical)
    }

    fn verify_control_identity(&self, control: &Path) -> Result<(), String> {
        let observed = match std::fs::symlink_metadata(control) {
            Ok(metadata) => Some(metadata),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(format!(
                    "could not recheck shared-cwd Lethetic control directory {}: {error}",
                    control.display()
                ));
            }
        };
        match (self.control_identity, observed) {
            (None, None) => Ok(()),
            (Some(expected), Some(metadata)) => {
                validate_control_metadata(&metadata)?;
                if expected.matches(&metadata) {
                    Ok(())
                } else {
                    Err("shared-cwd .lethetic changed after safety validation".to_string())
                }
            }
            _ => Err("shared-cwd .lethetic changed after safety validation".to_string()),
        }
    }

    fn verify_workspace_identity(&self) -> Result<(), String> {
        let metadata = std::fs::symlink_metadata(&self.canonical).map_err(|error| {
            format!(
                "could not recheck shared launch cwd {}: {error}",
                self.canonical.display()
            )
        })?;
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || !self.workspace_identity.matches(&metadata)
        {
            return Err("shared launch cwd changed after safety validation".to_string());
        }
        Ok(())
    }
}

pub(crate) fn validate_external_workspace(path: &Path) -> Result<PathBuf, String> {
    Ok(classify_external_workspace(path)?.canonical)
}

pub(super) fn classify_external_workspace(path: &Path) -> Result<ExternalWorkspacePlan, String> {
    let control_roots = vec![
        crate::platform::lethetic_config_dir(),
        crate::platform::lethetic_state_dir(),
        ManagedWorkspaceStore::default_root()?,
    ];
    classify_external_workspace_against(path, &control_roots)
}

fn canonicalize_control_location(path: &Path) -> PathBuf {
    let mut ancestor = path;
    let mut missing = Vec::new();
    loop {
        if let Ok(mut canonical) = ancestor.canonicalize() {
            for component in missing.iter().rev() {
                canonical.push(component);
            }
            return canonical;
        }
        let Some(name) = ancestor.file_name() else {
            return path.to_path_buf();
        };
        missing.push(name.to_os_string());
        let Some(parent) = ancestor.parent() else {
            return path.to_path_buf();
        };
        ancestor = parent;
    }
}

#[cfg(test)]
pub(super) fn validate_external_workspace_against(
    path: &Path,
    control_roots: &[PathBuf],
) -> Result<PathBuf, String> {
    Ok(classify_external_workspace_against(path, control_roots)?.canonical)
}

pub(super) fn classify_external_workspace_against(
    path: &Path,
    control_roots: &[PathBuf],
) -> Result<ExternalWorkspacePlan, String> {
    let canonical = validate_managed_directory(path, "shared launch cwd", false)?;
    if canonical == Path::new("/")
        || canonical
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == ".lethetic")
    {
        return Err(
            "shared launch cwd cannot be a filesystem or Lethetic control root".to_string(),
        );
    }
    for control_root in control_roots {
        let control_root = canonicalize_control_location(control_root);
        if control_root == canonical
            || control_root.starts_with(&canonical)
            || canonical.starts_with(&control_root)
        {
            return Err(format!(
                "shared launch cwd overlaps Lethetic control root {}",
                control_root.display()
            ));
        }
    }

    let workspace_metadata = std::fs::symlink_metadata(&canonical).map_err(|error| {
        format!(
            "could not capture shared launch cwd identity {}: {error}",
            canonical.display()
        )
    })?;
    if workspace_metadata.file_type().is_symlink()
        || !workspace_metadata.is_dir()
        || workspace_metadata.uid() != rustix::process::geteuid().as_raw()
    {
        return Err("shared launch cwd changed during safety validation".to_string());
    }
    let control = canonical.join(".lethetic");
    let control_identity = classify_control_directory(&control)?;
    WorkspaceScan::new(&canonical, &control).validate()?;
    Ok(ExternalWorkspacePlan {
        canonical,
        workspace_identity: DirectoryIdentity::from_metadata(&workspace_metadata),
        control_identity,
    })
}

fn classify_control_directory(control: &Path) -> Result<Option<DirectoryIdentity>, String> {
    let metadata = match std::fs::symlink_metadata(control) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "could not inspect shared-cwd Lethetic control directory {}: {error}",
                control.display()
            ));
        }
    };
    validate_control_metadata(&metadata)?;
    Ok(Some(DirectoryIdentity::from_metadata(&metadata)))
}

fn validate_control_directory(control: &Path) -> Result<std::fs::Metadata, String> {
    let metadata = std::fs::symlink_metadata(control).map_err(|error| {
        format!(
            "could not inspect shared-cwd Lethetic control directory {}: {error}",
            control.display()
        )
    })?;
    validate_control_metadata(&metadata)?;
    Ok(metadata)
}

fn validate_control_metadata(metadata: &std::fs::Metadata) -> Result<(), String> {
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
    {
        return Err("shared-cwd .lethetic must be a real owner-controlled directory".to_string());
    }
    Ok(())
}

enum WorkspaceEntry {
    Ignore,
    File,
    Directory(PathBuf),
}

struct WorkspaceScan<'a> {
    control: &'a Path,
    pending: Vec<PathBuf>,
    inspected: usize,
}

impl<'a> WorkspaceScan<'a> {
    fn new(root: &Path, control: &'a Path) -> Self {
        Self {
            control,
            pending: vec![root.to_path_buf()],
            inspected: 0,
        }
    }

    fn validate(mut self) -> Result<(), String> {
        while let Some(directory) = self.pending.pop() {
            self.scan_directory(&directory)?;
        }
        Ok(())
    }

    fn scan_directory(&mut self, directory: &Path) -> Result<(), String> {
        let entries = std::fs::read_dir(directory).map_err(|error| {
            format!(
                "could not inspect shared launch cwd {}: {error}",
                directory.display()
            )
        })?;
        for entry in entries {
            let entry = entry
                .map_err(|error| format!("could not inspect shared launch cwd entry: {error}"))?;
            if let WorkspaceEntry::Directory(path) = self.classify(entry)? {
                self.pending.push(path);
            }
        }
        Ok(())
    }

    fn classify(&mut self, entry: std::fs::DirEntry) -> Result<WorkspaceEntry, String> {
        let path = entry.path();
        if path == self.control {
            return Ok(WorkspaceEntry::Ignore);
        }
        if entry.file_name() == std::ffi::OsStr::new(".lethetic") {
            return Err(format!(
                "shared launch cwd contains a nested Lethetic control directory: {}",
                path.display()
            ));
        }
        self.inspected = self.inspected.saturating_add(1);
        if self.inspected > MAX_SHARED_WORKSPACE_ENTRIES {
            return Err(
                "shared launch cwd contains more than 200000 entries; refusing an incomplete safety scan"
                    .to_string(),
            );
        }

        let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
            format!(
                "could not inspect shared launch cwd entry {}: {error}",
                path.display()
            )
        })?;
        let file_type = metadata.file_type();
        if file_type.is_socket()
            || file_type.is_fifo()
            || file_type.is_block_device()
            || file_type.is_char_device()
        {
            return Err(format!(
                "shared launch cwd contains a host IPC/device endpoint: {}",
                path.display()
            ));
        }
        if file_type.is_symlink() {
            // A symlink is resolved in the container mount namespace. Paths outside the
            // shared bind therefore target the container filesystem, not the host target.
            return Ok(WorkspaceEntry::Ignore);
        }
        if metadata.is_dir() {
            return Ok(WorkspaceEntry::Directory(path));
        }
        if metadata.is_file() {
            return Ok(WorkspaceEntry::File);
        }
        Err(format!(
            "shared launch cwd contains an unsupported file type: {}",
            path.display()
        ))
    }
}

pub(super) fn validate_managed_directory(
    path: &Path,
    label: &str,
    require_private: bool,
) -> Result<PathBuf, String> {
    let link_metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect {label} {}: {error}", path.display()))?;
    if link_metadata.file_type().is_symlink() || !link_metadata.is_dir() {
        return Err(format!(
            "{label} must be a real directory: {}",
            path.display()
        ));
    }
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("could not canonicalize {label} {}: {error}", path.display()))?;
    if canonical != path {
        return Err(format!(
            "{label} must already be canonical: {}",
            path.display()
        ));
    }
    #[cfg(unix)]
    {
        let metadata = canonical
            .metadata()
            .map_err(|error| format!("could not inspect canonical {label}: {error}"))?;
        if metadata.uid() != rustix::process::geteuid().as_raw() {
            return Err(format!("{label} is not owned by the invoking user"));
        }
        if require_private && metadata.permissions().mode() & 0o077 != 0 {
            return Err(format!("{label} must have mode 0700 or stricter"));
        }
    }
    if canonical.to_str().is_none() {
        return Err(format!("{label} is not valid UTF-8"));
    }
    Ok(canonical)
}
