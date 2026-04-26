use serde_json::json;
use crate::tools::{Tool, FunctionDefinition};
use super::icons;
use super::llm_tokens;
use std::fs;
use std::path::Path;

pub fn get_definition() -> Tool {
    Tool {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "write_file".to_string(),
            description: "Write content to a file (overwrites existing)".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The path to the file"
                    },
                    "content": {
                        "type": "string",
                        "description": "The full content to write"
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
                "required": ["path", "content", "description", "tool_call_id"]
            }),
        },
    }
}

pub fn get_prompt_template() -> String {
    format!("{}declaration:write_file{{description:<|\">Create a new file or overwrite an existing one with the provided content.<|\">,parameters:{{properties:{{content:{{description:<|\">The complete content to write to the file<|\">,type:<|\">STRING<|\">}},path:{{description:<|\">The path to the file to write<|\">,type:<|\">STRING<|\">}},description:{{description:<|\">Short description of the action<|\">,type:<|\">STRING<|\">}},tool_call_id:{{description:<|\">A unique, descriptive string identifier for this call (e.g., 'read_main_rs', 'check_folders'). Do not use simple numbers.<|\">,type:<|\">STRING<|\">}}}},required:[<|\">path<|\">,<|\">content<|\">,<|\">description<|\">,<|\">tool_call_id<|\">],type:<|\">OBJECT<|\">}}}}{}", llm_tokens::TOOL_CALL_OPEN, llm_tokens::TOOL_CALL_CLOSE)
}

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    if let Some(desc) = arguments["description"].as_str() {
        return format!("{} {}", icons::SUCCESS, desc);
    }
    let path = arguments["path"].as_str().unwrap_or("");
    format!("{} Writing file: `{}`", icons::SUCCESS, path)
}

pub async fn execute(path: &str, content: &str, cwd: &str, cancellation_token: tokio_util::sync::CancellationToken) -> String {

    let full_path = Path::new(cwd).join(path);
    
    tokio::select! {
        _ = cancellation_token.cancelled() => {
            "[Operation Cancelled by User]".to_string()
        }
        res = async {
            // Ensure parent directory exists
            if let Some(parent) = full_path.parent() {
                let _ = fs::create_dir_all(parent);
            }

            match fs::write(&full_path, content) {
                Ok(_) => format!("Successfully wrote to {}", full_path.display()),
                Err(e) => format!("ERROR: Failed to write to {}: {}", full_path.display(), e),
            }
        } => res
    }
}
