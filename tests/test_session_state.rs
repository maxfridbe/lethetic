use lethetic::app::{BlockType, RenderBlock, SessionState};
use lethetic::context::Message;
use tempfile::TempDir;

fn sample_state() -> SessionState {
    SessionState {
        messages: vec![Message {
            role: "user".to_string(),
            content: "hello".to_string(),
            tool_calls: None,
        }],
        blocks: vec![RenderBlock {
            block_type: BlockType::User,
            content: "hello".to_string(),
            title: None,
            success: None,
            prompt_tokens: None,
            completion_tokens: None,
            cached_lines: None,
            cached_line_count: None,
        }],
        history: vec!["hello".to_string()],
        theme_name: "Matrix".to_string(),
    }
}

/// Resume must read the unified session_state.json that save_session writes.
#[test]
fn test_load_unified_session_state() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().to_str().unwrap();
    let json = serde_json::to_string_pretty(&sample_state()).unwrap();
    std::fs::write(format!("{}/session_state.json", path), json).unwrap();

    let loaded = SessionState::load(path);
    assert_eq!(loaded.blocks.len(), 1, "blocks must load from session_state.json");
    assert_eq!(loaded.blocks[0].content, "hello");
    assert_eq!(loaded.messages.len(), 1);
    assert_eq!(loaded.history, vec!["hello".to_string()]);
    assert_eq!(loaded.theme_name, "Matrix");
}

/// Sessions saved by older builds used ui_state.json + context.json.
#[test]
fn test_load_legacy_session_files() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().to_str().unwrap();
    let state = sample_state();
    std::fs::write(
        format!("{}/ui_state.json", path),
        serde_json::to_string(&state.blocks).unwrap(),
    ).unwrap();
    std::fs::write(
        format!("{}/context.json", path),
        serde_json::to_string(&state.messages).unwrap(),
    ).unwrap();

    let loaded = SessionState::load(path);
    assert_eq!(loaded.blocks.len(), 1, "blocks must load from legacy ui_state.json");
    assert_eq!(loaded.messages.len(), 1);
    assert!(loaded.history.is_empty());
}

/// Older session_state.json files may lack newer fields; they must still parse.
#[test]
fn test_load_unified_with_missing_fields() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().to_str().unwrap();
    std::fs::write(
        format!("{}/session_state.json", path),
        r#"{"messages": [], "blocks": [{"block_type": "Text", "content": "x", "title": null, "success": null}]}"#,
    ).unwrap();

    let loaded = SessionState::load(path);
    assert_eq!(loaded.blocks.len(), 1);
    assert!(loaded.history.is_empty());
    assert!(loaded.theme_name.is_empty());
}

#[test]
fn test_load_missing_session_is_empty() {
    let dir = TempDir::new().unwrap();
    let loaded = SessionState::load(dir.path().to_str().unwrap());
    assert!(loaded.blocks.is_empty());
    assert!(loaded.messages.is_empty());
}
