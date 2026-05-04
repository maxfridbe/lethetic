use serde_json::json;
use crate::tools::{Tool, FunctionDefinition};
use super::icons;
use std::fs;
use std::path::Path;

pub fn get_definition() -> Tool {
    Tool {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "write_file".to_string(),
            description: "Create a new file or overwrite an existing one with the provided content. Parent directories are created automatically if they do not exist. Use this tool for writing full files or large blocks of content.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The full path to the file including the filename (e.g., 'src/utils/math.rs')."
                    },
                    "content": {
                        "type": "string",
                        "description": "The complete literal content to write. You MUST wrap this value in asymmetric markers: <|tool_parameter>your content here<tool_parameter|>"
                    },
                    "description": {
                        "type": "string",
                        "description": "Short description of what you are writing (e.g., 'Create Game Design Document')."
                    },
                    "tool_call_id": {
                        "type": "string",
                        "description": "A unique identifier for this specific call."
                    }
                },
                "required": ["path", "content", "description", "tool_call_id"]
            }),
        },
    }
}

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    if let Some(desc) = arguments["description"].as_str() {
        return format!("{} {}", icons::SUCCESS, desc);
    }
    let path = arguments["path"].as_str().unwrap_or("");
    format!("{} Writing file: `{}`", icons::SUCCESS, path)
}

pub async fn execute(path: &str, content: &str, cwd: &str, cancellation_token: tokio_util::sync::CancellationToken) -> String {

    let full_file_path = Path::new(cwd).join(path);
    
    tokio::select! {
        _ = cancellation_token.cancelled() => {
            "[Operation Cancelled by User]".to_string()
        }
        res = async {
            // Check if the path is actually a directory
            if full_file_path.is_dir() {
                return format!("ERROR: '{}' is a directory. Please specify a full filename (e.g., '{}/filename.txt').", path, path);
            }

            // Ensure parent directory exists
            if let Some(parent) = full_file_path.parent() {
                let _ = fs::create_dir_all(parent);
            }

            match fs::write(&full_file_path, content) {
                Ok(_) => format!("Successfully wrote to {}", full_file_path.display()),
                Err(e) => format!("ERROR: Failed to write to {}: {}", full_file_path.display(), e),
            }
        } => res
    }
}
