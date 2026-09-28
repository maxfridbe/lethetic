//! Linux-first, rooted, read-only filesystem capability for the optional WFE.
//!
//! Filesystem authority is the pinned launch-root descriptor. User paths are
//! never joined to a host path for I/O.

mod archive;
mod git;
#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod platform;
#[cfg(not(target_os = "linux"))]
#[path = "unsupported.rs"]
mod platform;
mod policy;

pub use policy::DisclosurePolicy;

use crate::wfe::file_contracts::{
    FileEntryKind, FileExclusionCounts, FileListEntry, FilesApiError, FilesArchiveRequest,
    FilesDownloadRequest, FilesErrorCode, FilesListRequest, FilesListResponse, FilesReadRequest,
    FilesReadResponse, GitDiffRequest, GitDiffResponse, GitStatusRequest, GitStatusResponse,
    WFE_FILES_DOWNLOAD_BYTES, WFE_FILES_LIST_ENTRIES, WFE_FILES_PATH_BYTES,
    WFE_FILES_PATH_DEPTH, WFE_FILES_VIEW_BYTES,
};
use platform::{ObjectKind, PinnedRoot};
use policy::ExclusionKind;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Component, Path};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

const ORDINARY_OPERATION_DEADLINE: Duration = Duration::from_secs(15);
const ARCHIVE_OPERATION_DEADLINE: Duration = Duration::from_secs(45);
const FILE_OPERATION_CAPACITY: usize = 2;
const ARCHIVE_OPERATION_CAPACITY: usize = 1;
pub const MAX_REGISTERED_SECRET_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(crate) struct FileIdentity {
    device: u64,
    inode: u64,
}

#[derive(Clone)]
pub struct RootedFiles {
    root: Arc<PinnedRoot>,
    policy: Arc<DisclosurePolicy>,
    limits: Arc<WorkerLimits>,
}

struct WorkerLimits {
    all: Arc<Semaphore>,
    archives: Arc<Semaphore>,
}

pub struct FileOperation<T> {
    value: T,
    guard: FileOperationGuard,
}

pub struct FileOperationGuard {
    _permits: WorkerPermits,
    cancellation: CancellationToken,
}

struct WorkerPermits {
    _all: OwnedSemaphorePermit,
    _archive: Option<OwnedSemaphorePermit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDownload {
    pub bytes: Vec<u8>,
    pub filename: String,
    pub content_type: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileArchive {
    pub bytes: Vec<u8>,
    pub filename: String,
    pub exclusions: FileExclusionCounts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilesError {
    BadRequest,
    NotFound,
    Protected,
    Unsupported,
    PreviewUnavailable,
    TooLarge,
    SensitiveContent,
    Changed,
    Busy,
    Deadline,
    Cancelled,
    Unavailable,
}

impl RootedFiles {
    /// Pins `root` before deriving its canonical display identity. No later cwd,
    /// session, or application-state change can retarget this capability.
    pub fn open(root: &Path, policy: DisclosurePolicy) -> Result<Self, FilesError> {
        let root = PinnedRoot::open(root)?;
        let canonical = root
            .canonical_path()
            .to_str()
            .ok_or(FilesError::Unsupported)?;
        let components: Vec<&str> = canonical
            .split('/')
            .filter(|component| !component.is_empty())
            .collect();
        if policy.root_is_protected(&components) {
            return Err(FilesError::Protected);
        }
        Ok(Self {
            root: Arc::new(root),
            policy: Arc::new(policy),
            limits: Arc::new(WorkerLimits {
                all: Arc::new(Semaphore::new(FILE_OPERATION_CAPACITY)),
                archives: Arc::new(Semaphore::new(ARCHIVE_OPERATION_CAPACITY)),
            }),
        })
    }

    /// Protects the supplied logical path and, when it exists, its current
    /// device/inode identity. This performs no content read.
    pub fn protect_path(&mut self, path: &Path) -> Result<(), FilesError> {
        let mut relative_components = Vec::new();
        if path.is_absolute() {
            if self.root.canonical_path().starts_with(path) {
                return Err(FilesError::Protected);
            }
            if let Ok(relative) = path.strip_prefix(self.root.canonical_path()) {
                relative_components.push(path_components(relative)?);
            }
            if let Ok(canonical) = std::fs::canonicalize(path) {
                if self.root.canonical_path().starts_with(&canonical) {
                    return Err(FilesError::Protected);
                }
                if let Ok(relative) = canonical.strip_prefix(self.root.canonical_path()) {
                    let components = path_components(relative)?;
                    if !relative_components.contains(&components) {
                        relative_components.push(components);
                    }
                }
            }
        } else {
            let encoded = path.to_str().ok_or(FilesError::BadRequest)?;
            let relative = RelativePath::parse(encoded, false)?;
            relative_components.push(relative.owned_components());
        }
        if relative_components.iter().any(Vec::is_empty) {
            return Err(FilesError::Protected);
        }

        let identity = self.root.identity_for_explicit_path(path)?;
        let policy = Arc::make_mut(&mut self.policy);
        for components in relative_components {
            policy.protect_components(components);
        }
        if let Some(identity) = identity {
            policy.protect_identity(identity);
        }
        Ok(())
    }

    /// Adds one exact in-memory credential to the refusal scanner. Empty values
    /// are ignored; values are zeroized with the policy clone that owns them.
    pub fn register_secret(&mut self, secret: &str) -> Result<(), FilesError> {
        Arc::make_mut(&mut self.policy).register_secret(secret)
    }

    pub async fn list(
        &self,
        request: FilesListRequest,
        cancellation: CancellationToken,
    ) -> Result<FileOperation<FilesListResponse>, FilesError> {
        self.run(false, cancellation, move |root, policy, context| {
            list_sync(root, policy, request, context)
        })
        .await
    }

    pub async fn read(
        &self,
        request: FilesReadRequest,
        cancellation: CancellationToken,
    ) -> Result<FileOperation<FilesReadResponse>, FilesError> {
        self.run(false, cancellation, move |root, policy, context| {
            read_sync(root, policy, request, context)
        })
        .await
    }

    pub async fn download(
        &self,
        request: FilesDownloadRequest,
        cancellation: CancellationToken,
    ) -> Result<FileOperation<FileDownload>, FilesError> {
        self.run(false, cancellation, move |root, policy, context| {
            download_sync(root, policy, request, context)
        })
        .await
    }

    pub async fn archive(
        &self,
        request: FilesArchiveRequest,
        cancellation: CancellationToken,
    ) -> Result<FileOperation<FileArchive>, FilesError> {
        self.run(true, cancellation, move |root, policy, context| {
            archive::archive_sync(root, policy, request, context)
        })
        .await
    }

    /// Lists changed files in the launch root's git work tree.
    pub async fn git_status(
        &self,
        request: GitStatusRequest,
        cancellation: CancellationToken,
    ) -> Result<FileOperation<GitStatusResponse>, FilesError> {
        self.run(false, cancellation, move |root, policy, context| {
            git::status_sync(root, policy, request, context)
        })
        .await
    }

    /// Returns one changed file's `HEAD` and working-tree content.
    pub async fn git_diff(
        &self,
        request: GitDiffRequest,
        cancellation: CancellationToken,
    ) -> Result<FileOperation<GitDiffResponse>, FilesError> {
        self.run(false, cancellation, move |root, policy, context| {
            git::diff_sync(root, policy, request, context)
        })
        .await
    }

    async fn run<T, F>(
        &self,
        archive: bool,
        server_cancellation: CancellationToken,
        operation: F,
    ) -> Result<FileOperation<T>, FilesError>
    where
        T: Send + 'static,
        F: FnOnce(&PinnedRoot, &DisclosurePolicy, &OperationContext) -> Result<T, FilesError>
            + Send
            + 'static,
    {
        let all = self
            .limits
            .all
            .clone()
            .try_acquire_owned()
            .map_err(|_| FilesError::Busy)?;
        let archive_permit = if archive {
            Some(
                self.limits
                    .archives
                    .clone()
                    .try_acquire_owned()
                    .map_err(|_| FilesError::Busy)?,
            )
        } else {
            None
        };
        let permits = WorkerPermits {
            _all: all,
            _archive: archive_permit,
        };
        let cancellation = server_cancellation.child_token();
        let cancellation_for_worker = cancellation.clone();
        let root = Arc::clone(&self.root);
        let policy = Arc::clone(&self.policy);
        let deadline = Instant::now()
            + if archive {
                ARCHIVE_OPERATION_DEADLINE
            } else {
                ORDINARY_OPERATION_DEADLINE
            };
        let worker = tokio::task::spawn_blocking(move || {
            let context = OperationContext {
                cancellation: cancellation_for_worker,
                deadline,
            };
            let result = operation(&root, &policy, &context);
            (result, permits)
        });

        // This local guard makes dropping an in-flight HTTP handler observable to
        // chunked traversal/read loops. The blocking syscall itself is not claimed
        // to be forcibly interruptible.
        let cancel_on_drop = CancelOnDrop::new(cancellation);
        let (result, permits) = worker.await.map_err(|_| FilesError::Unavailable)?;
        let value = result?;
        let cancellation = cancel_on_drop.into_token();
        Ok(FileOperation {
            value,
            guard: FileOperationGuard {
                _permits: permits,
                cancellation,
            },
        })
    }
}

impl<T> FileOperation<T> {
    pub fn value(&self) -> &T {
        &self.value
    }

    pub fn into_parts(self) -> (T, FileOperationGuard) {
        (self.value, self.guard)
    }

    pub fn into_value(self) -> T {
        self.value
    }
}

impl Drop for FileOperationGuard {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

impl<T> fmt::Debug for FileOperation<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("FileOperation(<guarded response>)")
    }
}

impl fmt::Debug for RootedFiles {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RootedFiles(<pinned launch root>)")
    }
}

impl fmt::Debug for FileOperationGuard {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("FileOperationGuard(<active>)")
    }
}

impl FilesError {
    pub const fn code(self) -> FilesErrorCode {
        match self {
            Self::BadRequest => FilesErrorCode::BadRequest,
            Self::NotFound => FilesErrorCode::NotFound,
            Self::Protected => FilesErrorCode::Protected,
            Self::Unsupported => FilesErrorCode::Unsupported,
            Self::PreviewUnavailable => FilesErrorCode::PreviewUnavailable,
            Self::TooLarge => FilesErrorCode::TooLarge,
            Self::SensitiveContent => FilesErrorCode::SensitiveContent,
            Self::Changed => FilesErrorCode::Changed,
            Self::Busy => FilesErrorCode::Busy,
            Self::Deadline => FilesErrorCode::Deadline,
            Self::Cancelled => FilesErrorCode::Cancelled,
            Self::Unavailable => FilesErrorCode::Unavailable,
        }
    }

    pub const fn message(self) -> &'static str {
        match self {
            Self::BadRequest => "The file request was invalid",
            Self::NotFound => "The selected item was not found",
            Self::Protected => "The selected item is protected",
            Self::Unsupported => "The selected item is not an eligible ordinary file or directory",
            Self::PreviewUnavailable => "This file cannot be displayed as a bounded UTF-8 preview",
            Self::TooLarge => "The file operation exceeded a fixed limit",
            Self::SensitiveContent => {
                "The selected content matched the credential disclosure policy"
            }
            Self::Changed => "The selected item changed during the operation",
            Self::Busy => "The file service is busy",
            Self::Deadline => "The file operation exceeded its deadline",
            Self::Cancelled => "The file operation was cancelled",
            Self::Unavailable => "The file service could not complete the operation",
        }
    }

    pub const fn retryable(self) -> bool {
        matches!(
            self,
            Self::Changed | Self::Busy | Self::Deadline | Self::Cancelled | Self::Unavailable
        )
    }

    pub fn api_error(self) -> FilesApiError {
        FilesApiError {
            code: self.code(),
            message: self.message().to_string(),
            retryable: self.retryable(),
        }
    }
}

impl fmt::Display for FilesError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for FilesError {}

pub(crate) struct OperationContext {
    cancellation: CancellationToken,
    deadline: Instant,
}

impl OperationContext {
    pub(crate) fn checkpoint(&self) -> Result<(), FilesError> {
        if self.cancellation.is_cancelled() {
            return Err(FilesError::Cancelled);
        }
        if Instant::now() >= self.deadline {
            return Err(FilesError::Deadline);
        }
        Ok(())
    }
}

struct CancelOnDrop(Option<CancellationToken>);

impl CancelOnDrop {
    fn new(token: CancellationToken) -> Self {
        Self(Some(token))
    }

    fn into_token(mut self) -> CancellationToken {
        self.0
            .take()
            .expect("cancellation guard is armed until conversion")
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(token) = self.0.take() {
            token.cancel();
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RelativePath {
    encoded: String,
    depth: usize,
}

impl RelativePath {
    pub(crate) fn parse(value: &str, allow_root: bool) -> Result<Self, FilesError> {
        if value.is_empty() {
            return allow_root
                .then(|| Self {
                    encoded: String::new(),
                    depth: 0,
                })
                .ok_or(FilesError::BadRequest);
        }
        if value.len() > WFE_FILES_PATH_BYTES
            || value.starts_with('/')
            || value.ends_with('/')
            || value.contains('\\')
            || value.chars().any(char::is_control)
        {
            return Err(FilesError::BadRequest);
        }
        let mut depth = 0_usize;
        for component in value.split('/') {
            if !valid_component(component) || windows_prefix_component(component) {
                return Err(FilesError::BadRequest);
            }
            depth = depth.checked_add(1).ok_or(FilesError::BadRequest)?;
        }
        if depth == 0 || depth > WFE_FILES_PATH_DEPTH {
            return Err(FilesError::BadRequest);
        }
        Ok(Self {
            encoded: value.to_string(),
            depth,
        })
    }

    pub(crate) fn child(&self, name: &str) -> Result<Self, FilesError> {
        if !valid_component(name) || windows_prefix_component(name) {
            return Err(FilesError::BadRequest);
        }
        let depth = self.depth.checked_add(1).ok_or(FilesError::TooLarge)?;
        if depth > WFE_FILES_PATH_DEPTH {
            return Err(FilesError::TooLarge);
        }
        let required = self
            .encoded
            .len()
            .checked_add(usize::from(!self.encoded.is_empty()))
            .and_then(|length| length.checked_add(name.len()))
            .ok_or(FilesError::TooLarge)?;
        if required > WFE_FILES_PATH_BYTES {
            return Err(FilesError::TooLarge);
        }
        let mut encoded = String::with_capacity(required);
        if !self.encoded.is_empty() {
            encoded.push_str(&self.encoded);
            encoded.push('/');
        }
        encoded.push_str(name);
        Ok(Self { encoded, depth })
    }

    pub(crate) fn is_root(&self) -> bool {
        self.encoded.is_empty()
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.encoded
    }

    pub(crate) fn components(&self) -> impl Iterator<Item = &str> {
        self.encoded
            .split('/')
            .filter(|component| !component.is_empty())
    }

    pub(crate) fn owned_components(&self) -> Vec<String> {
        self.components().map(str::to_string).collect()
    }

    pub(crate) fn basename(&self) -> Option<&str> {
        self.encoded
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
    }
}

pub(crate) fn valid_component(component: &str) -> bool {
    !component.is_empty()
        && !matches!(component, "." | "..")
        && !component.contains(['/', '\\'])
        && !component.chars().any(char::is_control)
}

fn windows_prefix_component(component: &str) -> bool {
    let bytes = component.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn path_components(path: &Path) -> Result<Vec<String>, FilesError> {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(component) => {
                let component = component.to_str().ok_or(FilesError::BadRequest)?;
                if !valid_component(component) || windows_prefix_component(component) {
                    return Err(FilesError::BadRequest);
                }
                components.push(component.to_string());
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(FilesError::BadRequest);
            }
        }
    }
    Ok(components)
}

fn list_sync(
    root: &PinnedRoot,
    policy: &DisclosurePolicy,
    request: FilesListRequest,
    context: &OperationContext,
) -> Result<FilesListResponse, FilesError> {
    let path = RelativePath::parse(&request.path, true)?;
    let selected = authorized_object(root, policy, &path, context)?;
    if selected.kind() != ObjectKind::Directory {
        return Err(FilesError::Unsupported);
    }

    let mut selected_entries: BTreeMap<(u8, String), FileListEntry> = BTreeMap::new();
    let mut eligible = 0_usize;
    let mut exclusions = FileExclusionCounts::default();
    let invalid_names = root.visit_directory(&path, &selected, context, |name| {
        let child = match path.child(name) {
            Ok(child) => child,
            Err(FilesError::TooLarge | FilesError::BadRequest) => {
                increment_exclusion(&mut exclusions, ExclusionKind::Unsupported)?;
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        if policy.path_is_protected(&child.components().collect::<Vec<_>>()) {
            increment_exclusion(&mut exclusions, ExclusionKind::Protected)?;
            return Ok(());
        }
        let object = match root.pin(&child) {
            Ok(object) => object,
            Err(FilesError::Unsupported) => {
                increment_exclusion(&mut exclusions, ExclusionKind::Unsupported)?;
                return Ok(());
            }
            Err(FilesError::NotFound | FilesError::Changed | FilesError::Unavailable) => {
                increment_exclusion(&mut exclusions, ExclusionKind::Unreadable)?;
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        if policy.identity_is_protected(object.identity()) {
            increment_exclusion(&mut exclusions, ExclusionKind::Protected)?;
            return Ok(());
        }
        let (kind, order, size) = match object.kind() {
            ObjectKind::Directory => (FileEntryKind::Directory, 0, None),
            ObjectKind::File => (FileEntryKind::File, 1, Some(object.size().to_string())),
        };
        eligible = eligible.checked_add(1).ok_or(FilesError::TooLarge)?;
        let entry = FileListEntry {
            name: name.to_string(),
            path: child.as_str().to_string(),
            kind,
            size,
        };
        selected_entries.insert((order, name.to_string()), entry);
        if selected_entries.len() > WFE_FILES_LIST_ENTRIES {
            selected_entries.pop_last();
        }
        Ok(())
    })?;
    exclusions.unsupported = exclusions
        .unsupported
        .checked_add(invalid_names)
        .ok_or(FilesError::TooLarge)?;
    let response = FilesListResponse {
        path: path.as_str().to_string(),
        entries: selected_entries.into_values().collect(),
        truncated: eligible > WFE_FILES_LIST_ENTRIES,
        exclusions,
    };
    scan_json_response(policy, &response)?;
    Ok(response)
}

fn read_sync(
    root: &PinnedRoot,
    policy: &DisclosurePolicy,
    request: FilesReadRequest,
    context: &OperationContext,
) -> Result<FilesReadResponse, FilesError> {
    let path = RelativePath::parse(&request.path, false)?;
    let object = authorized_object(root, policy, &path, context)?;
    if object.kind() != ObjectKind::File {
        return Err(FilesError::Unsupported);
    }
    let bytes = match root.read_regular(&object, WFE_FILES_VIEW_BYTES, context) {
        Err(FilesError::TooLarge) => return Err(FilesError::PreviewUnavailable),
        result => result?,
    };
    ensure_disclosable(policy, &bytes)?;
    let content = String::from_utf8(bytes).map_err(|_| FilesError::PreviewUnavailable)?;
    let response = FilesReadResponse {
        path: path.as_str().to_string(),
        size: content.len().to_string(),
        content,
    };
    scan_json_response(policy, &response)?;
    Ok(response)
}

fn download_sync(
    root: &PinnedRoot,
    policy: &DisclosurePolicy,
    request: FilesDownloadRequest,
    context: &OperationContext,
) -> Result<FileDownload, FilesError> {
    let path = RelativePath::parse(&request.path, false)?;
    let object = authorized_object(root, policy, &path, context)?;
    if object.kind() != ObjectKind::File {
        return Err(FilesError::Unsupported);
    }
    let bytes = root.read_regular(&object, WFE_FILES_DOWNLOAD_BYTES, context)?;
    ensure_disclosable(policy, &bytes)?;
    let basename = path.basename().ok_or(FilesError::BadRequest)?;
    let filename = safe_attachment_filename(basename, false);
    ensure_disclosable(policy, filename.as_bytes())?;
    Ok(FileDownload {
        bytes,
        filename,
        content_type: content_type_for(basename),
    })
}

pub(crate) fn authorized_object(
    root: &PinnedRoot,
    policy: &DisclosurePolicy,
    path: &RelativePath,
    context: &OperationContext,
) -> Result<platform::PinnedObject, FilesError> {
    context.checkpoint()?;
    let components: Vec<&str> = path.components().collect();
    if policy.path_is_protected(&components) {
        return Err(FilesError::Protected);
    }
    if path.is_root() {
        let object = root.pin(path)?;
        return if policy.identity_is_protected(object.identity()) {
            Err(FilesError::Protected)
        } else {
            Ok(object)
        };
    }

    // Every prefix is independently resolved from the pinned launch root. This
    // makes a renamed protected directory remain protected by identity without
    // granting authority through a cached/stale subdirectory descriptor.
    let mut prefix = RelativePath {
        encoded: String::new(),
        depth: 0,
    };
    let mut pinned_prefixes = Vec::with_capacity(components.len() + 1);
    let root_object = root.pin(&prefix)?;
    if policy.identity_is_protected(root_object.identity()) {
        return Err(FilesError::Protected);
    }
    pinned_prefixes.push((prefix.clone(), root_object));
    for (index, component) in components.iter().enumerate() {
        context.checkpoint()?;
        prefix = prefix.child(component)?;
        let object = root.pin(&prefix)?;
        if policy.identity_is_protected(object.identity()) {
            return Err(FilesError::Protected);
        }
        if index + 1 < components.len() && object.kind() != ObjectKind::Directory {
            return Err(FilesError::Unsupported);
        }
        pinned_prefixes.push((prefix.clone(), object));
    }
    for (prefix, before) in &pinned_prefixes {
        context.checkpoint()?;
        let after = root.pin(prefix)?;
        if !before.same_snapshot(&after) {
            return Err(FilesError::Changed);
        }
        if policy.identity_is_protected(after.identity()) {
            return Err(FilesError::Protected);
        }
    }
    pinned_prefixes
        .pop()
        .map(|(_, selected)| selected)
        .ok_or(FilesError::BadRequest)
}

pub(crate) fn ensure_disclosable(
    policy: &DisclosurePolicy,
    bytes: &[u8],
) -> Result<(), FilesError> {
    if policy.bytes_are_sensitive(bytes) {
        Err(FilesError::SensitiveContent)
    } else {
        Ok(())
    }
}

fn scan_json_response(
    policy: &DisclosurePolicy,
    response: &impl serde::Serialize,
) -> Result<(), FilesError> {
    let encoded = serde_json::to_vec(response).map_err(|_| FilesError::Unavailable)?;
    ensure_disclosable(policy, &encoded)
}

pub(crate) fn increment_exclusion(
    exclusions: &mut FileExclusionCounts,
    kind: ExclusionKind,
) -> Result<(), FilesError> {
    let counter = match kind {
        ExclusionKind::Protected => &mut exclusions.protected,
        ExclusionKind::Unsupported => &mut exclusions.unsupported,
        ExclusionKind::Unreadable => &mut exclusions.unreadable,
    };
    *counter = counter.checked_add(1).ok_or(FilesError::TooLarge)?;
    Ok(())
}

pub(crate) fn safe_attachment_filename(name: &str, archive: bool) -> String {
    let suffix = if archive { ".zip" } else { "" };
    let maximum_stem = 96_usize.saturating_sub(suffix.len());
    let mut sanitized = String::with_capacity(name.len().min(maximum_stem) + suffix.len());
    for character in name.chars() {
        if sanitized.len() >= maximum_stem {
            break;
        }
        let safe = if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
            character
        } else {
            '_'
        };
        if safe.len_utf8() + sanitized.len() <= maximum_stem {
            sanitized.push(safe);
        }
    }
    while sanitized.starts_with('.') {
        sanitized.replace_range(..1, "_");
    }
    if sanitized.is_empty() || matches!(sanitized.as_str(), "." | "..") {
        sanitized.push_str(if archive { "workspace" } else { "download" });
    }
    if archive && sanitized.to_ascii_lowercase().ends_with(".zip") {
        sanitized.truncate(sanitized.len() - 4);
        if sanitized.is_empty() {
            sanitized.push_str("workspace");
        }
    }
    sanitized.push_str(suffix);
    sanitized
}

fn content_type_for(name: &str) -> &'static str {
    let extension = name.rsplit_once('.').map(|(_, extension)| extension);
    match extension.map(str::to_ascii_lowercase).as_deref() {
        Some("txt" | "md" | "rs" | "toml" | "yaml" | "yml" | "py" | "sh" | "css") => {
            "text/plain; charset=utf-8"
        }
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("js" | "mjs" | "cjs") => "text/javascript; charset=utf-8",
        Some("ts" | "tsx") => "text/plain; charset=utf-8",
        Some("json") => "application/json",
        Some("xml") => "application/xml",
        Some("pdf") => "application/pdf",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("wasm") => "application/wasm",
        Some("zip") => "application/zip",
        Some("gz") => "application/gzip",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_reject_platform_and_escape_forms() {
        for invalid in [
            "/absolute",
            "../escape",
            "a/../b",
            "a/./b",
            "a//b",
            "a/",
            "C:/windows",
            "a\\b",
            "a\0b",
        ] {
            assert_eq!(
                RelativePath::parse(invalid, true),
                Err(FilesError::BadRequest),
                "{invalid:?}"
            );
        }
        assert!(RelativePath::parse("src/lib.rs", false).is_ok());
        assert!(RelativePath::parse("", true).is_ok());
        assert_eq!(RelativePath::parse("", false), Err(FilesError::BadRequest));
    }

    #[test]
    fn attachment_names_are_ascii_bounded_and_not_hidden() {
        assert_eq!(safe_attachment_filename(".env", false), "_env");
        assert_eq!(safe_attachment_filename("résumé", false), "r_sum_");
        assert_eq!(safe_attachment_filename("folder", true), "folder.zip");
        assert!(safe_attachment_filename(&"a".repeat(500), true).len() <= 96);
    }
}
