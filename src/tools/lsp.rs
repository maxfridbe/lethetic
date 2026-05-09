use serde_json::json;
use std::path::Path;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::client::StreamEvent;
use crate::lsp::{self, registry};
use crate::tools::{FunctionDefinition, Tool};
use super::icons;

pub fn get_definition() -> Tool {
    Tool {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "lsp".to_string(),
            description: "Query the Language Server Protocol for precise, type-aware code intelligence. \
                Operations: goToDefinition (exact jump-to-definition), findReferences (all usages), \
                hover (type info and docs), documentSymbol (file outline), workspaceSymbol (search all symbols). \
                Falls back to regex search if the language server is not installed. \
                Prefer this over find_symbol for accurate results.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "operation": {
                        "type": "string",
                        "enum": ["goToDefinition", "findReferences", "hover", "documentSymbol", "workspaceSymbol"],
                        "description": "The LSP operation to perform"
                    },
                    "filePath": {
                        "type": "string",
                        "description": "Relative or absolute path to the file (required for goToDefinition, findReferences, hover, documentSymbol)"
                    },
                    "line": {
                        "type": "integer",
                        "description": "1-based line number (required for goToDefinition, findReferences, hover)"
                    },
                    "character": {
                        "type": "integer",
                        "description": "1-based character offset (required for goToDefinition, findReferences, hover)"
                    },
                    "query": {
                        "type": "string",
                        "description": "Symbol query string (required for workspaceSymbol)"
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
                "required": ["operation", "description", "tool_call_id"]
            }),
        },
    }
}

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    if let Some(desc) = arguments["description"].as_str() {
        return format!("{} {}", icons::SEARCH, desc);
    }
    let op = arguments["operation"].as_str().unwrap_or("lsp");
    let file = arguments["filePath"].as_str().unwrap_or("");
    format!("{} LSP {} — {}", icons::SEARCH, op, file)
}

async fn auto_install(def: &registry::LspServerDef, tx: &mpsc::UnboundedSender<StreamEvent>) -> Result<(), String> {
    let _ = tx.send(StreamEvent::ToolProgress(
        format!("LSP: `{}` not found — auto-installing… ({})", def.binary, def.install_cmd)
    ));
    let out = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(def.install_cmd)
        .output()
        .await
        .map_err(|e| format!("Failed to run install command: {}", e))?;
    if out.status.success() {
        let _ = tx.send(StreamEvent::ToolProgress(
            format!("LSP: `{}` installed successfully", def.binary)
        ));
        Ok(())
    } else {
        Err(format!(
            "Auto-install of `{}` failed.\nCommand: {}\nStderr:\n{}",
            def.binary, def.install_cmd,
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

pub async fn execute(
    operation: &str,
    file_path: Option<&str>,
    line: Option<u32>,
    character: Option<u32>,
    query: Option<&str>,
    cwd: &str,
    _cancellation_token: CancellationToken,
    tx: mpsc::UnboundedSender<StreamEvent>,
) -> String {
    // Resolve file to absolute path
    let abs_path: Option<String> = file_path.map(|fp| {
        let p = Path::new(fp);
        if p.is_absolute() {
            fp.to_string()
        } else {
            Path::new(cwd).join(fp).to_string_lossy().into_owned()
        }
    });

    // Determine language from file extension
    let language = abs_path.as_deref().and_then(|p| {
        Path::new(p).extension()
            .and_then(|e| e.to_str())
            .and_then(|ext| registry::language_for_extension(ext))
    });

    // Convert to 0-based for LSP protocol
    let lsp_line = line.map(|l| l.saturating_sub(1));
    let lsp_char = character.map(|c| c.saturating_sub(1));

    let file_uri = abs_path.as_deref().map(|p| format!("file://{}", p));

    // Auto-install the language server if needed, before acquiring the manager lock
    if let Some(lang) = language {
        if let Some(def) = registry::server_for_language(lang) {
            if !registry::check_installed(def) {
                if let Err(e) = auto_install(def, &tx).await {
                    return e;
                }
            }
        }
    }

    let manager = lsp::get_manager();
    let mut mgr = manager.lock().await;

    match operation {
        "goToDefinition" => {
            let (fp, lang, uri, ln, ch) = match (&abs_path, language, &file_uri, lsp_line, lsp_char) {
                (Some(fp), Some(lang), Some(uri), Some(ln), Some(ch)) => (fp.as_str(), lang, uri.as_str(), ln, ch),
                _ => return "goToDefinition requires filePath, line, and character.".to_string(),
            };
            let params = json!({
                "textDocument": { "uri": uri },
                "position": { "line": ln, "character": ch }
            });
            match mgr.request(lang, cwd, Some(fp), "textDocument/definition", params).await {
                Ok(result) => lsp::format_locations(&result, cwd),
                Err(e) => format!("LSP error: {}", e),
            }
        }
        "findReferences" => {
            let (fp, lang, uri, ln, ch) = match (&abs_path, language, &file_uri, lsp_line, lsp_char) {
                (Some(fp), Some(lang), Some(uri), Some(ln), Some(ch)) => (fp.as_str(), lang, uri.as_str(), ln, ch),
                _ => return "findReferences requires filePath, line, and character.".to_string(),
            };
            let params = json!({
                "textDocument": { "uri": uri },
                "position": { "line": ln, "character": ch },
                "context": { "includeDeclaration": true }
            });
            match mgr.request(lang, cwd, Some(fp), "textDocument/references", params).await {
                Ok(result) => lsp::format_locations(&result, cwd),
                Err(e) if e.contains("not found") => {
                    drop(mgr);
                    fallback_find_symbol("references", query.unwrap_or(""), file_path.unwrap_or("."), cwd).await
                        + &format!("\n\n(LSP unavailable: {})", e)
                }
                Err(e) => format!("LSP error: {}", e),
            }
        }
        "hover" => {
            let (fp, lang, uri, ln, ch) = match (&abs_path, language, &file_uri, lsp_line, lsp_char) {
                (Some(fp), Some(lang), Some(uri), Some(ln), Some(ch)) => (fp.as_str(), lang, uri.as_str(), ln, ch),
                _ => return "hover requires filePath, line, and character.".to_string(),
            };
            let params = json!({
                "textDocument": { "uri": uri },
                "position": { "line": ln, "character": ch }
            });
            match mgr.request(lang, cwd, Some(fp), "textDocument/hover", params).await {
                Ok(result) => lsp::format_hover(&result),
                Err(e) => format!("LSP error: {}", e),
            }
        }
        "documentSymbol" => {
            let (fp, lang, uri) = match (&abs_path, language, &file_uri) {
                (Some(fp), Some(lang), Some(uri)) => (fp.as_str(), lang, uri.as_str()),
                _ => return "documentSymbol requires filePath.".to_string(),
            };
            let params = json!({ "textDocument": { "uri": uri } });
            match mgr.request(lang, cwd, Some(fp), "textDocument/documentSymbol", params).await {
                Ok(result) => lsp::format_symbols(&result),
                Err(e) if e.contains("not found") => {
                    drop(mgr);
                    fallback_find_symbol("symbols", "", file_path.unwrap_or("."), cwd).await
                        + &format!("\n\n(LSP unavailable: {})", e)
                }
                Err(e) => format!("LSP error: {}", e),
            }
        }
        "workspaceSymbol" => {
            let q = query.unwrap_or("");
            // We need a language to pick a server. If file_path given, use its language;
            // otherwise try to use any running server.
            let lang = match language {
                Some(l) => l,
                None => {
                    // find any currently running server
                    if mgr.is_running("rust") { "rust" }
                    else if mgr.is_running("typescript") { "typescript" }
                    else if mgr.is_running("python") { "python" }
                    else if mgr.is_running("go") { "go" }
                    else {
                        return format!(
                            "workspaceSymbol requires a running LSP server. Provide filePath to hint which language to use, or run a file-level operation first to start the server."
                        );
                    }
                }
            };
            let params = json!({ "query": q });
            match mgr.request(lang, cwd, None, "workspace/symbol", params).await {
                Ok(result) => lsp::format_symbols(&result),
                Err(e) if e.contains("not found") => {
                    drop(mgr);
                    fallback_find_symbol("symbols", q, ".", cwd).await
                        + &format!("\n\n(LSP unavailable: {})", e)
                }
                Err(e) => format!("LSP error: {}", e),
            }
        }
        other => format!("Unknown LSP operation: '{}'. Valid: goToDefinition, findReferences, hover, documentSymbol, workspaceSymbol", other),
    }
}

async fn fallback_find_symbol(operation: &str, symbol: &str, path: &str, cwd: &str) -> String {
    let cancel = CancellationToken::new();
    let result = super::find_symbol::execute(operation, symbol, path, cwd, cancel).await;
    format!("[find_symbol fallback]\n{}", result)
}
