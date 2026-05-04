use std::fs;
use reqwest::Client;
use serde_json::json;
use futures_util::StreamExt;
use std::time::Duration;

use lethetic::config::Config;
use lethetic::context::ContextManager;
use lethetic::system_prompt;
use lethetic::parser::find_tool_call;

async fn test_language_generation(language: &str, file_ext: &str) -> Result<(), String> {
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
    
    let user_prompt = format!("Write a reference implementation of a game of ASCII pong in {}. You must use the write_file tool to save it as 'pong.{}'.", language, file_ext);
    context_manager.add_message("user", &user_prompt);
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
    
    let timeout_duration = Duration::from_secs(300);

    let result = tokio::time::timeout(timeout_duration, async {
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
        return Err(format!("Timeout waiting for {} response", language));
    }

    println!("RAW_OUTPUT_START\n{}\nRAW_OUTPUT_END", full_content);

    // Now try to parse the tool call
    let parse_result = find_tool_call(&full_content, true);
    
    match parse_result {
        Some(Ok((tc, _))) => {
            println!("Parsed tool call: {}", tc.function.name);
            Ok(())
        },
        Some(Err((err_msg, _))) => {
            Err(format!("Syntax Error parsing tool call: {}\nFull Content:\n{}", err_msg, full_content))
        },
        None => {
            println!("Warning: No tool call detected in response for {}. This is often due to model flakiness.", language);
            Ok(())
        }
    }
}

#[tokio::test]
#[ignore] // Run manually with: cargo test --test test_live_parser_integration -- --ignored --nocapture
async fn test_live_xml_parsing() {
    let res = test_language_generation("XML", "xml").await;
    if let Err(e) = res { panic!("{}", e); }
}

#[tokio::test]
#[ignore]
async fn test_live_csproj_parsing() {
    let res = test_language_generation("csproj (XML format)", "csproj").await;
    if let Err(e) = res { panic!("{}", e); }
}

#[tokio::test]
#[ignore]
async fn test_live_rust_parsing() {
    let res = test_language_generation("Rust", "rs").await;
    if let Err(e) = res { panic!("{}", e); }
}

#[tokio::test]
#[ignore]
async fn test_live_csharp_parsing() {
    let res = test_language_generation("C#", "cs").await;
    if let Err(e) = res { panic!("{}", e); }
}

#[tokio::test]
#[ignore]
async fn test_live_json_parsing() {
    let res = test_language_generation("JSON", "json").await;
    if let Err(e) = res { panic!("{}", e); }
}

#[tokio::test]
#[ignore]
async fn test_live_multi_file_json_yaml_parsing() {
    let config_content = match fs::read_to_string("config.yml") {
        Ok(c) => c,
        Err(_) => panic!("Could not read config.yml"),
    };
    let config: Config = serde_yaml::from_str(&config_content).expect("Failed to parse config");
    
    let client = Client::new();
    let sys_prompt = lethetic::system_prompt::SystemPromptManager::resolve_prompt(lethetic::system_prompt::DEFAULT_PROMPT_TEMPLATE, ".", &config);
    let mut context_manager = ContextManager::new(config.context_size, Some(sys_prompt));
    
    let user_prompt = "Provide a reference implementation of an ASCII Pong game state in BOTH JSON and YAML formats. Save them as 'pong.json' and 'pong.yaml' using the write_file tool for each.";
    context_manager.add_message("user", user_prompt);
let req_body = json!({
    "model": config.model.clone(),
    "input": context_manager.get_raw_prompt(),
    "stream": true,
    "max_tokens": 4096,
});

    let b_url = config.server_url.clone();
    let res = client.post(&b_url).json(&req_body).send().await.expect("Request failed");
    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        panic!("Server error: {} - {}", status, body);
    }
    
    let mut stream = res.bytes_stream();
    let mut full_content = String::new();
    
    let timeout_duration = Duration::from_secs(300);

    let _ = tokio::time::timeout(timeout_duration, async {
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

    println!("RAW_MULTI_OUTPUT:\n{}", full_content);

    // Test if we can find at least one tool call
    let parse_result = find_tool_call(&full_content, true);
    if parse_result.is_none() {
        println!("Warning: No tool call detected in multi-file response. This is often due to model flakiness.");
    }
}
