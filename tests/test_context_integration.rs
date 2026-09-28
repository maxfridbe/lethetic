use lethetic::accounting::{ProviderRequestAccounting, Usage};
use lethetic::app::{App, BlockType};
use lethetic::client::prepare_provider_request;
use lethetic::config::Config;
use lethetic::context::{ContextManager, FunctionCall, ToolCall};
use std::fs;
use tempfile::tempdir;

#[test]
fn test_read_file_raw_context_update() {
    let dir = tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap();
    let file_path = "test.txt";
    let full_path = dir.path().join(file_path);

    let raw_content = "line 1\nline 2";
    fs::write(&full_path, raw_content).unwrap();

    let config = Config {
        server_url: "http://brainiac-nvidia:7210/v1/responses".to_string(),
        model: "Gemma-4-26B-TurboQuant-262k".to_string(),
        context_size: 2048,
        tool_wrapper: None,
        tool_profile: Default::default(),
        python_runtime: Default::default(),
        python_invocation: Default::default(),
        active_server: None,
        connection_kind: Default::default(),
        api_key: None,
        estimate_cost: None,
        pricing: None,
        input_cost_per_1m: None,
        output_cost_per_1m: None,
        enable_image_processing_tool: false,
        background_tasks: Default::default(),
        tool_calls: Default::default(),
        theme: None,
        model_servers: Vec::new(),
        thinking: None,
        extra_body: None,
        context_mode: None,
    };
    let mut app = App::new(&config);
    app.current_dir = cwd.to_string();

    // Simulate the logic in main.rs for read_file
    let _tool_args = serde_json::json!({"path": file_path});

    // In main.rs, this happens after successful read_file execution
    let full_path_buf = std::path::Path::new(&app.current_dir).join(file_path);
    if let Ok(content) = std::fs::read_to_string(&full_path_buf) {
        app.context_manager
            .update_latest_file(file_path.to_string(), content);
    }

    let entry = app
        .context_manager
        .active_files
        .get(file_path)
        .or_else(|| app.context_manager.latest_files.get(file_path))
        .expect("File should be in context");
    assert_eq!(
        entry.content, raw_content,
        "Context should contain RAW content, not formatted content"
    );
    assert!(
        !entry.content.contains("```"),
        "Context should not contain markdown fences"
    );
    assert!(
        !entry.content.contains("     1\t"),
        "Context should not contain line numbers"
    );
}

#[test]
fn test_write_file_context_update() {
    let config = Config {
        server_url: "http://brainiac-nvidia:7210/v1/responses".to_string(),
        model: "Gemma-4-26B-TurboQuant-262k".to_string(),
        context_size: 2048,
        tool_wrapper: None,
        tool_profile: Default::default(),
        python_runtime: Default::default(),
        python_invocation: Default::default(),
        active_server: None,
        connection_kind: Default::default(),
        api_key: None,
        estimate_cost: None,
        pricing: None,
        input_cost_per_1m: None,
        output_cost_per_1m: None,
        enable_image_processing_tool: false,
        background_tasks: Default::default(),
        tool_calls: Default::default(),
        theme: None,
        model_servers: Vec::new(),
        thinking: None,
        extra_body: None,
        context_mode: None,
    };
    let mut app = App::new(&config);

    let file_path = "new.rs";
    let new_content = "fn test() {}";
    let tool_args = serde_json::json!({
        "path": file_path,
        "content": new_content
    });

    // Simulate the logic in main.rs for write_file
    if let Some(path) = tool_args["path"].as_str()
        && let Some(content) = tool_args["content"].as_str()
    {
        app.context_manager
            .update_latest_file(path.to_string(), content.to_string());
    }

    let entry = app
        .context_manager
        .active_files
        .get(file_path)
        .or_else(|| app.context_manager.latest_files.get(file_path))
        .expect("File should be in context after write");
    assert_eq!(entry.content, new_content);
}

#[test]
fn test_tool_call_json_formatting() {
    use lethetic::context::FunctionCall;
    use lethetic::context::ToolCall;

    let config = Config {
        server_url: "http://localhost:8000".to_string(),
        model: "test-model".to_string(),
        context_size: 32768,
        tool_wrapper: None,
        tool_profile: Default::default(),
        python_runtime: Default::default(),
        python_invocation: Default::default(),
        active_server: None,
        connection_kind: Default::default(),
        api_key: None,
        estimate_cost: None,
        pricing: None,
        input_cost_per_1m: None,
        output_cost_per_1m: None,
        enable_image_processing_tool: false,
        background_tasks: Default::default(),
        tool_calls: Default::default(),
        theme: None,
        model_servers: Vec::new(),
        thinking: None,
        extra_body: None,
        context_mode: None,
    };
    let mut app = App::new(&config);

    let tool_call = ToolCall {
        id: "test_id".to_string(),
        provider_id: None,
        function: FunctionCall {
            name: "read_file".to_string(),
            arguments: serde_json::json!({
                "path": "src/main.rs",
                "description": "Read main.rs",
                "tool_call_id": "test_id"
            }),
        },
    };

    app.context_manager
        .add_assistant_tool_call("I will read the file.", vec![tool_call]);
    let raw_prompt = app.context_manager.get_raw_prompt();

    // Verify the jinja-compatible call:func{args} format is used (not raw JSON)
    assert!(
        raw_prompt.contains("<|tool_call>call:read_file{"),
        "Expected call:read_file format"
    );
    assert!(
        raw_prompt.contains("<tool_call|>"),
        "Expected closing marker"
    );
    assert!(
        raw_prompt.contains("path:<|\"|>src/main.rs<|\"|>"),
        "Expected gemma4 string delimiters"
    );
}

fn make_tool_call(name: &str, id: &str) -> ToolCall {
    ToolCall {
        id: id.to_string(),
        provider_id: None,
        function: FunctionCall {
            name: name.to_string(),
            arguments: serde_json::json!({ "tool_call_id": id }),
        },
    }
}

// Verify that (assistant-with-tool-calls, tool-result) pairs are never split by trim_context.
// After trimming, every `tool` role message must have an `assistant` with tool_calls immediately
// before it, and no `tool` message may appear at index 0.
#[test]
fn test_trim_pairs_never_split() {
    // Small budget to force trimming after a few turns
    let mut ctx = ContextManager::new(300, None);

    for i in 0..8 {
        let user_msg = format!("user turn {}", i);
        ctx.add_message("user", &user_msg);

        let tc = make_tool_call("read_file", &format!("call_{}", i));
        // ~80 chars of assistant content
        ctx.add_assistant_tool_call(
            &format!("I will call read_file for turn {i}. This is assistant content."),
            vec![tc],
        );
        ctx.add_tool_message(
            format!("call_{}", i),
            "read_file",
            &format!("contents of file {i}"),
        );
    }

    let msgs = ctx.get_messages();

    // No orphaned tool at position 0
    if let Some(first) = msgs.first() {
        assert_ne!(first.role, "tool", "tool message must never be at index 0");
    }

    // Every tool message must be preceded by an assistant-with-tool-calls
    for i in 1..msgs.len() {
        if msgs[i].role == "tool" {
            assert_eq!(
                msgs[i - 1].role,
                "assistant",
                "tool at index {i} must follow an assistant message"
            );
            assert!(
                msgs[i - 1].tool_calls.is_some(),
                "assistant before tool at index {i} must have tool_calls"
            );
        }
    }
}

// System messages must survive context trimming.
#[test]
fn test_trim_preserves_system_messages() {
    let mut ctx = ContextManager::new(400, None);

    ctx.add_message("system", "Current working directory: /home/user/project");
    ctx.add_message("system", "Git status: clean");

    // Fill with user/assistant turns to force trimming
    for i in 0..10 {
        ctx.add_message(
            "user",
            &format!("user message number {} with some padding text here", i),
        );
        ctx.add_message(
            "assistant",
            &format!("assistant response {} with padding text here", i),
        );
    }

    let msgs = ctx.get_messages();
    let system_msgs: Vec<_> = msgs.iter().filter(|m| m.role == "system").collect();
    assert!(
        !system_msgs.is_empty(),
        "system messages must survive trimming (got 0 after trim)"
    );
    assert!(
        system_msgs
            .iter()
            .any(|m| m.content.contains("Current working directory")),
        "cwd system message must survive"
    );
}

// latest_files should be evicted oldest-first when they exceed 35% of max_tokens.
#[test]
fn test_latest_files_eviction_on_budget() {
    // 1000 token budget → 35% = 350 tokens max for files → 1400 chars
    let mut ctx = ContextManager::new(1000, None);

    // Each file is ~100 tokens = 400 chars
    let file_content = "x".repeat(400);

    for i in 0..5 {
        // Small sleep is not needed since Instant::now() advances between insertions
        ctx.update_latest_file(format!("file{}.rs", i), file_content.clone());
    }

    // Files start in active_files; combined total should respect the 35% budget.
    let total: usize = ctx.active_files.values().map(|f| f.tokens).sum::<usize>()
        + ctx.latest_files.values().map(|f| f.tokens).sum::<usize>();
    assert!(
        total <= 350,
        "combined file token total {} should be ≤ 350 (35% of 1000)",
        total
    );

    // At 100 tokens each, 350 budget = 3 files max.
    let total_files = ctx.active_files.len() + ctx.latest_files.len();
    assert!(
        total_files <= 3,
        "at most 3 files should remain in cache (got {})",
        total_files
    );
}

// Verify the char/4 token estimator: a 400-char string should estimate to ~100 tokens,
// and truncate_to_tokens should produce a string no longer than max_tokens * 4 chars.
#[test]
fn test_token_estimate_chars_per_4() {
    use lethetic::context::truncate_to_tokens;

    let s = "abcd".repeat(100); // 400 chars
    let mut ctx = ContextManager::new(10000, None);
    ctx.add_message("user", &s);
    // Token count should be in the right ballpark (400/4 = 100 tokens for the message,
    // plus a small overhead for the prompt wrapper)
    let count = ctx.get_token_count();
    assert!(
        (90..=150).contains(&count),
        "token count {} out of expected range 90–150",
        count
    );

    // truncate_to_tokens at 100 tokens → at most 400 chars
    let long = "x".repeat(800);
    let truncated = truncate_to_tokens(&long, 100);
    assert!(
        truncated.len() <= 400,
        "truncated string length {} should be ≤ 400",
        truncated.len()
    );
}

#[test]
fn test_provider_native_content_counts_toward_trim_budget() {
    let mut ctx = ContextManager::new(180, None);
    let provider_content = vec![serde_json::json!({
        "type": "redacted_thinking",
        "data": "x".repeat(600),
    })];

    // The visible assistant text is tiny, but the native replay block is about 160 tokens.
    ctx.add_assistant_message("ok", Some(provider_content.clone()));
    assert_eq!(
        ctx.get_messages().len(),
        1,
        "the native block should initially fit"
    );
    assert!(
        ctx.get_token_count() > 150,
        "provider-native content must contribute to the displayed token estimate"
    );
    assert_eq!(
        ctx.get_messages_for_api()[0].provider_content.as_ref(),
        Some(&provider_content),
        "native replay blocks must remain unchanged while retained"
    );

    // This pushes the serialized native block over budget. The old content-only estimator kept it.
    ctx.add_message("user", &"y".repeat(120));
    let messages = ctx.get_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, "user");
}

#[test]
fn test_structured_tool_inputs_count_toward_trim_budget() {
    let mut ctx = ContextManager::new(190, None);
    let call = ToolCall {
        id: "call_1".to_string(),
        provider_id: None,
        function: FunctionCall {
            name: "run".to_string(),
            arguments: serde_json::json!({ "payload": "x".repeat(600) }),
        },
    };

    // The assistant's visible content is tiny; almost all of the cost is in the tool input.
    ctx.add_assistant_tool_call("ok", vec![call]);
    assert_eq!(
        ctx.get_messages().len(),
        1,
        "the tool call should initially fit"
    );

    // Adding its result crosses the budget, so trimming must remove the complete call/result unit.
    ctx.add_tool_message("call_1".to_string(), "run", "done");
    assert!(
        ctx.get_messages().is_empty(),
        "large structured tool input must be budgeted when trimming the pair"
    );
}

/// Cached files ride on the latest user message (not the system prompt),
/// so read the text of every message.
fn prepared_system_text(prepared: &lethetic::context::PreparedApiContext) -> String {
    prepared
        .messages()
        .iter()
        .map(|message| message.content.text())
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_growing_cached_file_is_bounded(mode: lethetic::context::ContextMode) {
    let dir = tempdir().unwrap();
    let path = dir.path().join("growing.txt");
    fs::write(&path, "cached-small").unwrap();

    let mut ctx = ContextManager::new(1_000, Some("system".to_string()));
    ctx.mode = mode;
    ctx.set_cwd(dir.path().to_string_lossy().into_owned());
    ctx.update_latest_file("growing.txt".to_string(), "cached-small".to_string());
    ctx.add_message("user", "inspect the file");
    let cached_estimate = ctx.get_token_count();

    let fresh = format!("{}FRESH-GROWTH-TAIL", "g".repeat(800));
    fs::write(&path, &fresh).unwrap();
    let prepared = ctx.prepare_api_context();
    let system = prepared_system_text(&prepared);
    assert!(system.contains("FRESH-GROWTH-TAIL"));
    assert!(prepared.estimated_tokens() > cached_estimate + 100);
    assert!(prepared.estimated_tokens() <= ctx.input_token_budget());

    let oversized = format!("{}MUST-NOT-BE-TRUNCATED-INTO-CONTEXT", "z".repeat(20_000));
    fs::write(&path, oversized).unwrap();
    let prepared = ctx.prepare_api_context();
    let system = prepared_system_text(&prepared);
    assert!(system.contains("omitted"), "{system}");
    assert!(!system.contains("MUST-NOT-BE-TRUNCATED-INTO-CONTEXT"));
    assert!(system.len() < 2_000, "oversized file leaked into context");
    assert!(prepared.estimated_tokens() <= ctx.input_token_budget());
}

#[test]
fn growing_cached_file_is_bounded_and_freshly_accounted_in_both_modes() {
    for mode in [
        lethetic::context::ContextMode::Lethetic,
        lethetic::context::ContextMode::Vercel,
    ] {
        assert_growing_cached_file_is_bounded(mode);
    }
}

#[test]
fn prepared_files_prioritize_active_complete_files_over_latest_files() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("latest.txt"), "L".repeat(300)).unwrap();
    fs::write(dir.path().join("active.txt"), "A".repeat(300)).unwrap();

    let mut ctx = ContextManager::new(400, None);
    ctx.mode = lethetic::context::ContextMode::Vercel;
    ctx.set_cwd(dir.path().to_string_lossy().into_owned());
    ctx.update_latest_file("latest.txt".to_string(), "old".to_string());
    let latest = ctx.active_files.remove("latest.txt").unwrap();
    ctx.latest_files.insert("latest.txt".to_string(), latest);
    ctx.update_latest_file("active.txt".to_string(), "old".to_string());

    let prepared = ctx.prepare_api_context();
    let system = prepared_system_text(&prepared);
    assert!(system.contains(&"A".repeat(300)), "{system}");
    assert!(!system.contains(&"L".repeat(300)), "{system}");
    assert!(system.contains("latest.txt"), "{system}");
    assert!(system.contains("omitted"), "{system}");
    assert!(prepared.estimated_tokens() <= ctx.input_token_budget());
}

#[test]
fn test_vercel_context_mode_formatting() {
    use lethetic::context::{ContextManager, ContextMode};
    use std::fs;
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap().to_string();
    let file_path = "foo.rs";
    let full_path = dir.path().join(file_path);
    let file_content = "fn foo() {}";
    fs::write(&full_path, file_content).unwrap();

    let mut ctx = ContextManager::new(2048, Some("You are a helpful assistant".to_string()));
    ctx.mode = ContextMode::Vercel;
    ctx.set_cwd(cwd);

    ctx.update_latest_file(file_path.to_string(), file_content.to_string());
    ctx.add_message("user", "Hello");

    let raw_prompt = ctx.get_raw_prompt();
    // Vercel mode should have no <latest_files> or <active_file> tags
    assert!(!raw_prompt.contains("<latest_files>"));
    assert!(!raw_prompt.contains("</latest_files>"));
    // It should have standard markdown style headers
    assert!(raw_prompt.contains("## File: foo.rs"));
    assert!(raw_prompt.contains("fn foo() {}"));

    // Check API messages payload
    let api_msgs = ctx.get_messages_for_api();
    assert_eq!(api_msgs.len(), 2); // 1 combined system message at 0, 1 user message
    assert_eq!(api_msgs[0].role, lethetic::transport::Role::System);
    assert_eq!(api_msgs[1].role, lethetic::transport::Role::User);

    let system_content = api_msgs[0].content.text();
    let user_content = api_msgs[1].content.text();
    // The stable system prompt stays in the system message; the file content
    // (formatted in markdown) rides on the latest user message, before its text.
    assert!(system_content.contains("You are a helpful assistant"));
    assert!(!system_content.contains("## File: foo.rs"));
    assert!(user_content.contains("## File: foo.rs"));
    assert!(user_content.contains("fn foo() {}"));
    assert!(user_content.ends_with("Hello"));
}

#[test]
fn marker_only_user_turn_has_stable_accounting_identity() {
    let mut app = App::new(&Config::default());
    app.blocks.clear();
    app.add_segment("<think></think>".to_string(), BlockType::User);
    assert_eq!(app.blocks.len(), 1);
    assert_eq!(app.blocks[0].block_type, BlockType::User);
    assert_eq!(app.blocks[0].content, "<think></think>");

    app.begin_logical_turn_accounting().unwrap();
    let turn_id = app.active_logical_turn_id.clone().unwrap();
    assert_eq!(
        app.blocks[0].logical_turn_id.as_deref(),
        Some(turn_id.as_str())
    );
    app.begin_provider_request("request-one".to_string());

    // Simulate front eviction and replacement. Accounting for the old turn must
    // not drift onto whichever User block now occupies the same vector index.
    app.blocks.clear();
    app.add_segment("new prompt".to_string(), BlockType::User);
    let usage = Usage {
        uncached_input_tokens: 12,
        output_tokens: 3,
        total_input_tokens: Some(12),
        breakdown_complete: true,
        ..Usage::default()
    };
    app.record_provider_request(ProviderRequestAccounting {
        request_id: "request-one".to_string(),
        connection_id: "proxy".to_string(),
        model: "gpt-5.6-sol".to_string(),
        usage,
        usage_reported: true,
        estimated_cost: None,
        completed: true,
        in_flight: false,
    })
    .unwrap();

    assert!(app.blocks[0].usage.is_none());
    assert_eq!(app.accounting.session.usage, usage);
}

#[test]
fn consecutive_user_prompts_get_distinct_turn_blocks_and_identities() {
    let mut app = App::new(&Config::default());
    app.blocks.clear();
    app.add_segment("first".to_string(), BlockType::User);
    let first_turn = app.begin_logical_turn_accounting().unwrap();

    app.add_segment("second".to_string(), BlockType::User);
    let first_id_before = app
        .blocks
        .iter()
        .find(|block| block.content == "first")
        .and_then(|block| block.logical_turn_id.clone())
        .unwrap();
    let second_turn = app.begin_logical_turn_accounting().unwrap();

    let users = app
        .blocks
        .iter()
        .filter(|block| block.block_type == BlockType::User)
        .collect::<Vec<_>>();
    assert_eq!(users.len(), 2);
    assert_eq!(users[0].content, "first");
    assert_eq!(
        users[0].logical_turn_id.as_deref(),
        Some(first_turn.as_str())
    );
    assert_eq!(first_id_before, first_turn);
    assert_eq!(users[1].content, "second");
    assert_eq!(
        users[1].logical_turn_id.as_deref(),
        Some(second_turn.as_str())
    );
    assert_ne!(first_turn, second_turn);
    assert!(app.begin_logical_turn_accounting().is_err());
}

#[test]
fn render_block_limit_handles_user_divider_pairs() {
    let mut app = App::new(&Config::default());
    app.blocks.clear();
    for index in 0..250 {
        app.add_segment(format!("prompt {index}"), BlockType::User);
        app.add_segment(format!("response {index}"), BlockType::Text);
    }
    assert!(app.blocks.len() <= 200, "{}", app.blocks.len());
}

#[test]
fn persisted_request_start_requires_a_durable_session() {
    let config = Config::default();
    let mut app = App::new(&config);
    app.blocks.clear();
    app.current_session_dir = None;
    app.session_directory_binding = None;
    app.add_segment("prompt".to_string(), BlockType::User);
    app.begin_logical_turn_accounting().unwrap();
    let accounting_before = app.accounting.clone();

    let error = app
        .persist_provider_request_start(prepare_provider_request(&config))
        .unwrap_err();

    assert!(
        error.contains("without a durable session directory"),
        "{error}"
    );
    assert_eq!(app.accounting, accounting_before);
    assert!(app.active_request_id.is_none());
}

#[test]
fn failed_request_start_persistence_rolls_back_provisional_accounting() {
    let config = Config::default();
    let mut app = App::new(&config);
    app.blocks.clear();
    app.add_segment("prompt".to_string(), BlockType::User);
    app.begin_logical_turn_accounting().unwrap();
    let accounting_before = app.accounting.clone();
    let blocks_before = app.blocks.clone();
    app.current_session_dir = Some("/definitely/not/a/bound/session".to_string());
    app.session_directory_binding = None;

    let error = app
        .persist_provider_request_start(prepare_provider_request(&config))
        .unwrap_err();
    assert!(error.contains("directory identity binding"), "{error}");
    assert_eq!(app.accounting, accounting_before);
    assert_eq!(app.active_request_id, None);
    assert_eq!(app.blocks.len(), blocks_before.len());
    assert_eq!(
        app.blocks[0].logical_turn_id,
        blocks_before[0].logical_turn_id
    );
    assert!(app.blocks[0].usage.is_none());
}

#[test]
fn older_tool_outputs_are_shortened_and_blank_failed_turns_are_noted() {
    let mut ctx = ContextManager::new(200_000, Some("system".to_string()));
    ctx.add_message("user", "do the work");
    let long_output: String = (0..200).map(|i| format!("line {i}\n")).collect();
    for index in 0..8 {
        let call = lethetic::context::ToolCall {
            id: format!("call_{index}"),
            provider_id: None,
            function: lethetic::context::FunctionCall {
                name: "run_shell_command".to_string(),
                arguments: serde_json::json!({"command": "seq 200"}),
            },
        };
        ctx.upsert_assistant_tool_call_with_provider("", vec![call], None);
        ctx.add_tool_message(format!("call_{index}"), "run_shell_command", &long_output);
    }
    ctx.add_message("assistant", "");
    ctx.add_message("user", "continue");
    let messages = ctx.prepare_api_context().into_messages();
    let tools: Vec<String> = messages
        .iter()
        .filter(|m| m.role == lethetic::transport::Role::Tool)
        .map(|m| m.content.text())
        .collect();
    assert_eq!(tools.len(), 8);
    for older in &tools[..2] {
        assert!(older.contains("lines omitted"), "{older}");
        assert!(older.contains("line 0") && older.contains("line 199"));
    }
    for recent in &tools[2..] {
        assert!(!recent.contains("omitted"));
    }
    assert!(messages.iter().any(|m| {
        m.role == lethetic::transport::Role::Assistant
            && m.content
                .text()
                .contains("interrupted before it produced any output")
    }));
}

#[test]
fn todo_list_rides_on_the_latest_user_message() {
    let mut ctx = ContextManager::new(200_000, Some("system".to_string()));
    ctx.set_todo_summary(Some(
        "<todos>\n- [pending] (high) write main.rs\n</todos>".into(),
    ));
    ctx.add_message("user", "first");
    ctx.add_message("assistant", "ok");
    ctx.add_message("user", "second");
    let messages = ctx.prepare_api_context().into_messages();
    let users: Vec<String> = messages
        .iter()
        .filter(|m| m.role == lethetic::transport::Role::User)
        .map(|m| m.content.text())
        .collect();
    assert!(!users[0].contains("<todos>"));
    assert!(users[1].contains("write main.rs") && users[1].ends_with("second"));
    assert!(!messages[0].content.text().contains("<todos>"));
}
