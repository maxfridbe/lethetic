//! Exact JSON contracts for the opt-in rooted read-only file service.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const WFE_FILES_REQUEST_BYTES: usize = 8 * 1024;
pub const WFE_FILES_PATH_BYTES: usize = 4 * 1024;
pub const WFE_FILES_PATH_DEPTH: usize = 64;
pub const WFE_FILES_LIST_ENTRIES: usize = 1_000;
pub const WFE_FILES_VIEW_BYTES: usize = 2 * 1024 * 1024;
pub const WFE_FILES_DOWNLOAD_BYTES: usize = 32 * 1024 * 1024;
pub const WFE_FILES_ARCHIVE_FILES: usize = 2_048;
pub const WFE_FILES_ARCHIVE_DIRECTORIES: usize = 2_048;
pub const WFE_FILES_ARCHIVE_INPUT_BYTES: usize = 56 * 1024 * 1024;
pub const WFE_FILES_ARCHIVE_OUTPUT_BYTES: usize = 64 * 1024 * 1024;
pub const WFE_FILES_READ_CHUNK_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum FileEntryKind {
    File,
    Directory,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct FileExclusionCounts {
    pub protected: u32,
    pub unsupported: u32,
    pub unreadable: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct FileListEntry {
    pub name: String,
    pub path: String,
    pub kind: FileEntryKind,
    /// Decimal bytes. A string avoids loss above JavaScript's safe-integer range.
    pub size: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct FilesListRequest {
    /// Empty selects the pinned launch root.
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct FilesListResponse {
    pub path: String,
    pub entries: Vec<FileListEntry>,
    pub truncated: bool,
    pub exclusions: FileExclusionCounts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct FilesReadRequest {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct FilesReadResponse {
    pub path: String,
    pub content: String,
    /// Decimal bytes. A string keeps the wire representation exact.
    pub size: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct FilesDownloadRequest {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct FilesArchiveRequest {
    /// Empty selects the pinned launch root.
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum FilesErrorCode {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct FilesApiError {
    pub code: FilesErrorCode,
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct FilesErrorResponse {
    pub error: FilesApiError,
}

/// Most changed files one git status response lists.
pub const WFE_GIT_STATUS_FILES: usize = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum GitChangeKind {
    Added,
    Modified,
    Deleted,
    Untracked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct GitChangedFile {
    /// Launch-root-relative path.
    pub path: String,
    pub kind: GitChangeKind,
    /// Added and removed line counts; `None` for binary files.
    pub added: Option<u32>,
    pub removed: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct GitStatusRequest {
    /// Empty selects the whole launch root.
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct GitStatusResponse {
    /// False when the launch root is not inside a git work tree.
    pub repository: bool,
    pub branch: Option<String>,
    pub files: Vec<GitChangedFile>,
    pub truncated: bool,
    /// Changed paths hidden by the disclosure policy.
    pub protected: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct GitDiffRequest {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct GitDiffResponse {
    pub path: String,
    /// Content at `HEAD`; empty when the file is new.
    pub original: String,
    /// Working-tree content; empty when the file was deleted.
    pub modified: String,
}
