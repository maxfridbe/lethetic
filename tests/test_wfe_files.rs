#![cfg(target_os = "linux")]

use lethetic::wfe::file_contracts::{
    FilesArchiveRequest, FilesDownloadRequest, FilesListRequest, FilesReadRequest,
    WFE_FILES_ARCHIVE_DIRECTORIES, WFE_FILES_ARCHIVE_FILES, WFE_FILES_ARCHIVE_INPUT_BYTES,
    WFE_FILES_DOWNLOAD_BYTES, WFE_FILES_LIST_ENTRIES, WFE_FILES_PATH_BYTES, WFE_FILES_PATH_DEPTH,
    WFE_FILES_VIEW_BYTES,
};
use lethetic::wfe::files::{DisclosurePolicy, FilesError, RootedFiles};
use std::fs;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio_util::sync::CancellationToken;

fn service(root: &Path) -> RootedFiles {
    RootedFiles::open(root, DisclosurePolicy::new()).expect("Linux openat2 file capability")
}

fn list(path: &str) -> FilesListRequest {
    FilesListRequest {
        path: path.to_string(),
    }
}

fn read(path: &str) -> FilesReadRequest {
    FilesReadRequest {
        path: path.to_string(),
    }
}

fn download(path: &str) -> FilesDownloadRequest {
    FilesDownloadRequest {
        path: path.to_string(),
    }
}

fn archive(path: &str) -> FilesArchiveRequest {
    FilesArchiveRequest {
        path: path.to_string(),
    }
}

#[tokio::test]
async fn basic_operations_are_rooted_read_only_and_zip_is_normalized() {
    let temporary = tempfile::tempdir().unwrap();
    fs::create_dir(temporary.path().join("src")).unwrap();
    fs::write(temporary.path().join("src/lib.rs"), "fn main() {}\n").unwrap();
    fs::write(temporary.path().join("data.bin"), [0_u8, 159, 255]).unwrap();
    let before: Vec<_> = fs::read_dir(temporary.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    let files = service(temporary.path());

    let root = files
        .list(list(""), CancellationToken::new())
        .await
        .unwrap()
        .into_value();
    assert_eq!(root.entries.len(), 2);
    assert_eq!(root.entries[0].name, "src");
    assert_eq!(root.entries[0].path, "src");
    assert_eq!(root.entries[1].name, "data.bin");
    assert!(!root.truncated);

    let source = files
        .read(read("src/lib.rs"), CancellationToken::new())
        .await
        .unwrap()
        .into_value();
    assert_eq!(source.path, "src/lib.rs");
    assert_eq!(source.content, "fn main() {}\n");
    assert_eq!(source.size, "13");

    let binary = files
        .download(download("data.bin"), CancellationToken::new())
        .await
        .unwrap()
        .into_value();
    assert_eq!(binary.bytes, [0_u8, 159, 255]);
    assert_eq!(binary.filename, "data.bin");
    assert_eq!(binary.content_type, "application/octet-stream");
    assert_eq!(
        files
            .read(read("data.bin"), CancellationToken::new())
            .await
            .unwrap_err(),
        FilesError::PreviewUnavailable
    );

    let zipped = files
        .archive(archive("src"), CancellationToken::new())
        .await
        .unwrap()
        .into_value();
    assert_eq!(zipped.filename, "src.zip");
    assert_eq!(zipped.exclusions, Default::default());
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(zipped.bytes)).unwrap();
    assert_eq!(zip.len(), 2);
    let directory = zip.by_name("src/").unwrap();
    assert!(directory.is_dir());
    assert_eq!(directory.last_modified().unwrap().year(), 1980);
    drop(directory);
    let mut entry = zip.by_name("src/lib.rs").unwrap();
    assert_eq!(entry.compression(), zip::CompressionMethod::Stored);
    assert_eq!(entry.unix_mode().unwrap() & 0o777, 0o644);
    let mut contents = String::new();
    entry.read_to_string(&mut contents).unwrap();
    assert_eq!(contents, "fn main() {}\n");

    let after: Vec<_> = fs::read_dir(temporary.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(
        before, after,
        "file operations must not create workspace state"
    );
}

#[tokio::test]
async fn traversal_absolute_prefix_and_malformed_paths_are_rejected() {
    let temporary = tempfile::tempdir().unwrap();
    fs::write(temporary.path().join("ok.txt"), "ok").unwrap();
    let files = service(temporary.path());
    let too_deep = std::iter::repeat_n("a", WFE_FILES_PATH_DEPTH + 1)
        .collect::<Vec<_>>()
        .join("/");
    let too_long = "a".repeat(WFE_FILES_PATH_BYTES + 1);

    for path in [
        "/etc/passwd",
        "../escape",
        "a/../escape",
        "a/./b",
        "a//b",
        "a/",
        "C:/Windows/system.ini",
        "folder/D:/escape",
        "a\\b",
        "control\u{7f}name",
        &too_deep,
        &too_long,
    ] {
        assert_eq!(
            files
                .read(read(path), CancellationToken::new())
                .await
                .unwrap_err(),
            FilesError::BadRequest,
            "path {path:?}"
        );
    }
    assert_eq!(
        files
            .download(download(""), CancellationToken::new())
            .await
            .unwrap_err(),
        FilesError::BadRequest
    );
    assert_eq!(
        RootedFiles::open(Path::new("/"), DisclosurePolicy::new()).unwrap_err(),
        FilesError::Protected
    );
}

#[tokio::test]
async fn symlinks_hardlinks_fifo_and_sockets_are_never_eligible() {
    use std::os::unix::fs::symlink;

    let temporary = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("synthetic.txt"), "synthetic outside").unwrap();
    fs::write(temporary.path().join("ordinary.txt"), "ordinary").unwrap();
    fs::hard_link(
        temporary.path().join("ordinary.txt"),
        temporary.path().join("hardlink.txt"),
    )
    .unwrap();
    symlink("ordinary.txt", temporary.path().join("link.txt")).unwrap();
    symlink(
        outside.path().join("synthetic.txt"),
        temporary.path().join("outside.txt"),
    )
    .unwrap();
    rustix::fs::mkfifoat(
        rustix::fs::CWD,
        temporary.path().join("pipe"),
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )
    .unwrap();
    let _socket = std::os::unix::net::UnixListener::bind(temporary.path().join("socket")).unwrap();
    let files = service(temporary.path());

    for path in [
        "ordinary.txt",
        "hardlink.txt",
        "link.txt",
        "outside.txt",
        "pipe",
        "socket",
    ] {
        assert_eq!(
            files
                .download(download(path), CancellationToken::new())
                .await
                .unwrap_err(),
            FilesError::Unsupported,
            "path {path}"
        );
    }
    let listing = files
        .list(list(""), CancellationToken::new())
        .await
        .unwrap()
        .into_value();
    assert!(listing.entries.is_empty());
    assert_eq!(listing.exclusions.unsupported, 6);
}

#[tokio::test]
async fn protected_names_paths_identities_and_contents_fail_closed() {
    let temporary = tempfile::tempdir().unwrap();
    fs::create_dir(temporary.path().join(".git")).unwrap();
    fs::write(temporary.path().join(".git/config"), "synthetic").unwrap();
    fs::write(temporary.path().join(".env.local"), "SYNTHETIC=1").unwrap();
    fs::create_dir(temporary.path().join("private-control")).unwrap();
    fs::write(
        temporary.path().join("private-control/state.json"),
        "synthetic state",
    )
    .unwrap();
    fs::write(
        temporary.path().join("secret.txt"),
        "prefix SYNTHETIC-EXACT-SECRET-123 suffix",
    )
    .unwrap();
    fs::write(
        temporary.path().join("private.pem"),
        "-----BEGIN OPENSSH PRIVATE KEY-----\nsynthetic-only",
    )
    .unwrap();
    fs::write(temporary.path().join("public.txt"), "ordinary").unwrap();

    let mut files = service(temporary.path());
    files
        .protect_path(&temporary.path().join("private-control"))
        .unwrap();
    files.register_secret("SYNTHETIC-EXACT-SECRET-123").unwrap();
    let listing = files
        .list(list(""), CancellationToken::new())
        .await
        .unwrap()
        .into_value();
    assert_eq!(
        listing
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["private.pem", "public.txt", "secret.txt"]
    );
    assert_eq!(listing.exclusions.protected, 3);

    for path in [".git/config", ".env.local", "private-control/state.json"] {
        assert_eq!(
            files
                .read(read(path), CancellationToken::new())
                .await
                .unwrap_err(),
            FilesError::Protected
        );
    }
    for path in ["secret.txt", "private.pem"] {
        assert_eq!(
            files
                .read(read(path), CancellationToken::new())
                .await
                .unwrap_err(),
            FilesError::SensitiveContent
        );
        assert_eq!(
            files
                .download(download(path), CancellationToken::new())
                .await
                .unwrap_err(),
            FilesError::SensitiveContent
        );
    }
    assert_eq!(
        files
            .archive(archive(""), CancellationToken::new())
            .await
            .unwrap_err(),
        FilesError::SensitiveContent
    );

    fs::rename(
        temporary.path().join("private-control"),
        temporary.path().join("renamed-control"),
    )
    .unwrap();
    assert_eq!(
        files
            .read(read("renamed-control/state.json"), CancellationToken::new())
            .await
            .unwrap_err(),
        FilesError::Protected,
        "renaming a protected directory must not bypass its recorded identity"
    );

    let control_parent = tempfile::tempdir().unwrap();
    let workspace = control_parent.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let mut nested_service = service(&workspace);
    assert_eq!(
        nested_service
            .protect_path(control_parent.path())
            .unwrap_err(),
        FilesError::Protected
    );
}

#[tokio::test]
async fn credential_bearing_paths_are_excluded_before_json_escaping() {
    let temporary = tempfile::tempdir().unwrap();
    let secret = "SYNTHETIC-\"EXACT-SECRET-123";
    let secret_file = format!("copy-{secret}.txt");
    let secret_directory = format!("folder-{secret}");
    let nested_file = format!("{secret_directory}/ordinary.txt");
    fs::write(temporary.path().join("public.txt"), "ordinary").unwrap();
    fs::write(temporary.path().join(&secret_file), "ordinary").unwrap();
    fs::create_dir(temporary.path().join(&secret_directory)).unwrap();
    fs::write(temporary.path().join(&nested_file), "ordinary").unwrap();
    let mut files = service(temporary.path());
    files.register_secret(secret).unwrap();

    let listing = files
        .list(list(""), CancellationToken::new())
        .await
        .unwrap()
        .into_value();
    assert_eq!(listing.entries.len(), 1);
    assert_eq!(listing.entries[0].name, "public.txt");
    assert_eq!(listing.exclusions.protected, 2);
    for path in [&secret_file, &nested_file] {
        assert_eq!(
            files
                .read(read(path), CancellationToken::new())
                .await
                .unwrap_err(),
            FilesError::Protected
        );
        assert_eq!(
            files
                .download(download(path), CancellationToken::new())
                .await
                .unwrap_err(),
            FilesError::Protected
        );
    }
    assert_eq!(
        files
            .list(list(&secret_directory), CancellationToken::new())
            .await
            .unwrap_err(),
        FilesError::Protected
    );
    assert_eq!(
        files
            .archive(archive(&secret_directory), CancellationToken::new())
            .await
            .unwrap_err(),
        FilesError::Protected
    );
    let zipped = files
        .archive(archive(""), CancellationToken::new())
        .await
        .unwrap()
        .into_value();
    assert_eq!(zipped.exclusions.protected, 2);
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(zipped.bytes)).unwrap();
    assert_eq!(zip.len(), 1);
    assert_eq!(zip.by_index(0).unwrap().name(), "public.txt");
    assert_eq!(
        fs::read(temporary.path().join(&secret_file)).unwrap(),
        b"ordinary"
    );
}

#[tokio::test]
async fn pinned_launch_root_cannot_be_retargeted_by_path_replacement() {
    let parent = tempfile::tempdir().unwrap();
    let launch = parent.path().join("workspace");
    let moved = parent.path().join("original-workspace");
    fs::create_dir(&launch).unwrap();
    fs::write(launch.join("trusted.txt"), "trusted bytes").unwrap();
    let files = service(&launch);

    fs::rename(&launch, &moved).unwrap();
    fs::create_dir(&launch).unwrap();
    fs::write(launch.join("replacement.txt"), "replacement bytes").unwrap();

    let listing = files
        .list(list(""), CancellationToken::new())
        .await
        .unwrap()
        .into_value();
    assert_eq!(listing.entries.len(), 1);
    assert_eq!(listing.entries[0].name, "trusted.txt");
    let contents = files
        .read(read("trusted.txt"), CancellationToken::new())
        .await
        .unwrap()
        .into_value();
    assert_eq!(contents.content, "trusted bytes");
    assert_eq!(
        files
            .read(read("replacement.txt"), CancellationToken::new())
            .await
            .unwrap_err(),
        FilesError::NotFound
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn replacement_race_never_mixes_or_redirects_read_bytes() {
    let temporary = tempfile::tempdir().unwrap();
    let target = temporary.path().join("target.bin");
    let standby = temporary.path().join("standby.bin");
    let swap = temporary.path().join("swap.bin");
    let first = vec![b'A'; 4 * 64 * 1024];
    let second = vec![b'B'; 4 * 64 * 1024];
    fs::write(&target, &first).unwrap();
    fs::write(&standby, &second).unwrap();
    let files = service(temporary.path());
    assert_eq!(
        files
            .download(download("target.bin"), CancellationToken::new())
            .await
            .unwrap()
            .into_value()
            .bytes,
        first
    );
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let target_for_thread = target.clone();
    let standby_for_thread = standby.clone();
    let swap_for_thread = swap.clone();
    let racer = std::thread::spawn(move || {
        while !thread_stop.load(Ordering::Relaxed) {
            if fs::rename(&target_for_thread, &swap_for_thread).is_ok() {
                let _ = fs::rename(&standby_for_thread, &target_for_thread);
                let _ = fs::rename(&swap_for_thread, &standby_for_thread);
            }
        }
    });

    for _ in 0..80 {
        match files
            .download(download("target.bin"), CancellationToken::new())
            .await
        {
            Ok(operation) => {
                let bytes = operation.into_value().bytes;
                assert!(bytes == first || bytes == second);
            }
            Err(FilesError::NotFound | FilesError::Changed) => {}
            Err(error) => panic!("unexpected race result: {error:?}"),
        }
    }
    stop.store(true, Ordering::Relaxed);
    racer.join().unwrap();
}

#[tokio::test]
async fn list_view_and_download_boundaries_are_exact() {
    let temporary = tempfile::tempdir().unwrap();
    for index in 0..=WFE_FILES_LIST_ENTRIES {
        fs::write(temporary.path().join(format!("f{index:04}.txt")), "x").unwrap();
    }
    let at_view_limit = vec![b'x'; WFE_FILES_VIEW_BYTES];
    fs::write(temporary.path().join("view-limit.txt"), &at_view_limit).unwrap();
    fs::write(
        temporary.path().join("view-over.txt"),
        vec![b'x'; WFE_FILES_VIEW_BYTES + 1],
    )
    .unwrap();
    let raw_limit = fs::File::create(temporary.path().join("raw-limit.bin")).unwrap();
    raw_limit.set_len(WFE_FILES_DOWNLOAD_BYTES as u64).unwrap();
    let raw_over = fs::File::create(temporary.path().join("raw-over.bin")).unwrap();
    raw_over
        .set_len((WFE_FILES_DOWNLOAD_BYTES + 1) as u64)
        .unwrap();
    let files = service(temporary.path());

    let listing = files
        .list(list(""), CancellationToken::new())
        .await
        .unwrap()
        .into_value();
    assert_eq!(listing.entries.len(), WFE_FILES_LIST_ENTRIES);
    assert!(listing.truncated);
    assert_eq!(
        files
            .read(read("view-limit.txt"), CancellationToken::new())
            .await
            .unwrap()
            .value()
            .content
            .len(),
        WFE_FILES_VIEW_BYTES
    );
    assert_eq!(
        files
            .read(read("view-over.txt"), CancellationToken::new())
            .await
            .unwrap_err(),
        FilesError::PreviewUnavailable
    );
    assert_eq!(
        files
            .download(download("view-over.txt"), CancellationToken::new())
            .await
            .unwrap()
            .value()
            .bytes
            .len(),
        WFE_FILES_VIEW_BYTES + 1
    );
    assert_eq!(
        files
            .download(download("raw-limit.bin"), CancellationToken::new())
            .await
            .unwrap()
            .value()
            .bytes
            .len(),
        WFE_FILES_DOWNLOAD_BYTES
    );
    assert_eq!(
        files
            .download(download("raw-over.bin"), CancellationToken::new())
            .await
            .unwrap_err(),
        FilesError::TooLarge
    );
}

#[tokio::test]
async fn archive_aggregate_file_directory_depth_and_input_caps_are_errors() {
    let files_root = tempfile::tempdir().unwrap();
    for index in 0..=WFE_FILES_ARCHIVE_FILES {
        fs::write(files_root.path().join(format!("f{index:04}")), []).unwrap();
    }
    assert_eq!(
        service(files_root.path())
            .archive(archive(""), CancellationToken::new())
            .await
            .unwrap_err(),
        FilesError::TooLarge
    );

    let directories_root = tempfile::tempdir().unwrap();
    for index in 0..WFE_FILES_ARCHIVE_DIRECTORIES {
        fs::create_dir(directories_root.path().join(format!("d{index:04}"))).unwrap();
    }
    assert_eq!(
        service(directories_root.path())
            .archive(archive(""), CancellationToken::new())
            .await
            .unwrap_err(),
        FilesError::TooLarge
    );

    let depth_root = tempfile::tempdir().unwrap();
    let mut current = depth_root.path().to_path_buf();
    for _ in 0..=WFE_FILES_PATH_DEPTH {
        current.push("d");
        fs::create_dir(&current).unwrap();
    }
    assert_eq!(
        service(depth_root.path())
            .archive(archive(""), CancellationToken::new())
            .await
            .unwrap_err(),
        FilesError::TooLarge
    );

    let bytes_root = tempfile::tempdir().unwrap();
    let oversized = fs::File::create(bytes_root.path().join("oversized.bin")).unwrap();
    oversized
        .set_len((WFE_FILES_ARCHIVE_INPUT_BYTES + 1) as u64)
        .unwrap();
    assert_eq!(
        service(bytes_root.path())
            .archive(archive(""), CancellationToken::new())
            .await
            .unwrap_err(),
        FilesError::TooLarge
    );
}

#[tokio::test]
async fn cancellation_and_shared_worker_limits_fail_without_queueing() {
    let temporary = tempfile::tempdir().unwrap();
    fs::write(temporary.path().join("file.txt"), "ordinary").unwrap();
    fs::create_dir(temporary.path().join("folder")).unwrap();
    let files = service(temporary.path());

    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert_eq!(
        files.read(read("file.txt"), cancelled).await.unwrap_err(),
        FilesError::Cancelled
    );

    let first = files
        .read(read("file.txt"), CancellationToken::new())
        .await
        .unwrap();
    let second = files
        .read(read("file.txt"), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        files
            .read(read("file.txt"), CancellationToken::new())
            .await
            .unwrap_err(),
        FilesError::Busy
    );
    drop(first);
    assert!(
        files
            .read(read("file.txt"), CancellationToken::new())
            .await
            .is_ok()
    );
    drop(second);

    let active_archive = files
        .archive(archive("folder"), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        files
            .archive(archive("folder"), CancellationToken::new())
            .await
            .unwrap_err(),
        FilesError::Busy
    );
    assert!(
        files
            .read(read("file.txt"), CancellationToken::new())
            .await
            .is_ok(),
        "the second total slot remains available while one archive response is held"
    );
    drop(active_archive);
}
