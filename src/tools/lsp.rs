use serde_json::json;
use std::path::Path;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::icons;
use crate::client::StreamEvent;
use crate::lsp::{self, registry};
use crate::tools::{FunctionDefinition, Tool};

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

async fn auto_install(
    def: &registry::LspServerDef,
    tx: &mpsc::UnboundedSender<StreamEvent>,
    cancellation: &CancellationToken,
) -> Result<(), String> {
    let install_cmd = def.install_cmd.ok_or_else(|| {
        format!(
            "No safe automatic installer is configured for `{}`. {}",
            def.binary, def.install_note
        )
    })?;
    let _ = tx.send(StreamEvent::ToolProgress(format!(
        "LSP: `{}` not found — auto-installing… ({install_cmd})",
        def.binary
    )));
    let out = tokio::select! {
        biased;
        result = crate::platform::shell_output(install_cmd, None) => result
            .map_err(|error| format!("Failed to run install command: {error}"))?,
        _ = cancellation.cancelled() => {
            return Err("LSP auto-install was cancelled".to_string());
        }
    };
    if out.status.success() {
        let _ = tx.send(StreamEvent::ToolProgress(format!(
            "LSP: `{}` installed successfully",
            def.binary
        )));
        Ok(())
    } else {
        Err(format!(
            "Auto-install of `{}` failed.\nCommand: {}\nStderr:\n{}",
            def.binary,
            install_cmd,
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

fn validate_operation_inputs(
    operation: &str,
    has_file: bool,
    language: Option<&str>,
    line: Option<u32>,
    character: Option<u32>,
    query: Option<&str>,
) -> Result<(), String> {
    match operation {
        "goToDefinition" | "findReferences" | "hover" => {
            if has_file && language.is_some() && line.is_some() && character.is_some() {
                Ok(())
            } else {
                Err(format!(
                    "{operation} requires filePath, line, and character."
                ))
            }
        }
        "documentSymbol" => {
            if has_file && language.is_some() {
                Ok(())
            } else {
                Err("documentSymbol requires filePath.".to_string())
            }
        }
        "workspaceSymbol" => {
            if query.is_some_and(|query| !query.trim().is_empty()) {
                Ok(())
            } else {
                Err("workspaceSymbol requires query.".to_string())
            }
        }
        other => Err(format!(
            "Unknown LSP operation: '{other}'. Valid: goToDefinition, findReferences, hover, documentSymbol, workspaceSymbol"
        )),
    }
}

pub async fn execute(
    operation: &str,
    file_path: Option<&str>,
    line: Option<u32>,
    character: Option<u32>,
    query: Option<&str>,
    cwd: &str,
    cancellation_token: CancellationToken,
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
        Path::new(p)
            .extension()
            .and_then(|e| e.to_str())
            .and_then(|ext| registry::language_for_extension(ext))
    });

    // Convert to 0-based for LSP protocol
    let lsp_line = line.map(|l| l.saturating_sub(1));
    let lsp_char = character.map(|c| c.saturating_sub(1));

    let file_uri = abs_path.as_deref().map(|p| format!("file://{}", p));

    if let Err(error) = validate_operation_inputs(
        operation,
        abs_path.is_some(),
        language,
        lsp_line,
        lsp_char,
        query,
    ) {
        return format!("ERROR: {error}");
    }

    // Auto-install the language server if needed, before acquiring the manager lock
    if let Some(lang) = language
        && let Some(def) = registry::server_for_language(lang)
        && !registry::check_installed(def)
        && let Err(error) = auto_install(def, &tx, &cancellation_token).await
    {
        return format!("ERROR: {error}");
    }

    let operation_result = async {
        let manager = lsp::get_manager();
        let mut mgr = manager.lock().await;

        match operation {
            "goToDefinition" => {
                let (fp, lang, uri, ln, ch) =
                    match (&abs_path, language, &file_uri, lsp_line, lsp_char) {
                        (Some(fp), Some(lang), Some(uri), Some(ln), Some(ch)) => {
                            (fp.as_str(), lang, uri.as_str(), ln, ch)
                        }
                        _ => {
                            return "ERROR: goToDefinition requires filePath, line, and character."
                                .to_string();
                        }
                    };
                let params = json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": ln, "character": ch }
                });
                match mgr
                    .request(lang, cwd, Some(fp), "textDocument/definition", params)
                    .await
                {
                    Ok(result) => lsp::format_locations(&result, cwd),
                    Err(e) => format!("ERROR: LSP error: {}", e),
                }
            }
            "findReferences" => {
                let (fp, lang, uri, ln, ch) =
                    match (&abs_path, language, &file_uri, lsp_line, lsp_char) {
                        (Some(fp), Some(lang), Some(uri), Some(ln), Some(ch)) => {
                            (fp.as_str(), lang, uri.as_str(), ln, ch)
                        }
                        _ => {
                            return "ERROR: findReferences requires filePath, line, and character."
                                .to_string();
                        }
                    };
                let params = json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": ln, "character": ch },
                    "context": { "includeDeclaration": true }
                });
                match mgr
                    .request(lang, cwd, Some(fp), "textDocument/references", params)
                    .await
                {
                    Ok(result) => lsp::format_locations(&result, cwd),
                    Err(e) if e.contains("not found") => {
                        drop(mgr);
                        fallback_find_symbol(
                            "references",
                            query.unwrap_or(""),
                            file_path.unwrap_or("."),
                            cwd,
                        )
                        .await
                            + &format!("\n\n(LSP unavailable: {})", e)
                    }
                    Err(e) => format!("ERROR: LSP error: {}", e),
                }
            }
            "hover" => {
                let (fp, lang, uri, ln, ch) =
                    match (&abs_path, language, &file_uri, lsp_line, lsp_char) {
                        (Some(fp), Some(lang), Some(uri), Some(ln), Some(ch)) => {
                            (fp.as_str(), lang, uri.as_str(), ln, ch)
                        }
                        _ => {
                            return "ERROR: hover requires filePath, line, and character."
                                .to_string();
                        }
                    };
                let params = json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": ln, "character": ch }
                });
                match mgr
                    .request(lang, cwd, Some(fp), "textDocument/hover", params)
                    .await
                {
                    Ok(result) => lsp::format_hover(&result),
                    Err(e) => format!("ERROR: LSP error: {}", e),
                }
            }
            "documentSymbol" => {
                let (fp, lang, uri) = match (&abs_path, language, &file_uri) {
                    (Some(fp), Some(lang), Some(uri)) => (fp.as_str(), lang, uri.as_str()),
                    _ => return "ERROR: documentSymbol requires filePath.".to_string(),
                };
                let params = json!({ "textDocument": { "uri": uri } });
                match mgr
                    .request(lang, cwd, Some(fp), "textDocument/documentSymbol", params)
                    .await
                {
                    Ok(result) => lsp::format_symbols(&result),
                    Err(e) if e.contains("not found") => {
                        drop(mgr);
                        fallback_find_symbol("symbols", "", file_path.unwrap_or("."), cwd).await
                            + &format!("\n\n(LSP unavailable: {})", e)
                    }
                    Err(e) => format!("ERROR: LSP error: {}", e),
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
                        if mgr.is_running("rust") {
                            "rust"
                        } else if mgr.is_running("typescript") {
                            "typescript"
                        } else if mgr.is_running("python") {
                            "python"
                        } else if mgr.is_running("go") {
                            "go"
                        } else {
                            return "ERROR: workspaceSymbol requires a running LSP server. Provide filePath to hint which language to use, or run a file-level operation first to start the server.".to_string();
                        }
                    }
                };
                let params = json!({ "query": q });
                match mgr
                    .request(lang, cwd, None, "workspace/symbol", params)
                    .await
                {
                    Ok(result) => lsp::format_symbols(&result),
                    Err(e) if e.contains("not found") => {
                        drop(mgr);
                        fallback_find_symbol("symbols", q, ".", cwd).await
                            + &format!("\n\n(LSP unavailable: {})", e)
                    }
                    Err(e) => format!("ERROR: LSP error: {}", e),
                }
            }
            other => format!(
                "ERROR: Unknown LSP operation: '{}'. Valid: goToDefinition, findReferences, hover, documentSymbol, workspaceSymbol",
                other
            ),
        }
    };

    let outcome = tokio::select! {
        biased;
        result = tokio::time::timeout(std::time::Duration::from_secs(30), operation_result) => {
            Some(result)
        }
        _ = cancellation_token.cancelled() => None,
    };
    match outcome {
        Some(Ok(result)) => result,
        Some(Err(_)) => {
            if let Some(language) = language {
                lsp::get_manager().lock().await.stop_server(language);
            }
            "ERROR: LSP operation timed out after 30 seconds.".to_string()
        }
        None => {
            if let Some(language) = language {
                lsp::get_manager().lock().await.stop_server(language);
            }
            "ERROR: LSP operation was cancelled.".to_string()
        }
    }
}

async fn fallback_find_symbol(operation: &str, symbol: &str, path: &str, cwd: &str) -> String {
    let cancel = CancellationToken::new();
    let result = super::find_symbol::execute(operation, symbol, path, cwd, cancel).await;
    format_fallback_result(result)
}

fn format_fallback_result(result: String) -> String {
    if super::legacy_output_is_error(&result) {
        format!("{result}\n\n[find_symbol fallback]")
    } else {
        format!("[find_symbol fallback]\n{result}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lsp_inputs_are_validated_before_an_installer_can_run() {
        assert!(
            validate_operation_inputs("goToDefinition", true, Some("cpp"), None, Some(0), None,)
                .unwrap_err()
                .contains("requires filePath, line, and character")
        );
        assert!(
            validate_operation_inputs("notAnOperation", true, Some("cpp"), Some(0), Some(0), None,)
                .unwrap_err()
                .contains("Unknown LSP operation")
        );
        assert!(
            validate_operation_inputs("documentSymbol", true, Some("cpp"), None, None, None,)
                .is_ok()
        );
        assert!(
            validate_operation_inputs("workspaceSymbol", true, Some("cpp"), None, None, None,)
                .unwrap_err()
                .contains("requires query")
        );
    }

    #[test]
    fn unsupported_servers_have_no_automatic_install_command() {
        for language in ["cpp", "lua"] {
            let def = registry::server_for_language(language).unwrap();
            assert!(def.install_cmd.is_none(), "{language}");
        }
    }

    #[test]
    fn fallback_does_not_mask_an_inner_error() {
        let error = format_fallback_result("ERROR: symbol is required".to_string());
        assert!(error.starts_with("ERROR:"), "{error}");
        assert!(super::super::legacy_output_is_error(&error));

        let success = format_fallback_result("match.rs:1".to_string());
        assert!(success.starts_with("[find_symbol fallback]"), "{success}");
        assert!(!super::super::legacy_output_is_error(&success));
    }
}
