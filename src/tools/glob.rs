use super::icons;
use crate::tools::{FunctionDefinition, Tool, ToolExecution};
use serde_json::json;
use std::path::Path;

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
    execute_classified(pattern, path, cwd, cancellation_token)
        .await
        .output
}

pub(super) async fn execute_classified(
    pattern: &str,
    path: &str,
    cwd: &str,
    cancellation_token: tokio_util::sync::CancellationToken,
) -> ToolExecution {
    let search_path = if path.is_empty() { "." } else { path };
    let full_path = Path::new(cwd).join(search_path);
    let full_path_str = full_path.to_string_lossy().to_string();

    let result = tokio::select! {
        biased;
        _ = cancellation_token.cancelled() => {
            return ToolExecution::error("[Operation Cancelled by User]", cwd);
        }
        result = run_glob(pattern, &full_path_str, cwd) => result,
    };

    match result {
        Ok(output) => ToolExecution::success(output, cwd),
        Err(error) => ToolExecution::error(error, cwd),
    }
}

async fn run_glob(pattern: &str, search_path: &str, cwd: &str) -> Result<String, String> {
    // Try ripgrep first (respects .gitignore, fast).
    let rg_result =
        crate::platform::command_output("rg", ["--files", "-g", pattern, search_path], Some(cwd))
            .await;

    match rg_result {
        Ok(out) => classify_file_list_output(
            "rg",
            true,
            out.status.success(),
            out.status.code(),
            &String::from_utf8_lossy(&out.stdout),
            &String::from_utf8_lossy(&out.stderr),
            cwd,
        ),
        Err(rg_error) => {
            // rg is unavailable; find is the compatibility fallback.
            let name_part = pattern.split('/').next_back().unwrap_or(pattern);
            let find_result = crate::platform::command_output(
                "find",
                [
                    search_path,
                    "-name",
                    name_part,
                    "-not",
                    "-path",
                    "*/target/*",
                    "-not",
                    "-path",
                    "*/.git/*",
                    "-not",
                    "-path",
                    "*/node_modules/*",
                ],
                Some(cwd),
            )
            .await;

            match find_result {
                Ok(out) => classify_file_list_output(
                    "find",
                    false,
                    out.status.success(),
                    out.status.code(),
                    &String::from_utf8_lossy(&out.stdout),
                    &String::from_utf8_lossy(&out.stderr),
                    cwd,
                ),
                Err(find_error) => Err(format!(
                    "ERROR: Failed to launch rg: {rg_error}\nFailed to launch find fallback: {find_error}"
                )),
            }
        }
    }
}

fn classify_file_list_output(
    command: &str,
    exit_one_means_no_matches: bool,
    success: bool,
    exit_code: Option<i32>,
    stdout: &str,
    stderr: &str,
    cwd: &str,
) -> Result<String, String> {
    let stdout = stdout.trim_end();
    let stderr = stderr.trim_end();
    if success && stderr.is_empty() {
        return if stdout.trim().is_empty() {
            Ok("No files found matching pattern.".to_string())
        } else {
            Ok(format_file_list(stdout, cwd, 200))
        };
    }
    if exit_one_means_no_matches
        && exit_code == Some(1)
        && stdout.trim().is_empty()
        && stderr.is_empty()
    {
        return Ok("No files found matching pattern.".to_string());
    }

    let status = exit_code
        .map(|code| code.to_string())
        .unwrap_or_else(|| "terminated by signal".to_string());
    Err(format!(
        "ERROR: {command} file search failed (exit {status}).\nSTDOUT:\n{stdout}\nSTDERR:\n{stderr}"
    ))
}

fn format_file_list(raw: &str, cwd: &str, limit: usize) -> String {
    let cwd_prefix = format!("{}/", cwd);
    let mut lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();

    let total = lines.len();
    lines.truncate(limit);

    let output: Vec<String> = lines
        .iter()
        .map(|l| l.strip_prefix(&cwd_prefix).unwrap_or(l).to_string())
        .collect();

    if total > limit {
        format!(
            "{}\n... ({} more, refine your pattern)",
            output.join("\n"),
            total - limit
        )
    } else {
        format!("{} file(s) found:\n{}", total, output.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_glob_finds_rs_files() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();
        fs::write(dir.path().join("lib.rs"), "pub fn foo() {}").unwrap();
        fs::write(dir.path().join("config.toml"), "[package]").unwrap();

        let token = tokio_util::sync::CancellationToken::new();
        let result = execute("*.rs", ".", dir.path().to_str().unwrap(), token).await;

        assert!(
            result.contains("main.rs") || result.contains("lib.rs"),
            "Expected .rs files in output, got: {}",
            result
        );
        assert!(
            !result.contains("config.toml"),
            "Should not include .toml files"
        );
    }

    #[tokio::test]
    async fn test_glob_no_matches() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();

        let token = tokio_util::sync::CancellationToken::new();
        let result = execute("*.py", ".", dir.path().to_str().unwrap(), token).await;

        assert!(
            result.contains("No files found"),
            "Expected no-match message, got: {}",
            result
        );
    }

    #[test]
    fn failed_file_search_with_partial_stdout_is_not_a_success() {
        let result = classify_file_list_output(
            "rg",
            true,
            false,
            Some(2),
            "/etc/ssl/openssl.cnf",
            "rg: /etc/ssl/private: Permission denied",
            "/etc/ssl",
        )
        .unwrap_err();

        assert!(result.contains("exit 2"), "{result}");
        assert!(result.contains("openssl.cnf"), "{result}");
        assert!(result.contains("Permission denied"), "{result}");
    }

    #[test]
    fn find_failure_with_empty_output_is_not_a_no_match() {
        let result =
            classify_file_list_output("find", false, false, Some(1), "", "missing path", ".")
                .unwrap_err();

        assert!(result.contains("missing path"), "{result}");
    }

    #[tokio::test]
    async fn pre_cancelled_glob_is_a_typed_error() {
        let token = tokio_util::sync::CancellationToken::new();
        token.cancel();

        let result = execute_classified("*", ".", ".", token).await;

        assert!(result.is_error);
        assert_eq!(result.output, "[Operation Cancelled by User]");
    }
}
