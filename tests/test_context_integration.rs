use lethetic::app::{App, BlockType};
use lethetic::config::Config;
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
