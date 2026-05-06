/// Integration test to discover how supergemma4 natively formats tool calls
/// when given a tool definition WITHOUT our custom system prompt.
/// Run with: cargo test test_native_tool_format -- --nocapture
use std::fs;
use reqwest::Client;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use lethetic::config::Config;
use lethetic::context::ContextManager;
use lethetic::client::{trigger_llm_request, StreamEvent};
use lethetic::tools::run_shell_command;

fn build_prompt(tool_block: &str, user_message: &str) -> String {
    format!(
        "<bos><|turn>user\n{tool_block}\n{user_message}<turn|>\n<|turn>model\n",
        tool_block = tool_block,
        user_message = user_message,
    )
}

async fn collect_raw_response(config: &Config, raw_prompt: &str) -> Result<String, String> {
    use lethetic::context::ContextManager;
    use serde_json::json;

    // Build a context manager that will emit this exact prompt via get_raw_prompt().
    // We do this by using a no-system-prompt context and injecting the prompt as a
    // pre-formatted user message — but since get_raw_prompt() wraps everything, the
    // cleanest path is to POST directly with reqwest instead of trigger_llm_request.
    let client = Client::new();
    let req = json!({
        "prompt": raw_prompt,
        "stream": false,
        "n_predict": 512,
    });

    let res = client
        .post(&config.server_url)
        .json(&req)
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(format!("Server {}: {}", status, body));
    }

    let val: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    Ok(val["content"].as_str().unwrap_or("").to_string())
}

#[tokio::test]
async fn test_live_native_tool_format() -> Result<(), String> {
    let config_content = fs::read_to_string("config.yml")
        .map_err(|_| "Could not read config.yml".to_string())?;
    let config: Config = serde_yaml::from_str(&config_content)
        .map_err(|e| format!("Failed to parse config: {}", e))?;

    let tool_def = run_shell_command::get_definition();
    let tool_json = serde_json::to_string_pretty(&tool_def.function).unwrap();

    // --- Attempt 1: plain JSON block in user turn ---
    let prompt_plain = build_prompt(
        &format!("You have access to this tool:\n{}", tool_json),
        "Use run_shell_command to run: echo tool_calling_test",
    );

    println!("\n=== PROMPT (plain JSON) ===\n{}", prompt_plain);
    let response_plain = collect_raw_response(&config, &prompt_plain).await?;
    println!("\n=== RAW RESPONSE (plain JSON) ===\n{}", response_plain);

    // --- Attempt 2: Gemma-4 native <tool> marker ---
    let prompt_marker = build_prompt(
        &format!("<tool>{}</tool>", tool_json),
        "Use run_shell_command to run: echo tool_calling_test",
    );

    println!("\n=== PROMPT (tool marker) ===\n{}", prompt_marker);
    let response_marker = collect_raw_response(&config, &prompt_marker).await?;
    println!("\n=== RAW RESPONSE (tool marker) ===\n{}", response_marker);

    // --- Attempt 3: our existing <|tool>...<tool|> markers ---
    let prompt_pipe = build_prompt(
        &format!("<|tool>\n{}\n<tool|>", tool_json),
        "Use run_shell_command to run: echo tool_calling_test",
    );

    println!("\n=== PROMPT (pipe marker) ===\n{}", prompt_pipe);
    let response_pipe = collect_raw_response(&config, &prompt_pipe).await?;
    println!("\n=== RAW RESPONSE (pipe marker) ===\n{}", response_pipe);

    // At least one response should reference the tool name
    let any_mentions_tool = [&response_plain, &response_marker, &response_pipe]
        .iter()
        .any(|r| r.contains("run_shell_command") || r.contains("echo") || r.contains("tool_calling_test"));

    if !any_mentions_tool {
        println!("\n[WARN] No response mentioned the tool or command — model may not have called the tool.");
    }

    Ok(())
}
