use super::icons;
use crate::tools::{FunctionDefinition, Tool, ToolExecution};
use serde_json::json;

pub fn get_definition() -> Tool {
    Tool {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "find_symbol".to_string(),
            description: "Find where a symbol is defined, all places it is referenced, or list all symbols in a file. Use 'definition' to jump to where something is declared, 'references' to find all usages, and 'symbols' to get an outline of a file.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "operation": {
                        "type": "string",
                        "enum": ["definition", "references", "symbols"],
                        "description": "'definition': find where symbol is declared. 'references': find all usages. 'symbols': list all top-level symbols in a file."
                    },
                    "symbol": {
                        "type": "string",
                        "description": "Symbol name to search for. Required for 'definition' and 'references'."
                    },
                    "path": {
                        "type": "string",
                        "description": "File or directory to search. Defaults to '.'."
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
                "required": ["operation", "description", "tool_call_id"]
            }),
        },
    }
}

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    let op = arguments["operation"].as_str().unwrap_or("search");
    let sym = arguments["symbol"].as_str().unwrap_or("");
    if let Some(desc) = arguments["description"].as_str() {
        return format!("{} {}", icons::SEARCH, desc);
    }
    format!("{} find_symbol {} `{}`", icons::SEARCH, op, sym)
}

pub async fn execute(
    operation: &str,
    symbol: &str,
    path: &str,
    cwd: &str,
    cancellation_token: tokio_util::sync::CancellationToken,
) -> String {
    execute_classified(operation, symbol, path, cwd, cancellation_token)
        .await
        .output
}

pub(super) async fn execute_classified(
    operation: &str,
    symbol: &str,
    path: &str,
    cwd: &str,
    cancellation_token: tokio_util::sync::CancellationToken,
) -> ToolExecution {
    let search_path = if path.is_empty() { "." } else { path };

    let result = tokio::select! {
        biased;
        _ = cancellation_token.cancelled() => {
            return ToolExecution::error("[Operation Cancelled by User]", cwd);
        }
        result = run_find_symbol(operation, symbol, search_path, cwd) => result,
    };

    match result {
        Ok(output) => ToolExecution::success(output, cwd),
        Err(error) => ToolExecution::error(error, cwd),
    }
}

async fn run_find_symbol(
    operation: &str,
    symbol: &str,
    search_path: &str,
    cwd: &str,
) -> Result<String, String> {
    let pattern = match operation {
        "definition" => {
            if symbol.is_empty() {
                return Err(
                    "ERROR: 'symbol' is required for the 'definition' operation".to_string()
                );
            }
            // Match common declaration forms: fn, struct, enum, trait, type, const, impl, mod, let, static
            format!(
                r"(pub(\([^)]*\))?\s+)?(async\s+)?(fn|struct|enum|trait|type|const|impl|mod|static)\s+{}\b",
                regex_escape(symbol)
            )
        }
        "references" => {
            if symbol.is_empty() {
                return Err(
                    "ERROR: 'symbol' is required for the 'references' operation".to_string()
                );
            }
            format!(r"\b{}\b", regex_escape(symbol))
        }
        "symbols" => {
            // List all top-level symbol declarations in a file or directory
            r"(pub(\([^)]*\))?\s+)?(async\s+)?(fn|struct|enum|trait|type|const|impl|mod|static)\s+\w".to_string()
        }
        _ => {
            return Err(format!(
                "ERROR: unknown operation '{}'. Use: definition, references, symbols",
                operation
            ));
        }
    };

    let output = crate::platform::command_output(
        "rg",
        [
            "-n",
            "--color=never",
            "--no-heading",
            "--glob=!target",
            "--glob=!.git",
            "--glob=!node_modules",
            &pattern,
            search_path,
        ],
        Some(cwd),
    )
    .await;

    match output {
        Ok(out) => classify_search_output(
            operation,
            symbol,
            search_path,
            out.status.success(),
            out.status.code(),
            &String::from_utf8_lossy(&out.stdout),
            &String::from_utf8_lossy(&out.stderr),
        ),
        Err(rg_error) => {
            // rg not available — fall back to grep.
            grep_fallback(operation, symbol, search_path, cwd)
                .await
                .map_err(|grep_error| {
                    format!(
                        "ERROR: Failed to launch rg: {rg_error}\nThe grep fallback also failed:\n{grep_error}"
                    )
                })
        }
    }
}

fn classify_search_output(
    operation: &str,
    symbol: &str,
    search_path: &str,
    success: bool,
    exit_code: Option<i32>,
    stdout: &str,
    stderr: &str,
) -> Result<String, String> {
    let stdout = stdout.trim_end();
    let stderr = stderr.trim_end();
    let no_matches = || match operation {
        "definition" => format!("No definition found for '{symbol}' in {search_path}"),
        "references" => format!("No references found for '{symbol}' in {search_path}"),
        _ => format!("No symbols found in {search_path}"),
    };

    if success && stderr.is_empty() {
        if stdout.trim().is_empty() {
            return Ok(no_matches());
        }
        return Ok(format_search_matches(stdout));
    }
    if exit_code == Some(1) && stdout.trim().is_empty() && stderr.is_empty() {
        return Ok(no_matches());
    }

    let status = exit_code
        .map(|code| code.to_string())
        .unwrap_or_else(|| "terminated by signal".to_string());
    Err(format!(
        "ERROR: Symbol search command failed (exit {status}).\nSTDOUT:\n{stdout}\nSTDERR:\n{stderr}"
    ))
}

fn format_search_matches(stdout: &str) -> String {
    let lines: Vec<&str> = stdout.lines().collect();
    let total = lines.len();
    let limit = 100;
    let mut result = lines
        .iter()
        .take(limit)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    if total > limit {
        result.push_str(&format!(
            "\n... ({} more results, narrow your search path)",
            total - limit
        ));
    }
    result
}

async fn grep_fallback(
    operation: &str,
    symbol: &str,
    search_path: &str,
    cwd: &str,
) -> Result<String, String> {
    let pattern = match operation {
        "definition" => format!(
            r"(fn|struct|enum|trait|type|const|impl|mod|static) {}",
            symbol
        ),
        "references" => format!(r"\b{}\b", symbol),
        "symbols" => r"(fn|struct|enum|trait|type|const|impl|mod|static) ".to_string(),
        _ => return Err(format!("ERROR: unknown operation '{}'", operation)),
    };

    let output = crate::platform::command_output(
        "grep",
        [
            "-rn",
            "--color=never",
            "-I",
            "--exclude-dir=target",
            "--exclude-dir=.git",
            "--exclude-dir=node_modules",
            "-E",
            &pattern,
            search_path,
        ],
        Some(cwd),
    )
    .await;

    match output {
        Ok(out) => classify_search_output(
            operation,
            symbol,
            search_path,
            out.status.success(),
            out.status.code(),
            &String::from_utf8_lossy(&out.stdout),
            &String::from_utf8_lossy(&out.stderr),
        ),
        Err(error) => Err(format!("ERROR: Failed to launch grep: {error}")),
    }
}

fn regex_escape(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            if "^$.*+?()[]{}|\\".contains(c) {
                vec!['\\', c]
            } else {
                vec![c]
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_find_definition() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("lib.rs"),
            "pub fn my_function() {}\npub struct MyStruct;\n",
        )
        .unwrap();

        let token = tokio_util::sync::CancellationToken::new();
        let result = execute(
            "definition",
            "my_function",
            ".",
            dir.path().to_str().unwrap(),
            token,
        )
        .await;

        assert!(
            result.contains("my_function"),
            "Expected to find definition, got: {}",
            result
        );
    }

    #[tokio::test]
    async fn test_find_references() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("main.rs"),
            "fn main() { my_func(); my_func(); }\nfn my_func() {}\n",
        )
        .unwrap();

        let token = tokio_util::sync::CancellationToken::new();
        let result = execute(
            "references",
            "my_func",
            ".",
            dir.path().to_str().unwrap(),
            token,
        )
        .await;

        let count = result.matches("my_func").count();
        assert!(
            count >= 2,
            "Expected at least 2 references, got: {}",
            result
        );
    }

    #[tokio::test]
    async fn test_symbols_list() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("lib.rs"),
            "pub fn alpha() {}\npub struct Beta;\npub enum Gamma { A }\n",
        )
        .unwrap();

        let token = tokio_util::sync::CancellationToken::new();
        let result = execute(
            "symbols",
            "",
            dir.path().join("lib.rs").to_str().unwrap(),
            dir.path().to_str().unwrap(),
            token,
        )
        .await;

        assert!(
            result.contains("alpha") || result.contains("Beta") || result.contains("Gamma"),
            "Expected symbols listed, got: {}",
            result
        );
    }

    #[test]
    fn failed_search_with_partial_stdout_is_not_a_success() {
        let result = classify_search_output(
            "references",
            "default",
            ".",
            false,
            Some(2),
            "openssl.cnf:1:default",
            "rg: ./private: Permission denied (os error 13)",
        )
        .unwrap_err();

        assert!(result.contains("exit 2"), "{result}");
        assert!(result.contains("openssl.cnf"), "{result}");
        assert!(result.contains("Permission denied"), "{result}");
    }

    #[test]
    fn exit_one_without_output_is_a_clean_no_match() {
        let result =
            classify_search_output("definition", "missing", ".", false, Some(1), "", "").unwrap();

        assert_eq!(result, "No definition found for 'missing' in .");
    }

    #[tokio::test]
    async fn pre_cancelled_search_is_a_typed_error() {
        let token = tokio_util::sync::CancellationToken::new();
        token.cancel();

        let result = execute_classified("references", "anything", ".", ".", token).await;

        assert!(result.is_error);
        assert_eq!(result.output, "[Operation Cancelled by User]");
    }
}
