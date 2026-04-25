use serde_json::json;
use crate::tools::{Tool, FunctionDefinition};
use super::icons;
use super::llm_tokens;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

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

    let child = Command::new("bash")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("Failed to spawn bash");

    let res_str = tokio::select! {
        _ = cancellation_token.cancelled() => {
            format!("EXIT_CODE: signaled\nSTDOUT:\n[Process Killed by User]\nSTDERR:\n[Process Killed by User]")
        }
        output = child.wait_with_output() => {
            match output {
                Ok(out) => {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    let status = out.status.code().map_or("signaled".to_string(), |c| c.to_string());
                    format!("EXIT_CODE: {}\nSTDOUT:\n{}\nSTDERR:\n{}", status, stdout, stderr)
                }
                Err(e) => format!("ERROR: {}", e),
            }
        }
    };

    (res_str, cwd.to_string())
}
