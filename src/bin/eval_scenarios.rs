use futures_util::StreamExt;
use lethetic::config::Config;
use lethetic::context::ContextManager;
use lethetic::parser::find_tool_call;
use lethetic::system_prompt;
use lethetic::transport::{self, StreamEvent};
use reqwest::Client;

struct Scenario {
    name: &'static str,
    prompt: &'static str,
    expected_tool: &'static str,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let scenarios = vec![
        Scenario {
            name: "Basic LS",
            prompt: "list files in current directory",
            expected_tool: "read_folder",
        },
        Scenario {
            name: "Read File",
            prompt: "read the contents of Cargo.toml",
            expected_tool: "read_file",
        },
        Scenario {
            name: "Large File",
            prompt: "read the first 10 lines of src/main.rs",
            expected_tool: "read_file_lines",
        },
        Scenario {
            name: "Math",
            prompt: "what is 1234 * 5678?",
            expected_tool: "calculate",
        },
        Scenario {
            name: "Recursive LS",
            prompt: "show me all files in this project recursively",
            expected_tool: "run_shell_command",
        },
        Scenario {
            name: "Grep",
            prompt: "search for the word 'ratatui' in src/main.rs",
            expected_tool: "search_text",
        },
        Scenario {
            name: "Create File",
            prompt: "create a file called 'hello.txt' with content 'world'",
            expected_tool: "write_file",
        },
        Scenario {
            name: "Read Specific Line",
            prompt: "read exactly line 50 of src/main.rs",
            expected_tool: "read_file_lines",
        },
        Scenario {
            name: "Complex Shell",
            prompt: "find all rs files and count them",
            expected_tool: "run_shell_command",
        },
        Scenario {
            name: "Check Git",
            prompt: "what is the current git status?",
            expected_tool: "run_shell_command",
        },
        Scenario {
            name: "Math Expression",
            prompt: "calculate the square root of 144",
            expected_tool: "calculate",
        },
        Scenario {
            name: "Patch Attempt",
            prompt: "change 'lethetic' to 'le-thetic' in README.md using replace_text",
            expected_tool: "replace_text",
        },
        Scenario {
            name: "Unified Patch",
            prompt: "apply this unified diff to README.md: --- README.md\n+++ README.md\n@@ -1,1 +1,1 @@\n-# Lethetic\n+# Le-thetic",
            expected_tool: "apply_patch",
        },
        Scenario {
            name: "Disk Usage",
            prompt: "how much space is left on the disk?",
            expected_tool: "run_shell_command",
        },
        Scenario {
            name: "File Info",
            prompt: "get the details of the 'src' directory",
            expected_tool: "run_shell_command",
        },
        Scenario {
            name: "Verify File",
            prompt: "check if jokes.txt exists",
            expected_tool: "run_shell_command",
        },
        Scenario {
            name: "Read Config",
            prompt: "show me the contents of config.yml",
            expected_tool: "run_shell_command",
        },
        Scenario {
            name: "Math Logic",
            prompt: "if i have 50 apples and give 12 away, how many are left?",
            expected_tool: "calculate",
        },
        Scenario {
            name: "Path Check",
            prompt: "what is the full path of the current directory?",
            expected_tool: "run_shell_command",
        },
        Scenario {
            name: "Environment",
            prompt: "print the current user name",
            expected_tool: "run_shell_command",
        },
        Scenario {
            name: "Code Search",
            prompt: "where is the handle_key function defined?",
            expected_tool: "search_text",
        },
        Scenario {
            name: "Finalization",
            prompt: "all tasks are done, summarize the project",
            expected_tool: "NONE",
        },
    ];

    let mut config = Config::load("config.yml")?;
    config.merge_matching_server_settings();
    let client = Client::new();

    println!("--- Lethetic Tool-Calling Evaluation ---");
    println!("Model: {}\n", config.model);

    for (i, s) in scenarios.iter().enumerate() {
        println!("[{}/{}] Testing: {}", i + 1, scenarios.len(), s.name);
        let result = run_scenario(&client, &config, s).await?;
        println!("Result: {}\n", result);
    }

    Ok(())
}

async fn run_scenario(
    client: &Client,
    config: &Config,
    scenario: &Scenario,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut context_manager = ContextManager::new(
        config.input_token_budget(),
        Some(system_prompt::SystemPromptManager::resolve_prompt(
            system_prompt::DEFAULT_PROMPT_TEMPLATE,
            ".",
            config,
        )),
    );
    if let Some(mode) = config.context_mode {
        context_manager.mode = mode;
    }
    context_manager.add_message("user", scenario.prompt);

    let messages = context_manager.get_messages_for_api();
    let tools = lethetic::tools::get_api_tools(config, lethetic::tools::ToolSurface::Interactive);
    let mut full_content = String::new();
    let mut raw_tool_detected_at: Option<usize> = None;
    let mut structured_tools: Vec<transport::ToolCall> = Vec::new();
    let mut tool_started = false;
    let mut stopped_after_tool = true;

    let timeout_duration = std::time::Duration::from_secs(30);
    let result = tokio::time::timeout(timeout_duration, async {
        let mut stream = transport::stream(client, config, &messages, &tools, 16_384)
            .await
            .map_err(std::io::Error::other)?;

        while let Some(event) = stream.next().await {
            match event {
                StreamEvent::ReasoningDelta { .. } => {}
                StreamEvent::TextDelta(delta) => {
                    if (tool_started
                        || !structured_tools.is_empty()
                        || raw_tool_detected_at.is_some())
                        && !delta.trim().is_empty()
                    {
                        stopped_after_tool = false;
                    }
                    full_content.push_str(&delta);

                    if raw_tool_detected_at.is_none()
                        && let Some(Ok((_, pos))) = find_tool_call(&full_content, false)
                    {
                        raw_tool_detected_at = Some(pos);
                    }
                }
                StreamEvent::ToolCallStart { .. } => {
                    tool_started = true;
                }
                StreamEvent::ToolCalls { calls, .. } => {
                    tool_started = true;
                    structured_tools.extend(calls);
                }
                StreamEvent::Done { .. } => break,
                StreamEvent::Error(error) => {
                    return Err(std::io::Error::other(error));
                }
                StreamEvent::UsageUpdate(_) | StreamEvent::ToolCallDelta { .. } => {}
            }
        }
        Ok::<(), std::io::Error>(())
    })
    .await;

    match result {
        Err(_) => return Ok("FAILED (Timed out - 30s limit reached)".to_string()),
        Ok(Err(error)) => return Err(error.into()),
        Ok(Ok(())) => {}
    }

    Ok(evaluate_response(
        scenario,
        &structured_tools,
        &full_content,
        raw_tool_detected_at.is_some(),
        stopped_after_tool,
    ))
}

fn evaluate_response(
    scenario: &Scenario,
    structured_tools: &[transport::ToolCall],
    full_content: &str,
    raw_tool_detected: bool,
    stopped_after_tool: bool,
) -> String {
    let tool_detected = !structured_tools.is_empty() || raw_tool_detected;
    if scenario.expected_tool == "NONE" {
        if tool_detected {
            return "FAILED (Unexpected tool call)".to_string();
        }
        return "PASSED".to_string();
    }

    let actual_tool = match structured_tools {
        [] => match find_tool_call(full_content, true) {
            Some(Ok((call, _))) => call.function.name,
            Some(Err((error, _))) => {
                return format!("FAILED (Invalid tool call: {error})");
            }
            None => return "FAILED (No tool call detected)".to_string(),
        },
        [call] => call.name.clone(),
        calls => {
            let names = calls
                .iter()
                .map(|call| call.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return format!(
                "FAILED (Expected exactly one tool call, got {}: {names})",
                calls.len()
            );
        }
    };

    let is_match = actual_tool == scenario.expected_tool;
    let is_research = ((actual_tool == "read_file_lines" || actual_tool == "read_file")
        && (scenario.expected_tool == "apply_patch" || scenario.expected_tool == "replace_text"))
        || (actual_tool == "read_folder"
            && (scenario.expected_tool == "run_shell_command"
                || scenario.expected_tool == "search_text"));

    if is_match {
        if !stopped_after_tool {
            let hallucinated = if structured_tools.is_empty() {
                let start = full_content.find("<|tool_call>").unwrap_or(0);
                &full_content[start..]
            } else {
                full_content
            };
            format!(
                "FAILED (Did not stop. Hallucinated: {})",
                hallucinated.replace('\n', "\\n")
            )
        } else {
            "PASSED".to_string()
        }
    } else if is_research {
        format!("PASSED (Researching with {actual_tool})")
    } else {
        format!("FAILED (Wrong tool: {actual_tool})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn structured_call(name: &str) -> transport::ToolCall {
        transport::ToolCall {
            id: format!("{name}-id"),
            name: name.to_string(),
            arguments: serde_json::json!({}),
        }
    }

    #[test]
    fn structured_calls_require_exactly_one_expected_tool() {
        let scenario = Scenario {
            name: "cardinality regression",
            prompt: "call the expected tool",
            expected_tool: "expected_tool",
        };

        let expected_only = vec![structured_call("expected_tool")];
        assert_eq!(
            evaluate_response(&scenario, &expected_only, "", false, true),
            "PASSED"
        );

        let expected_then_unexpected = vec![
            structured_call("expected_tool"),
            structured_call("unexpected_tool"),
        ];
        assert_eq!(
            evaluate_response(&scenario, &expected_then_unexpected, "", false, true),
            "FAILED (Expected exactly one tool call, got 2: expected_tool, unexpected_tool)"
        );
    }
}
