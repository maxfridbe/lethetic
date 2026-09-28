//! Read-only git change view for the rooted file capability.
//!
//! Git runs in the pinned launch root's canonical directory with hooks,
//! external diff drivers, text conversion and fsmonitor disabled. Every path
//! it reports passes the same disclosure policy as the file browser, and
//! working-tree content is read only through the pinned root.

use super::platform::{ObjectKind, PinnedRoot};
use super::{
    DisclosurePolicy, FilesError, OperationContext, RelativePath, authorized_object,
    ensure_disclosable, scan_json_response,
};
use crate::wfe::file_contracts::{
    GitChangeKind, GitChangedFile, GitDiffRequest, GitDiffResponse, GitStatusRequest,
    GitStatusResponse, WFE_FILES_VIEW_BYTES, WFE_GIT_STATUS_FILES,
};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Git's well-known empty tree, the base when the repository has no commit.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
/// Bound for git's own listing output (paths and counts, not file content).
const GIT_LISTING_BYTES: usize = 4 * 1024 * 1024;

pub(super) fn status_sync(
    root: &PinnedRoot,
    policy: &DisclosurePolicy,
    request: GitStatusRequest,
    context: &OperationContext,
) -> Result<GitStatusResponse, FilesError> {
    let scope = RelativePath::parse(&request.path, true)?;
    let directory = root.canonical_path();
    let not_repository = GitStatusResponse {
        repository: false,
        branch: None,
        files: Vec::new(),
        truncated: false,
        protected: 0,
    };
    match run_git(directory, &["rev-parse", "--is-inside-work-tree"], 64, context)? {
        Some(output) if output.starts_with(b"true") => {}
        _ => return Ok(not_repository),
    }
    let branch = branch_name(directory, context)?;
    let base = if has_head(directory, context)? {
        "HEAD"
    } else {
        EMPTY_TREE
    };

    let diff = |format: &str| {
        run_git(
            directory,
            &[
                "diff",
                format,
                "-z",
                "--no-renames",
                "--relative",
                "--no-ext-diff",
                "--no-textconv",
                base,
                "--",
            ],
            GIT_LISTING_BYTES,
            context,
        )
    };
    let mut changes: BTreeMap<String, GitChangedFile> = BTreeMap::new();
    if let Some(names) = diff("--name-status")? {
        let mut fields = names.split(|byte| *byte == 0).filter(|f| !f.is_empty());
        while let (Some(status), Some(path)) = (fields.next(), fields.next()) {
            let kind = match status.first() {
                Some(b'A') => GitChangeKind::Added,
                Some(b'D') => GitChangeKind::Deleted,
                _ => GitChangeKind::Modified,
            };
            if let Ok(path) = std::str::from_utf8(path) {
                changes.insert(
                    path.to_string(),
                    GitChangedFile {
                        path: path.to_string(),
                        kind,
                        added: None,
                        removed: None,
                    },
                );
            }
        }
    }
    if let Some(counts) = diff("--numstat")? {
        for record in counts.split(|byte| *byte == 0).filter(|r| !r.is_empty()) {
            let Ok(record) = std::str::from_utf8(record) else {
                continue;
            };
            let mut parts = record.splitn(3, '\t');
            let (Some(added), Some(removed), Some(path)) =
                (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            if let Some(change) = changes.get_mut(path) {
                change.added = added.parse().ok();
                change.removed = removed.parse().ok();
            }
        }
    }
    let untracked = run_git(
        directory,
        &["ls-files", "--others", "--exclude-standard", "-z"],
        GIT_LISTING_BYTES,
        context,
    )?
    .unwrap_or_default();
    for path in untracked.split(|byte| *byte == 0).filter(|p| !p.is_empty()) {
        if let Ok(path) = std::str::from_utf8(path) {
            changes.insert(
                path.to_string(),
                GitChangedFile {
                    path: path.to_string(),
                    kind: GitChangeKind::Untracked,
                    added: None,
                    removed: Some(0),
                },
            );
        }
    }

    let mut files = Vec::new();
    let mut protected = 0_u32;
    let mut truncated = false;
    for (path, mut change) in changes {
        context.checkpoint()?;
        let Ok(relative) = RelativePath::parse(&path, false) else {
            continue;
        };
        if !scope.is_root()
            && !path
                .strip_prefix(scope.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
        {
            continue;
        }
        if policy.path_is_protected(&relative.components().collect::<Vec<_>>()) {
            protected = protected.saturating_add(1);
            continue;
        }
        // Existing files must also pass identity protection; untracked ones
        // are counted here because git reported no line numbers for them.
        match authorized_object(root, policy, &relative, context) {
            Ok(object) if change.kind == GitChangeKind::Untracked => {
                change.added = root
                    .read_regular(&object, WFE_FILES_VIEW_BYTES, context)
                    .ok()
                    .filter(|bytes| !policy.bytes_are_sensitive(bytes))
                    .and_then(|bytes| String::from_utf8(bytes).ok())
                    .map(|text| line_count(&text));
                if change.added.is_none() {
                    change.removed = None;
                }
            }
            Ok(_) => {}
            Err(FilesError::Protected) => {
                protected = protected.saturating_add(1);
                continue;
            }
            Err(FilesError::Cancelled) => return Err(FilesError::Cancelled),
            Err(FilesError::Deadline) => return Err(FilesError::Deadline),
            Err(_) => {}
        }
        if files.len() == WFE_GIT_STATUS_FILES {
            truncated = true;
            break;
        }
        files.push(change);
    }
    let response = GitStatusResponse {
        repository: true,
        branch,
        files,
        truncated,
        protected,
    };
    scan_json_response(policy, &response)?;
    Ok(response)
}

pub(super) fn diff_sync(
    root: &PinnedRoot,
    policy: &DisclosurePolicy,
    request: GitDiffRequest,
    context: &OperationContext,
) -> Result<GitDiffResponse, FilesError> {
    let path = RelativePath::parse(&request.path, false)?;
    if policy.path_is_protected(&path.components().collect::<Vec<_>>()) {
        return Err(FilesError::Protected);
    }
    let modified = match authorized_object(root, policy, &path, context) {
        Ok(object) if object.kind() == ObjectKind::File => {
            match root.read_regular(&object, WFE_FILES_VIEW_BYTES, context) {
                Err(FilesError::TooLarge) => return Err(FilesError::PreviewUnavailable),
                result => result?,
            }
        }
        Ok(_) => return Err(FilesError::Unsupported),
        Err(FilesError::NotFound) => Vec::new(),
        Err(error) => return Err(error),
    };
    let directory = root.canonical_path();
    let original = if has_head(directory, context)? {
        let object = format!("HEAD:./{}", path.as_str());
        match run_git(
            directory,
            &["show", "--no-textconv", &object],
            WFE_FILES_VIEW_BYTES,
            context,
        ) {
            Err(FilesError::TooLarge) => return Err(FilesError::PreviewUnavailable),
            result => result?.unwrap_or_default(),
        }
    } else {
        Vec::new()
    };
    ensure_disclosable(policy, &original)?;
    ensure_disclosable(policy, &modified)?;
    let original = String::from_utf8(original).map_err(|_| FilesError::PreviewUnavailable)?;
    let modified = String::from_utf8(modified).map_err(|_| FilesError::PreviewUnavailable)?;
    let response = GitDiffResponse {
        path: path.as_str().to_string(),
        original,
        modified,
    };
    scan_json_response(policy, &response)?;
    Ok(response)
}

fn line_count(text: &str) -> u32 {
    let newlines = text.bytes().filter(|byte| *byte == b'\n').count();
    let partial = usize::from(!text.is_empty() && !text.ends_with('\n'));
    u32::try_from(newlines + partial).unwrap_or(u32::MAX)
}

fn has_head(directory: &Path, context: &OperationContext) -> Result<bool, FilesError> {
    Ok(run_git(
        directory,
        &["rev-parse", "-q", "--verify", "HEAD^{commit}"],
        128,
        context,
    )?
    .is_some())
}

fn branch_name(directory: &Path, context: &OperationContext) -> Result<Option<String>, FilesError> {
    let named = run_git(directory, &["symbolic-ref", "--short", "-q", "HEAD"], 512, context)?;
    let output = match named {
        Some(output) => Some(output),
        None => run_git(directory, &["rev-parse", "--short", "HEAD"], 128, context)?,
    };
    Ok(output
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty() && name.len() <= 256))
}

/// Runs one read-only git command. `Ok(None)` means git exited unsuccessfully
/// (or is not installed); output above `maximum_bytes` is `TooLarge`.
fn run_git(
    directory: &Path,
    arguments: &[&str],
    maximum_bytes: usize,
    context: &OperationContext,
) -> Result<Option<Vec<u8>>, FilesError> {
    context.checkpoint()?;
    if !directory.is_absolute() {
        return Err(FilesError::Unsupported);
    }
    let spawned = Command::new("git")
        .args([
            "--no-pager",
            "--no-optional-locks",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "diff.external=",
            "-c",
            "core.quotePath=false",
        ])
        .args(arguments)
        .current_dir(directory)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_EXTERNAL_DIFF")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = spawned else {
        return Ok(None);
    };
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(FilesError::Unavailable);
    };
    let child = Arc::new(Mutex::new(child));
    let finished = Arc::new(AtomicBool::new(false));
    let expired = Arc::new(AtomicBool::new(false));
    // A watchdog enforces the operation deadline and cancellation even while
    // this thread is blocked reading git's output.
    let watchdog = {
        let child = Arc::clone(&child);
        let finished = Arc::clone(&finished);
        let expired = Arc::clone(&expired);
        let cancellation = context.cancellation.clone();
        let deadline = context.deadline;
        std::thread::spawn(move || {
            while !finished.load(Ordering::Acquire) {
                if cancellation.is_cancelled() || std::time::Instant::now() >= deadline {
                    expired.store(true, Ordering::Release);
                    if let Ok(mut child) = child.lock() {
                        let _ = child.kill();
                    }
                    return;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        })
    };
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 64 * 1024];
    let outcome = loop {
        match stdout.read(&mut chunk) {
            Ok(0) => break Ok(()),
            Ok(read) => {
                if bytes.len() + read > maximum_bytes {
                    break Err(FilesError::TooLarge);
                }
                bytes.extend_from_slice(&chunk[..read]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break Err(FilesError::Unavailable),
        }
    };
    drop(stdout);
    let status = {
        let mut child: std::sync::MutexGuard<'_, Child> =
            child.lock().map_err(|_| FilesError::Unavailable)?;
        if outcome.is_err() {
            let _ = child.kill();
        }
        child.wait()
    };
    finished.store(true, Ordering::Release);
    let _ = watchdog.join();
    if expired.load(Ordering::Acquire) {
        context.checkpoint()?;
        return Err(FilesError::Deadline);
    }
    outcome?;
    match status {
        Ok(status) if status.success() => Ok(Some(bytes)),
        Ok(_) => Ok(None),
        Err(_) => Err(FilesError::Unavailable),
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::wfe::file_contracts::GitChangeKind;
    use crate::wfe::files::RootedFiles;
    use tokio_util::sync::CancellationToken;

    fn git(directory: &Path, arguments: &[&str]) {
        let status = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
            .args(arguments)
            .current_dir(directory)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {arguments:?}");
    }

    fn repository() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        git(root, &["init", "-q", "-b", "main"]);
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}\nold\n").unwrap();
        std::fs::write(root.join("src/deep/gone.txt"), "a\nb\n").unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "init"]);
        std::fs::write(root.join("src/main.rs"), "fn main() {}\nnew\nmore\n").unwrap();
        std::fs::remove_file(root.join("src/deep/gone.txt")).unwrap();
        std::fs::write(root.join("notes.md"), "one\ntwo\nthree").unwrap();
        std::fs::write(root.join(".env"), "TOKEN=x\n").unwrap();
        directory
    }

    fn files(directory: &Path) -> RootedFiles {
        RootedFiles::open(
            &std::fs::canonicalize(directory).unwrap(),
            DisclosurePolicy::new(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn status_lists_changes_with_line_counts_and_hides_protected_paths() {
        let directory = repository();
        let status = files(directory.path())
            .git_status(
                GitStatusRequest {
                    path: String::new(),
                },
                CancellationToken::new(),
            )
            .await
            .unwrap()
            .into_value();
        assert!(status.repository);
        assert_eq!(status.branch.as_deref(), Some("main"));
        let find = |path: &str| status.files.iter().find(|file| file.path == path).cloned();
        let main = find("src/main.rs").unwrap();
        assert_eq!(main.kind, GitChangeKind::Modified);
        assert_eq!((main.added, main.removed), (Some(2), Some(1)));
        let gone = find("src/deep/gone.txt").unwrap();
        assert_eq!(gone.kind, GitChangeKind::Deleted);
        assert_eq!((gone.added, gone.removed), (Some(0), Some(2)));
        let notes = find("notes.md").unwrap();
        assert_eq!(notes.kind, GitChangeKind::Untracked);
        assert_eq!((notes.added, notes.removed), (Some(3), Some(0)));
        assert!(find(".env").is_none());
        assert_eq!(status.protected, 1);
    }

    #[tokio::test]
    async fn diff_returns_head_and_working_tree_content() {
        let directory = repository();
        let files = files(directory.path());
        let diff = files
            .git_diff(
                GitDiffRequest {
                    path: "src/main.rs".to_string(),
                },
                CancellationToken::new(),
            )
            .await
            .unwrap()
            .into_value();
        assert_eq!(diff.original, "fn main() {}\nold\n");
        assert_eq!(diff.modified, "fn main() {}\nnew\nmore\n");
        let deleted = files
            .git_diff(
                GitDiffRequest {
                    path: "src/deep/gone.txt".to_string(),
                },
                CancellationToken::new(),
            )
            .await
            .unwrap()
            .into_value();
        assert_eq!((deleted.original.as_str(), deleted.modified.as_str()), ("a\nb\n", ""));
        let protected = files
            .git_diff(
                GitDiffRequest {
                    path: ".env".to_string(),
                },
                CancellationToken::new(),
            )
            .await;
        assert_eq!(protected.err(), Some(FilesError::Protected));
    }

    #[tokio::test]
    async fn non_repository_reports_no_changes() {
        let directory = tempfile::tempdir().unwrap();
        let status = files(directory.path())
            .git_status(
                GitStatusRequest {
                    path: String::new(),
                },
                CancellationToken::new(),
            )
            .await
            .unwrap()
            .into_value();
        assert!(!status.repository);
        assert!(status.files.is_empty());
    }
}
