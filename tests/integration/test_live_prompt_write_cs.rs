/// Full integration test: send a real prompt asking the model to write a Hello World C# file.
/// Verifies the entire pipeline: prompt → model → tool call parse → write_file execution.
/// Run with: cargo test --test test_live_prompt_write_cs -- --ignored --nocapture
use std::fs;
use reqwest::Client;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tempfile::TempDir;

use lethetic::config::Config;
use lethetic::context::ContextManager;
use lethetic::system_prompt;
use lethetic::client::{trigger_llm_request, StreamEvent};
use lethetic::parser;

#[tokio::test]
#[ignore]
async fn test_live_prompt_write_cs_helloworld() -> Result<(), String> {
    let config_content = fs::read_to_string("config.yml")
        .map_err(|_| "Could not read config.yml".to_string())?;
    let config: Config = serde_yaml::from_str(&config_content)
        .map_err(|e| format!("Failed to parse config: {}", e))?;

    let tmp = TempDir::new().map_err(|e| e.to_string())?;
    let cwd = tmp.path().to_str().unwrap().to_string();

    let client = Client::new();
    let sys = system_prompt::SystemPromptManager::resolve_prompt(
        system_prompt::DEFAULT_PROMPT_TEMPLATE, &cwd, &config,
    );
    let mut ctx = ContextManager::new(config.context_size, Some(sys));
    ctx.set_cwd(cwd.clone());
    ctx.add_message("user", "Write a Hello World C# console app to helloworld.cs using write_file.");

    let (tx, mut rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    trigger_llm_request(client.clone(), config.clone(), &ctx, tx, cancel, false,
        Some("ignore/.lethetic/sessions/test_live_write_cs".to_string()));

    let mut full = String::new();
    let mut got_tool_call = false;
    let mut error: Option<String> = None;

    while let Some(ev) = rx.recv().await {
        match ev {
            StreamEvent::Chunk(c) => full.push_str(&c),
            StreamEvent::Error(e) => { error = Some(e); break; }
            StreamEvent::Done { .. } => break,
            _ => {}
        }
    }

    if let Some(e) = error {
        return Err(format!("Stream error: {}", e));
    }

    println!("RAW_OUTPUT:\n{}", full);

    // Parse tool call
    let tc = parser::find_tool_call(&full, true)
        .ok_or("No tool call found in response")?
        .map_err(|(e, _)| format!("Tool call parse error: {}", e))?
        .0;

    println!("Tool call: {} args={}", tc.function.name, tc.function.arguments);

    if tc.function.name != "write_file" {
        return Err(format!("Expected write_file, got {}", tc.function.name));
    }

    let path = tc.function.arguments["path"].as_str().unwrap_or("");
    let content = tc.function.arguments["content"].as_str().unwrap_or("");

    if path.is_empty() {
        return Err("write_file path is empty".to_string());
    }
    if !content.contains("Hello") {
        return Err(format!("Content doesn't look like Hello World: {:?}", &content[..content.len().min(200)]));
    }

    // Actually execute the write
    let (result, _) = lethetic::tools::execute(
        "write_file", &tc.function.arguments, &cwd,
        CancellationToken::new(), mpsc::unbounded_channel().0, &client, &config,
    ).await;

    println!("write_file result: {}", result);

    if result.contains("Error") || result.contains("error") {
        return Err(format!("write_file failed: {}", result));
    }

    // Verify file was written
    let written_path = std::path::Path::new(&cwd).join(path);
    let written = fs::read_to_string(&written_path)
        .map_err(|e| format!("Could not read written file {}: {}", path, e))?;

    println!("Written file ({} bytes):\n{}", written.len(), written);

    if !written.contains("Hello") {
        return Err(format!("Written file doesn't contain Hello World content"));
    }

    got_tool_call = true;
    assert!(got_tool_call);
    Ok(())
}
