use serde_json::json;
use crate::tools::{Tool, FunctionDefinition};
use super::icons;
use std::fs;
use std::path::Path;

pub fn get_definition() -> Tool {
    Tool {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "replace_text".to_string(),
            description: "Replace a literal string within a file. By default requires exactly one match; set replace_all:true to replace every occurrence. For multi-line or whitespace-sensitive edits, prefer the 'edit' tool.".to_string(),
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
                        "description": "The replacement string"
                    },
                    "replace_all": {
                        "type": "boolean",
                        "description": "If true, replace all occurrences. If false (default), fail when more than one occurrence is found."
                    },
                    "description": {
                        "type": "string",
                        "description": "Short description of the action"
                    },
                    "tool_call_id": {
                        "type": "string",
                        "description": "A unique, descriptive string identifier for this call"
                    }
                },
                "required": ["path", "old_string", "new_string", "description", "tool_call_id"]
            }),
        },
    }
}

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    if let Some(desc) = arguments["description"].as_str() {
        return format!("{} {}", icons::SUCCESS, desc);
    }
    let path = arguments["path"].as_str().unwrap_or("");
    format!("{} Replacing text in `{}`", icons::SUCCESS, path)
}

pub async fn execute(
    path: &str,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
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
                    let count = content.matches(old_string).count();
                    if count == 0 {
                        return format!("ERROR: old_string not found in {}", path);
                    }
                    if count > 1 && !replace_all {
                        let line_nums: Vec<String> = find_match_lines(&content, old_string)
                            .iter().map(|n| n.to_string()).collect();
                        return format!(
                            "ERROR: old_string matches {} occurrences in {} (lines {}).\nAdd more surrounding context to make it unique, or set replace_all:true.",
                            count, path, line_nums.join(", ")
                        );
                    }
                    let new_content = if replace_all {
                        content.replace(old_string, new_string)
                    } else {
                        content.replacen(old_string, new_string, 1)
                    };
                    match fs::write(&full_path, new_content) {
                        Ok(_) => {
                            if replace_all && count > 1 {
                                format!("Successfully replaced {} occurrences in {}", count, path)
                            } else {
                                format!("Successfully replaced text in {}", path)
                            }
                        }
                        Err(e) => format!("ERROR: Failed to write to {}: {}", path, e),
                    }
                }
                Err(e) => format!("ERROR: Failed to read file {}: {}", path, e),
            }
        } => res
    }
}

fn find_match_lines(content: &str, needle: &str) -> Vec<usize> {
    let mut results = Vec::new();
    let mut search_from = 0;
    while let Some(pos) = content[search_from..].find(needle) {
        let abs_pos = search_from + pos;
        let line_no = content[..abs_pos].lines().count() + 1;
        results.push(line_no);
        search_from = abs_pos + needle.len().max(1);
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use std::fs;

    #[tokio::test]
    async fn test_replace_single_match() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("f.txt"), "hello world").unwrap();
        let token = tokio_util::sync::CancellationToken::new();
        let r = execute("f.txt", "world", "Rust", false, dir.path().to_str().unwrap(), token).await;
        assert!(r.contains("Successfully"), "{}", r);
        assert_eq!(fs::read_to_string(dir.path().join("f.txt")).unwrap(), "hello Rust");
    }

    #[tokio::test]
    async fn test_replace_multi_match_error() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("f.txt"), "foo foo foo").unwrap();
        let token = tokio_util::sync::CancellationToken::new();
        let r = execute("f.txt", "foo", "bar", false, dir.path().to_str().unwrap(), token).await;
        assert!(r.contains("ERROR") && r.contains("3"), "{}", r);
    }

    #[tokio::test]
    async fn test_replace_all() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("f.txt"), "foo foo foo").unwrap();
        let token = tokio_util::sync::CancellationToken::new();
        let r = execute("f.txt", "foo", "bar", true, dir.path().to_str().unwrap(), token).await;
        assert!(r.contains("3"), "{}", r);
        assert_eq!(fs::read_to_string(dir.path().join("f.txt")).unwrap(), "bar bar bar");
    }
}
