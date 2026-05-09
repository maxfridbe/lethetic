use serde_json::json;
use crate::tools::{Tool, FunctionDefinition};
use super::icons;
use tokio::process::Command;

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
    let search_path = if path.is_empty() { "." } else { path };

    tokio::select! {
        _ = cancellation_token.cancelled() => "[Operation Cancelled by User]".to_string(),
        result = run_find_symbol(operation, symbol, search_path, cwd) => result,
    }
}

async fn run_find_symbol(operation: &str, symbol: &str, search_path: &str, cwd: &str) -> String {
    let pattern = match operation {
        "definition" => {
            if symbol.is_empty() {
                return "ERROR: 'symbol' is required for the 'definition' operation".to_string();
            }
            // Match common declaration forms: fn, struct, enum, trait, type, const, impl, mod, let, static
            format!(
                r"(pub(\([^)]*\))?\s+)?(async\s+)?(fn|struct|enum|trait|type|const|impl|mod|static)\s+{}\b",
                regex_escape(symbol)
            )
        }
        "references" => {
            if symbol.is_empty() {
                return "ERROR: 'symbol' is required for the 'references' operation".to_string();
            }
            format!(r"\b{}\b", regex_escape(symbol))
        }
        "symbols" => {
            // List all top-level symbol declarations in a file or directory
            r"(pub(\([^)]*\))?\s+)?(async\s+)?(fn|struct|enum|trait|type|const|impl|mod|static)\s+\w".to_string()
        }
        _ => return format!("ERROR: unknown operation '{}'. Use: definition, references, symbols", operation),
    };

    let output = Command::new("rg")
        .arg("-n")
        .arg("--color=never")
        .arg("--no-heading")
        .arg("--glob=!target")
        .arg("--glob=!.git")
        .arg("--glob=!node_modules")
        .arg(&pattern)
        .arg(search_path)
        .current_dir(cwd)
        .output()
        .await;

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);

            if stdout.trim().is_empty() {
                if operation == "definition" {
                    return format!("No definition found for '{}' in {}", symbol, search_path);
                } else if operation == "references" {
                    return format!("No references found for '{}' in {}", symbol, search_path);
                } else {
                    return format!("No symbols found in {}", search_path);
                }
            }

            let lines: Vec<&str> = stdout.lines().collect();
            let total = lines.len();
            let limit = 100;
            let shown: Vec<&str> = lines.iter().take(limit).cloned().collect();
            let mut result = shown.join("\n");
            if total > limit {
                result.push_str(&format!("\n... ({} more results, narrow your search path)", total - limit));
            }
            if !stderr.trim().is_empty() {
                result.push_str(&format!("\nSTDERR: {}", stderr.trim()));
            }
            result
        }
        Err(_) => {
            // rg not available — fall back to grep
            grep_fallback(operation, symbol, search_path, cwd).await
        }
    }
}

async fn grep_fallback(operation: &str, symbol: &str, search_path: &str, cwd: &str) -> String {
    let pattern = match operation {
        "definition" => format!(r"(fn|struct|enum|trait|type|const|impl|mod|static) {}", symbol),
        "references" => format!(r"\b{}\b", symbol),
        "symbols"    => r"(fn|struct|enum|trait|type|const|impl|mod|static) ".to_string(),
        _ => return format!("ERROR: unknown operation '{}'", operation),
    };

    let output = Command::new("grep")
        .arg("-rn")
        .arg("--color=never")
        .arg("-I")
        .arg("--exclude-dir=target")
        .arg("--exclude-dir=.git")
        .arg("--exclude-dir=node_modules")
        .arg("-E")
        .arg(&pattern)
        .arg(search_path)
        .current_dir(cwd)
        .output()
        .await;

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            if stdout.trim().is_empty() { "No matches found.".to_string() } else { stdout.to_string() }
        }
        Err(e) => format!("ERROR: {}", e),
    }
}

fn regex_escape(s: &str) -> String {
    s.chars().flat_map(|c| {
        if "^$.*+?()[]{}|\\".contains(c) { vec!['\\', c] } else { vec![c] }
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use std::fs;

    #[tokio::test]
    async fn test_find_definition() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("lib.rs"), "pub fn my_function() {}\npub struct MyStruct;\n").unwrap();

        let token = tokio_util::sync::CancellationToken::new();
        let result = execute("definition", "my_function", ".", dir.path().to_str().unwrap(), token).await;

        assert!(result.contains("my_function"), "Expected to find definition, got: {}", result);
    }

    #[tokio::test]
    async fn test_find_references() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("main.rs"), "fn main() { my_func(); my_func(); }\nfn my_func() {}\n").unwrap();

        let token = tokio_util::sync::CancellationToken::new();
        let result = execute("references", "my_func", ".", dir.path().to_str().unwrap(), token).await;

        let count = result.matches("my_func").count();
        assert!(count >= 2, "Expected at least 2 references, got: {}", result);
    }

    #[tokio::test]
    async fn test_symbols_list() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("lib.rs"),
            "pub fn alpha() {}\npub struct Beta;\npub enum Gamma { A }\n").unwrap();

        let token = tokio_util::sync::CancellationToken::new();
        let result = execute("symbols", "", dir.path().join("lib.rs").to_str().unwrap(),
            dir.path().to_str().unwrap(), token).await;

        assert!(result.contains("alpha") || result.contains("Beta") || result.contains("Gamma"),
            "Expected symbols listed, got: {}", result);
    }
}
