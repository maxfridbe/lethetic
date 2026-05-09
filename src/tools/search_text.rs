use serde_json::json;
use crate::tools::{Tool, FunctionDefinition};
use super::icons;
use tokio::process::Command;

pub fn get_definition() -> Tool {
    Tool {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "search_text".to_string(),
            description: "Search for a regular expression pattern within files in a directory. Automatically excludes build artifacts (target/, node_modules/) and version control (.git/). Uses ripgrep when available.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Directory or file to search. Defaults to '.' if omitted."
                    },
                    "pattern": {
                        "type": "string",
                        "description": "The regex pattern to search for"
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
                "required": ["pattern", "description", "tool_call_id"]
            }),
        },
    }
}

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    if let Some(desc) = arguments["description"].as_str() {
        return format!("{} {}", icons::SEARCH, desc);
    }
    let pattern = arguments["pattern"].as_str().unwrap_or("");
    let path = arguments["path"].as_str().unwrap_or(".");
    format!("{} Searching for `{}` in `{}`", icons::SEARCH, pattern, path)
}

pub async fn execute(
    pattern: &str,
    path: &str,
    cwd: &str,
    cancellation_token: tokio_util::sync::CancellationToken,
) -> String {
    let search_path = if path.is_empty() { "." } else { path };

    tokio::select! {
        _ = cancellation_token.cancelled() => "[Operation Cancelled by User]".to_string(),
        result = run_search(pattern, search_path, cwd) => result,
    }
}

async fn run_search(pattern: &str, search_path: &str, cwd: &str) -> String {
    // Prefer ripgrep: faster, respects .gitignore automatically, better output
    let rg = Command::new("rg")
        .arg("-n")
        .arg("--color=never")
        .arg("--no-heading")
        .arg(pattern)
        .arg(search_path)
        .current_dir(cwd)
        .output()
        .await;

    if let Ok(out) = rg {
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        // rg exits 1 when no matches (not an error), exits 2 on actual error
        if out.status.code() == Some(2) {
            // Fall through to grep
        } else {
            if stdout.trim().is_empty() && stderr.trim().is_empty() {
                return "No matches found.".to_string();
            }
            let status = out.status.code().map_or("signaled".to_string(), |c| c.to_string());
            return format!("EXIT_CODE: {}\nSTDOUT:\n{}\nSTDERR:\n{}", status, stdout, stderr);
        }
    }

    // Fallback: grep with explicit exclusions
    let child = Command::new("grep")
        .arg("-rn")
        .arg("--color=never")
        .arg("-I")
        .arg("--exclude-dir=target")
        .arg("--exclude-dir=.git")
        .arg("--exclude-dir=node_modules")
        .arg("--exclude-dir=.lethetic")
        .arg(pattern)
        .arg(search_path)
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("Failed to spawn grep");

    match child.wait_with_output().await {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let status = out.status.code().map_or("signaled".to_string(), |c| c.to_string());
            if stdout.is_empty() && stderr.is_empty() && status == "1" {
                return "No matches found.".to_string();
            }
            format!("EXIT_CODE: {}\nSTDOUT:\n{}\nSTDERR:\n{}", status, stdout, stderr)
        }
        Err(e) => format!("ERROR: {}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use std::fs;

    #[tokio::test]
    async fn test_search_finds_pattern() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.rs"), "fn hello() {}\nfn world() {}\n").unwrap();
        let token = tokio_util::sync::CancellationToken::new();
        let r = execute("fn hello", ".", dir.path().to_str().unwrap(), token).await;
        assert!(r.contains("hello"), "{}", r);
    }

    #[tokio::test]
    async fn test_search_no_match() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.rs"), "fn hello() {}").unwrap();
        let token = tokio_util::sync::CancellationToken::new();
        let r = execute("fn xyz_nonexistent", ".", dir.path().to_str().unwrap(), token).await;
        assert!(r.contains("No matches"), "{}", r);
    }
}
