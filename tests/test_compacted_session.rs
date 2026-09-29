#![cfg(target_os = "linux")]
//! A compacted session keeps the plan and the settings of its source.

use lethetic::app::{App, SessionState};
use lethetic::config::Config;

const CHILD_MARKER: &str = "LETHETIC_COMPACTED_SESSION_TEST_CHILD";

#[tokio::test(flavor = "current_thread")]
async fn compaction_keeps_the_plan_and_session_settings() {
    if std::env::var_os(CHILD_MARKER).is_none() {
        let project = tempfile::tempdir().unwrap();
        let state_home = tempfile::tempdir().unwrap();
        let config_home = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "compaction_keeps_the_plan_and_session_settings",
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

    let cwd = std::env::current_dir().unwrap();
    let store = lethetic::todo_store::TodoStore::open(&cwd).unwrap();
    store
        .replace_current(
            lethetic::todo_store::TodoStore::parse_todos(&serde_json::json!([
                {"id": "cube_height", "content": "Lift cube", "status": "completed", "priority": "high"},
                {"id": "fix_plane", "content": "Fix normal binding", "status": "in_progress", "priority": "high"}
            ]))
            .unwrap(),
        )
        .unwrap();

    let mut app = App::new(&Config::default());
    app.loop_detector.config.mode = lethetic::loop_detector::LoopDetectionMode::NGram;
    app.tool_use_counts.insert("run_shell_command".to_string(), 12);
    app.remote_control_target = Some("https://127.0.0.1:11223".to_string());
    app.remote_control_files = true;
    app.save_session_checked().unwrap();
    let source_id = app.session_id.clone();

    let compacted_id = app
        .create_compacted_session(&source_id, "Summary of the work so far.".to_string())
        .unwrap();
    let path = app.session_path_for_id(&compacted_id).unwrap();
    let state = SessionState::load_checked(&path).unwrap();

    let opening = &state.messages[0].content;
    assert!(opening.contains("Summary of the work so far."));
    assert!(
        opening.contains("- fix_plane: [in_progress] (high) Fix normal binding"),
        "the plan travels with its ids:\n{opening}"
    );
    assert_eq!(
        state.loop_mode,
        Some(lethetic::loop_detector::LoopDetectionMode::NGram)
    );
    assert_eq!(state.tool_use_counts.get("run_shell_command"), Some(&12));
    assert_eq!(
        state.remote_control.as_deref(),
        Some("https://127.0.0.1:11223")
    );
    assert!(state.remote_control_files);
    assert!(state.python_policy.is_some(), "Agent Mode carries over");
}
