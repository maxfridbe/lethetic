use std::fs;
use reqwest::Client;
use serde_json::json;
use futures_util::StreamExt;
use std::time::Duration;

use lethetic::config::Config;
use lethetic::context::ContextManager;
use lethetic::system_prompt;

#[tokio::test]
async fn test_live_hello() -> Result<(), String> {
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
    
    context_manager.add_message("user", "hello");
    
    let req_body = json!({
        "model": config.model.clone(),
        "input": context_manager.get_raw_prompt(),
        "stream": true,
        "max_tokens": 16384,
    });

    println!("Sending request to: {}", config.server_url);
    let b_url = config.server_url.clone();
    let res = client.post(&b_url).json(&req_body).send().await.map_err(|e| e.to_string())?;
    
    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(format!("Server error: {} - {}", status, body));
    }
    
    let mut stream = res.bytes_stream();
    let timeout_duration = Duration::from_secs(60);

    let result = tokio::time::timeout(timeout_duration, async {
        let mut buffer = String::new();
        while let Some(item) = stream.next().await {
            match item {
                Ok(bytes) => {
                    if let Ok(chunk_str) = String::from_utf8(bytes.to_vec()) {
                        println!("RAW CHUNK:\n{}", chunk_str);
                        buffer.push_str(&chunk_str);
                        while let Some(pos) = buffer.find('\n') {
                            let line = buffer.drain(..=pos).collect::<String>();
                            let trimmed = line.trim();
                            if trimmed.is_empty() { continue; }
                            println!("PROCESSED LINE: {}", trimmed);
                            
                            // Re-implement the parsing logic from src/client.rs here to see where it fails
                            if trimmed.starts_with("data: ") {
                                let json_str = &trimmed[6..];
                                if json_str == "[DONE]" { 
                                    println!("RECEIVED [DONE]");
                                    break; 
                                }
                                
                                let val: serde_json::Value = match serde_json::from_str(json_str) {
                                    Ok(v) => v,
                                    Err(e) => {
                                        println!("JSON PARSE ERROR: {} on string: {}", e, json_str);
                                        continue;
                                    }
                                };
                                println!("PARSED JSON: {:?}", val);
                            } else if !trimmed.starts_with("event: ") && !trimmed.starts_with(":") {
                                // Sometimes vLLM just sends bare JSON without data: prefix, especially for errors
                                if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
                                    println!("PARSED BARE JSON: {:?}", val);
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    println!("STREAM ERROR: {}", e);
                    break;
                }
            }
        }
        Ok::<(), String>(())
    }).await;

    match result {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("Timeout waiting for stream".to_string()),
    }
}
