pub mod registry;

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use tokio::sync::Mutex;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use serde_json::{json, Value};

use registry::{check_installed, LspServerDef, SERVERS};

pub static LSP_MANAGER: OnceLock<Arc<Mutex<LspManager>>> = OnceLock::new();

pub fn get_manager() -> Arc<Mutex<LspManager>> {
    LSP_MANAGER.get_or_init(|| Arc::new(Mutex::new(LspManager::new()))).clone()
}

struct LspServerProcess {
    _child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    did_open_uris: std::collections::HashSet<String>,
}

pub struct LspManager {
    servers: HashMap<String, LspServerProcess>,
}

impl LspManager {
    pub fn new() -> Self {
        Self { servers: HashMap::new() }
    }

    async fn start_server(&mut self, def: &LspServerDef, workspace_root: &str) -> Result<(), String> {
        let mut cmd = Command::new(def.binary);
        cmd.args(def.start_args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);

        let mut child = cmd.spawn()
            .map_err(|e| format!("Failed to spawn {}: {}", def.binary, e))?;

        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;

        let mut proc = LspServerProcess {
            _child: child,
            stdin: BufWriter::new(stdin),
            stdout: BufReader::new(stdout),
            next_id: 1,
            did_open_uris: std::collections::HashSet::new(),
        };

        // Send initialize
        let root_uri = path_to_uri(workspace_root);
        let id = proc.next_id;
        proc.next_id += 1;
        write_msg(&mut proc.stdin, &json!({
            "jsonrpc": "2.0", "id": id,
            "method": "initialize",
            "params": {
                "processId": std::process::id(),
                "rootUri": root_uri,
                "capabilities": {
                    "textDocument": {
                        "hover": { "contentFormat": ["plaintext", "markdown"] },
                        "definition": {},
                        "references": {},
                        "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
                        "implementation": {}
                    },
                    "workspace": { "symbol": {} }
                }
            }
        })).await?;

        // Read until we get the initialize response (id match), skip notifications
        loop {
            let msg = read_msg(&mut proc.stdout).await?;
            if msg.get("id").and_then(|v| v.as_u64()) == Some(id) {
                break;
            }
        }

        // Send initialized notification
        write_msg(&mut proc.stdin, &json!({
            "jsonrpc": "2.0", "method": "initialized", "params": {}
        })).await?;

        self.servers.insert(def.language.to_string(), proc);
        Ok(())
    }

    async fn ensure_did_open(&mut self, language: &str, file_uri: &str, file_path: &str) -> Result<(), String> {
        let proc = self.servers.get_mut(language).ok_or("server not started")?;
        if proc.did_open_uris.contains(file_uri) {
            return Ok(());
        }
        let content = std::fs::read_to_string(file_path).unwrap_or_default();
        let lang_id = language.to_string();
        write_msg(&mut proc.stdin, &json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": file_uri,
                    "languageId": lang_id,
                    "version": 1,
                    "text": content
                }
            }
        })).await?;
        proc.did_open_uris.insert(file_uri.to_string());
        Ok(())
    }

    /// Send a request and wait for its response by id. Skips notifications.
    async fn request_inner(&mut self, language: &str, method: &str, params: Value) -> Result<Value, String> {
        let proc = self.servers.get_mut(language).ok_or("server not started")?;
        let id = proc.next_id;
        proc.next_id += 1;
        write_msg(&mut proc.stdin, &json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params
        })).await?;
        loop {
            let msg = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                read_msg(&mut proc.stdout),
            ).await
                .map_err(|_| "LSP request timed out after 10s".to_string())??;

            if msg.get("id").and_then(|v| v.as_u64()) == Some(id) {
                if let Some(err) = msg.get("error") {
                    return Err(format!("LSP error: {}", err));
                }
                return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
            }
            // Notification — skip
        }
    }

    /// Ensure server running, open file, then send request.
    pub async fn request(
        &mut self,
        language: &str,
        workspace_root: &str,
        file_path: Option<&str>,   // absolute path of file to open, if applicable
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        if !self.servers.contains_key(language) {
            let def = SERVERS.iter().find(|s| s.language == language)
                .ok_or_else(|| format!("No LSP server defined for '{}'", language))?;
            if !check_installed(def) {
                return Err(format!(
                    "`{}` not found.\nInstall with: {}\nThen open the palette (Ctrl-P) → LSP Servers to verify.",
                    def.binary, def.install_cmd
                ));
            }
            self.start_server(def, workspace_root).await?;
        }

        if let Some(fp) = file_path {
            let uri = path_to_uri(fp);
            self.ensure_did_open(language, &uri, fp).await?;
        }

        self.request_inner(language, method, params).await
    }

    pub fn is_running(&self, language: &str) -> bool {
        self.servers.contains_key(language)
    }
}

fn path_to_uri(path: &str) -> String {
    // Ensure absolute path
    let abs = if path.starts_with('/') {
        path.to_string()
    } else {
        std::env::current_dir()
            .map(|d| d.join(path).to_string_lossy().into_owned())
            .unwrap_or_else(|_| path.to_string())
    };
    format!("file://{}", abs)
}

async fn write_msg(writer: &mut BufWriter<ChildStdin>, msg: &Value) -> Result<(), String> {
    let body = serde_json::to_string(msg).map_err(|e| e.to_string())?;
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    writer.write_all(header.as_bytes()).await.map_err(|e| e.to_string())?;
    writer.write_all(body.as_bytes()).await.map_err(|e| e.to_string())?;
    writer.flush().await.map_err(|e| e.to_string())?;
    Ok(())
}

async fn read_msg(reader: &mut BufReader<ChildStdout>) -> Result<Value, String> {
    let mut content_length: usize = 0;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).await.map_err(|e| e.to_string())?;
        if n == 0 { return Err("LSP server closed connection".to_string()); }
        let trimmed = line.trim_end_matches(|c| c == '\r' || c == '\n');
        if trimmed.is_empty() { break; }
        if let Some(val) = trimmed.strip_prefix("Content-Length: ") {
            content_length = val.trim().parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
        }
    }
    if content_length == 0 { return Err("LSP: missing Content-Length".to_string()); }
    let mut buf = vec![0u8; content_length];
    reader.read_exact(&mut buf).await.map_err(|e| e.to_string())?;
    serde_json::from_slice(&buf).map_err(|e| e.to_string())
}

/// Format an LSP Location/Range result as human-readable text.
pub fn format_locations(results: &Value, workspace_root: &str) -> String {
    let items: Vec<&Value> = if let Some(arr) = results.as_array() {
        arr.iter().collect()
    } else {
        vec![results]
    };

    if items.is_empty() || items.iter().all(|v| v.is_null()) {
        return "No results found.".to_string();
    }

    let mut out = Vec::new();
    for loc in &items {
        let uri = loc.get("uri").and_then(|v| v.as_str()).unwrap_or("");
        let path = uri.strip_prefix("file://").unwrap_or(uri);
        let rel = if let Ok(stripped) = std::path::Path::new(path)
            .strip_prefix(workspace_root)
        {
            stripped.to_string_lossy().into_owned()
        } else {
            path.to_string()
        };
        let range = loc.get("range")
            .or_else(|| loc.get("location").and_then(|l| l.get("range")));
        let line = range
            .and_then(|r| r.get("start"))
            .and_then(|s| s.get("line"))
            .and_then(|l| l.as_u64())
            .map(|l| l + 1)  // 0-based → 1-based
            .unwrap_or(0);
        let col = range
            .and_then(|r| r.get("start"))
            .and_then(|s| s.get("character"))
            .and_then(|c| c.as_u64())
            .map(|c| c + 1)
            .unwrap_or(0);
        out.push(format!("{}:{}:{}", rel, line, col));
    }
    out.join("\n")
}

/// Format a documentSymbol/workspaceSymbol result.
pub fn format_symbols(results: &Value) -> String {
    let items: Vec<&Value> = if let Some(arr) = results.as_array() {
        arr.iter().collect()
    } else {
        return "No symbols found.".to_string();
    };

    if items.is_empty() { return "No symbols found.".to_string(); }

    let kind_name = |k: u64| match k {
        1 => "file", 2 => "module", 3 => "namespace", 4 => "package",
        5 => "class", 6 => "method", 7 => "property", 8 => "field",
        9 => "constructor", 10 => "enum", 11 => "interface", 12 => "function",
        13 => "variable", 14 => "constant", 15 => "string", 16 => "number",
        17 => "boolean", 18 => "array", 19 => "object", 20 => "key",
        21 => "null", 22 => "enum_member", 23 => "struct", 24 => "event",
        25 => "operator", 26 => "type_param", _ => "symbol",
    };

    let mut out = Vec::new();
    for sym in &items {
        let name = sym.get("name").and_then(|v| v.as_str()).unwrap_or("?");
        let kind = sym.get("kind").and_then(|v| v.as_u64()).unwrap_or(0);
        let detail = sym.get("detail").and_then(|v| v.as_str()).unwrap_or("");
        let loc_uri = sym.get("location").and_then(|l| l.get("uri"))
            .and_then(|v| v.as_str())
            .or_else(|| sym.get("uri").and_then(|v| v.as_str()))
            .unwrap_or("");
        let path = loc_uri.strip_prefix("file://").unwrap_or(loc_uri);
        let line = sym.get("location").and_then(|l| l.get("range"))
            .or_else(|| sym.get("range"))
            .and_then(|r| r.get("start"))
            .and_then(|s| s.get("line"))
            .and_then(|l| l.as_u64())
            .map(|l| l + 1)
            .unwrap_or(0);

        let entry = if !path.is_empty() {
            format!("{} ({}) — {}:{}", name, kind_name(kind), path, line)
        } else if !detail.is_empty() {
            format!("{} ({}) — {}", name, kind_name(kind), detail)
        } else {
            format!("{} ({})", name, kind_name(kind))
        };
        out.push(entry);
    }
    out.join("\n")
}

/// Format hover result.
pub fn format_hover(result: &Value) -> String {
    if result.is_null() { return "No hover information available.".to_string(); }
    let contents = result.get("contents");
    match contents {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Object(obj)) => {
            obj.get("value").and_then(|v| v.as_str()).unwrap_or("").to_string()
        }
        Some(Value::Array(arr)) => {
            arr.iter().filter_map(|v| {
                if let Value::String(s) = v { Some(s.as_str()) }
                else { v.get("value").and_then(|x| x.as_str()) }
            }).collect::<Vec<_>>().join("\n")
        }
        _ => format!("{}", result),
    }
}
