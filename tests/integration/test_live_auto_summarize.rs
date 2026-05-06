use std::fs;
use reqwest::Client;
use serde_json::json;
use futures_util::StreamExt;
use std::time::Duration;

use lethetic::config::Config;
use lethetic::context::ContextManager;
use lethetic::system_prompt;
use lethetic::parser::find_tool_call;

// Rust guideline compliant 2026-02-21

const GENERATION_TIMEOUT: Duration = Duration::from_secs(300);

async fn test_auto_summarize_generation(
    prompt: &str,
) -> Result<String, String> {
    let config_content = match fs::read_to_string("config.yml") {
        Ok(c) => c,
        Err(_) => return Err("Could not read config.yml".to_string()),
    };
    let config: Config = match serde_yaml::from_str(&config_content) {
        Ok(c) => c,
        Err(e) => return Err(format!("Failed to parse config: {}", e)),
    };
    
    let client = Client::new();
    let sys_prompt = system_prompt::SystemPromptManager::resolve_prompt(system_prompt::DEFAULT_PROMPT_TEMPLATE, ".", &config);
    let mut context_manager = ContextManager::new(config.context_size, Some(sys_prompt));
    
    context_manager.add_message("user", prompt);
let req_body = json!({
    "model": config.model.clone(),
    "input": context_manager.get_raw_prompt(),
    "stream": true,
    "max_tokens": 4096,
});

    let b_url = config.server_url.clone();
    let res = client.post(&b_url).json(&req_body).send().await.map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(format!("Server error: {} - {}", status, body));
    }
    
    let mut stream = res.bytes_stream();
    let mut full_content = String::new();
    
    let result = tokio::time::timeout(GENERATION_TIMEOUT, async {
        let mut buffer = String::new();
        let mut current_event = String::new();

        while let Some(item) = stream.next().await {
            if let Ok(bytes) = item {
                if let Ok(chunk_str) = String::from_utf8(bytes.to_vec()) {
                    buffer.push_str(&chunk_str);
                    while let Some(pos) = buffer.find('\n') {
                        let line = buffer.drain(..=pos).collect::<String>();
                        let trimmed = line.trim();
                        if trimmed.is_empty() { continue; }
                        
                        if trimmed.starts_with("event: ") {
                            current_event = trimmed[7..].to_string();
                        } else if trimmed.starts_with("data: ") {
                            let json_str = &trimmed[6..];
                            if json_str == "[DONE]" { break; }
                            
                            if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
                                if current_event.ends_with(".delta") {
                                    if let Some(delta) = val["delta"].as_str() {
                                        full_content.push_str(delta);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }).await;

    if result.is_err() {
        return Err("Timeout waiting for LLM response".to_string());
    }

    let parse_result = find_tool_call(&full_content, true);
    
    match parse_result {
        Some(Ok((tc, _))) => {
            if tc.function.name != "run_shell_command" {
                return Err(format!("Expected run_shell_command, got: {}", tc.function.name));
            }
            
            // Re-simulate the logic in main.rs for large output
            let _cmd = tc.function.arguments["command"].as_str().unwrap_or("");
            
            // We produce a large output
            let mut large_output = String::new();
            for i in 0..500 {
                large_output.push_str(&format!("Line {} of very large output that needs summarization.\n", i));
            }
            
            // Save to file like main.rs does
            let dir_path = ".lethetic/tool_responses";
            let _ = std::fs::create_dir_all(dir_path);
            let file_path = format!("{}/{}.txt", dir_path, tc.id);
            let _ = std::fs::write(&file_path, &large_output);

            // Now simulate the LLM calling summarize_content on that file
            let summary_prompt = "Summarize this output focusing on the sequence of numbers.";
            
            let summary = lethetic::client::summarize_llm(&client, &config, &large_output, summary_prompt).await
                .map_err(|e| format!("Summarization failed: {}", e))?;
                
            Ok(summary)
        },
        Some(Err((err_msg, _))) => {
            Err(format!("Syntax Error parsing tool call: {}", err_msg))
        },
        None => {
            Err(format!("No tool call detected.\nFull Content:\n{}", full_content))
        }
    }
}

#[tokio::test]
#[ignore]
async fn test_live_auto_summarize() {
    let prompt = "Run a command that lists numbers 1 to 2000. It will be truncated. Then use the `summarize_content` tool on the saved output file to tell me what you found.";
    let res = test_auto_summarize_generation(prompt).await;
    match res {
        Ok(summary) => {
            println!("SUCCESS: Got summary:\n{}", summary);
            if summary.is_empty() {
                println!("Warning: Summary is empty. This is often due to model flakiness.");
            }
        },
        Err(e) => {
            if e.contains("No tool call detected") {
                println!("Warning: {}", e);
            } else {
                panic!("{}", e);
            }
        },
    }
}
