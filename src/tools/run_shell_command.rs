use serde_json::json;
use crate::tools::{Tool, FunctionDefinition};
use super::icons;
use super::llm_tokens;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use tokio::io::{AsyncBufReadExt, BufReader};
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;
use crate::client::StreamEvent;

pub fn get_definition() -> Tool {
    Tool {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "run_shell_command".to_string(),
            description: "Run a bash command on the local system and return the output".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The exact bash command to execute"
                    },
                    "description": {
                        "type": "string",
                        "description": "Short description of the action"
                    },
                    "tool_call_id": {
                        "type": "string",
                        "description": "A unique, descriptive string identifier for this call (e.g., 'read_main_rs', 'check_folders'). Do not use simple numbers."
                    }
                },
                "required": ["command", "description", "tool_call_id"]
            }),
        },
    }
}

pub fn get_prompt_template() -> String {
    format!("{}declaration:run_shell_command{{description:<|\">Execute a bash shell command. Only use this if no specialized tool is available.<|\">,parameters:{{properties:{{command:{{description:<|\">The bash command to execute<|\">,type:<|\">STRING<|\">}},description:{{description:<|\">Short description of the action<|\">,type:<|\">STRING<|\">}},tool_call_id:{{description:<|\">A unique, descriptive string identifier for this call (e.g., 'read_main_rs', 'check_folders'). Do not use simple numbers.<|\">,type:<|\">STRING<|\">}}}},required:[<|\">command<|\">,<|\">description<|\">,<|\">tool_call_id<|\">],type:<|\">OBJECT<|\">}}}}{}", llm_tokens::TOOL_CALL_OPEN, llm_tokens::TOOL_CALL_CLOSE)
}

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    if let Some(desc) = arguments["description"].as_str() {
        return format!("{} {}", icons::SHELL, desc);
    }
    let command = arguments["command"].as_str().unwrap_or("");
    format!("{} Executing shell command: `{}`", icons::SHELL, command)
}

pub async fn execute(command: &str, cwd: &str, cancellation_token: CancellationToken, tx: mpsc::UnboundedSender<StreamEvent>) -> (String, String) {
    let mut child = Command::new("bash")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("Failed to spawn bash");

    let stdout = child.stdout.take().expect("Failed to open stdout");
    let stderr = child.stderr.take().expect("Failed to open stderr");

    let mut stdout_reader = BufReader::new(stdout).lines();
    let mut stderr_reader = BufReader::new(stderr).lines();

    let mut full_stdout = String::new();
    let mut full_stderr = String::new();
    let mut streaming_lines: Vec<String> = Vec::new();

    let res_str = tokio::select! {
        _ = cancellation_token.cancelled() => {
            let _ = child.kill().await;
            format!("EXIT_CODE: signaled\nSTDOUT:\n[Process Killed by User]\nSTDERR:\n[Process Killed by User]")
        }
        status = async {
            loop {
                tokio::select! {
                    Ok(Some(line)) = stdout_reader.next_line() => {
                        full_stdout.push_str(&line);
                        full_stdout.push('\n');
                        streaming_lines.push(line);
                        if streaming_lines.len() > 5 { streaming_lines.remove(0); }
                        let _ = tx.send(StreamEvent::ToolProgress(streaming_lines.join("\n")));
                    }
                    Ok(Some(line)) = stderr_reader.next_line() => {
                        full_stderr.push_str(&line);
                        full_stderr.push('\n');
                        streaming_lines.push(format!("[STDERR] {}", line));
                        if streaming_lines.len() > 5 { streaming_lines.remove(0); }
                        let _ = tx.send(StreamEvent::ToolProgress(streaming_lines.join("\n")));
                    }
                    else => {
                        break child.wait().await;
                    }
                }
            }
        } => {
            match status {
                Ok(s) => {
                    let exit_status = s.code().map_or("signaled".to_string(), |c| c.to_string());
                    format!("EXIT_CODE: {}\nSTDOUT:\n{}\nSTDERR:\n{}", exit_status, full_stdout, full_stderr)
                }
                Err(e) => format!("ERROR: {}", e),
            }
        }
    };

    (res_str, cwd.to_string())
}
