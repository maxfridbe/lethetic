use std::time::Duration;
use serial_test::serial;
use tokio_util::sync::CancellationToken;

use lethetic::config::Config;
use lethetic::context::ContextManager;
use lethetic::system_prompt;
use lethetic::client::trigger_llm_request;
use lethetic::client::StreamEvent;

fn azure_config() -> Result<Config, String> {
    let cfg = Config::load("config.yml")?;
    
    // Resolve the Azure server from model_servers
    let azure = cfg.model_servers.iter()
        .find(|s| s.name.contains("Azure") || s.model.contains("DeepSeek-V4"))
        .ok_or_else(|| "No Azure server defined in config.yml model_servers".to_string())?;

    Ok(Config {
        server_url: azure.url.clone(),
        model: azure.model.clone(),
        context_size: azure.context_size.unwrap_or(262144),
        tool_wrapper: None,
        api_key: azure.api_key.clone(),
        estimate_cost: None,
        input_cost_per_1m: azure.input_cost_per_1m,
        output_cost_per_1m: azure.output_cost_per_1m,
        enable_image_processing_tool: false,
        theme: None,
        model_servers: cfg.model_servers.clone(),
        thinking: azure.thinking,
        extra_body: azure.extra_body.clone(),
        context_mode: azure.context_mode,
    })
}

/// Run a prompt through the Azure server and return the final text response.
async fn run_azure(prompt: &str) -> Result<String, String> {
    let config = azure_config()?;
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

/// Run a prompt that must produce a tool call; return the tool name and argument value.
async fn run_azure_tool(prompt: &str) -> Result<(String, serde_json::Value), String> {
    let config = azure_config()?;
    let client = reqwest::Client::new();

    let sys = system_prompt::SystemPromptManager::resolve_prompt(
        system_prompt::DEFAULT_PROMPT_TEMPLATE, ".", &config,
    );
    let mut ctx = ContextManager::new(config.context_size, Some(sys));
    ctx.add_message("user", prompt);

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    trigger_llm_request(client, config.clone(), &ctx, tx, cancel, false, None);

    let mut text = String::new();
    let mut tool_name: Option<String> = None;
    let mut tool_args: Option<serde_json::Value> = None;

    let result = tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            match rx.recv().await {
                Some(StreamEvent::Chunk(c)) => text.push_str(&c),
                Some(StreamEvent::ToolCalls(calls)) => {
                    if let Some(tc) = calls.first() {
                        tool_name = Some(tc.function.name.clone());
                        tool_args = Some(tc.function.arguments.clone());
                    }
                }
                Some(StreamEvent::Done { .. }) => break,
                Some(StreamEvent::Error(e)) => return Err(e),
                None => break,
                _ => {}
            }
        }
        Ok(())
    }).await;

    result.map_err(|_| "Timeout after 180s".to_string())??;

    if let (Some(name), Some(args)) = (tool_name, tool_args) {
        return Ok((name, args));
    }

    // Fallback parser check
    match lethetic::parser::find_tool_call(&text, true) {
        Some(Ok((tc, _))) => Ok((tc.function.name, tc.function.arguments)),
        Some(Err((e, _))) => Err(format!("Syntax error: {}\nContent:\n{}", e, text)),
        None => Err(format!("No tool call detected.\nContent:\n{}", text)),
    }
}

#[tokio::test]
#[serial(llm)]
async fn test_azure_hello() {
    match run_azure("Reply with exactly: Hello from Azure").await {
        Ok(resp) => {
            println!("Response: {}", resp);
            assert!(resp.to_lowercase().contains("hello"), "Unexpected response: {}", resp);
        }
        Err(e) => panic!("{}", e),
    }
}

#[tokio::test]
#[serial(llm)]
async fn test_azure_write_file_no_markers() {
    let (tool, args) = run_azure_tool(
        "Use the 'write_file' tool to write 'fn main() {}' to 'src/main.rs'. Output ONLY the tool call."
    ).await.expect("write_file tool call failed");
    
    assert_eq!(tool, "write_file");
    let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
    assert!(!content.contains("<|\"|>"), "Content should NOT contain asymmetric markers: {}", content);
    assert!(content.contains("fn main"), "Content should contain the code block: {}", content);
}

#[tokio::test]
#[serial(llm)]
async fn test_azure_write_csharp_helloworld() {
    let (tool, args) = run_azure_tool(
        "Use the 'write_file' tool to write a simple C# Hello World console application to Program.cs. Output ONLY the tool call."
    ).await.expect("write_file C# tool call failed");

    assert_eq!(tool, "write_file");
    
    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
    assert!(path.contains("Program.cs"), "Path should contain Program.cs: {}", path);
    
    let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
    assert!(!content.contains("<|\"|>"), "Content should NOT contain asymmetric markers: {}", content);
    assert!(content.contains("Console.WriteLine"), "Content should contain Console.WriteLine: {}", content);
    assert!(content.contains("Hello World") || content.contains("Hello, World"), "Content should contain hello world string: {}", content);
}
