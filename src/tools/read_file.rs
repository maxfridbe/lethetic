use serde_json::json;
use crate::tools::{Tool, FunctionDefinition};
use super::icons;
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


pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    if let Some(desc) = arguments["description"].as_str() {
        return format!("{} {}", icons::PATH, desc);
    }
    let path = arguments["path"].as_str().unwrap_or("");
    format!("{} Reading file: `{}`", icons::PATH, path)
}

pub async fn execute(path: &str, cwd: &str, cancellation_token: tokio_util::sync::CancellationToken) -> String {
    let path = path.trim_matches(|c| c == '\'' || c == '\"');
    let full_path = Path::new(cwd).join(path);
    
    tokio::select! {
        _ = cancellation_token.cancelled() => {
            "[Operation Cancelled by User]".to_string()
        }
        res = async {
            match fs::read_to_string(&full_path) {
                Ok(content) => {
                    let mut result = String::new();
                    
                    // Determine language for syntax highlighting
                    let ext = full_path.extension().and_then(|e| e.to_str()).unwrap_or("");
                    let lang = match ext {
                        "rs" => "rust",
                        "py" => "python",
                        "js" => "javascript",
                        "ts" => "typescript",
                        "cs" => "csharp",
                        "cpp" | "cc" | "h" => "cpp",
                        "c" => "c",
                        "json" => "json",
                        "yml" | "yaml" => "yaml",
                        "md" => "markdown",
                        "sh" => "bash",
                        "xml" | "pom" => "xml",
                        "html" => "html",
                        "css" => "css",
                        "go" => "go",
                        "java" => "java",
                        "rb" => "ruby",
                        "php" => "php",
                        "sql" => "sql",
                        "toml" => "toml",
                        _ => "",
                    };

                    result.push_str(&format!("```{}\n", lang));
                    for (i, line) in content.lines().enumerate() {
                        result.push_str(&format!("{:6}\t{}\n", i + 1, line));
                    }
                    result.push_str("```\n");
                    result
                }
                Err(e) => format!("ERROR: Failed to read file {}: {}", full_path.display(), e),
            }
        } => res
    }
}
