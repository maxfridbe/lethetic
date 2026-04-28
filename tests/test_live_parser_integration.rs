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
    let sys_prompt = system_prompt::SystemPromptManager::resolve_prompt(system_prompt::DEFAULT_PROMPT_TEMPLATE, ".");
    let mut context_manager = ContextManager::new(config.context_size, Some(sys_prompt));
    
    let user_prompt = format!("Write a reference implementation of a game of ASCII pong in {}. You must use the write_file tool to save it as 'pong.{}'.", language, file_ext);
    context_manager.add_message("user", &user_prompt);

    let req_body = json!({
        "model": config.model,
        "prompt": context_manager.get_raw_prompt(),
        "raw": true,
        "stream": true,
        "temperature": 0.0,
        "stop": ["<turn|>", "<eos>", "<tool_response|>", "<|tool_response|>"],
        "num_ctx": config.context_size,
    });

    let b_url = config.server_url.replace("/api/chat", "/api/generate");
    let res = client.post(&b_url).json(&req_body).send().await.map_err(|e| e.to_string())?;
    
    let mut stream = res.bytes_stream();
    let mut full_content = String::new();
    
    let timeout_duration = Duration::from_secs(300);

    let result = tokio::time::timeout(timeout_duration, async {
        while let Some(item) = stream.next().await {
            if let Ok(bytes) = item {
                if let Ok(chunk_str) = String::from_utf8(bytes.to_vec()) {
                    for line in chunk_str.lines() {
                        let trimmed = line.trim();
                        if trimmed.is_empty() { continue; }
                        
                        let json_str = if trimmed.starts_with("data: ") {
                            &trimmed[6..]
                        } else {
                            trimmed
                        };

                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
                            if let Some(content) = val["response"].as_str().or_else(|| val["content"].as_str()) {
                                full_content.push_str(content);
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
            if tc.function.name != "write_file" {
                return Err(format!("Expected write_file tool, got: {}", tc.function.name));
            }
            if !tc.function.arguments.get("content").is_some() {
                return Err("Missing 'content' argument".to_string());
            }
            let content = tc.function.arguments.get("content").unwrap().as_str().unwrap_or("");
            if content.is_empty() {
                return Err("Generated content was empty".to_string());
            }
            // Success!
            Ok(())
        },
        Some(Err((err_msg, _))) => {
            Err(format!("Syntax Error parsing tool call: {}\nFull Content:\n{}", err_msg, full_content))
        },
        None => {
            Err(format!("No tool call detected in response for {}.\nFull Content:\n{}", language, full_content))
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
    let sys_prompt = lethetic::system_prompt::SystemPromptManager::resolve_prompt(lethetic::system_prompt::DEFAULT_PROMPT_TEMPLATE, ".");
    let mut context_manager = ContextManager::new(config.context_size, Some(sys_prompt));
    
    let user_prompt = "Provide a reference implementation of an ASCII Pong game state in BOTH JSON and YAML formats. Save them as 'pong.json' and 'pong.yaml' using the write_file tool for each.";
    context_manager.add_message("user", user_prompt);

    let req_body = json!({
        "model": config.model,
        "prompt": context_manager.get_raw_prompt(),
        "raw": true,
        "stream": true,
        "temperature": 0.0,
        "stop": ["<turn|>", "<eos>", "<tool_response|>", "<|tool_response|>"],
        "num_ctx": config.context_size,
    });

    let b_url = config.server_url.replace("/api/chat", "/api/generate");
    let res = client.post(&b_url).json(&req_body).send().await.expect("Request failed");
    
    let mut stream = res.bytes_stream();
    let mut full_content = String::new();
    
    let timeout_duration = Duration::from_secs(300);

    let _ = tokio::time::timeout(timeout_duration, async {
        while let Some(item) = stream.next().await {
            if let Ok(bytes) = item {
                if let Ok(chunk_str) = String::from_utf8(bytes.to_vec()) {
                    for line in chunk_str.lines() {
                        let trimmed = line.trim();
                        if trimmed.is_empty() { continue; }
                        let json_str = if trimmed.starts_with("data: ") { &trimmed[6..] } else { trimmed };
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
                            if let Some(content) = val["response"].as_str().or_else(|| val["content"].as_str()) {
                                full_content.push_str(content);
                            }
                        }
                    }
                }
            }
        }
    }).await;

    println!("RAW_MULTI_OUTPUT:\n{}", full_content);

    // Verify both files were attempted to be written
    assert!(full_content.contains("pong.json"), "Should mention pong.json");
    assert!(full_content.contains("pong.yaml"), "Should mention pong.yaml");
    assert!(full_content.contains("write_file"), "Should call write_file");
    
    // Test if we can find at least one tool call
    let parse_result = find_tool_call(&full_content, true);
    assert!(parse_result.is_some(), "Should find at least one tool call in multi-file response");
}
