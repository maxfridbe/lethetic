use serde::{Deserialize, Serialize};
use reqwest::Client;
use std::fs;
use std::io::Write;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use futures_util::StreamExt;
use crate::context::{ContextManager, ToolCall};
use crate::config::Config;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GenerateRequest {
    pub model: String,
    pub prompt: String,
    pub raw: bool,
    pub stream: bool,
    pub options: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<String>>,
}

#[derive(Deserialize, Debug)]
pub struct GenerateResponse {
    #[serde(alias = "content", alias = "text")]
    #[serde(default)]
    pub response: String,
    #[serde(alias = "stop")]
    #[serde(default)]
    pub done: bool,
    #[serde(alias = "tokens_evaluated")]
    pub eval_count: Option<u32>,
    pub eval_duration: Option<u64>,
    pub tokens_predicted: Option<u32>,
    pub timings: Option<Timings>,
    pub choices: Option<Vec<Choice>>,
}

#[derive(Deserialize, Debug)]
pub struct Choice {
    pub delta: Option<Delta>,
    pub text: Option<String>,
    pub finish_reason: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct Delta {
    pub content: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct Timings {
    #[serde(alias = "predicted_ms")]
    pub predicted_ms: Option<f64>,
    #[serde(alias = "predicted_per_token_ms")]
    pub predicted_per_token_ms: Option<f64>,
    #[serde(alias = "predicted_per_second")]
    pub predicted_per_second: Option<f64>,
}

/// A model offered by a configured server, as shown in the model switcher.
#[derive(Clone, Debug)]
pub struct ModelChoice {
    pub display: String,
    pub url: String,
    pub model_id: String,
}

#[derive(Clone, Debug)]
pub enum StreamEvent {
    Chunk(String),
    PreparingToolCall(String), // tool name being streamed — show "preparing" status
    ToolCalls(Vec<ToolCall>),
    ToolResult { id: Option<String>, func_name: String, result: String, cwd: String },
    ToolProgress(String),
    LoadProgress(f32, String),
    SessionLoaded { dir: String, state: crate::app::SessionState },
    Done { completion_tokens: Option<u32>, prompt_tokens: Option<u32>, tg_per_s: Option<f64>, pp_per_s: Option<f64> },
    Error(String),
    DebugLog(String),
    TokenUpdate(u32, f64), // (count, ms)
    ModelsReady(Vec<ModelChoice>),
    CompactionChunk(String),
    CompactionDone { new_session_dir: String },
}

/// Maximum tokens the model may generate per completion.
const MAX_COMPLETION_TOKENS: u32 = 24_576;

fn base_url(server_url: &str) -> String {
    for suffix in &["/v1/responses", "/v1/chat/completions", "/completion"] {
        if let Some(base) = server_url.strip_suffix(suffix) {
            return format!("{}/v1", base);
        }
    }
    server_url.to_string()
}

pub fn trigger_llm_request(client: Client, config: Config, context_manager: &ContextManager, tx: mpsc::UnboundedSender<StreamEvent>, token: CancellationToken, _is_debug: bool, session_dir: Option<String>) {
    let messages = context_manager.get_messages_for_api();
    let tools    = context_manager.get_tools_for_api();
    let base     = base_url(&config.server_url);
    let model    = config.model.clone();
    let ctx_len  = context_manager.get_token_count();
    let api_key  = config.api_key.clone();

    let req_body = gemma_chat::build_request(&model, &messages, &tools, MAX_COMPLETION_TOKENS, config.thinking, config.extra_body.as_ref());

    let log_tx = tx.clone();
    let server_url = config.server_url.clone();

    let prefix = session_dir.clone().unwrap_or_else(|| ".lethetic/".to_string());
    let _ = fs::create_dir_all(&prefix);

    // Raw Gemma4 prompt (what the model conceptually sees, not serialized JSON)
    let _ = fs::write(format!("{}/last_raw_prompt.txt", prefix), context_manager.get_raw_prompt());
    // API JSON body (last request only, no append)
    if let Ok(body) = serde_json::to_string_pretty(&req_body) {
        let _ = fs::write(format!("{}/last_request.json", prefix), body);
    }
    // Reset tokens.jsonl for this request
    let _ = fs::write(format!("{}/tokens.jsonl", prefix), "");

    let prefix_clone = prefix.clone();
    let log_tx_spawn = log_tx.clone();
    let token_spawn = token.clone();
    let server_url_spawn = server_url.clone();

    tokio::spawn(async move {
        let _ = log_tx_spawn.send(StreamEvent::DebugLog(format!("CALL_START|{}|{}", server_url_spawn, ctx_len)));

        let request_start = std::time::Instant::now();

        // Appends one JSON line to tokens.jsonl
        let append_token = |prefix: &str, val: serde_json::Value| {
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true)
                .open(format!("{}/tokens.jsonl", prefix))
            {
                let _ = writeln!(f, "{}", val);
            }
        };

        let mut event_stream = match gemma_chat::stream_chat(&client, &base, &model, &messages, &tools, MAX_COMPLETION_TOKENS, api_key.as_deref(), config.thinking, config.extra_body.as_ref()).await {
            Ok(s) => s,
            Err(e) => {
                let _ = log_tx_spawn.send(StreamEvent::Error(e));
                let _ = log_tx_spawn.send(StreamEvent::Done { completion_tokens: None, prompt_tokens: None, tg_per_s: None, pp_per_s: None });
                return;
            }
        };

        let mut in_thought_mode = true;
        let mut emitted_think_open = false;

        loop {
            let ev = tokio::select! {
                e = event_stream.next() => e,
                _ = token_spawn.cancelled() => None,
            };
            let Some(ev) = ev else { break; };

            match ev {
                gemma_chat::StreamEvent::ReasoningDelta(text) => {
                    let ms = request_start.elapsed().as_millis();
                    // Emit <think> before the first reasoning chunk so strip_thinking() can
                    // find the <think>...</think> pair and remove it from history.
                    if !emitted_think_open {
                        emitted_think_open = true;
                        append_token(&prefix_clone, serde_json::json!({"c": "<think>\n", "t": ms, "kind": "synthetic"}));
                        let _ = log_tx_spawn.send(StreamEvent::Chunk("<think>\n".to_string()));
                    }
                    append_token(&prefix_clone, serde_json::json!({"c": text, "t": ms, "kind": "reasoning"}));
                    let _ = log_tx_spawn.send(StreamEvent::Chunk(text));
                }
                gemma_chat::StreamEvent::TextDelta(text) => {
                    if in_thought_mode {
                        in_thought_mode = false;
                        if emitted_think_open {
                            let ms = request_start.elapsed().as_millis();
                            append_token(&prefix_clone, serde_json::json!({"c": "</think>\n", "t": ms, "kind": "synthetic"}));
                            let _ = log_tx_spawn.send(StreamEvent::Chunk("</think>\n".to_string()));
                        }
                    }
                    let ms = request_start.elapsed().as_millis();
                    append_token(&prefix_clone, serde_json::json!({"c": text, "t": ms, "kind": "text"}));
                    let _ = log_tx_spawn.send(StreamEvent::Chunk(text));
                }
                gemma_chat::StreamEvent::ToolCallStart { name, .. } => {
                    let _ = log_tx_spawn.send(StreamEvent::PreparingToolCall(name));
                }
                gemma_chat::StreamEvent::ToolCallComplete { id, name, arguments, .. } => {
                    let ms = request_start.elapsed().as_millis();
                    // Prefer the model's own tool_call_id from its arguments over the
                    // server-generated random UUID. The peg-gemma4 template embeds both
                    // the call ID and the tool_response ID into the native prompt; if they
                    // differ the model sees a broken pair and immediately emits EOS.
                    let effective_id = arguments["tool_call_id"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string())
                        .unwrap_or(id);
                    append_token(&prefix_clone, serde_json::json!({"c": "", "t": ms, "kind": "tool", "name": name, "id": effective_id}));
                    let tc = ToolCall {
                        id: effective_id,
                        function: crate::context::FunctionCall { name, arguments },
                    };
                    let _ = log_tx_spawn.send(StreamEvent::ToolCalls(vec![tc]));
                }
                gemma_chat::StreamEvent::Done { completion_tokens, prompt_tokens, tg_per_s, pp_per_s } => {
                    let _ = log_tx_spawn.send(StreamEvent::Done { completion_tokens, prompt_tokens, tg_per_s, pp_per_s });
                    break;
                }
                gemma_chat::StreamEvent::Error(e) => {
                    let _ = log_tx_spawn.send(StreamEvent::Error(e));
                }
                _ => {}
            }
        }
    });
}

pub async fn summarize_llm(client: &reqwest::Client, config: &Config, context: &str, prompt: &str) -> Result<String, String> {
    let truncated_context = crate::context::truncate_to_tokens(context, 160000);
    let user_text = format!("{}\n\nContext to summarize:\n{}", prompt, truncated_context);
    let messages = vec![gemma_chat::Message::user(user_text)];
    gemma_chat::complete(client, &base_url(&config.server_url), &config.model, &messages, 4096, config.api_key.as_deref(), config.thinking, config.extra_body.as_ref()).await
}

/// Strip file bodies from a `ui_log.txt` before sending for compaction.
///
/// Two classes of content are removed:
/// - The `content` / `new_string` / `old_string` / `new_content` fields inside
///   `write_file`, `edit_file`, and `replace_text` tool-call JSON lines, which
///   contain the full body of written/replaced files.
/// - The entire TOOL RESULT block that follows a `read_file` or
///   `read_file_lines` call, since those files will be re-read dynamically.
///
/// Everything else (user messages, shell output, build results, todo updates,
/// short tool results) is preserved so the model understands what happened.
pub fn strip_file_content_from_log(log: &str) -> String {
    /// Tools whose TOOL RESULT blocks are pure file content and can be dropped.
    fn is_file_read_tool(name: &str) -> bool {
        matches!(name, "read_file" | "read_file_lines")
    }

    /// Tools whose call JSON contains large embedded file bodies.
    fn is_file_write_tool(name: &str) -> bool {
        matches!(name, "write_file" | "edit_file" | "replace_text" | "apply_patch")
    }

    /// JSON fields whose values may be large file bodies.
    const LARGE_FIELDS: &[&str] = &["content", "new_string", "old_string", "new_content", "patch"];

    /// Elide the value of a single JSON string field in a raw call line.
    fn elide_field(line: &str, field: &str) -> String {
        let needle = format!("\"{}\":\"", field);
        let Some(start) = line.find(&needle) else { return line.to_string() };
        let val_start = start + needle.len();
        // Walk forward respecting \" escapes to find closing quote.
        let bytes = line.as_bytes();
        let mut pos = val_start;
        while pos < bytes.len() {
            if bytes[pos] == b'\\' {
                pos += 2; // skip escaped char
            } else if bytes[pos] == b'"' {
                break;
            } else {
                pos += 1;
            }
        }
        let original_len = pos - val_start;
        if original_len < 120 {
            return line.to_string(); // short value — not a file body, keep it
        }
        format!("{}{}\"[elided {} chars]\"{}",
            &line[..val_start],
            "", // value replaced
            original_len,
            if pos < line.len() { &line[pos + 1..] } else { "" })
    }

    let mut out = String::with_capacity(log.len() / 2);
    let mut last_tool: Option<String> = None;
    let mut skip_until_next_section = false;

    for line in log.lines() {
        // Detect section headers.
        if line.starts_with("=== ") {
            skip_until_next_section = false;

            if line.starts_with("=== TOOL RESULT ===") {
                let is_read = last_tool.as_deref().map(is_file_read_tool).unwrap_or(false);
                out.push_str(line);
                out.push('\n');
                if is_read {
                    out.push_str("[file content elided — will be re-read dynamically]\n");
                    skip_until_next_section = true;
                }
                continue;
            }

            out.push_str(line);
            out.push('\n');
            continue;
        }

        if skip_until_next_section {
            continue;
        }

        // Detect tool calls: lines starting with "call:<name>{".
        if let Some(rest) = line.strip_prefix("call:") {
            let tool_name = rest.split(|c| c == '{' || c == ' ').next().unwrap_or("").to_string();
            last_tool = Some(tool_name.clone());

            if is_file_write_tool(&tool_name) {
                let mut elided = line.to_string();
                for field in LARGE_FIELDS {
                    elided = elide_field(&elided, field);
                }
                out.push_str(&elided);
            } else {
                out.push_str(line);
            }
            out.push('\n');
            continue;
        }

        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Compact a session log using a proper system + user message pair so the
/// compaction prompt is in the system role (better instruction-following) and
/// Window size for multipass compaction (chars after stripping).
/// Small enough that the model handles each section without looping.
const COMPACT_WINDOW: usize = 80_000;
const COMPACT_OVERLAP: usize = 8_000;

/// Split `text` into overlapping windows of at most `window` chars,
/// stepping by `window - overlap` each time. Splits only on newline
/// boundaries so lines are never torn mid-way.
fn sliding_windows(text: &str, window: usize, overlap: usize) -> Vec<&str> {
    let step = window.saturating_sub(overlap);
    let mut windows = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let end = (start + window).min(text.len());
        // snap end back to a newline so we don't cut mid-line
        let end = if end < text.len() {
            text[..end].rfind('\n').map(|p| p + 1).unwrap_or(end)
        } else { end };
        windows.push(&text[start..end]);
        if end >= text.len() { break; }
        start += step;
        // snap start forward to next newline as well
        if start < text.len() {
            if let Some(nl) = text[start..].find('\n') {
                start += nl + 1;
            }
        }
    }
    windows
}

/// Run one model pass (non-streaming) and return post-processed text.
async fn run_pass(
    client: &reqwest::Client,
    config: &Config,
    extra: &serde_json::Value,
    system: &str,
    user: &str,
    max_tokens: u32,
    cancel: &CancellationToken,
) -> Result<String, String> {
    let url = base_url(&config.server_url);
    let messages = vec![
        gemma_chat::Message::system(system),
        gemma_chat::Message::user(user),
    ];
    let fut = gemma_chat::complete(
        client,
        &url,
        &config.model,
        &messages,
        max_tokens,
        config.api_key.as_deref(),
        None,
        Some(extra),
    );
    let raw = tokio::select! {
        _ = cancel.cancelled() => return Err("Cancelled".to_string()),
        result = fut => result?,
    };
    Ok(truncate_at_repetition(&strip_think_tags(&raw)))
}

/// Run one model pass (streaming), forwarding chunks via `tx`.
async fn run_pass_streaming(
    client: &reqwest::Client,
    config: &Config,
    extra: &serde_json::Value,
    system: &str,
    user: &str,
    max_tokens: u32,
    tx: &mpsc::UnboundedSender<StreamEvent>,
    cancel: &CancellationToken,
) -> Result<String, String> {
    use futures_util::StreamExt as _;
    let messages = vec![
        gemma_chat::Message::system(system),
        gemma_chat::Message::user(user),
    ];
    let mut stream = gemma_chat::stream_chat(
        client,
        &base_url(&config.server_url),
        &config.model,
        &messages,
        &[],
        max_tokens,
        config.api_key.as_deref(),
        None,
        Some(extra),
    ).await?;

    let mut accumulated = String::new();
    let mut line_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut last_checked = 0usize;
    let mut loop_detected = false;

    loop {
        let event = tokio::select! {
            _ = cancel.cancelled() => return Err("Cancelled".to_string()),
            ev = stream.next() => ev,
        };
        match event {
            Some(gemma_chat::StreamEvent::TextDelta(text))
            | Some(gemma_chat::StreamEvent::ReasoningDelta(text)) => {
                let _ = tx.send(StreamEvent::CompactionChunk(text.clone()));
                accumulated.push_str(&text);
                let slice = &accumulated[last_checked..];
                if let Some(nl) = slice.rfind('\n') {
                    for line in slice[..nl].lines() {
                        let t = line.trim();
                        if t.is_empty() { continue; }
                        let c = line_counts.entry(t.to_string()).or_insert(0);
                        *c += 1;
                        if *c >= 3 { loop_detected = true; break; }
                    }
                    last_checked += nl + 1;
                }
                if loop_detected { break; }
            }
            Some(gemma_chat::StreamEvent::Done { .. }) | None => break,
            Some(gemma_chat::StreamEvent::Error(e)) => return Err(e),
            Some(_) => {}
        }
    }
    Ok(truncate_at_repetition(&strip_think_tags(accumulated.trim())))
}

fn compaction_extra(config: &Config) -> serde_json::Value {
    let mut extra = config.extra_body.clone()
        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
    if let Some(obj) = extra.as_object_mut() {
        obj.insert("temperature".to_string(), serde_json::json!(0.1));
    }
    extra
}

const WINDOW_SYSTEM: &str = "\
You are summarizing one section of a coding session log. \
Capture what happened: user requests, files created/modified (exact paths), \
key decisions, errors encountered, and state at the end of this section.\n\
Rules: output ONLY the summary, begin immediately, no preamble, no planning. \
Preserve exact file paths, function names, and command lines. Be terse.";

const MERGE_SYSTEM: &str = "\
You are merging partial summaries of a coding session into one complete summary.\n\
Produce a single concise summary covering: the user's original goal, all files \
created or modified (exact paths), key decisions and reasoning, current project \
state (done / pending / blocked), important errors and how they were resolved.\n\
Rules: output ONLY the merged summary, begin immediately, no preamble, no planning. \
Preserve exact paths, names, and command lines. The result must be shorter than \
the combined partials.";

/// Run all window passes in parallel, returning ordered partial summaries.
async fn run_windows_parallel(
    client: &reqwest::Client,
    config: &Config,
    extra: &serde_json::Value,
    windows: &[&str],
    cancel: &CancellationToken,
) -> Result<Vec<String>, String> {
    let n = windows.len();
    let mut set = tokio::task::JoinSet::new();
    for (i, window) in windows.iter().enumerate() {
        let client = client.clone();
        let config = config.clone();
        let extra = extra.clone();
        let cancel = cancel.clone();
        let user = format!("Section {}/{} of the session log:\n\n{}", i + 1, n, window);
        set.spawn(async move {
            run_pass(&client, &config, &extra, WINDOW_SYSTEM, &user, 1024, &cancel)
                .await
                .map(|r| (i, r))
        });
    }
    let mut partials = vec![String::new(); n];
    while let Some(join_result) = set.join_next().await {
        if cancel.is_cancelled() {
            set.abort_all();
            return Err("Cancelled".to_string());
        }
        let (i, text) = join_result.map_err(|e| e.to_string())??;
        partials[i] = text;
    }
    Ok(partials)
}

/// CLI variant: window passes run in parallel, merge pass sequential.
pub async fn compact_llm(client: &reqwest::Client, config: &Config, log: &str, _system_prompt: &str) -> Result<String, String> {
    let cancel = CancellationToken::new(); // CLI: never cancelled
    let stripped = strip_file_content_from_log(log);
    let extra = compaction_extra(config);
    let window_strs: Vec<&str> = sliding_windows(&stripped, COMPACT_WINDOW, COMPACT_OVERLAP);
    let n = window_strs.len();
    eprintln!("Compacting: {} windows in parallel", n);

    let mut partials = run_windows_parallel(client, config, &extra, &window_strs, &cancel).await?;
    eprintln!("  All passes done, merging…");

    if partials.len() == 1 {
        return Ok(partials.remove(0));
    }

    let combined = partials.iter().enumerate()
        .map(|(i, p)| format!("=== Section {} ===\n{}", i + 1, p))
        .collect::<Vec<_>>()
        .join("\n\n");
    run_pass(client, config, &extra, MERGE_SYSTEM,
        &format!("Partial summaries to merge:\n\n{}", combined), 2048, &cancel).await
}

/// UI variant: window passes run in parallel (reporting completions), then
/// the merge pass streams live into the popup.
pub async fn compact_llm_streaming(
    client: &reqwest::Client,
    config: &Config,
    log: &str,
    _system_prompt: &str,
    tx: &mpsc::UnboundedSender<StreamEvent>,
    cancel: &CancellationToken,
) -> Result<String, String> {
    let stripped = strip_file_content_from_log(log);
    let extra = compaction_extra(config);
    let window_strs: Vec<&str> = sliding_windows(&stripped, COMPACT_WINDOW, COMPACT_OVERLAP);
    let n = window_strs.len();

    let _ = tx.send(StreamEvent::CompactionChunk(
        format!("Running {} window passes in parallel…\n", n)
    ));

    // Spawn all window passes concurrently; report each as it finishes.
    let mut set = tokio::task::JoinSet::new();
    for (i, window) in window_strs.iter().enumerate() {
        let client = client.clone();
        let config = config.clone();
        let extra = extra.clone();
        let cancel = cancel.clone();
        let user = format!("Section {}/{} of the session log:\n\n{}", i + 1, n, window);
        set.spawn(async move {
            run_pass(&client, &config, &extra, WINDOW_SYSTEM, &user, 1024, &cancel)
                .await
                .map(|r| (i, r))
        });
    }

    let mut partials = vec![String::new(); n];
    let mut done = 0usize;
    while let Some(join_result) = set.join_next().await {
        if cancel.is_cancelled() {
            set.abort_all();
            return Err("Cancelled".to_string());
        }
        let (i, text) = join_result.map_err(|e| e.to_string())??;
        partials[i] = text;
        done += 1;
        let _ = tx.send(StreamEvent::CompactionChunk(
            format!("  ✓ Pass {}/{} complete\n", done, n)
        ));
    }

    if partials.len() == 1 {
        return Ok(partials.remove(0));
    }

    let _ = tx.send(StreamEvent::CompactionChunk(
        format!("\n── Merging {} sections (streaming) ──\n", n)
    ));
    let combined = partials.iter().enumerate()
        .map(|(i, p)| format!("=== Section {} ===\n{}", i + 1, p))
        .collect::<Vec<_>>()
        .join("\n\n");
    run_pass_streaming(client, config, &extra, MERGE_SYSTEM,
        &format!("Partial summaries to merge:\n\n{}", combined), 2048, tx, cancel).await
}

/// Detect repeated-line loops (a common Gemma 4 failure mode when reasoning
/// leaks into output) and truncate before the third occurrence of any line.
fn truncate_at_repetition(text: &str) -> String {
    let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let mut out = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            out.push(line);
            continue;
        }
        let count = seen.entry(trimmed).or_insert(0);
        *count += 1;
        if *count >= 3 {
            break;
        }
        out.push(line);
    }
    out.join("\n").trim_end().to_string()
}

/// Remove `<think>…</think>` blocks from model output so leaked reasoning
/// tokens don't end up in compacted summaries.
fn strip_think_tags(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<think>") {
        result.push_str(&rest[..start]);
        match rest[start..].find("</think>") {
            Some(rel_end) => rest = &rest[start + rel_end + "</think>".len()..],
            None => return result.trim().to_string(), // unclosed tag — drop tail
        }
    }
    result.push_str(rest);
    result.trim().to_string()
}

/// Query a server's /v1/models endpoint and return (display_name, model_id) pairs.
pub async fn get_available_models(client: &Client, server_url: &str, api_key: Option<&str>) -> Vec<(String, String)> {
    let base = base_url(server_url);
    // strip trailing /v1 to get root, then re-add /v1/models
    let models_url = if base.ends_with("/v1") {
        format!("{}/models", base)
    } else if base.contains("/chat/completions") {
        let base_no_query = base.split('?').next().unwrap_or(&base);
        if let Some(idx) = base_no_query.rfind("/chat/completions") {
            let mut prefix = base_no_query[..idx].to_string();
            if !prefix.ends_with("/v1") {
                prefix.push_str("/v1");
            }
            format!("{}/models", prefix)
        } else {
            format!("{}/v1/models", base)
        }
    } else {
        format!("{}/v1/models", base)
    };
    let mut req = client.get(&models_url);
    if let Some(key) = api_key {
        req = req.bearer_auth(key);
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(_) => return vec![],
    };
    let json: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(_) => return vec![],
    };
    json["data"].as_array()
        .map(|arr| arr.iter().filter_map(|m| {
            let id = m["id"].as_str()?;
            Some((id.to_string(), id.to_string()))
        }).collect())
        .unwrap_or_default()
}

pub async fn get_single_response(client: &Client, config: &Config, prompt: String, _images: Option<Vec<String>>, tx: Option<&mpsc::UnboundedSender<StreamEvent>>) -> Result<String, String> {
    let base = base_url(&config.server_url);
    if let Some(log_tx) = tx {
        let _ = log_tx.send(StreamEvent::DebugLog(format!("SINGLE_CALL_START|{}", base)));
    }
    let messages = vec![gemma_chat::Message::user(prompt)];
    gemma_chat::complete(client, &base, &config.model, &messages, 4096, config.api_key.as_deref(), config.thinking, config.extra_body.as_ref()).await
}


#[cfg(test)]
mod tests {
    use super::strip_file_content_from_log;

    #[test]
    fn strips_write_file_content() {
        let log = "=== TOOL CALL: Write CLI ===\ncall:write_file{\"content\":\"using System;\\nnamespace Foo { class Bar { } }\",\"path\":\"/foo/Bar.cs\",\"tool_call_id\":\"w1\"}\n=== TOOL RESULT ===\nSuccessfully wrote to /foo/Bar.cs\n";
        let out = strip_file_content_from_log(log);
        assert!(!out.contains("using System"), "file body should be elided");
        assert!(out.contains("elided"), "should mention elided");
        assert!(out.contains("/foo/Bar.cs"), "path should survive");
    }

    #[test]
    fn strips_read_file_result() {
        let log = "=== TOOL CALL: Read config ===\ncall:read_file{\"path\":\"/foo/x.csproj\",\"tool_call_id\":\"r1\"}\n=== TOOL RESULT ===\n<Project Sdk=\"...\"><lots of xml/></Project>\n=== THOUGHT ===\n\n";
        let out = strip_file_content_from_log(log);
        assert!(!out.contains("<Project"), "read result should be elided");
        assert!(out.contains("elided"), "should mention elided");
        assert!(out.contains("=== THOUGHT ==="), "subsequent sections survive");
    }

    #[test]
    fn keeps_short_tool_fields() {
        // A field with a value under 120 chars should not be elided
        let log = "=== TOOL CALL: Shell ===\ncall:run_shell_command{\"command\":\"dotnet build\",\"tool_call_id\":\"sh1\"}\n=== TOOL RESULT ===\nEXIT_CODE: 0\n";
        let out = strip_file_content_from_log(log);
        assert!(out.contains("dotnet build"), "short command should survive");
    }

    #[test]
    fn keeps_shell_results() {
        let log = "=== TOOL CALL: Build ===\ncall:run_shell_command{\"command\":\"make\",\"tool_call_id\":\"sh2\"}\n=== TOOL RESULT ===\nBuild succeeded.\n";
        let out = strip_file_content_from_log(log);
        assert!(out.contains("Build succeeded"), "shell output should survive");
    }
}
