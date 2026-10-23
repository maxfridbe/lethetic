use super::*;

#[test]
fn migrates_legacy_and_reopens_a_sqlite_database() {
    let temporary = tempfile::tempdir().unwrap();
    let session = temporary.path().join(".lethetic/sessions/one");
    std::fs::create_dir_all(&session).unwrap();
    std::fs::write(session.join("ui_log.txt"), b"legacy\n").unwrap();
    assert_eq!(read(&session, "ui_log.txt").unwrap().unwrap(), b"legacy\n");
    assert!(!session.join("ui_log.txt").exists());
    append(&session, "ui_log.txt", b"next\n").unwrap();
    write(&session, "session_state.json", b"{}").unwrap();
    let notebook = session.join("python.ipynb");
    std::fs::write(&notebook, br#"{"nbformat":4,"cells":[]}"#).unwrap();
    read(&session, "session_state.json").unwrap();
    assert_eq!(
        std::fs::read(&notebook).unwrap(),
        br#"{"nbformat":4,"cells":[]}"#
    );

    assert_eq!(
        read_path(&session.join("ui_log.txt")).unwrap(),
        b"legacy\nnext\n"
    );
    let path = session.join("lethetic.db");
    let bytes = std::fs::read(&path).unwrap();
    assert!(bytes.starts_with(b"SQLite format 3\0"));
    let portable = ContentDatabase::memory(&bytes).unwrap();
    assert_eq!(
        portable.read("", "session_state.json").unwrap().unwrap(),
        b"{}"
    );
    // Browser exports of this shared schema are checked with Python sqlite3.
    remove_scope(&session).unwrap();
    assert!(read(&session, "ui_log.txt").unwrap().is_none());
}

#[test]
fn concurrent_session_writers_do_not_lose_chunks() {
    let temporary = tempfile::tempdir().unwrap();
    let session = temporary.path().join(".lethetic/sessions/one");
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let path = &session;
            scope.spawn(move || {
                for _ in 0..10 {
                    append(path, "tokens.jsonl", b"event\n").unwrap();
                }
            });
        }
    });
    assert_eq!(
        read(&session, "tokens.jsonl").unwrap().unwrap(),
        b"event\n".repeat(40)
    );
}

#[cfg(unix)]
#[test]
fn rejects_database_links_and_keeps_legacy_on_failed_migration() {
    use std::os::unix::fs::symlink;
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join(".lethetic");
    let session = root.join("sessions/one");
    std::fs::create_dir_all(&session).unwrap();
    let outside = temporary.path().join("outside");
    std::fs::write(&outside, b"untouched").unwrap();
    let legacy = session.join("ui_log.txt");
    std::fs::write(&legacy, b"legacy").unwrap();
    for hard in [false, true] {
        let destination = session.join(DATABASE_NAME);
        if hard {
            std::fs::hard_link(&outside, &destination).unwrap();
        } else {
            symlink(&outside, &destination).unwrap();
        }
        assert!(read(&session, "ui_log.txt").is_err());
        assert_eq!(std::fs::read(&legacy).unwrap(), b"legacy");
        assert_eq!(std::fs::read(&outside).unwrap(), b"untouched");
        std::fs::remove_file(destination).unwrap();
    }
    symlink(&outside, session.join("logs.txt")).unwrap();
    assert!(append(&session, "logs.txt", b"bad").is_err());
}

#[test]
fn headless_diagnostics_use_logical_paths_and_notebooks_stay_files() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join(".lethetic");
    write(&root, "last_request.json", b"{}\n").unwrap();
    assert_eq!(read_path(&root.join("last_request.json")).unwrap(), b"{}\n");
    assert!(!root.join("last_request.json").exists());
    assert!(write(&root, DATABASE_NAME, b"bad").is_err());
    assert!(
        std::fs::read(root.join(DATABASE_NAME))
            .unwrap()
            .starts_with(b"SQLite format 3\0")
    );
    let notebook = root.join("python.ipynb");
    std::fs::write(&notebook, b"notebook").unwrap();
    assert_eq!(read_path(&notebook).unwrap(), b"notebook");
    assert!(notebook.is_file());
}

#[test]
fn sessions_have_independent_files_and_upgrade_aggregate_records() {
    let temporary = tempfile::tempdir().unwrap();
    let project = temporary.path().join(".lethetic");
    std::fs::create_dir_all(&project).unwrap();
    let source = ContentDatabase::open(Arc::new(NativeIo(project.clone())), DATABASE_NAME).unwrap();
    source.put("sessions/one", "ui_log.txt", b"one").unwrap();
    source.put("sessions/two", "ui_log.txt", b"two").unwrap();
    source.put("", "history.json", b"[]").unwrap();
    source
        .put("sessions/one", "python.ipynb", b"notebook")
        .unwrap();
    source.close().unwrap();
    let one = project.join("sessions/one");
    let two = project.join("sessions/two");
    std::fs::create_dir_all(&one).unwrap();
    std::fs::create_dir_all(&two).unwrap();
    assert_eq!(read(&one, "ui_log.txt").unwrap().unwrap(), b"one");
    assert_eq!(read(&two, "ui_log.txt").unwrap().unwrap(), b"two");
    assert_eq!(
        std::fs::read(one.join("python.ipynb")).unwrap(),
        b"notebook"
    );
    for (session, expected) in [(&one, b"one"), (&two, b"two")] {
        let image =
            ContentDatabase::memory(&std::fs::read(session.join(DATABASE_NAME)).unwrap()).unwrap();
        assert_eq!(image.read("", "ui_log.txt").unwrap().unwrap(), expected);
        assert!(image.read("", "history.json").unwrap().is_none());
        assert!(image.read("", "python.ipynb").unwrap().is_none());
    }
    let source = ContentDatabase::open(Arc::new(NativeIo(project.clone())), DATABASE_NAME).unwrap();
    assert!(source.names("sessions/one").unwrap().is_empty());
    assert!(source.names("sessions/two").unwrap().is_empty());
    assert_eq!(source.read("", "history.json").unwrap().unwrap(), b"[]");
    source.close().unwrap();
    remove_scope(&one).unwrap();
    assert_eq!(read(&two, "ui_log.txt").unwrap().unwrap(), b"two");
}
