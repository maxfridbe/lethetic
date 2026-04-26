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
            name: "read_file_lines".to_string(),
            description: "Read a specific range of lines from a file. The output will include line numbers (cat -n format) to help with patching.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The path to the file"
                    },
                    "start_line": {
                        "type": "integer",
                        "description": "The first line to read (1-indexed)"
                    },
                    "end_line": {
                        "type": "integer",
                        "description": "The last line to read (inclusive)"
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
                "required": ["path", "start_line", "end_line", "description", "tool_call_id"]
            }),
        },
    }
}

pub fn get_prompt_template() -> String {
    format!("{}declaration:read_file_lines{{description:<|\">Read a specific range of lines from a file. The output will include line numbers.<|\">,parameters:{{properties:{{end_line:{{description:<|\">The line number to end reading at (inclusive)<|\">,type:<|\">INTEGER<|\">}},path:{{description:<|\">The path to the file<|\">,type:<|\">STRING<|\">}},start_line:{{description:<|\">The line number to start reading from<|\">,type:<|\">INTEGER<|\">}},description:{{description:<|\">Short description of the action<|\">,type:<|\">STRING<|\">}},tool_call_id:{{description:<|\">A unique, descriptive string identifier for this call (e.g., 'read_main_rs', 'check_folders'). Do not use simple numbers.<|\">,type:<|\">STRING<|\">}}}},required:[<|\">path<|\">,<|\">start_line<|\">,<|\">end_line<|\">,<|\">description<|\">,<|\">tool_call_id<|\">],type:<|\">OBJECT<|\">}}}}{}", llm_tokens::TOOL_CALL_OPEN, llm_tokens::TOOL_CALL_CLOSE)
}

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    if let Some(desc) = arguments["description"].as_str() {
        return format!("{} {}", icons::PATH, desc);
    }
    let path = arguments["path"].as_str().unwrap_or("");
    let start = arguments["start_line"].as_u64().unwrap_or(1);
    let end = arguments["end_line"].as_u64().unwrap_or(1);
    format!("{} Reading lines {}-{} of: `{}`", icons::PATH, start, end, path)
}

pub async fn execute(path: &str, start_line: usize, end_line: usize, cwd: &str, cancellation_token: tokio_util::sync::CancellationToken) -> String {
    let full_path = Path::new(cwd).join(path);
    
    tokio::select! {
        _ = cancellation_token.cancelled() => {
            "[Operation Cancelled by User]".to_string()
        }
        res = async {
            match fs::read_to_string(&full_path) {
                Ok(content) => {
                    let lines: Vec<&str> = content.lines().collect();
                    let start = start_line.saturating_sub(1);
                    let end = end_line.min(lines.len());
                    if start >= lines.len() || start > end {
                        return format!("ERROR: Invalid line range {}-{} for file with {} lines", start + 1, end, lines.len());
                    }
                    
                    let mut result = String::new();
                    for (i, line) in lines[start..end].iter().enumerate() {
                        result.push_str(&format!("{:6}\t{}\n", start + i + 1, line));
                    }
                    result
                }
                Err(e) => format!("ERROR: Failed to read file {}: {}", full_path.display(), e),
            }
        } => res
    }
}
