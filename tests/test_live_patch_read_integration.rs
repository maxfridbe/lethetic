use std::fs;
use std::time::Duration;
use lethetic::config::Config;
use lethetic::context::ContextManager;
use lethetic::system_prompt;
use lethetic::parser::find_tool_call;
use lethetic::tools::apply_patch;
use tempfile::tempdir;
use reqwest::Client;
use serde_json::json;
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

const GENERATION_TIMEOUT: Duration = Duration::from_secs(300);

async fn do_generation_turn(context_manager: &mut ContextManager, config: &Config, client: &Client) -> Result<String, String> {
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

    Ok(full_content)
}

async fn test_patch_generation(
    prompt: &str,
    original_content: &str,
    expected_new_content: &str,
    file_path: &str,
) -> Result<(), String> {
    let config_content = match fs::read_to_string("config.yml") {
        Ok(c) => c,
        Err(_) => return Err("Could not read config.yml".to_string()),
    };
    let config: Config = serde_yaml::from_str(&config_content).expect("Failed to parse config");

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
        return Err("Timeout waiting for apply_patch response".to_string());
    }

    println!("RAW_OUTPUT_START
{}
RAW_OUTPUT_END", full_content);

    let parse_result = find_tool_call(&full_content, true);
    
    if let Some(Ok((tc, _))) = parse_result {
        println!("Parsed tool call: {}", tc.function.name);
    } else {
        println!("Warning: No valid tool call detected. Response:\n{}", full_content);
    }
    
    Ok(())
}

#[tokio::test]
#[ignore]
async fn test_patch_rename_variable() {
    let original = "private int _foo = 1;";
    let new_content = "private int _bar = 1;";
    let prompt = format!("In `App.cs`, replace the following line:
```csharp
{}
```
With:
```csharp
{}
```
Use the `apply_patch` tool directly without checking if the file exists.", original, new_content);
    test_patch_generation(&prompt, original, new_content, "App.cs").await.unwrap();
}

#[tokio::test]
#[ignore]
async fn test_patch_add_function() {
    let original = "int main() {
    return 0;
}";
    let new_content = "void hello() {}

int main() {
    return 0;
}";
    let prompt = format!("In `main.cpp`, please add a `hello` function before main. Replace the old code:
```cpp
{}
```
With the new code:
```cpp
{}
```
Use the `apply_patch` tool directly without checking if the file exists.", original, new_content);
    test_patch_generation(&prompt, original, "void hello() {}", "main.cpp").await.unwrap();
}

#[tokio::test]
#[ignore]
async fn test_patch_delete_line() {
    let original = "line 1
line 2
line 3";
    let new_content = "line 1
line 3";
    let prompt = format!("In `file.txt`, delete line 2. The old code is:
```
{}
```
The new code is:
```
{}
```
Use the `apply_patch` tool directly without checking if the file exists.", original, new_content);
    test_patch_generation(&prompt, original, new_content, "file.txt").await.unwrap();
}

#[tokio::test]
#[ignore]
async fn test_patch_multiline_two_lines_changed() {
    let original = "function process() {\n    step1();\n    step2();\n    step3();\n    step4();\n    step5();\n}";
    let new_content = "function process() {\n    step1_modified();\n    step2();\n    step3();\n    step4_modified();\n    step5();\n}";
    let prompt = format!("In `script.js`, modify the `process` function to change `step1` to `step1_modified` and `step4` to `step4_modified`. The old code is:\n```javascript\n{}\n```\nThe new code is:\n```javascript\n{}\n```\nUse the `apply_patch` tool directly without checking if the file exists.", original, new_content);
    test_patch_generation(&prompt, original, new_content, "script.js").await.unwrap();
}

#[tokio::test]
#[ignore]
async fn test_patch_multiline_read_then_patch() {
    let config_content = match fs::read_to_string("config.yml") {
        Ok(c) => c,
        Err(_) => panic!("Could not read config.yml"),
    };
    let config: Config = serde_yaml::from_str(&config_content).expect("Failed to parse config");

    let client = Client::new();
    let sys_prompt = system_prompt::SystemPromptManager::resolve_prompt(system_prompt::DEFAULT_PROMPT_TEMPLATE, ".", &config);
    let mut context_manager = ContextManager::new(config.context_size, Some(sys_prompt));
    
    let original_content = "function old_func() {\n    let a = 1;\n    let b = 2;\n    return a + b;\n}";
    let expected_new_content = "function old_func() {\n    let a = 1;\n    let b = 2;\n    let c = 3;\n    return a + b + c;\n}";
    let file_path = "math.js";

    let dir = tempdir().expect("Failed to create tempdir");
    let cwd = dir.path().to_str().unwrap();
    let full_path = dir.path().join(file_path);
    fs::write(&full_path, original_content).expect("Failed to write test file");

    let prompt = format!("Please read `{}` and then use the `apply_patch` tool to add `let c = 3;` and change the return statement to `return a + b + c;`.", file_path);
    context_manager.add_message("user", &prompt);

    // Turn 1: Should call read_file
    let content_t1 = do_generation_turn(&mut context_manager, &config, &client).await.unwrap();
    println!("TURN 1: {}", content_t1);

    let parse_result = find_tool_call(&content_t1, true);
    if let Some(Ok((tc, _))) = parse_result {
        println!("Turn 1 parsed tool call: {}", tc.function.name);
    } else {
        println!("Warning: No read_file tool call detected in Turn 1. Response: {}", content_t1);
        return;
    }

    // Turn 2: Should call apply_patch
    let content_t2 = do_generation_turn(&mut context_manager, &config, &client).await.unwrap();
    println!("TURN 2: {}", content_t2);

    let parse_result2 = find_tool_call(&content_t2, true);
    if let Some(Ok((tc, _))) = parse_result2 {
        println!("Turn 2 parsed tool call: {}", tc.function.name);
    } else {
        println!("Warning: No apply_patch tool call detected in Turn 2. Response: {}", content_t2);
    }
}
