use lethetic::app::{
    BlockType, RenderBlock, SessionDirectoryBinding, SessionState, normalize_session_display_name,
};
use lethetic::context::{FunctionCall, Message, ToolCall};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

fn sample_state() -> SessionState {
    SessionState {
        schema_version: 1,
        session_id: Some("11111111-2222-4333-8444-555555555555".to_string()),
        display_name: None,
        session_directory_binding: None,
        python_runtime_id: None,
        managed_python_workspace: None,
        shared_python_workspace: None,
        messages: vec![Message {
            role: "user".to_string(),
            content: "hello".to_string(),
            tool_calls: None,
            provider_content: None,
            tool_result_is_error: false,
        }],
        blocks: vec![RenderBlock {
            block_type: BlockType::User,
            content: "hello".to_string(),
            title: None,
            success: None,
            prompt_tokens: None,
            completion_tokens: None,
            usage: None,
            estimated_cost: None,
            logical_turn_id: None,
            cached_lines: None,
            cached_line_count: None,
        }],
        history: vec!["hello".to_string()],
        theme_name: "Matrix".to_string(),
        accounting: Default::default(),
        needs_migration_save: false,
    }
}

fn bind_state_to_directory(state: &mut SessionState, directory: &TempDir) {
    let canonical_path = directory.path().canonicalize().unwrap();
    let session_id = state.session_id.as_deref().unwrap();
    let device = 7_u64;
    let inode = 11_u64;
    let mut hash = Sha256::new();
    hash.update(b"lethetic-session-directory-v1\0");
    hash.update(session_id.as_bytes());
    hash.update(b"\0");
    hash.update(canonical_path.as_os_str().as_encoded_bytes());
    hash.update(b"\0");
    hash.update(device.to_le_bytes());
    hash.update(inode.to_le_bytes());
    state.session_directory_binding = Some(SessionDirectoryBinding {
        canonical_path,
        device,
        inode,
        binding_hash: format!("{:x}", hash.finalize()),
    });
}

fn task_tool_conversation(payload: &str, is_error: bool) -> Vec<Message> {
    let tool_call_id = "legacy-task-call";
    vec![
        Message {
            role: "assistant".to_string(),
            content: String::new(),
            tool_calls: Some(vec![ToolCall {
                id: tool_call_id.to_string(),
                provider_id: None,
                function: FunctionCall {
                    name: "task".to_string(),
                    arguments: serde_json::json!({"prompt": "validate"}),
                },
            }]),
            provider_content: None,
            tool_result_is_error: false,
        },
        Message {
            role: "tool".to_string(),
            content: format!(
                "<|tool_response>response:task{{result:<|'|>{payload}<|'|>,tool_call_id:<|'|>{tool_call_id}<|'|>}}<tool_response|><turn|>"
            ),
            tool_calls: None,
            provider_content: None,
            tool_result_is_error: is_error,
        },
    ]
}

#[test]
fn test_session_display_name_normalization_and_limits() {
    assert_eq!(
        normalize_session_display_name("  Research notes  ").unwrap(),
        Some("Research notes".to_string())
    );
    assert_eq!(normalize_session_display_name(" \n\t ").unwrap(), None);
    assert!(normalize_session_display_name(&"a".repeat(81)).is_err());
    assert!(normalize_session_display_name(&"🦀".repeat(65)).is_err());
    assert!(normalize_session_display_name("unsafe\u{202e}name").is_err());
    assert!(normalize_session_display_name("unsafe\u{1b}name").is_err());
}

#[test]
fn test_session_display_name_roundtrips_and_migrates() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().to_str().unwrap();
    let mut state = sample_state();
    state.display_name = Some("Research notes".to_string());
    std::fs::write(
        format!("{path}/session_state.json"),
        serde_json::to_string(&state).unwrap(),
    )
    .unwrap();

    let loaded = SessionState::load_checked(path).unwrap();
    assert_eq!(loaded.schema_version, 5);
    assert_eq!(loaded.display_name.as_deref(), Some("Research notes"));
    assert!(loaded.needs_migration_save);
}

/// Resume must read the unified session_state.json that save_session writes.
#[test]
fn test_load_unified_session_state() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().to_str().unwrap();
    let json = serde_json::to_string_pretty(&sample_state()).unwrap();
    std::fs::write(format!("{}/session_state.json", path), json).unwrap();

    let loaded = SessionState::load(path);
    assert_eq!(
        loaded.blocks.len(),
        1,
        "blocks must load from session_state.json"
    );
    assert_eq!(loaded.blocks[0].content, "hello");
    assert_eq!(loaded.messages.len(), 1);
    assert_eq!(loaded.history, vec!["hello".to_string()]);
    assert_eq!(loaded.theme_name, "Matrix");
}

#[test]
fn test_usage_and_request_ledger_roundtrip_and_rebuild() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().to_str().unwrap();
    let usage = lethetic::accounting::Usage {
        uncached_input_tokens: 10,
        cache_read_input_tokens: 80,
        cache_creation_input_tokens: 10,
        output_tokens: 7,
        total_input_tokens: Some(100),
        breakdown_complete: true,
    };
    let mut state = sample_state();
    state.blocks[0].logical_turn_id = Some("turn-1".to_string());
    state.blocks[0].usage = None;
    state
        .accounting
        .record_request(lethetic::accounting::RequestAccounting {
            request_id: "request-1".to_string(),
            logical_turn_id: "turn-1".to_string(),
            connection_id: "proxy".to_string(),
            model: "gpt-5.6-sol".to_string(),
            usage,
            estimated_cost: None,
            completed: true,
            in_flight: false,
        })
        .unwrap();
    // Persist deliberately stale derived totals; load must rebuild from requests.
    state.accounting.session = Default::default();
    std::fs::write(
        format!("{path}/session_state.json"),
        serde_json::to_string(&state).unwrap(),
    )
    .unwrap();

    let loaded = SessionState::load(path);
    assert_eq!(loaded.blocks[0].usage, Some(usage));
    assert_eq!(loaded.blocks[0].logical_turn_id.as_deref(), Some("turn-1"));
    assert_eq!(loaded.blocks[0].prompt_tokens, Some(100));
    assert_eq!(loaded.blocks[0].completion_tokens, Some(7));
    assert_eq!(
        loaded.accounting.latest_logical_turn_id.as_deref(),
        Some("turn-1")
    );
    assert_eq!(loaded.accounting.requests.len(), 1);
    assert_eq!(loaded.accounting.session.request_count, 1);
    assert_eq!(loaded.accounting.session.usage, usage);
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
    )
    .unwrap();
    std::fs::write(
        format!("{}/context.json", path),
        serde_json::to_string(&state.messages).unwrap(),
    )
    .unwrap();

    let loaded = SessionState::load(path);
    let loaded_again = SessionState::load(path);
    assert_eq!(
        loaded.session_id, loaded_again.session_id,
        "an interrupted legacy migration must retry with the same durable identity"
    );
    assert_eq!(
        loaded.blocks.len(),
        1,
        "blocks must load from legacy ui_state.json"
    );
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
    let loaded_again = SessionState::load(path);
    assert_eq!(loaded.session_id, loaded_again.session_id);
    assert_eq!(loaded.blocks.len(), 1);
    assert!(loaded.blocks[0].usage.is_none());
    assert_eq!(loaded.accounting.session.request_count, 0);
    assert!(loaded.history.is_empty());
    assert!(loaded.theme_name.is_empty());
    assert_eq!(loaded.schema_version, 5);
    assert!(loaded.session_id.is_some());
    assert!(loaded.needs_migration_save);
}

#[test]
fn legacy_error_blocks_migrate_to_typed_provenance_without_losing_local_detail() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().to_str().unwrap();
    let provider_sentinel = "abcabcabcabcabcabcabcabcabcabcabcabc";
    let tool_sentinel = "defdefdefdefdefdefdefdefdefdefdefdef";
    let mut state = sample_state();
    state.schema_version = 0;
    state.blocks = vec![
        RenderBlock {
            block_type: BlockType::Text,
            content: format!(
                "Partial assistant response.\n\n{} ERROR: quota denied {provider_sentinel}\n",
                lethetic::icons::WARNING
            ),
            title: None,
            success: Some(true),
            prompt_tokens: None,
            completion_tokens: None,
            usage: None,
            estimated_cost: None,
            logical_turn_id: None,
            cached_lines: None,
            cached_line_count: None,
        },
        RenderBlock {
            block_type: BlockType::ToolResult,
            content: format!("Sub-agent failed: quota denied {tool_sentinel}"),
            title: Some("Run validation".to_string()),
            success: Some(true),
            prompt_tokens: None,
            completion_tokens: None,
            usage: None,
            estimated_cost: None,
            logical_turn_id: None,
            cached_lines: None,
            cached_line_count: None,
        },
    ];
    std::fs::write(
        format!("{path}/session_state.json"),
        serde_json::to_string(&state).unwrap(),
    )
    .unwrap();

    let loaded = SessionState::load_checked(path).unwrap();
    assert_eq!(loaded.schema_version, 5);
    assert!(loaded.needs_migration_save);
    assert_eq!(loaded.blocks.len(), 3);
    assert_eq!(loaded.blocks[0].block_type, BlockType::Text);
    assert_eq!(
        loaded.blocks[0].content.trim(),
        "Partial assistant response."
    );
    assert_eq!(loaded.blocks[1].block_type, BlockType::ProviderError);
    assert_eq!(loaded.blocks[1].success, Some(false));
    assert!(loaded.blocks[1].content.contains(provider_sentinel));
    assert_eq!(loaded.blocks[2].block_type, BlockType::ToolError);
    assert_eq!(loaded.blocks[2].success, Some(false));
    assert!(loaded.blocks[2].content.contains(tool_sentinel));
}

#[test]
fn unified_legacy_tool_message_status_migrates_without_changing_content() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().to_str().unwrap();
    let sentinel = "Sub-agent failed: quota denied tenant violet request-local-42";
    let mut state = sample_state();
    state.schema_version = 4;
    state.messages = task_tool_conversation(sentinel, false);
    state.blocks.clear();
    bind_state_to_directory(&mut state, &dir);
    let original_content = state.messages[1].content.clone();
    std::fs::write(
        format!("{path}/session_state.json"),
        serde_json::to_string(&state).unwrap(),
    )
    .unwrap();

    let loaded = SessionState::load_checked(path).unwrap();

    assert_eq!(loaded.schema_version, 5);
    assert!(loaded.needs_migration_save);
    assert_eq!(loaded.messages[1].content, original_content);
    assert!(loaded.messages[1].tool_result_is_error);
    assert!(loaded.messages[1].content.contains(sentinel));
}

#[test]
fn split_legacy_context_migrates_tool_message_status() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().to_str().unwrap();
    let sentinel = "Sub-agent failed: quota denied tenant violet split-local-42";
    let messages = task_tool_conversation(sentinel, false);
    let original_content = messages[1].content.clone();
    std::fs::write(
        format!("{path}/context.json"),
        serde_json::to_string(&messages).unwrap(),
    )
    .unwrap();
    std::fs::write(format!("{path}/ui_state.json"), "[]").unwrap();

    let loaded = SessionState::load_checked(path).unwrap();

    assert_eq!(loaded.messages[1].content, original_content);
    assert!(loaded.messages[1].tool_result_is_error);
    assert!(loaded.messages[1].content.contains(sentinel));
}

#[test]
fn current_schema_false_tool_status_is_authoritative() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().to_str().unwrap();
    let mut state = sample_state();
    state.schema_version = 5;
    state.messages = task_tool_conversation("Sub-agent failed: ordinary fixture text", false);
    state.blocks.clear();
    bind_state_to_directory(&mut state, &dir);
    std::fs::write(
        format!("{path}/session_state.json"),
        serde_json::to_string(&state).unwrap(),
    )
    .unwrap();

    let loaded = SessionState::load_checked(path).unwrap();

    assert!(!loaded.messages[1].tool_result_is_error);
    assert!(!loaded.needs_migration_save);
}

#[test]
fn interrupted_tool_call_repair_adds_a_matching_ui_error() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().to_str().unwrap();
    let mut state = sample_state();
    state.schema_version = 5;
    let arguments = serde_json::json!({
        "description": "Calculate",
        "expression": "2 + 2"
    });
    state.messages = vec![Message {
        role: "assistant".to_string(),
        content: String::new(),
        tool_calls: Some(vec![ToolCall {
            id: "interrupted-call".to_string(),
            provider_id: None,
            function: FunctionCall {
                name: "calculate".to_string(),
                arguments: arguments.clone(),
            },
        }]),
        provider_content: None,
        tool_result_is_error: false,
    }];
    state.blocks = vec![RenderBlock {
        block_type: BlockType::ToolCall,
        content: format!(
            "call:calculate{}",
            serde_json::to_string(&arguments).unwrap()
        ),
        title: Some("Calculate".to_string()),
        success: Some(true),
        prompt_tokens: None,
        completion_tokens: None,
        usage: None,
        estimated_cost: None,
        logical_turn_id: None,
        cached_lines: None,
        cached_line_count: None,
    }];
    bind_state_to_directory(&mut state, &dir);
    std::fs::write(
        format!("{path}/session_state.json"),
        serde_json::to_string(&state).unwrap(),
    )
    .unwrap();

    let loaded = SessionState::load_checked(path).unwrap();

    assert!(loaded.needs_migration_save);
    assert_eq!(loaded.messages.len(), 2);
    assert!(loaded.messages[1].tool_result_is_error);
    assert_eq!(loaded.blocks.len(), 2);
    assert_eq!(loaded.blocks[1].block_type, BlockType::ToolError);
    assert_eq!(loaded.blocks[1].success, Some(false));
    assert!(loaded.blocks[1].content.contains("interrupted"));
}

#[test]
fn obsolete_empty_session_provider_error_is_only_removed_from_legacy_schema() {
    let obsolete = format!(
        "\n{} ERROR: Loaded session has no conversation content\n",
        lethetic::icons::WARNING
    );
    for (schema_version, expected_blocks) in [(4, 0), (5, 1)] {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_str().unwrap();
        let mut state = sample_state();
        state.schema_version = schema_version;
        state.messages.clear();
        state.blocks = vec![RenderBlock {
            block_type: BlockType::ProviderError,
            content: obsolete.clone(),
            title: None,
            success: Some(false),
            prompt_tokens: None,
            completion_tokens: None,
            usage: None,
            estimated_cost: None,
            logical_turn_id: None,
            cached_lines: None,
            cached_line_count: None,
        }];
        bind_state_to_directory(&mut state, &dir);
        std::fs::write(
            format!("{path}/session_state.json"),
            serde_json::to_string(&state).unwrap(),
        )
        .unwrap();

        let loaded = SessionState::load_checked(path).unwrap();
        assert_eq!(
            loaded.blocks.len(),
            expected_blocks,
            "schema {schema_version}"
        );
    }
}

#[test]
fn test_future_session_schema_is_rejected_without_legacy_fallback() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().to_str().unwrap();
    std::fs::write(
        format!("{path}/session_state.json"),
        r#"{"schema_version": 6, "messages": [], "blocks": []}"#,
    )
    .unwrap();
    std::fs::write(format!("{path}/ui_state.json"), "[]").unwrap();

    let error = SessionState::load_checked(path).unwrap_err();
    assert!(error.contains("unsupported session state schema 6"));
}

#[test]
fn test_runtime_identity_requires_workspace_binding() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().to_str().unwrap();
    let mut state = sample_state();
    state.python_runtime_id = Some("01234567-89ab-4def-8123-456789abcdef".to_string());
    std::fs::write(
        format!("{path}/session_state.json"),
        serde_json::to_string(&state).unwrap(),
    )
    .unwrap();

    let error = SessionState::load_checked(path).unwrap_err();
    assert!(error.contains("missing its managed workspace binding"));
}

#[test]
fn test_provider_replay_and_tool_error_roundtrip() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().to_str().unwrap();
    let mut state = sample_state();
    state.messages = vec![
        Message {
            role: "assistant".to_string(),
            content: String::new(),
            tool_calls: Some(vec![lethetic::context::ToolCall {
                id: "toolu_signed".to_string(),
                provider_id: Some("toolu_signed".to_string()),
                function: lethetic::context::FunctionCall {
                    name: "python".to_string(),
                    arguments: serde_json::json!({"code": "1 / 0"}),
                },
            }]),
            provider_content: Some(vec![serde_json::json!({
                "type": "thinking",
                "thinking": "signed reasoning",
                "signature": "sig_opaque"
            }), serde_json::json!({
                "type": "tool_use",
                "id": "toolu_signed",
                "name": "python",
                "input": {"code": "1 / 0"}
            })]),
            tool_result_is_error: false,
        },
        Message {
            role: "tool".to_string(),
            content: "<|tool_response>response:python{result:<|'|>ZeroDivisionError<|'|>,tool_call_id:<|'|>toolu_signed<|'|>}<tool_response|><turn|>".to_string(),
            tool_calls: None,
            provider_content: None,
            tool_result_is_error: true,
        },
    ];
    std::fs::write(
        format!("{path}/session_state.json"),
        serde_json::to_string_pretty(&state).unwrap(),
    )
    .unwrap();

    let loaded = SessionState::load(path);
    assert_eq!(loaded.messages.len(), 2);
    assert_eq!(
        loaded.messages[0].provider_content.as_ref().unwrap()[0]["signature"],
        "sig_opaque"
    );
    assert_eq!(
        loaded.messages[0].tool_calls.as_ref().unwrap()[0]
            .provider_id
            .as_deref(),
        Some("toolu_signed")
    );
    assert!(loaded.messages[1].tool_result_is_error);
}

#[test]
fn test_legacy_message_defaults_provider_and_error_fields() {
    let message: Message =
        serde_json::from_str(r#"{"role":"tool","content":"legacy","tool_calls":null}"#).unwrap();
    assert!(message.provider_content.is_none());
    assert!(!message.tool_result_is_error);
}

#[test]
fn test_load_missing_session_is_empty() {
    let dir = TempDir::new().unwrap();
    let error = SessionState::load_checked(dir.path().to_str().unwrap()).unwrap_err();
    assert!(error.contains("no durable or legacy state"), "{error}");

    let loaded = SessionState::load(dir.path().to_str().unwrap());
    assert!(loaded.blocks.is_empty());
    assert!(loaded.messages.is_empty());
}
