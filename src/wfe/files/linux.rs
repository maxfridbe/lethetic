use super::{FileIdentity, FilesError, OperationContext, RelativePath};
use crate::wfe::file_contracts::WFE_FILES_READ_CHUNK_BYTES;
use rustix::fs::{FileType, Mode, OFlags, ResolveFlags};
use std::io::Read;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::{Path, PathBuf};

const RESOLVE_FLAGS: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS)
    .union(ResolveFlags::NO_XDEV);

pub(crate) struct PinnedRoot {
    fd: OwnedFd,
    canonical_path: PathBuf,
    snapshot: ObjectSnapshot,
}

pub(crate) struct PinnedObject {
    fd: OwnedFd,
    snapshot: ObjectSnapshot,
    kind: ObjectKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ObjectKind {
    File,
    Directory,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ObjectSnapshot {
    identity: FileIdentity,
    mode: u32,
    links: u64,
    size: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

impl PinnedRoot {
    pub(crate) fn open(path: &Path) -> Result<Self, FilesError> {
        if !path.is_absolute() {
            return Err(FilesError::BadRequest);
        }
        let fd = rustix::fs::open(
            path,
            OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(map_root_open_error)?;
        let root_snapshot = snapshot(&fd)?;
        if object_kind(&root_snapshot) != Some(ObjectKind::Directory) {
            return Err(FilesError::Unsupported);
        }

        let filesystem_root = rustix::fs::open(
            "/",
            OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| FilesError::Unavailable)?;
        if snapshot(&filesystem_root)?.identity == root_snapshot.identity {
            return Err(FilesError::Protected);
        }

        // Probe the exact kernel primitive and every resolve flag at startup.
        // EINVAL and ENOSYS therefore fail before an HTTP listener is exposed.
        let probe = rustix::fs::openat2(
            &fd,
            ".",
            OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            RESOLVE_FLAGS,
        )
        .map_err(|_| FilesError::Unsupported)?;
        if snapshot(&probe)?.identity != root_snapshot.identity {
            return Err(FilesError::Unavailable);
        }

        let canonical_path = trusted_procfd_path(&fd)?;
        if !canonical_path.is_absolute()
            || canonical_path.to_str().is_none()
            || canonical_path
                .as_os_str()
                .to_string_lossy()
                .ends_with(" (deleted)")
        {
            return Err(FilesError::Unsupported);
        }
        let reopened = reopen_procfd(&fd, ObjectKind::Directory)?;
        if snapshot(&reopened)? != root_snapshot {
            return Err(FilesError::Changed);
        }

        Ok(Self {
            fd,
            canonical_path,
            snapshot: root_snapshot,
        })
    }

    pub(crate) fn canonical_path(&self) -> &Path {
        &self.canonical_path
    }

    pub(crate) fn identity_for_explicit_path(
        &self,
        path: &Path,
    ) -> Result<Option<FileIdentity>, FilesError> {
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.canonical_path.join(path)
        };
        match rustix::fs::open(&resolved, OFlags::PATH | OFlags::CLOEXEC, Mode::empty()) {
            Ok(fd) => Ok(Some(snapshot(&fd)?.identity)),
            Err(error)
                if error == rustix::io::Errno::NOENT
                    || error == rustix::io::Errno::NOTDIR
                    || error == rustix::io::Errno::ACCESS
                    || error == rustix::io::Errno::PERM =>
            {
                Ok(None)
            }
            Err(_) => Err(FilesError::Unavailable),
        }
    }

    pub(crate) fn pin(&self, path: &RelativePath) -> Result<PinnedObject, FilesError> {
        let fd = if path.is_root() {
            rustix::io::fcntl_dupfd_cloexec(&self.fd, 0).map_err(|_| FilesError::Unavailable)?
        } else {
            rustix::fs::openat2(
                &self.fd,
                path.as_str(),
                OFlags::PATH | OFlags::CLOEXEC,
                Mode::empty(),
                RESOLVE_FLAGS,
            )
            .map_err(map_candidate_open_error)?
        };
        let object_snapshot = snapshot(&fd)?;
        let kind = object_kind(&object_snapshot).ok_or(FilesError::Unsupported)?;
        if object_snapshot.identity.device != self.snapshot.identity.device {
            return Err(FilesError::Unsupported);
        }
        if kind == ObjectKind::File && object_snapshot.links != 1 {
            return Err(FilesError::Unsupported);
        }
        Ok(PinnedObject {
            fd,
            snapshot: object_snapshot,
            kind,
        })
    }

    pub(crate) fn visit_directory(
        &self,
        path: &RelativePath,
        expected: &PinnedObject,
        context: &OperationContext,
        mut visit: impl FnMut(&str) -> Result<(), FilesError>,
    ) -> Result<u32, FilesError> {
        context.checkpoint()?;
        let pinned = self.pin(path)?;
        if !pinned.same_snapshot(expected) {
            return Err(FilesError::Changed);
        }
        if pinned.kind != ObjectKind::Directory {
            return Err(FilesError::Unsupported);
        }
        let readable = reopen_procfd(&pinned.fd, ObjectKind::Directory)?;
        if snapshot(&readable)? != pinned.snapshot {
            return Err(FilesError::Changed);
        }
        let mut directory = rustix::fs::Dir::new(readable).map_err(|_| FilesError::Unavailable)?;
        let mut invalid_names = 0_u32;
        while let Some(entry) = directory.read() {
            context.checkpoint()?;
            let entry = entry.map_err(|_| FilesError::Unavailable)?;
            let name_bytes = entry.file_name().to_bytes();
            if matches!(name_bytes, b"." | b"..") {
                continue;
            }
            let Ok(name) = std::str::from_utf8(name_bytes) else {
                invalid_names = invalid_names.checked_add(1).ok_or(FilesError::TooLarge)?;
                continue;
            };
            if !super::valid_component(name) || super::windows_prefix_component(name) {
                invalid_names = invalid_names.checked_add(1).ok_or(FilesError::TooLarge)?;
                continue;
            }
            visit(name)?;
        }
        let after = directory.stat().map_err(|_| FilesError::Unavailable)?;
        if snapshot_from_stat(&after)? != pinned.snapshot {
            return Err(FilesError::Changed);
        }
        let current = self.pin(path)?;
        if current.snapshot != pinned.snapshot || current.kind != ObjectKind::Directory {
            return Err(FilesError::Changed);
        }
        Ok(invalid_names)
    }

    pub(crate) fn read_regular(
        &self,
        pinned: &PinnedObject,
        maximum_bytes: usize,
        context: &OperationContext,
    ) -> Result<Vec<u8>, FilesError> {
        context.checkpoint()?;
        if pinned.kind != ObjectKind::File || pinned.snapshot.links != 1 {
            return Err(FilesError::Unsupported);
        }
        if pinned.snapshot.size > maximum_bytes as u64 {
            return Err(FilesError::TooLarge);
        }

        // The candidate was first pinned and type-checked with O_PATH. Only this
        // process-constructed procfd path is followed for the readable reopen.
        let readable = reopen_procfd(&pinned.fd, ObjectKind::File)?;
        let before = snapshot(&readable)?;
        if before != pinned.snapshot || before.links != 1 {
            return Err(FilesError::Changed);
        }
        let capacity = usize::try_from(before.size)
            .unwrap_or(maximum_bytes)
            .min(maximum_bytes);
        let mut bytes = Vec::with_capacity(capacity);
        let mut file = std::fs::File::from(readable);
        let mut chunk = [0_u8; WFE_FILES_READ_CHUNK_BYTES];
        loop {
            context.checkpoint()?;
            let read = file.read(&mut chunk).map_err(|_| FilesError::Unavailable)?;
            if read == 0 {
                break;
            }
            let new_length = bytes.len().checked_add(read).ok_or(FilesError::TooLarge)?;
            if new_length > maximum_bytes {
                return Err(FilesError::TooLarge);
            }
            bytes.extend_from_slice(&chunk[..read]);
        }
        let after = rustix::fs::fstat(&file).map_err(|_| FilesError::Unavailable)?;
        let after = snapshot_from_stat(&after)?;
        if after != before || after.links != 1 || after.size != bytes.len() as u64 {
            return Err(FilesError::Changed);
        }
        Ok(bytes)
    }
}

impl PinnedObject {
    pub(crate) fn kind(&self) -> ObjectKind {
        self.kind
    }

    pub(crate) fn identity(&self) -> FileIdentity {
        self.snapshot.identity
    }

    pub(crate) fn size(&self) -> u64 {
        self.snapshot.size
    }

    pub(crate) fn same_snapshot(&self, other: &Self) -> bool {
        self.kind == other.kind && self.snapshot == other.snapshot
    }
}

fn reopen_procfd(fd: &OwnedFd, expected: ObjectKind) -> Result<OwnedFd, FilesError> {
    let path = format!("/proc/self/fd/{}", fd.as_raw_fd());
    let mut flags = OFlags::RDONLY | OFlags::CLOEXEC;
    if expected == ObjectKind::Directory {
        flags |= OFlags::DIRECTORY;
    }
    let reopened = rustix::fs::open(path, flags, Mode::empty()).map_err(|error| {
        if error == rustix::io::Errno::ACCESS || error == rustix::io::Errno::PERM {
            FilesError::Unavailable
        } else {
            FilesError::Unsupported
        }
    })?;
    let observed = snapshot(&reopened)?;
    if object_kind(&observed) != Some(expected) {
        return Err(FilesError::Changed);
    }
    Ok(reopened)
}

fn trusted_procfd_path(fd: &OwnedFd) -> Result<PathBuf, FilesError> {
    std::fs::read_link(format!("/proc/self/fd/{}", fd.as_raw_fd()))
        .map_err(|_| FilesError::Unsupported)
}

fn snapshot(fd: &impl std::os::fd::AsFd) -> Result<ObjectSnapshot, FilesError> {
    let stat = rustix::fs::fstat(fd).map_err(|_| FilesError::Unavailable)?;
    snapshot_from_stat(&stat)
}

fn snapshot_from_stat(stat: &rustix::fs::Stat) -> Result<ObjectSnapshot, FilesError> {
    let size = u64::try_from(stat.st_size).map_err(|_| FilesError::Unsupported)?;
    let modified_nanoseconds =
        i64::try_from(stat.st_mtime_nsec).map_err(|_| FilesError::Unsupported)?;
    let changed_nanoseconds =
        i64::try_from(stat.st_ctime_nsec).map_err(|_| FilesError::Unsupported)?;
    Ok(ObjectSnapshot {
        identity: FileIdentity {
            device: stat.st_dev,
            inode: stat.st_ino,
        },
        mode: stat.st_mode,
        links: widen_u64(stat.st_nlink),
        size,
        modified_seconds: stat.st_mtime,
        modified_nanoseconds,
        changed_seconds: stat.st_ctime,
        changed_nanoseconds,
    })
}

fn widen_u64<T: Into<u64>>(value: T) -> u64 {
    value.into()
}

fn object_kind(snapshot: &ObjectSnapshot) -> Option<ObjectKind> {
    let file_type = FileType::from_raw_mode(snapshot.mode);
    if file_type.is_file() {
        Some(ObjectKind::File)
    } else if file_type.is_dir() {
        Some(ObjectKind::Directory)
    } else {
        None
    }
}

fn map_root_open_error(error: rustix::io::Errno) -> FilesError {
    if error == rustix::io::Errno::NOENT || error == rustix::io::Errno::NOTDIR {
        FilesError::NotFound
    } else if error == rustix::io::Errno::LOOP {
        FilesError::Unsupported
    } else {
        FilesError::Unavailable
    }
}

fn map_candidate_open_error(error: rustix::io::Errno) -> FilesError {
    if error == rustix::io::Errno::NOENT || error == rustix::io::Errno::NOTDIR {
        FilesError::NotFound
    } else if error == rustix::io::Errno::LOOP || error == rustix::io::Errno::XDEV {
        FilesError::Unsupported
    } else {
        FilesError::Unavailable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn special_inode_types_are_not_candidates() {
        for mode in [
            libc::S_IFLNK,
            libc::S_IFIFO,
            libc::S_IFSOCK,
            libc::S_IFCHR,
            libc::S_IFBLK,
        ] {
            let snapshot = ObjectSnapshot {
                identity: FileIdentity {
                    device: 1,
                    inode: 1,
                },
                mode,
                links: 1,
                size: 0,
                modified_seconds: 0,
                modified_nanoseconds: 0,
                changed_seconds: 0,
                changed_nanoseconds: 0,
            };
            assert_eq!(object_kind(&snapshot), None);
        }
    }

    #[test]
    fn whole_path_resolution_enforces_all_confinement_flags() {
        assert!(RESOLVE_FLAGS.contains(ResolveFlags::BENEATH));
        assert!(RESOLVE_FLAGS.contains(ResolveFlags::NO_SYMLINKS));
        assert!(RESOLVE_FLAGS.contains(ResolveFlags::NO_MAGICLINKS));
        assert!(RESOLVE_FLAGS.contains(ResolveFlags::NO_XDEV));
        assert_eq!(
            map_candidate_open_error(rustix::io::Errno::XDEV),
            FilesError::Unsupported
        );
    }
}
