use serde_json::json;
use crate::tools::{Tool, FunctionDefinition};
use super::icons;
use super::llm_tokens;
use std::fs;
use std::path::Path;
use tokio::process::Command;

pub fn get_definition() -> Tool {
    Tool {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "apply_patch".to_string(),
            description: "Apply a unified diff patch to a file".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The path to the file to patch"
                    },
                    "patch": {
                        "type": "string",
                        "description": "The unified diff content to apply"
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
                "required": ["path", "patch", "description", "tool_call_id"]
            }),
        },
    }
}

pub fn get_prompt_template() -> String {
    format!("{}declaration:apply_patch{{description:<|\">Apply a unified diff patch to a file.<|\">,parameters:{{properties:{{patch:{{description:<|\">The unified diff patch to apply<|\">,type:<|\">STRING<|\">}},path:{{description:<|\">The path to the file to patch<|\">,type:<|\">STRING<|\">}},description:{{description:<|\">Short description of the action<|\">,type:<|\">STRING<|\">}},tool_call_id:{{description:<|\">A unique, descriptive string identifier for this call (e.g., 'read_main_rs', 'check_folders'). Do not use simple numbers.<|\">,type:<|\">STRING<|\">}}}},required:[<|\">path<|\">,<|\">patch<|\">,<|\">description<|\">,<|\">tool_call_id<|\">],type:<|\">OBJECT<|\">}}}}{}", llm_tokens::TOOL_CALL_OPEN, llm_tokens::TOOL_CALL_CLOSE)
}

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    if let Some(desc) = arguments["description"].as_str() {
        return format!("{} {}", icons::SUCCESS, desc);
    }
    let path = arguments["path"].as_str().unwrap_or("");
    format!("{} Applying patch to `{}`", icons::SUCCESS, path)
}

pub async fn execute(path: &str, patch: &str, cwd: &str, cancellation_token: tokio_util::sync::CancellationToken) -> String {
    let path = path.trim_matches(|c| c == '\'' || c == '\"');
    let patch_file = Path::new(cwd).join(".tmp.patch");
    if let Err(e) = fs::write(&patch_file, patch) {
        return format!("ERROR: Failed to write temp patch file: {}", e);
    }

    let mut final_path = path.to_string();
    
    // Heuristic: If path is empty, try to extract it from the patch header
    if final_path.is_empty() {
        for line in patch.lines() {
            if line.starts_with("--- ") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    let extracted = parts[1].trim();
                    if !extracted.is_empty() && extracted != "/dev/null" {
                        final_path = extracted.to_string();
                        break;
                    }
                }
            }
        }
    }

    let mut cmd = Command::new("patch");
    cmd.arg("-u");
    
    if !final_path.is_empty() {
        cmd.arg(final_path);
    } else {
        // If still empty, fall back to -p0 which uses the path in the header
        cmd.arg("-p0");
    }
    
    cmd.arg("-i")
        .arg(".tmp.patch")
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    let child = cmd.spawn().expect("Failed to spawn patch");

    let result = tokio::select! {
        _ = cancellation_token.cancelled() => {
            "[Operation Cancelled by User]".to_string()
        }
        output = child.wait_with_output() => {
            match output {
                Ok(out) => {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    format!("STDOUT:\n{}\nSTDERR:\n{}", stdout, stderr)
                }
                Err(e) => format!("ERROR: {}", e),
            }
        }
    };

    let _ = fs::remove_file(patch_file);
    result
}
