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

pub async fn execute(path: &str, patch: &str, cwd: &str) -> String {

    let patch_file = Path::new(cwd).join(".tmp.patch");
    if let Err(e) = fs::write(&patch_file, patch) {
        return format!("ERROR: Failed to write temp patch file: {}", e);
    }

    let output = Command::new("patch")
        .arg("-u")
        .arg(path)
        .arg("-i")
        .arg(".tmp.patch")
        .current_dir(cwd)
        .output()
        .await;

    let _ = fs::remove_file(patch_file);

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            format!("STDOUT:\n{}\nSTDERR:\n{}", stdout, stderr)
        }
        Err(e) => format!("ERROR: {}", e),
    }
}
