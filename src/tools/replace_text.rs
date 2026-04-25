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
            name: "replace_text".to_string(),
            description: "Replace a literal string within a file with a new string. MUST match exactly one occurrence.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The path to the file to modify"
                    },
                    "old_string": {
                        "type": "string",
                        "description": "The exact literal string to find and replace"
                    },
                    "new_string": {
                        "type": "string",
                        "description": "The new literal string to replace with"
                    },
                    "tool_call_id": {
                        "type": "string",
                        "description": "Required tracking ID"
                    }
                },
                "required": ["path", "old_string", "new_string", "tool_call_id"]
            }),
        },
    }
}

pub fn get_prompt_template() -> String {
    format!("{}declaration:replace_text{{description:<|\">Replace all occurrences of a string in a file with another string.<|\">,parameters:{{properties:{{new_string:{{description:<|\">The string to replace with<|\">,type:<|\">STRING<|\">}},old_string:{{description:<|\">The exact string to find and replace<|\">,type:<|\">STRING<|\">}},path:{{description:<|\">The path to the file<|\">,type:<|\">STRING<|\">}},tool_call_id:{{description:<|\">Required tracking ID<|\">,type:<|\">STRING<|\">}}}},required:[<|\">path<|\">,<|\">old_string<|\">,<|\">new_string<|\">,<|\">tool_call_id<|\">],type:<|\">OBJECT<|\">}}}}{}", llm_tokens::TOOL_CALL_OPEN, llm_tokens::TOOL_CALL_CLOSE)
}

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    let path = arguments["path"].as_str().unwrap_or("");
    format!("{} Replacing text in `{}`", icons::SUCCESS, path)
}

pub async fn execute(path: &str, old_string: &str, new_string: &str, cwd: &str) -> String {

    let full_path = Path::new(cwd).join(path);
    match fs::read_to_string(&full_path) {
        Ok(content) => {
            let matches: Vec<_> = content.matches(old_string).collect();
            if matches.is_empty() {
                return format!("ERROR: old_string not found in {}", path);
            }
            if matches.len() > 1 {
                return format!("ERROR: old_string matches {} occurrences in {}. It must be unique.", matches.len(), path);
            }
            let new_content = content.replace(old_string, new_string);
            match fs::write(&full_path, new_content) {
                Ok(_) => format!("Successfully replaced text in {}", path),
                Err(e) => format!("ERROR: Failed to write to {}: {}", path, e),
            }
        }
        Err(e) => format!("ERROR: Failed to read file {}: {}", path, e),
    }
}
