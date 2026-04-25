use serde_json::json;
use crate::tools::{Tool, FunctionDefinition};
use super::icons;
use super::llm_tokens;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use std::path::{Path, PathBuf};

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
                    "tool_call_id": {
                        "type": "string",
                        "description": "Required tracking ID"
                    }
                },
                "required": ["command", "tool_call_id"]
            }),
        },
    }
}

pub fn get_prompt_template() -> String {
    format!("{}declaration:run_shell_command{{description:<|\">Execute a bash shell command. Only use this if no specialized tool is available.<|\">,parameters:{{properties:{{command:{{description:<|\">The bash command to execute<|\">,type:<|\">STRING<|\">}},tool_call_id:{{description:<|\">Required tracking ID<|\">,type:<|\">STRING<|\">}}}},required:[<|\">command<|\">,<|\">tool_call_id<|\">],type:<|\">OBJECT<|\">}}}}{}", llm_tokens::TOOL_CALL_OPEN, llm_tokens::TOOL_CALL_CLOSE)
}

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    let command = arguments["command"].as_str().unwrap_or("");
    format!("{} Executing shell command: `{}`", icons::SHELL, command)
}

pub async fn execute(command: &str, cwd: &str, cancellation_token: CancellationToken) -> (String, String) {
    // Check if the command starts with 'cd' to update the persistent state
    let mut final_cwd = PathBuf::from(cwd);
    if command.trim().starts_with("cd ") {
        let parts: Vec<&str> = command.trim().split_whitespace().collect();
        if parts.len() > 1 {
            let target = parts[1];
            let new_path = if target == ".." {
                final_cwd.parent().map(|p| p.to_path_buf()).unwrap_or(final_cwd.clone())
            } else {
                final_cwd.join(target)
            };
            if new_path.exists() && new_path.is_dir() {
                final_cwd = new_path;
            }
        }
    }

    let child = Command::new("bash")
        .arg("-c")
        .arg(command)
        .current_dir(&final_cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("Failed to spawn bash");

    let res_str = tokio::select! {
        _ = cancellation_token.cancelled() => {
            format!("EXIT_CODE: signaled\nCWD: {}\nSTDOUT:\n[Process Killed by User]\nSTDERR:\n[Process Killed by User]", final_cwd.display())
        }
        output = child.wait_with_output() => {
            match output {
                Ok(out) => {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    let status = out.status.code().map_or("signaled".to_string(), |c| c.to_string());
                    format!("EXIT_CODE: {}\nCWD: {}\nSTDOUT:\n{}\nSTDERR:\n{}", status, final_cwd.display(), stdout, stderr)
                }
                Err(e) => format!("ERROR: {}", e),
            }
        }
    };

    (res_str, final_cwd.display().to_string())
}
