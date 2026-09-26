#![cfg(target_os = "linux")]

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use lethetic::app::{App, AppEventOutcome, SessionState, dispatch_command, handle_key};
use lethetic::commands::CommandId;
use lethetic::config::Config;

const CHILD_MARKER: &str = "LETETHIC_SESSION_NAME_TEST_CHILD";

#[test]
fn session_name_palette_modal_persists_validates_and_clears() {
    if std::env::var_os(CHILD_MARKER).is_none() {
        let project = tempfile::tempdir().unwrap();
        let state_home = tempfile::tempdir().unwrap();
        let config_home = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "session_name_palette_modal_persists_validates_and_clears",
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
    assert!(app.current_session_dir.is_some());
    app.show_palette = true;

    assert_eq!(
        dispatch_command(&mut app, CommandId::NameSession),
        AppEventOutcome::Continue
    );
    assert!(app.show_session_name_dialog);
    assert!(!app.show_palette);

    app.handle_paste("  Research notes  ");
    assert_eq!(
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        AppEventOutcome::Continue
    );
    assert!(!app.show_session_name_dialog);
    assert_eq!(app.display_name.as_deref(), Some("Research notes"));
    assert_eq!(
        app.command_view(CommandId::NameSession).label,
        "Rename Session: Research notes"
    );

    let session_dir = app.current_session_dir.clone().unwrap();
    assert_eq!(
        SessionState::load_checked(&session_dir)
            .unwrap()
            .display_name
            .as_deref(),
        Some("Research notes")
    );

    dispatch_command(&mut app, CommandId::NameSession);
    let original_input = app.session_name_input.clone();
    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('!'), KeyModifiers::SHIFT),
    );
    assert_eq!(app.session_name_input, format!("{original_input}!"));
    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
    );
    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT),
    );
    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
    );
    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('\u{1b}'), KeyModifiers::NONE),
    );
    assert_eq!(app.session_name_input, original_input);

    app.session_name_input = "unsafe\u{202e}name".to_string();
    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.show_session_name_dialog);
    assert!(app.session_name_error.is_some());
    assert_eq!(app.display_name.as_deref(), Some("Research notes"));

    handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    dispatch_command(&mut app, CommandId::NameSession);
    app.session_name_input.clear();
    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(!app.show_session_name_dialog);
    assert!(app.display_name.is_none());
    assert!(
        SessionState::load_checked(&session_dir)
            .unwrap()
            .display_name
            .is_none()
    );

    app.context_manager.add_message("user", "must survive");
    let context_before = app.context_manager.get_messages().to_vec();
    app.is_processing = true;
    app.show_palette = true;
    assert_eq!(
        dispatch_command(&mut app, CommandId::ClearContext),
        AppEventOutcome::Continue
    );
    assert_eq!(
        app.context_manager.get_messages(),
        context_before.as_slice()
    );
    assert!(!app.command_view(CommandId::ClearContext).enabled);
    app.is_processing = false;
    assert_eq!(
        dispatch_command(&mut app, CommandId::ClearContext),
        AppEventOutcome::NewSession
    );
    assert_eq!(
        app.context_manager.get_messages(),
        context_before.as_slice()
    );

    app.add_segment(
        "stale output".to_string(),
        lethetic::app::BlockType::Thought,
    );
    app.output_state.select(None);
    app.show_palette = true;
    assert_eq!(
        dispatch_command(&mut app, CommandId::ClearUi),
        AppEventOutcome::Continue
    );
    assert!(!app.show_palette);
    assert_eq!(app.blocks.len(), 1);
    assert_eq!(app.blocks[0].content, "UI Cleared. (Context preserved)");
    assert_eq!(app.blocks[0].success, Some(true));
    assert_eq!(app.output_state.selected(), None);
    assert!(app.auto_scroll);
    assert_eq!(app.total_line_count, 0);
    assert!(app.needs_save);
    assert_eq!(
        app.context_manager.get_messages(),
        context_before.as_slice()
    );

    app.blocks.clear();
    app.add_segment(
        "other stale output".to_string(),
        lethetic::app::BlockType::Thought,
    );
    app.output_state.select(None);
    app.needs_save = false;
    assert_eq!(
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL)
        ),
        AppEventOutcome::Continue
    );
    assert_eq!(app.blocks.len(), 1);
    assert_eq!(app.blocks[0].content, "UI Cleared. (Context preserved)");
    assert_eq!(app.blocks[0].success, Some(true));
    assert_eq!(app.output_state.selected(), None);
    assert!(app.auto_scroll);
    assert_eq!(app.total_line_count, 0);
    assert!(app.needs_save);
    assert_eq!(
        app.context_manager.get_messages(),
        context_before.as_slice()
    );

    let session_id_before_buffered_result = app.session_id.clone();
    let session_path_before_buffered_result = app.current_session_dir.clone();
    let (tool_result_tx, mut tool_result_rx) = tokio::sync::mpsc::unbounded_channel();
    tool_result_tx
        .send(lethetic::client::StreamEvent::ToolResult {
            id: Some("toolu_buffered_answer".to_string()),
            func_name: "ask_the_user".to_string(),
            result: "yes".to_string(),
            cwd: app.current_dir.clone(),
            is_error: false,
            provenance: lethetic::tools::ToolOutputProvenance::OrdinaryHost,
        })
        .unwrap();
    app.pending_tool_call = Some(lethetic::context::ToolCall {
        id: "toolu_buffered_answer".to_string(),
        provider_id: Some("toolu_buffered_answer".to_string()),
        function: lethetic::context::FunctionCall {
            name: "ask_the_user".to_string(),
            arguments: serde_json::json!({"question": "Continue?"}),
        },
    });
    app.is_asking_user = false;
    app.is_processing = false;
    app.is_executing_tool = false;
    app.show_approval_prompt = false;
    assert!(!app.is_fully_idle());
    assert!(!app.command_view(CommandId::ClearContext).enabled);
    app.show_palette = true;
    assert_eq!(
        dispatch_command(&mut app, CommandId::ClearContext),
        AppEventOutcome::Continue
    );
    assert_eq!(app.session_id, session_id_before_buffered_result);
    assert_eq!(app.current_session_dir, session_path_before_buffered_result);
    assert_eq!(
        app.context_manager.get_messages(),
        context_before.as_slice()
    );

    let buffered_event = tool_result_rx.try_recv().unwrap();
    match buffered_event {
        lethetic::client::StreamEvent::ToolResult {
            id,
            func_name,
            result,
            is_error,
            ..
        } => {
            assert_eq!(id.as_deref(), Some("toolu_buffered_answer"));
            assert_eq!(func_name, "ask_the_user");
            assert_eq!(result, "yes");
            assert!(!is_error);
        }
        other => panic!("unexpected buffered event: {other:?}"),
    }
    let buffered_result_owner = app.pending_tool_call.take().unwrap();
    assert_eq!(buffered_result_owner.function.name, "ask_the_user");
    assert!(app.is_fully_idle());
    assert_eq!(
        dispatch_command(&mut app, CommandId::ClearContext),
        AppEventOutcome::NewSession
    );
    assert_eq!(
        app.context_manager.get_messages(),
        context_before.as_slice()
    );
}
