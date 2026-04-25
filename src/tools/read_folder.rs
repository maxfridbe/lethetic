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
            name: "read_folder".to_string(),
            description: "List the names of files and subdirectories directly within a specified directory path.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The path to the directory to list. Defaults to '.' if omitted."
                    },
                    "tool_call_id": {
                        "type": "string",
                        "description": "Required tracking ID"
                    }
                },
                "required": ["tool_call_id"]
            }),
        },
    }
}

pub fn get_prompt_template() -> String {
    format!("{}declaration:read_folder{{description:<|\">List the names of files and subdirectories directly within a specified directory path.<|\">,parameters:{{properties:{{path:{{description:<|\">The path to the directory to list. Defaults to '.' if omitted.<|\">,type:<|\">STRING<|\">}},tool_call_id:{{description:<|\">Required tracking ID<|\">,type:<|\">STRING<|\">}}}},required:[<|\">tool_call_id<|\">],type:<|\">OBJECT<|\">}}}}{}", llm_tokens::TOOL_CALL_OPEN, llm_tokens::TOOL_CALL_CLOSE)
}

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    let path = arguments["path"].as_str().unwrap_or(".");
    format!("{} Reading folder: `{}`", icons::PATH, path)
}

pub async fn execute(path: &str, cwd: &str) -> String {
    let full_path = Path::new(cwd).join(if path.is_empty() { "." } else { path });
    match fs::read_dir(&full_path) {
        Ok(entries) => {
            let mut items = Vec::new();
            for entry in entries {
                if let Ok(entry) = entry {
                    let file_name = entry.file_name().to_string_lossy().to_string();
                    let file_type = entry.file_type().map(|t| if t.is_dir() { "DIR" } else { "FILE" }).unwrap_or("UNKNOWN");
                    items.push(format!("[{}] {}", file_type, file_name));
                }
            }
            items.sort();
            items.join("\n")
        }
        Err(e) => format!("ERROR: Failed to read directory {}: {}", full_path.display(), e),
    }
}
