#![cfg(target_os = "linux")]

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use lethetic::app::{App, AppEventOutcome, SessionState, handle_key};
use lethetic::config::Config;
use lethetic::session_store::SessionStore;
use std::os::unix::fs::PermissionsExt;

const CHILD_MARKER: &str = "LETETHIC_SESSION_UUID_TEST_CHILD";

#[tokio::test(flavor = "current_thread")]
async fn session_manager_actions_use_registered_uuids() {
    if std::env::var_os(CHILD_MARKER).is_none() {
        let project = tempfile::tempdir().unwrap();
        let state_home = tempfile::tempdir().unwrap();
        let config_home = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "session_manager_actions_use_registered_uuids",
                "--nocapture",
            ])
            .env(CHILD_MARKER, "1")
            .env("XDG_STATE_HOME", state_home.path())
            .env("XDG_CONFIG_HOME", config_home.path())
            .current_dir(project.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "isolated child failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    let mut app = App::new(&Config::default());
    app.set_session_display_name("First session").unwrap();
    let first_id = app.session_id.clone();
    let first_path = app.current_session_dir.clone().unwrap();

    app.start_new_session_checked().unwrap();
    let second_id = app.session_id.clone();
    let second_path = app.current_session_dir.clone().unwrap();
    assert_ne!(first_id, second_id);
    assert_ne!(first_id, first_path);
    assert_ne!(second_id, second_path);

    app.refresh_session_list();
    let first_index = app
        .session_summaries
        .iter()
        .position(|summary| summary.session_id == first_id)
        .unwrap();
    assert_eq!(
        app.session_summaries[first_index].display_name.as_deref(),
        Some("First session")
    );
    app.show_session_manager = true;
    app.session_list_state.select(Some(first_index));
    assert_eq!(
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        AppEventOutcome::ResumeSession(first_id.clone())
    );

    app.refresh_session_list();
    let second_index = app
        .session_summaries
        .iter()
        .position(|summary| summary.session_id == second_id)
        .unwrap();
    app.show_session_manager = true;
    app.session_list_state.select(Some(second_index));
    assert_eq!(
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        AppEventOutcome::Continue
    );
    assert!(!app.show_session_manager);
    assert_eq!(app.session_id, second_id);
    assert_eq!(
        app.current_session_dir.as_deref(),
        Some(second_path.as_str())
    );
    app.resume_registered_session(&second_id).await.unwrap();
    assert_eq!(app.session_id, second_id);

    app.show_session_manager = true;
    app.session_list_state.select(Some(second_index));
    assert_eq!(
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE)
        ),
        AppEventOutcome::DeleteSession(second_id.clone())
    );

    app.resume_registered_session(&first_id).await.unwrap();
    assert_eq!(app.session_id, first_id);
    assert_eq!(app.display_name.as_deref(), Some("First session"));
    assert_eq!(
        app.current_session_dir.as_deref(),
        Some(first_path.as_str())
    );
    app.resume_registered_session(&first_id).await.unwrap();
    assert_eq!(app.session_id, first_id);

    let sessions_root = std::env::current_dir()
        .unwrap()
        .join(".lethetic/sessions")
        .canonicalize()
        .unwrap();
    let create_private_session_dir = |name: &str| {
        let path = sessions_root.join(name);
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    };

    let copied_binding_path = create_private_session_dir("session_20991231_binding_copy");
    std::fs::copy(
        std::path::Path::new(&first_path).join("session_state.json"),
        copied_binding_path.join("session_state.json"),
    )
    .unwrap();
    app.refresh_session_list();
    assert_eq!(
        app.session_summaries
            .iter()
            .filter(|summary| summary.session_id == first_id)
            .count(),
        1
    );
    assert_eq!(app.session_path_for_id(&first_id).unwrap(), first_path);

    let duplicate_id = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    let duplicate_one = create_private_session_dir("session_20991231_duplicate_one");
    let duplicate_two = create_private_session_dir("session_20991231_duplicate_two");
    let duplicate_state = serde_json::json!({
        "schema_version": 2,
        "session_id": duplicate_id,
        "messages": [],
        "blocks": []
    });
    for path in [&duplicate_one, &duplicate_two] {
        std::fs::write(
            path.join("session_state.json"),
            serde_json::to_vec(&duplicate_state).unwrap(),
        )
        .unwrap();
    }
    app.refresh_session_list();
    assert!(
        app.session_summaries
            .iter()
            .all(|summary| summary.session_id != duplicate_id)
    );
    assert!(
        app.session_ids_for_cleanup()
            .iter()
            .all(|session_id| session_id != duplicate_id)
    );
    assert!(app.session_path_for_id(duplicate_id).is_err());

    std::fs::remove_dir_all(&copied_binding_path).unwrap();
    std::fs::remove_dir_all(&duplicate_one).unwrap();
    std::fs::remove_dir_all(&duplicate_two).unwrap();

    let cleanup_id = "cccccccc-dddd-4eee-8fff-000000000000";
    let store = SessionStore::open(&std::env::current_dir().unwrap()).unwrap();
    let (cleanup_path, cleanup_lease) = store
        .create_locked_session("session_20991231_cleanup", cleanup_id)
        .unwrap();
    let mut cleanup_state = SessionState::default();
    cleanup_state.session_id = Some(cleanup_id.to_string());
    cleanup_state.session_directory_binding = Some(cleanup_lease.binding().clone());
    cleanup_state
        .save_to_directory_checked(cleanup_path.to_str().unwrap())
        .unwrap();
    store
        .commit_locked_session_creation(&cleanup_lease)
        .unwrap();
    store
        .begin_locked_session_deletion(&cleanup_lease, None, None)
        .unwrap();
    drop(cleanup_lease);
    assert!(SessionState::load_checked(cleanup_path.to_str().unwrap()).is_ok());

    app.refresh_session_list();
    assert!(
        app.session_summaries
            .iter()
            .all(|summary| summary.session_id != cleanup_id)
    );
    assert!(
        app.session_ids_for_cleanup()
            .iter()
            .any(|session_id| session_id == cleanup_id)
    );
    assert!(!app.delete_session_transaction(cleanup_id).await.unwrap());
    assert!(!cleanup_path.exists());

    assert!(!app.delete_session_transaction(&second_id).await.unwrap());
    assert!(!std::path::Path::new(&second_path).exists());
    assert!(app.session_path_for_id(&second_id).is_err());
    assert!(app.session_path_for_id(&first_path).is_err());

    assert!(app.delete_session_transaction(&first_id).await.unwrap());
    assert!(app.current_session_dir.is_none());
    assert!(!std::path::Path::new(&first_path).exists());
}
