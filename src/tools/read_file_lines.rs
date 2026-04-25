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
            description: "Read a specific range of lines from a file".to_string(),
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
                    "tool_call_id": {
                        "type": "string",
                        "description": "Required tracking ID"
                    }
                },
                "required": ["path", "start_line", "end_line", "tool_call_id"]
            }),
        },
    }
}

pub fn get_prompt_template() -> String {
    format!("{}declaration:read_file_lines{{description:<|\">Read a specific range of lines from a file.<|\">,parameters:{{properties:{{end_line:{{description:<|\">The line number to end reading at (inclusive)<|\">,type:<|\">INTEGER<|\">}},path:{{description:<|\">The path to the file<|\">,type:<|\">STRING<|\">}},start_line:{{description:<|\">The line number to start reading from<|\">,type:<|\">INTEGER<|\">}},tool_call_id:{{description:<|\">Required tracking ID<|\">,type:<|\">STRING<|\">}}}},required:[<|\">path<|\">,<|\">start_line<|\">,<|\">end_line<|\">,<|\">tool_call_id<|\">],type:<|\">OBJECT<|\">}}}}{}", llm_tokens::TOOL_CALL_OPEN, llm_tokens::TOOL_CALL_CLOSE)
}

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    let path = arguments["path"].as_str().unwrap_or("");
    let start = arguments["start_line"].as_u64().unwrap_or(1);
    let end = arguments["end_line"].as_u64().unwrap_or(1);
    format!("{} Reading lines {}-{} of: `{}`", icons::PATH, start, end, path)
}

pub async fn execute(path: &str, start_line: usize, end_line: usize, cwd: &str) -> String {

    let full_path = Path::new(cwd).join(path);
    match fs::read_to_string(&full_path) {
        Ok(content) => {
            let lines: Vec<&str> = content.lines().collect();
            let start = start_line.saturating_sub(1);
            let end = end_line.min(lines.len());
            if start >= lines.len() || start > end {
                return format!("ERROR: Invalid line range {}-{} for file with {} lines", start + 1, end, lines.len());
            }
            lines[start..end].join("\n")
        }
        Err(e) => format!("ERROR: Failed to read file {}: {}", full_path.display(), e),
    }
}
