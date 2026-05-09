use lethetic::app::{App, BlockType};
use lethetic::config::Config;
use lethetic::context::{ContextManager, ToolCall, FunctionCall};
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
        enable_image_processing_tool: false,
            theme: None,
    };
    let mut app = App::new(&config);
    app.current_dir = cwd.to_string();

    // Simulate the logic in main.rs for read_file
    let tool_args = serde_json::json!({"path": file_path});
    
    // In main.rs, this happens after successful read_file execution
    let full_path_buf = std::path::Path::new(&app.current_dir).join(file_path);
    if let Ok(content) = std::fs::read_to_string(&full_path_buf) {
        app.context_manager.update_latest_file(file_path.to_string(), content);
    }

    let entry = app.context_manager.latest_files.get(file_path).expect("File should be in context");
    assert_eq!(entry.content, raw_content, "Context should contain RAW content, not formatted content");
    assert!(!entry.content.contains("```"), "Context should not contain markdown fences");
    assert!(!entry.content.contains("     1\t"), "Context should not contain line numbers");
}

#[test]
fn test_write_file_context_update() {
    let config = Config {
        server_url: "http://brainiac-nvidia:7210/v1/responses".to_string(),
        model: "Gemma-4-26B-TurboQuant-262k".to_string(),
        context_size: 2048,
        tool_wrapper: None,
        enable_image_processing_tool: false,
            theme: None,
    };
    let mut app = App::new(&config);

    let file_path = "new.rs";
    let new_content = "fn test() {}";
    let tool_args = serde_json::json!({
        "path": file_path,
        "content": new_content
    });

    // Simulate the logic in main.rs for write_file
    if let Some(path) = tool_args["path"].as_str() {
        if let Some(content) = tool_args["content"].as_str() {
            app.context_manager.update_latest_file(path.to_string(), content.to_string());
        }
    }

    let entry = app.context_manager.latest_files.get(file_path).expect("File should be in context after write");
    assert_eq!(entry.content, new_content);
}

#[test]
fn test_tool_call_json_formatting() {
    use lethetic::context::ToolCall;
    use lethetic::context::FunctionCall;

    let config = Config {
        server_url: "http://localhost:8000".to_string(),
        model: "test-model".to_string(),
        context_size: 32768,
        tool_wrapper: None,
        enable_image_processing_tool: false,
            theme: None,
    };
    let mut app = App::new(&config);
    
    let tool_call = ToolCall {
        id: "test_id".to_string(),
        function: FunctionCall {
            name: "read_file".to_string(),
            arguments: serde_json::json!({
                "path": "src/main.rs",
                "description": "Read main.rs",
                "tool_call_id": "test_id"
            }),
        },
    };

    app.context_manager.add_assistant_tool_call("I will read the file.", vec![tool_call]);
    let raw_prompt = app.context_manager.get_raw_prompt();

    // Verify the jinja-compatible call:func{args} format is used (not raw JSON)
    assert!(raw_prompt.contains("<|tool_call>call:read_file{"), "Expected call:read_file format");
    assert!(raw_prompt.contains("<tool_call|>"), "Expected closing marker");
    assert!(raw_prompt.contains("path:<|\"|>src/main.rs<|\"|>"), "Expected gemma4 string delimiters");
}

fn make_tool_call(name: &str, id: &str) -> ToolCall {
    ToolCall {
        id: id.to_string(),
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
                msgs[i - 1].role, "assistant",
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
        ctx.add_message("user", &format!("user message number {} with some padding text here", i));
        ctx.add_message("assistant", &format!("assistant response {} with padding text here", i));
    }

    let msgs = ctx.get_messages();
    let system_msgs: Vec<_> = msgs.iter().filter(|m| m.role == "system").collect();
    assert!(
        !system_msgs.is_empty(),
        "system messages must survive trimming (got 0 after trim)"
    );
    assert!(
        system_msgs.iter().any(|m| m.content.contains("Current working directory")),
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

    let total: usize = ctx.latest_files.values().map(|f| f.tokens).sum();
    assert!(
        total <= 350,
        "latest_files token total {} should be ≤ 350 (35% of 1000)",
        total
    );

    // The oldest files (file0, file1, ...) should have been evicted first.
    // At 100 tokens each, 350 budget = 3 files max.
    assert!(
        ctx.latest_files.len() <= 3,
        "at most 3 files should remain in cache (got {})",
        ctx.latest_files.len()
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
    assert!(count >= 90 && count <= 150, "token count {} out of expected range 90–150", count);

    // truncate_to_tokens at 100 tokens → at most 400 chars
    let long = "x".repeat(800);
    let truncated = truncate_to_tokens(&long, 100);
    assert!(
        truncated.len() <= 400,
        "truncated string length {} should be ≤ 400",
        truncated.len()
    );
}
