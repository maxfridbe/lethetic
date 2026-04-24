#[path = "../context.rs"]
mod context;
#[path = "../parser.rs"]
mod parser;
#[path = "../system_prompt.rs"]
mod system_prompt;
#[path = "../config.rs"]
mod config;
#[path = "../client.rs"]
mod client;

// Mock tools module to satisfy client.rs
mod tools {
    use serde::{Deserialize, Serialize};
    #[derive(Serialize, Deserialize, Debug, Clone)]
    pub struct Tool;
}

use crate::context::{ContextManager};
use crate::parser::find_tool_call;
use crate::config::Config;
use crate::client::{GenerateRequest, GenerateResponse};
use reqwest::Client;
use serde_json::json;
use futures_util::StreamExt;
use std::fs;

struct Scenario {
    name: &'static str,
    prompt: &'static str,
    expected_tool: &'static str,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let scenarios = vec![
        Scenario { name: "Basic LS", prompt: "list files in current directory", expected_tool: "read_folder" },
        Scenario { name: "Read File", prompt: "read the contents of Cargo.toml", expected_tool: "read_file" },
        Scenario { name: "Large File", prompt: "read the first 10 lines of src/main.rs", expected_tool: "read_file_lines" },
        Scenario { name: "Math", prompt: "what is 1234 * 5678?", expected_tool: "calculate" },
        Scenario { name: "Recursive LS", prompt: "show me all files in this project recursively", expected_tool: "run_shell_command" },
        Scenario { name: "Grep", prompt: "search for the word 'ratatui' in src/main.rs", expected_tool: "search_text" },
        Scenario { name: "Create File", prompt: "create a file called 'hello.txt' with content 'world'", expected_tool: "write_file" },
        Scenario { name: "Read Specific Line", prompt: "read exactly line 50 of src/main.rs", expected_tool: "read_file_lines" },
        Scenario { name: "Complex Shell", prompt: "find all rs files and count them", expected_tool: "run_shell_command" },
        Scenario { name: "Check Git", prompt: "what is the current git status?", expected_tool: "run_shell_command" },
        Scenario { name: "Math Expression", prompt: "calculate the square root of 144", expected_tool: "calculate" },
        Scenario { name: "Patch Attempt", prompt: "change 'lethetic' to 'le-thetic' in README.md using replace_text", expected_tool: "replace_text" },
        Scenario { name: "Unified Patch", prompt: "apply this unified diff to README.md: --- README.md\n+++ README.md\n@@ -1,1 +1,1 @@\n-# Lethetic\n+# Le-thetic", expected_tool: "apply_patch" },
        Scenario { name: "Disk Usage", prompt: "how much space is left on the disk?", expected_tool: "run_shell_command" },
        Scenario { name: "File Info", prompt: "get the details of the 'src' directory", expected_tool: "run_shell_command" },
        Scenario { name: "Verify File", prompt: "check if jokes.txt exists", expected_tool: "run_shell_command" },
        Scenario { name: "Read Config", prompt: "show me the contents of config.yml", expected_tool: "run_shell_command" },
        Scenario { name: "Math Logic", prompt: "if i have 50 apples and give 12 away, how many are left?", expected_tool: "calculate" },
        Scenario { name: "Path Check", prompt: "what is the full path of the current directory?", expected_tool: "run_shell_command" },
        Scenario { name: "Environment", prompt: "print the current user name", expected_tool: "run_shell_command" },
        Scenario { name: "Code Search", prompt: "where is the handle_key function defined?", expected_tool: "search_text" },
        Scenario { name: "Finalization", prompt: "all tasks are done, summarize the project", expected_tool: "NONE" },
    ];

    let config_content = fs::read_to_string("config.yml")?;
    let config: Config = serde_yaml::from_str(&config_content)?;
    let client = Client::new();

    println!("--- Gemma 4 Tool-Calling Evaluation ---");
    println!("Model: {}\n", config.model);

    for (i, s) in scenarios.iter().enumerate() {
        println!("[{}/{}] Testing: {}", i + 1, scenarios.len(), s.name);
        let result = run_scenario(&client, &config, s).await?;
        println!("Result: {}\n", result);
    }

    Ok(())
}

async fn run_scenario(client: &Client, config: &Config, scenario: &Scenario) -> Result<String, Box<dyn std::error::Error>> {
    let mut context_manager = ContextManager::new(config.context_size, Some(crate::system_prompt::EXPERT_ENGINEER.to_string()));
    context_manager.add_message("user", scenario.prompt);

    let mut req_body = json!({
        "model": config.model.clone(),
        "prompt": context_manager.get_raw_prompt(),
        "raw": true,
        "stream": true,
        "temperature": 0.0,
        "stop": ["<turn|>", "<eos>", "<tool_response|>", "<|tool_response|>"],
        "num_ctx": config.context_size,
    });

    if config.server_url.contains("/api/chat") {
        req_body = json!({
            "model": config.model.clone(),
            "prompt": context_manager.get_raw_prompt(),
            "raw": true,
            "stream": true,
            "options": { "num_ctx": config.context_size, "temperature": 0.0 }
        });
    }

    let b_url = config.server_url.replace("/api/chat", "/api/generate");
    let res = client.post(&b_url).json(&req_body).send().await?;
    // Note: trigger_llm_request in eval_scenarios is not actually called from this file's main loop, 
    // it uses a manual request body. But if there were calls, they would need the extra arg.
    // Actually, I see that run_scenario builds its own request.
    let mut stream = res.bytes_stream();
    
    let mut full_content = String::new();
    let mut tool_detected_at: Option<usize> = None;
    let mut stopped_after_tool = true;

    let timeout_duration = std::time::Duration::from_secs(30);

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

                        if let Ok(gen_res) = serde_json::from_str::<GenerateResponse>(json_str) {
                            let content = gen_res.response;
                            full_content.push_str(&content);
                            
                            if tool_detected_at.is_none() {
                                if let Some(Ok((_, pos))) = find_tool_call(&full_content, false) {
                                    tool_detected_at = Some(pos);
                                }
                            } else if !content.trim().is_empty() {
                                stopped_after_tool = false;
                            }
                        }
                    }
                }
            }
        }
        Ok::<(), Box<dyn std::error::Error>>(())
    }).await;

    if result.is_err() {
        return Ok("FAILED (Timed out - 30s limit reached)".to_string());
    }

    if scenario.expected_tool == "NONE" {
        if tool_detected_at.is_some() {
            return Ok("FAILED (Unexpected tool call)".to_string());
        }
        return Ok("PASSED".to_string());
    }

    match tool_detected_at {
        Some(_) => {
            let (tc, _) = find_tool_call(&full_content, true).unwrap().unwrap();
            let actual_tool = tc.function.name.as_str();
            
            let is_match = actual_tool == scenario.expected_tool;
            let is_research = ((actual_tool == "read_file_lines" || actual_tool == "read_file") && (scenario.expected_tool == "apply_patch" || scenario.expected_tool == "replace_text"))
                || (actual_tool == "read_folder" && (scenario.expected_tool == "run_shell_command" || scenario.expected_tool == "search_text"));

            if is_match {
                if !stopped_after_tool {
                    let start_tag = "<|tool_call>";
                    let after = &full_content[full_content.find(start_tag).unwrap_or(0)..];
                    Ok(format!("FAILED (Did not stop. Hallucinated: {})", after.replace('\n', "\\n")))
                } else {
                    Ok("PASSED".to_string())
                }
            } else if is_research {
                Ok(format!("PASSED (Researching with {})", actual_tool))
            } else {
                Ok(format!("FAILED (Wrong tool: {})", actual_tool))
            }
        },
        None => Ok("FAILED (No tool call detected)".to_string()),
    }
}
