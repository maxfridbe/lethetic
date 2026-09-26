use super::icons;
use crate::tools::{FunctionDefinition, Tool};
use serde_json::json;
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

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    if let Some(desc) = arguments["description"].as_str() {
        return format!("{} {}", icons::PATH, desc);
    }
    let path = arguments["path"].as_str().unwrap_or("");
    let start = arguments["start_line"].as_u64().unwrap_or(1);
    let end = arguments["end_line"].as_u64().unwrap_or(1);
    format!(
        "{} Reading lines {}-{} of: `{}`",
        icons::PATH,
        start,
        end,
        path
    )
}

pub async fn execute(
    path: &str,
    start_line: usize,
    end_line: usize,
    cwd: &str,
    cancellation_token: tokio_util::sync::CancellationToken,
) -> String {
    let path = path.trim_matches(|c| c == '\'' || c == '\"');
    let full_path = Path::new(cwd).join(path);

    tokio::select! {
        biased;
        _ = cancellation_token.cancelled() => {
            "[Operation Cancelled by User]".to_string()
        }
        res = async {
            if start_line == 0 || end_line == 0 || start_line > end_line {
                return format!(
                    "ERROR: Invalid line range {}-{}; line numbers are 1-indexed and the start must not exceed the end",
                    start_line, end_line
                );
            }
            match fs::read_to_string(&full_path) {
                Ok(content) => {
                    let lines: Vec<&str> = content.lines().collect();
                    let start = start_line - 1;
                    let end = end_line.min(lines.len());
                    if start >= lines.len() {
                        return format!("ERROR: Invalid line range {}-{} for file with {} lines", start_line, end_line, lines.len());
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn reversed_or_zero_ranges_are_errors() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("lines.txt"), "one\ntwo\n").unwrap();

        for (start, end) in [(1, 0), (2, 1), (0, 1)] {
            let result = execute(
                "lines.txt",
                start,
                end,
                dir.path().to_str().unwrap(),
                tokio_util::sync::CancellationToken::new(),
            )
            .await;
            assert!(result.starts_with("ERROR:"), "{start}-{end}: {result}");
        }
    }

    #[tokio::test]
    async fn pre_cancelled_read_does_not_win_the_ready_branch() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("lines.txt"), "one\n").unwrap();
        let token = tokio_util::sync::CancellationToken::new();
        token.cancel();

        let result = execute("lines.txt", 1, 1, dir.path().to_str().unwrap(), token).await;

        assert_eq!(result, "[Operation Cancelled by User]");
    }
}
