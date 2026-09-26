use super::{FileIdentity, FilesError, OperationContext, RelativePath};
use std::path::Path;

pub(crate) struct PinnedRoot;
pub(crate) struct PinnedObject;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ObjectKind {
    File,
    Directory,
}

impl PinnedRoot {
    pub(crate) fn open(_path: &Path) -> Result<Self, FilesError> {
        Err(FilesError::Unsupported)
    }

    pub(crate) fn canonical_path(&self) -> &Path {
        Path::new("")
    }

    pub(crate) fn identity_for_explicit_path(
        &self,
        _path: &Path,
    ) -> Result<Option<FileIdentity>, FilesError> {
        Err(FilesError::Unsupported)
    }

    pub(crate) fn pin(&self, _path: &RelativePath) -> Result<PinnedObject, FilesError> {
        Err(FilesError::Unsupported)
    }

    pub(crate) fn visit_directory(
        &self,
        _path: &RelativePath,
        _expected: &PinnedObject,
        _context: &OperationContext,
        _visit: impl FnMut(&str) -> Result<(), FilesError>,
    ) -> Result<u32, FilesError> {
        Err(FilesError::Unsupported)
    }

    pub(crate) fn read_regular(
        &self,
        _pinned: &PinnedObject,
        _maximum_bytes: usize,
        _context: &OperationContext,
    ) -> Result<Vec<u8>, FilesError> {
        Err(FilesError::Unsupported)
    }
}

impl PinnedObject {
    pub(crate) fn kind(&self) -> ObjectKind {
        ObjectKind::File
    }

    pub(crate) fn identity(&self) -> FileIdentity {
        FileIdentity {
            device: 0,
            inode: 0,
        }
    }

    pub(crate) fn size(&self) -> u64 {
        0
    }

    pub(crate) fn same_snapshot(&self, _other: &Self) -> bool {
        false
    }
}
