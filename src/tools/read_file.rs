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
            name: "read_file".to_string(),
            description: "Read the complete content of a file. The output will include line numbers (cat -n format) to help with patching.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The path to the file"
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
                "required": ["path", "description", "tool_call_id"]
            }),
        },
    }
}

pub fn get_prompt_template() -> String {
    format!("{}declaration:read_file{{description:<|\">Read the complete content of a file. The output will include line numbers.<|\">,parameters:{{properties:{{path:{{description:<|\">The path to the file<|\">,type:<|\">STRING<|\">}},description:{{description:<|\">Short description of the action<|\">,type:<|\">STRING<|\">}},tool_call_id:{{description:<|\">A unique, descriptive string identifier for this call (e.g., 'read_main_rs', 'check_folders'). Do not use simple numbers.<|\">,type:<|\">STRING<|\">}}}},required:[<|\">path<|\">,<|\">description<|\">,<|\">tool_call_id<|\">],type:<|\">OBJECT<|\">}}}}{}", llm_tokens::TOOL_CALL_OPEN, llm_tokens::TOOL_CALL_CLOSE)
}

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    if let Some(desc) = arguments["description"].as_str() {
        return format!("{} {}", icons::PATH, desc);
    }
    let path = arguments["path"].as_str().unwrap_or("");
    format!("{} Reading file: `{}`", icons::PATH, path)
}

pub async fn execute(path: &str, cwd: &str) -> String {

    let full_path = Path::new(cwd).join(path);
    match fs::read_to_string(&full_path) {
        Ok(content) => {
            let mut result = String::new();
            for (i, line) in content.lines().enumerate() {
                result.push_str(&format!("{:6}\t{}\n", i + 1, line));
            }
            if content.ends_with('\n') || content.is_empty() {
                // Keep trailing newline if it exists
            } else if !result.is_empty() {
                // If the last line didn't have a newline, content.lines() still yields it
                // and we already pushed a \n in the loop.
            }
            result
        }
        Err(e) => format!("ERROR: Failed to read file {}: {}", full_path.display(), e),
    }
}
