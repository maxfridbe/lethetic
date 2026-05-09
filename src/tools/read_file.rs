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
            description: "Read a file with line numbers. Use max_lines to limit output for large files — prefer read_file_lines for targeted range reads.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The path to the file"
                    },
                    "max_lines": {
                        "type": "integer",
                        "description": "Maximum number of lines to return. Omit to read the whole file. A truncation notice with total line count is appended when the file is longer."
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

pub async fn execute(
    path: &str,
    max_lines: Option<usize>,
    cwd: &str,
    cancellation_token: tokio_util::sync::CancellationToken,
) -> String {
    let path = path.trim_matches(|c| c == '\'' || c == '"');
    let full_path = Path::new(cwd).join(path);

    tokio::select! {
        _ = cancellation_token.cancelled() => "[Operation Cancelled by User]".to_string(),
        res = async {
            match fs::read_to_string(&full_path) {
                Ok(content) => {
                    let ext = full_path.extension().and_then(|e| e.to_str()).unwrap_or("");
                    let lang = match ext {
                        "rs" => "rust", "py" => "python",
                        "js" => "javascript", "ts" | "tsx" => "typescript",
                        "cs" => "csharp", "cpp" | "cc" | "h" => "cpp",
                        "c" => "c", "json" => "json",
                        "yml" | "yaml" => "yaml", "md" => "markdown",
                        "sh" => "bash", "xml" | "pom" => "xml",
                        "html" => "html", "css" => "css",
                        "go" => "go", "java" => "java",
                        "rb" => "ruby", "php" => "php",
                        "sql" => "sql", "toml" => "toml",
                        _ => "",
                    };

                    let all_lines: Vec<&str> = content.lines().collect();
                    let total = all_lines.len();
                    let limit = max_lines.unwrap_or(usize::MAX);
                    let lines = &all_lines[..limit.min(total)];

                    let mut result = format!("```{}\n", lang);
                    for (i, line) in lines.iter().enumerate() {
                        result.push_str(&format!("{:6}\t{}\n", i + 1, line));
                    }
                    result.push_str("```\n");

                    if limit < total {
                        result.push_str(&format!(
                            "\n[Showing lines 1–{} of {}. Use read_file_lines to read further.]\n",
                            limit, total
                        ));
                    }
                    result
                }
                Err(e) => format!("ERROR: Failed to read file {}: {}", full_path.display(), e),
            }
        } => res
    }
}
