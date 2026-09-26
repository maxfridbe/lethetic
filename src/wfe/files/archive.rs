use super::platform::{ObjectKind, PinnedRoot};
use super::policy::ExclusionKind;
use super::{
    DisclosurePolicy, FileArchive, FilesError, OperationContext, RelativePath, authorized_object,
    ensure_disclosable, increment_exclusion, safe_attachment_filename,
};
use crate::wfe::file_contracts::{
    FileExclusionCounts, FilesArchiveRequest, WFE_FILES_ARCHIVE_DIRECTORIES,
    WFE_FILES_ARCHIVE_FILES, WFE_FILES_ARCHIVE_INPUT_BYTES, WFE_FILES_ARCHIVE_OUTPUT_BYTES,
    WFE_FILES_READ_CHUNK_BYTES,
};
use std::io::{self, Cursor, Seek, SeekFrom, Write};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, System, ZipWriter};

pub(crate) fn archive_sync(
    root: &PinnedRoot,
    policy: &DisclosurePolicy,
    request: FilesArchiveRequest,
    context: &OperationContext,
) -> Result<FileArchive, FilesError> {
    let selected_path = RelativePath::parse(&request.path, true)?;
    let selected = authorized_object(root, policy, &selected_path, context)?;
    if selected.kind() != ObjectKind::Directory {
        return Err(FilesError::Unsupported);
    }

    let destination = CappedCursor::new(WFE_FILES_ARCHIVE_OUTPUT_BYTES);
    let writer = ZipWriter::new(destination);
    let mut state = ArchiveState {
        writer,
        files: 0,
        directories: 0,
        input_bytes: 0,
        exclusions: FileExclusionCounts::default(),
    };
    let archive_root = selected_path.basename().unwrap_or("").to_string();
    add_directory(
        root,
        policy,
        context,
        &mut state,
        &selected_path,
        &archive_root,
        Some(selected.identity()),
        !selected_path.is_root(),
    )?;
    context.checkpoint()?;
    let destination = state.writer.finish().map_err(map_zip_error)?;
    let bytes = destination.into_inner();
    if bytes.len() > WFE_FILES_ARCHIVE_OUTPUT_BYTES {
        return Err(FilesError::TooLarge);
    }
    ensure_disclosable(policy, &bytes)?;

    let filename = selected_path
        .basename()
        .map(|name| safe_attachment_filename(name, true))
        .unwrap_or_else(|| "workspace.zip".to_string());
    ensure_disclosable(policy, filename.as_bytes())?;
    Ok(FileArchive {
        bytes,
        filename,
        exclusions: state.exclusions,
    })
}

struct ArchiveState {
    writer: ZipWriter<CappedCursor>,
    files: usize,
    directories: usize,
    input_bytes: usize,
    exclusions: FileExclusionCounts,
}

#[allow(clippy::too_many_arguments)]
fn add_directory(
    root: &PinnedRoot,
    policy: &DisclosurePolicy,
    context: &OperationContext,
    state: &mut ArchiveState,
    path: &RelativePath,
    archive_path: &str,
    expected_identity: Option<super::FileIdentity>,
    emit_entry: bool,
) -> Result<(), FilesError> {
    context.checkpoint()?;
    let directory = authorized_object(root, policy, path, context)?;
    if directory.kind() != ObjectKind::Directory {
        return Err(FilesError::Changed);
    }
    if expected_identity.is_some_and(|identity| identity != directory.identity()) {
        return Err(FilesError::Changed);
    }
    state.directories = state
        .directories
        .checked_add(1)
        .ok_or(FilesError::TooLarge)?;
    if state.directories > WFE_FILES_ARCHIVE_DIRECTORIES {
        return Err(FilesError::TooLarge);
    }
    if emit_entry {
        let mut name = archive_path.to_string();
        if !name.ends_with('/') {
            name.push('/');
        }
        state
            .writer
            .add_directory(name, directory_options())
            .map_err(map_zip_error)?;
    }

    let invalid_names = root.visit_directory(path, &directory, context, |name| {
        let child = path.child(name)?;
        let child_archive_path = if archive_path.is_empty() {
            name.to_string()
        } else {
            format!("{archive_path}/{name}")
        };
        if policy.path_is_protected(&child.components().collect::<Vec<_>>()) {
            increment_exclusion(&mut state.exclusions, ExclusionKind::Protected)?;
            return Ok(());
        }
        let object = match root.pin(&child) {
            Ok(object) => object,
            Err(FilesError::Unsupported) => {
                increment_exclusion(&mut state.exclusions, ExclusionKind::Unsupported)?;
                return Ok(());
            }
            Err(FilesError::NotFound | FilesError::Changed) => return Err(FilesError::Changed),
            Err(FilesError::Unavailable) => {
                increment_exclusion(&mut state.exclusions, ExclusionKind::Unreadable)?;
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        if policy.identity_is_protected(object.identity()) {
            increment_exclusion(&mut state.exclusions, ExclusionKind::Protected)?;
            return Ok(());
        }
        match object.kind() {
            ObjectKind::Directory => add_directory(
                root,
                policy,
                context,
                state,
                &child,
                &child_archive_path,
                Some(object.identity()),
                true,
            ),
            ObjectKind::File => {
                add_file(root, policy, context, state, &object, &child_archive_path)
            }
        }
    })?;
    state.exclusions.unsupported = state
        .exclusions
        .unsupported
        .checked_add(invalid_names)
        .ok_or(FilesError::TooLarge)?;
    Ok(())
}

fn add_file(
    root: &PinnedRoot,
    policy: &DisclosurePolicy,
    context: &OperationContext,
    state: &mut ArchiveState,
    object: &super::platform::PinnedObject,
    archive_path: &str,
) -> Result<(), FilesError> {
    context.checkpoint()?;
    state.files = state.files.checked_add(1).ok_or(FilesError::TooLarge)?;
    if state.files > WFE_FILES_ARCHIVE_FILES {
        return Err(FilesError::TooLarge);
    }
    let declared_size = usize::try_from(object.size()).map_err(|_| FilesError::TooLarge)?;
    let prospective = state
        .input_bytes
        .checked_add(declared_size)
        .ok_or(FilesError::TooLarge)?;
    if prospective > WFE_FILES_ARCHIVE_INPUT_BYTES {
        return Err(FilesError::TooLarge);
    }
    let remaining = WFE_FILES_ARCHIVE_INPUT_BYTES - state.input_bytes;
    let bytes = root.read_regular(object, remaining, context)?;
    ensure_disclosable(policy, &bytes)?;
    state.input_bytes = state
        .input_bytes
        .checked_add(bytes.len())
        .ok_or(FilesError::TooLarge)?;
    if state.input_bytes > WFE_FILES_ARCHIVE_INPUT_BYTES {
        return Err(FilesError::TooLarge);
    }

    state
        .writer
        .start_file(archive_path, file_options())
        .map_err(map_zip_error)?;
    for chunk in bytes.chunks(WFE_FILES_READ_CHUNK_BYTES) {
        context.checkpoint()?;
        state.writer.write_all(chunk).map_err(map_io_error)?;
    }
    Ok(())
}

fn file_options() -> SimpleFileOptions {
    SimpleFileOptions::DEFAULT
        .compression_method(CompressionMethod::Stored)
        .unix_permissions(0o644)
        .system(System::Unix)
}

fn directory_options() -> SimpleFileOptions {
    SimpleFileOptions::DEFAULT
        .compression_method(CompressionMethod::Stored)
        .unix_permissions(0o755)
        .system(System::Unix)
}

fn map_zip_error(_error: zip::result::ZipError) -> FilesError {
    FilesError::TooLarge
}

fn map_io_error(_error: io::Error) -> FilesError {
    FilesError::TooLarge
}

struct CappedCursor {
    inner: Cursor<Vec<u8>>,
    maximum_bytes: u64,
}

impl CappedCursor {
    fn new(maximum_bytes: usize) -> Self {
        Self {
            inner: Cursor::new(Vec::new()),
            maximum_bytes: maximum_bytes as u64,
        }
    }

    fn into_inner(self) -> Vec<u8> {
        self.inner.into_inner()
    }
}

impl Write for CappedCursor {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let end = self
            .inner
            .position()
            .checked_add(buffer.len() as u64)
            .ok_or_else(output_limit_error)?;
        if end > self.maximum_bytes {
            return Err(output_limit_error());
        }
        self.inner.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Seek for CappedCursor {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let current = i128::from(self.inner.position());
        let end = i128::try_from(self.inner.get_ref().len()).map_err(|_| output_limit_error())?;
        let target = match position {
            SeekFrom::Start(position) => i128::from(position),
            SeekFrom::End(offset) => end
                .checked_add(i128::from(offset))
                .ok_or_else(output_limit_error)?,
            SeekFrom::Current(offset) => current
                .checked_add(i128::from(offset))
                .ok_or_else(output_limit_error)?,
        };
        if target < 0 || target > i128::from(self.maximum_bytes) {
            return Err(output_limit_error());
        }
        let target = u64::try_from(target).map_err(|_| output_limit_error())?;
        self.inner.set_position(target);
        Ok(target)
    }
}

fn output_limit_error() -> io::Error {
    io::Error::new(io::ErrorKind::FileTooLarge, "archive output limit exceeded")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capped_cursor_rejects_write_and_seek_past_limit() {
        let mut cursor = CappedCursor::new(4);
        cursor.write_all(b"1234").unwrap();
        assert!(cursor.write_all(b"5").is_err());
        assert!(cursor.seek(SeekFrom::Start(5)).is_err());
        cursor.seek(SeekFrom::Start(1)).unwrap();
        cursor.write_all(b"x").unwrap();
        assert_eq!(cursor.into_inner(), b"1x34");
    }
}
