use std::fs;
use reqwest::Client;
use serde_json::json;
use futures_util::StreamExt;
use std::time::Duration;

use lethetic::config::Config;
use lethetic::context::ContextManager;
use lethetic::system_prompt;
use lethetic::parser::find_tool_call;

async fn run_tool_prompt(prompt: &str, expected_tool: &str) -> Result<(), String> {
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

    context_manager.add_message("user", "Hello");
    context_manager.add_message("assistant", "Hello! I am a helpful AI assistant. I can use the tools available to me to perform actions and answer your questions. Just ask!");

    let modified_prompt = format!("{}\n\nYou MUST use the {} tool.", prompt, expected_tool);
    context_manager.add_message("user", &modified_prompt);

    let req_body = json!({
        "model": config.model.clone(),
        "input": context_manager.get_raw_prompt(),
        "stream": true,
        "max_tokens": 1024,
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
        let mut thought_started = false;

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
                                if current_event == "response.reasoning_text.delta" {
                                    if let Some(delta) = val["delta"].as_str() {
                                        if !thought_started {
                                            thought_started = true;
                                            full_content.push_str("<|think|>\n");
                                        }
                                        full_content.push_str(delta);
                                    }
                                } else if current_event == "response.output_text.delta" {
                                    if thought_started {
                                        thought_started = false;
                                        full_content.push_str("\n<turn|>\n");
                                    }
                                    if let Some(delta) = val["delta"].as_str() {
                                        full_content.push_str(delta);
                                    }
                                } else if let Some(delta) = val["delta"]["content"].as_str() {
                                    full_content.push_str(delta);
                                }
                            }
                        }
                    }
                }
            }
        }
    }).await;

    if result.is_err() {
        return Err("Timeout waiting for response".to_string());
    }

    println!("RAW_OUTPUT_START\n{}\nRAW_OUTPUT_END", full_content);

    let parse_result = find_tool_call(&full_content, true);

    match parse_result {
        Some(Ok((tc, _))) => {
            println!("Parsed tool call: {}", tc.function.name);
            if tc.function.name != expected_tool {
                return Err(format!("Expected tool {}, but got {}", expected_tool, tc.function.name));
            }
            Ok(())
        },
        Some(Err((err_msg, _))) => {
            Err(format!("Syntax Error parsing tool call: {}\nFull Content:\n{}", err_msg, full_content))
        },
        None => {
            Err(format!("No tool call detected for {}. Full content:\n{}", expected_tool, full_content))
        }
    }
}

#[tokio::test]
async fn test_live_calculate() {
    let res = run_tool_prompt("You MUST use the 'calculate' tool right now to evaluate the math expression '15 * 45'. Output ONLY the tool call.", "calculate").await;
    if let Err(e) = res { panic!("{}", e); }
}

#[tokio::test]
async fn test_live_read_folder() {
    let res = run_tool_prompt("You MUST use the 'read_folder' tool right now to list the files in the 'src' directory. Output ONLY the tool call.", "read_folder").await;
    if let Err(e) = res { panic!("{}", e); }
}

#[tokio::test]
async fn test_live_search_text() {
    let res = run_tool_prompt("You MUST use the 'search_text' tool right now to search for the regular expression 'struct' inside the file 'src/main.rs'. Output ONLY the tool call.", "search_text").await;
    if let Err(e) = res { panic!("{}", e); }
}

#[tokio::test]
async fn test_live_read_file_lines() {
    let res = run_tool_prompt("You MUST use the 'read_file_lines' tool right now to read lines 10 to 20 of 'Cargo.toml'. Output ONLY the tool call.", "read_file_lines").await;
    if let Err(e) = res { panic!("{}", e); }
}

#[tokio::test]
async fn test_live_replace_text() {
    let res = run_tool_prompt("You MUST use the 'replace_text' tool right now to replace the exact string 'foo' with 'bar' in 'test.txt'. Output ONLY the tool call.", "replace_text").await;
    if let Err(e) = res { panic!("{}", e); }
}

#[tokio::test]
async fn test_live_read_file() {
    let res = run_tool_prompt("You MUST use the 'read_file' tool right now to read 'README.md'. Output ONLY the tool call.", "read_file").await;
    if let Err(e) = res { panic!("{}", e); }
}

#[tokio::test]
async fn test_live_ask_the_user() {
    let res = run_tool_prompt("You MUST use the 'ask_the_user' tool right now to ask 'What is your favorite color?'. Output ONLY the tool call.", "ask_the_user").await;
    if let Err(e) = res { panic!("{}", e); }
}

#[tokio::test]
async fn test_live_summarize_content() {
    let res = run_tool_prompt("You MUST use the 'summarize_content' tool right now to summarize 'large_output.txt'. Output ONLY the tool call.", "summarize_content").await;
    if let Err(e) = res { panic!("{}", e); }
}

#[tokio::test]
async fn test_live_run_shell_command() {
    let res = run_tool_prompt("You MUST use the 'run_shell_command' tool right now to run 'ls -la'. Output ONLY the tool call.", "run_shell_command").await;
    if let Err(e) = res { panic!("{}", e); }
}

#[tokio::test]
async fn test_live_glob() {
    let res = run_tool_prompt(
        "You MUST use the 'glob' tool right now to find all Rust source files matching '**/*.rs' in the 'src' directory. Output ONLY the tool call.",
        "glob",
    ).await;
    if let Err(e) = res { panic!("{}", e); }
}

#[tokio::test]
async fn test_live_find_symbol() {
    let res = run_tool_prompt(
        "You MUST use the 'find_symbol' tool right now with operation 'definition' to find where 'ContextManager' is defined in the 'src' directory. Output ONLY the tool call.",
        "find_symbol",
    ).await;
    if let Err(e) = res { panic!("{}", e); }
}

#[tokio::test]
async fn test_live_edit() {
    let res = run_tool_prompt(
        "You MUST use the 'edit' tool right now to replace the string 'hello world' with 'hello Rust' in the file 'test.txt'. Output ONLY the tool call.",
        "edit",
    ).await;
    if let Err(e) = res { panic!("{}", e); }
}
