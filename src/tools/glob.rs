use serde_json::json;
use crate::tools::{Tool, FunctionDefinition};
use super::icons;
use std::path::Path;
use tokio::process::Command;

pub fn get_definition() -> Tool {
    Tool {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "glob".to_string(),
            description: "Find files matching a glob pattern. Use this to locate files by name or extension before reading them. Respects .gitignore and excludes build artifacts.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern, e.g. '**/*.rs', '*.toml', 'src/**/*.ts'"
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory to search. Defaults to '.' (current directory)."
                    },
                    "description": {
                        "type": "string",
                        "description": "Short description of the action"
                    },
                    "tool_call_id": {
                        "type": "string",
                        "description": "Unique identifier for this call"
                    }
                },
                "required": ["pattern", "description", "tool_call_id"]
            }),
        },
    }
}

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    if let Some(desc) = arguments["description"].as_str() {
        return format!("{} {}", icons::SEARCH, desc);
    }
    let pattern = arguments["pattern"].as_str().unwrap_or("*");
    format!("{} Glob: `{}`", icons::SEARCH, pattern)
}

pub async fn execute(
    pattern: &str,
    path: &str,
    cwd: &str,
    cancellation_token: tokio_util::sync::CancellationToken,
) -> String {
    let search_path = if path.is_empty() { "." } else { path };
    let full_path = Path::new(cwd).join(search_path);
    let full_path_str = full_path.to_string_lossy().to_string();

    tokio::select! {
        _ = cancellation_token.cancelled() => "[Operation Cancelled by User]".to_string(),
        result = run_glob(pattern, &full_path_str, cwd) => result,
    }
}

async fn run_glob(pattern: &str, search_path: &str, cwd: &str) -> String {
    // Try ripgrep first (respects .gitignore, fast)
    let rg_result = Command::new("rg")
        .arg("--files")
        .arg("-g")
        .arg(pattern)
        .arg(search_path)
        .current_dir(cwd)
        .output()
        .await;

    if let Ok(out) = rg_result {
        if out.status.success() || !out.stdout.is_empty() {
            return format_file_list(&String::from_utf8_lossy(&out.stdout), cwd, 200);
        }
        // rg found nothing (exit 1 with empty stdout = no matches)
        if out.stdout.is_empty() && out.status.code() == Some(1) {
            return "No files found matching pattern.".to_string();
        }
    }

    // Fallback: find (available everywhere)
    // Convert glob pattern to find -name format (best-effort for simple patterns)
    let name_part = pattern.split('/').last().unwrap_or(pattern);
    let find_result = Command::new("find")
        .arg(search_path)
        .arg("-name")
        .arg(name_part)
        .arg("-not")
        .arg("-path")
        .arg("*/target/*")
        .arg("-not")
        .arg("-path")
        .arg("*/.git/*")
        .arg("-not")
        .arg("-path")
        .arg("*/node_modules/*")
        .current_dir(cwd)
        .output()
        .await;

    match find_result {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            if stdout.trim().is_empty() {
                "No files found matching pattern.".to_string()
            } else {
                format_file_list(&stdout, cwd, 200)
            }
        }
        Err(e) => format!("ERROR: {}", e),
    }
}

fn format_file_list(raw: &str, cwd: &str, limit: usize) -> String {
    let cwd_prefix = format!("{}/", cwd);
    let mut lines: Vec<&str> = raw.lines()
        .filter(|l| !l.trim().is_empty())
        .collect();

    let total = lines.len();
    lines.truncate(limit);

    let output: Vec<String> = lines.iter()
        .map(|l| l.strip_prefix(&cwd_prefix).unwrap_or(l).to_string())
        .collect();

    if total > limit {
        format!("{}\n... ({} more, refine your pattern)", output.join("\n"), total - limit)
    } else {
        format!("{} file(s) found:\n{}", total, output.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use std::fs;

    #[tokio::test]
    async fn test_glob_finds_rs_files() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();
        fs::write(dir.path().join("lib.rs"), "pub fn foo() {}").unwrap();
        fs::write(dir.path().join("config.toml"), "[package]").unwrap();

        let token = tokio_util::sync::CancellationToken::new();
        let result = execute("*.rs", ".", dir.path().to_str().unwrap(), token).await;

        assert!(result.contains("main.rs") || result.contains("lib.rs"),
            "Expected .rs files in output, got: {}", result);
        assert!(!result.contains("config.toml"), "Should not include .toml files");
    }

    #[tokio::test]
    async fn test_glob_no_matches() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();

        let token = tokio_util::sync::CancellationToken::new();
        let result = execute("*.py", ".", dir.path().to_str().unwrap(), token).await;

        assert!(result.contains("No files found"), "Expected no-match message, got: {}", result);
    }
}
