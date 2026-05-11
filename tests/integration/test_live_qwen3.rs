/// Integration tests for the Qwen3-27B server on port 7211.
/// Uses the production gemma_chat streaming path so tool call parsing is model-agnostic.
use std::fs;
use std::time::Duration;
use futures_util::StreamExt;
use serial_test::serial;
use tokio_util::sync::CancellationToken;

use lethetic::config::Config;
use lethetic::context::ContextManager;
use lethetic::parser::ParserMode;
use lethetic::system_prompt;
use lethetic::client::trigger_llm_request;
use lethetic::client::StreamEvent;

fn qwen3_config() -> Result<Config, String> {
    let raw = fs::read_to_string("config.yml")
        .map_err(|_| "Could not read config.yml".to_string())?;
    let cfg: Config = serde_yaml::from_str(&raw)
        .map_err(|e| format!("Failed to parse config.yml: {}", e))?;
    // Resolve the Qwen3 server from model_servers
    let qwen = cfg.model_servers.iter()
        .find(|s| s.parser == "qwen3")
        .ok_or_else(|| "No qwen3 server defined in config.yml model_servers".to_string())?;
    Ok(Config {
        server_url: qwen.url.clone(),
        model: qwen.model.clone(),
        context_size: 131072,
        tool_wrapper: None,
        enable_image_processing_tool: false,
        theme: None,
        model_servers: cfg.model_servers.clone(),
    })
}

/// Run a prompt through the Qwen3 server and return the final text response.
/// Returns Err if the server is unavailable or times out.
async fn run_qwen3(prompt: &str) -> Result<String, String> {
    let config = qwen3_config()?;
    let client = reqwest::Client::new();

    let sys = system_prompt::SystemPromptManager::resolve_prompt(
        system_prompt::DEFAULT_PROMPT_TEMPLATE, ".", &config,
    );
    let mut ctx = ContextManager::new(config.context_size, Some(sys));
    ctx.add_message("user", prompt);

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    trigger_llm_request(
        client, config.clone(), &ctx,
        tx, cancel, false, None,
    );

    let mut text = String::new();
    let result = tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            match rx.recv().await {
                Some(StreamEvent::Chunk(c)) => text.push_str(&c),
                Some(StreamEvent::Done { .. }) => break,
                Some(StreamEvent::Error(e)) => return Err(e),
                None => break,
                _ => {}
            }
        }
        Ok(text.clone())
    }).await;

    result.map_err(|_| "Timeout after 180s".to_string())?
}

/// Run a prompt that must produce a tool call; return the tool name.
async fn run_qwen3_tool(prompt: &str) -> Result<String, String> {
    let content = run_qwen3(prompt).await?;
    println!("RAW_OUTPUT_START\n{}\nRAW_OUTPUT_END", content);
    match lethetic::parser::find_tool_call(&content, true) {
        Some(Ok((tc, _))) => Ok(tc.function.name),
        Some(Err((e, _))) => Err(format!("Syntax error: {}\nContent:\n{}", e, content)),
        None => Err(format!("No tool call detected.\nContent:\n{}", content)),
    }
}

// ── Connectivity ──────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn test_qwen3_hello() {
    match run_qwen3("Reply with exactly: Hello from Qwen3").await {
        Ok(resp) => {
            println!("Response: {}", resp);
            assert!(!resp.trim().is_empty(), "Empty response");
        }
        Err(e) => panic!("{}", e),
    }
}

// ── Tool calls ────────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn test_qwen3_calculate() {
    let tool = run_qwen3_tool(
        "Use the 'calculate' tool to evaluate: 7 * 8. Output ONLY the tool call."
    ).await.expect("calculate tool call");
    assert_eq!(tool, "calculate");
}

#[tokio::test]
#[serial]
async fn test_qwen3_run_shell_command() {
    let tool = run_qwen3_tool(
        "Use 'run_shell_command' to run: echo hello. Output ONLY the tool call."
    ).await.expect("run_shell_command tool call");
    assert_eq!(tool, "run_shell_command");
}

#[tokio::test]
#[serial]
async fn test_qwen3_read_file() {
    let tool = run_qwen3_tool(
        "Use 'read_file' to read the file at path 'config.yml'. Output ONLY the tool call."
    ).await.expect("read_file tool call");
    assert_eq!(tool, "read_file");
}

#[tokio::test]
#[serial]
async fn test_qwen3_search_text() {
    let tool = run_qwen3_tool(
        "Use 'search_text' to search for the pattern 'fn main' in the current directory. Output ONLY the tool call."
    ).await.expect("search_text tool call");
    assert_eq!(tool, "search_text");
}
