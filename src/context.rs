use serde::{Deserialize, Serialize};
use serde_json::json;

const CHARS_PER_TOKEN: usize = 4;
const ACTIVE_FILE_TURNS: usize = 3;
const LATEST_FILES_BUDGET_FRACTION: usize = 35; // 35% of max_tokens for all cached files

fn estimate_tokens(text: &str) -> usize {
    text.len() / CHARS_PER_TOKEN
}

fn format_gemma4_call(name: &str, args: &serde_json::Value) -> String {
    let args_str = if let Some(obj) = args.as_object() {
        obj.iter()
            .map(|(k, v)| {
                let val = match v {
                    serde_json::Value::String(s) => format!("<|\"|>{}<|\"|>", s),
                    serde_json::Value::Bool(b)   => b.to_string(),
                    serde_json::Value::Number(n)  => n.to_string(),
                    other                         => format!("<|\"|>{}<|\"|>", other),
                };
                format!("{}:{}", k, val)
            })
            .collect::<Vec<_>>()
            .join(",")
    } else {
        String::new()
    };
    format!("call:{}{{{}}}", name, args_str)
}

pub fn truncate_to_tokens(text: &str, max_tokens: usize) -> String {
    let max_chars = max_tokens * CHARS_PER_TOKEN;
    if text.len() <= max_chars {
        return text.to_string();
    }
    text.char_indices()
        .nth(max_chars)
        .map(|(i, _)| text[..i].to_string())
        .unwrap_or_else(|| text.to_string())
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Message {
    pub role: String,
    pub content: String,
    pub tool_calls: Option<Vec<ToolCall>>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub function: FunctionCall,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct CachedFile {
    pub content: String,
    pub timestamp: std::time::Instant,
    pub tokens: usize,
    /// Turn number when this file was last accessed (used for active→latest promotion)
    pub access_turn: usize,
}

pub struct ContextManager {
    max_tokens: usize,
    messages: Vec<Message>,
    system_prompt: Option<String>,
    cwd: String,
    turn_count: usize,
    /// Files accessed within the last ACTIVE_FILE_TURNS turns — injected right before generation
    pub active_files: std::collections::HashMap<String, CachedFile>,
    /// Files accessed more than ACTIVE_FILE_TURNS turns ago — injected before the system prompt
    pub latest_files: std::collections::HashMap<String, CachedFile>,
}

impl ContextManager {
    pub fn new(max_tokens: usize, system_prompt: Option<String>) -> Self {
        Self {
            max_tokens,
            messages: Vec::new(),
            system_prompt,
            cwd: ".".to_string(),
            turn_count: 0,
            active_files: std::collections::HashMap::new(),
            latest_files: std::collections::HashMap::new(),
        }
    }

    pub fn set_cwd(&mut self, cwd: String) {
        if self.cwd != cwd {
            self.cwd = cwd;
            self.add_message("system", &format!("Current working directory: {}", self.cwd));
        }
    }

    pub fn update_system_prompt(&mut self, prompt: String) {
        self.system_prompt = Some(prompt);
    }

    /// Called when a file is read or written. Places it in active_files (highest attention tier).
    pub fn update_latest_file(&mut self, path: String, content: String) {
        let tokens = estimate_tokens(&content);
        // Remove from latest_files if it was demoted there previously
        self.latest_files.remove(&path);
        self.active_files.insert(path, CachedFile {
            content,
            timestamp: std::time::Instant::now(),
            tokens,
            access_turn: self.turn_count,
        });
        self.evict_files_over_budget();
    }

    pub fn remove_latest_file(&mut self, path: &str) {
        self.active_files.remove(path);
        self.latest_files.remove(path);
    }

    /// Returns all cached files (active + latest) sorted newest-first, with active status flag.
    pub fn all_cached_files(&self) -> Vec<(String, &CachedFile, bool)> {
        let mut all: Vec<(String, &CachedFile, bool)> = self.active_files.iter()
            .map(|(k, v)| (k.clone(), v, true))
            .chain(self.latest_files.iter().map(|(k, v)| (k.clone(), v, false)))
            .collect();
        all.sort_by(|a, b| b.1.timestamp.cmp(&a.1.timestamp));
        all
    }

    fn all_cached_token_total(&self) -> usize {
        self.active_files.values().map(|f| f.tokens).sum::<usize>()
            + self.latest_files.values().map(|f| f.tokens).sum::<usize>()
    }

    /// If combined file cache exceeds 35% of max_tokens, evict oldest files.
    /// Eviction order: latest_files first (older), then active_files if still over budget.
    fn evict_files_over_budget(&mut self) {
        let budget = self.max_tokens * LATEST_FILES_BUDGET_FRACTION / 100;
        if self.all_cached_token_total() <= budget { return; }

        // Collect latest_files by age (oldest first)
        let mut latest_by_age: Vec<(std::time::Instant, String)> = self.latest_files
            .iter().map(|(k, v)| (v.timestamp, k.clone())).collect();
        latest_by_age.sort_by_key(|(t, _)| *t);
        for (_, path) in latest_by_age {
            if self.all_cached_token_total() <= budget { return; }
            self.latest_files.remove(&path);
        }

        // Still over budget — evict active_files oldest first
        let mut active_by_age: Vec<(std::time::Instant, String)> = self.active_files
            .iter().map(|(k, v)| (v.timestamp, k.clone())).collect();
        active_by_age.sort_by_key(|(t, _)| *t);
        for (_, path) in active_by_age {
            if self.all_cached_token_total() <= budget { return; }
            self.active_files.remove(&path);
        }
    }

    /// Promote active_files that are older than ACTIVE_FILE_TURNS turns to latest_files.
    fn promote_stale_active_files(&mut self) {
        let stale: Vec<String> = self.active_files.iter()
            .filter(|(_, f)| self.turn_count.saturating_sub(f.access_turn) > ACTIVE_FILE_TURNS)
            .map(|(p, _)| p.clone())
            .collect();
        for path in stale {
            if let Some(file) = self.active_files.remove(&path) {
                self.latest_files.insert(path, file);
            }
        }
    }

    pub fn add_message(&mut self, role: &str, content: &str) {
        if role == "user" {
            self.turn_count += 1;
            self.promote_stale_active_files();
        }
        self.messages.push(Message {
            role: role.to_string(),
            content: content.to_string(),
            tool_calls: None,
        });
        self.trim_context();
    }

    pub fn add_message_raw(&mut self, msg: Message) {
        if msg.role == "user" {
            self.turn_count += 1;
            self.promote_stale_active_files();
        }
        self.messages.push(msg);
        self.trim_context();
    }

    pub fn set_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages;
        self.trim_context();
    }

    pub fn add_assistant_tool_call(&mut self, content: &str, tool_calls: Vec<ToolCall>) {
        self.messages.push(Message {
            role: "assistant".to_string(),
            content: content.to_string(),
            tool_calls: Some(tool_calls),
        });
        self.trim_context();
    }

    pub fn add_tool_message(&mut self, tool_call_id: String, function_name: &str, content: &str) {
        let formatted_content = format!(
            "<|tool_response>response:{}{{result:<|'|>{}<|'|>,tool_call_id:<|'|>{}<|'|>}}<tool_response|><turn|>",
            function_name, content, tool_call_id
        );
        self.messages.push(Message {
            role: "tool".to_string(),
            content: formatted_content,
            tool_calls: None,
        });
        self.trim_context();
    }

    pub fn clear(&mut self) {
        self.messages.clear();
        self.turn_count = 0;
        self.active_files.clear();
        self.latest_files.clear();
    }

    pub fn get_messages(&self) -> Vec<Message> {
        self.messages.clone()
    }

    pub fn get_raw_prompt(&self) -> String {
        let mut prompt = String::from("<bos>");

        // ── 1. Latest files (background context — older files, before system prompt) ──
        if !self.latest_files.is_empty() {
            prompt.push_str("<|turn>system\n<|think|>\n<latest_files>\n");
            for (path, cached_file) in &self.latest_files {
                prompt.push_str(&format!("File: `{}`\n```\n{}\n```\n",
                    path, sanitize_file_content(&cached_file.content)));
            }
            prompt.push_str("</latest_files>\n<turn|>\n");
        }

        // ── 2. System prompt (instructions — near the conversation for attention) ──
        if let Some(sys) = &self.system_prompt {
            prompt.push_str("<|turn>system\n<|think|>\n");
            prompt.push_str(sys);
            prompt.push_str("<turn|>\n");
        }

        // ── 3. Message history ──
        let mut current_turn_role = String::new();

        for msg in &self.messages {
            match msg.role.as_str() {
                "system" => {
                    if current_turn_role == "model" { prompt.push_str("<turn|>\n"); }
                    prompt.push_str("<|turn>system\n<|think|>\n");
                    prompt.push_str(&msg.content);
                    prompt.push_str("<turn|>\n");
                    current_turn_role = String::new();
                }
                "user" => {
                    if current_turn_role == "model" { prompt.push_str("<turn|>\n"); }
                    prompt.push_str("<|turn>user\n");
                    prompt.push_str(&msg.content);
                    prompt.push_str("<turn|>\n");
                    current_turn_role = String::new();
                }
                "assistant" => {
                    if current_turn_role != "model" { prompt.push_str("<|turn>model\n"); }
                    let mut clean_content = msg.content.clone();
                    for (start_tag, end_tag) in [
                        ("<|channel>thought", "<channel|>"),
                        ("<thought>", "</thought>"),
                        ("<think>", "</think>"),
                    ] {
                        while let Some(start_idx) = clean_content.find(start_tag) {
                            if let Some(end_idx_rel) = clean_content[start_idx..].find(end_tag) {
                                let end_pos = start_idx + end_idx_rel + end_tag.len();
                                clean_content.replace_range(start_idx..end_pos, "");
                            } else {
                                clean_content.truncate(start_idx);
                                break;
                            }
                        }
                    }
                    clean_content = clean_content.replace("<|channel>text\n", "").replace("<|channel>text", "");
                    if msg.tool_calls.is_some() {
                        if let Some(idx) = clean_content.find("<|tool_call>") {
                            clean_content.truncate(idx);
                        }
                    }
                    prompt.push_str(clean_content.trim());
                    prompt.push('\n');
                    if let Some(calls) = &msg.tool_calls {
                        for tc in calls {
                            let call_str = format_gemma4_call(&tc.function.name, &tc.function.arguments);
                            prompt.push_str(&format!("<|tool_call>{}<tool_call|>", call_str));
                        }
                        current_turn_role = "model".to_string();
                    } else if msg.content.contains("<|tool_call>") {
                        current_turn_role = "model".to_string();
                    } else {
                        prompt.push_str("<turn|>\n");
                        current_turn_role = String::new();
                    }
                }
                "tool" => {
                    if current_turn_role != "model" { prompt.push_str("<|turn>model\n"); }
                    prompt.push_str(&msg.content);
                    current_turn_role = "model".to_string();
                }
                _ => {}
            }
        }

        // ── 4. Active file (currently worked on — highest attention, right before generation) ──
        if !self.active_files.is_empty() {
            if current_turn_role == "model" { prompt.push_str("<turn|>\n"); }
            prompt.push_str("<|turn>system\n<|think|>\n<active_file>\n");
            for (path, cached_file) in &self.active_files {
                prompt.push_str(&format!("File: `{}`\n```\n{}\n```\n",
                    path, sanitize_file_content(&cached_file.content)));
            }
            prompt.push_str("</active_file>\n<turn|>\n");
            current_turn_role = String::new();
        }

        // ── 5. Start model generation ──
        if current_turn_role != "model" {
            prompt.push_str("<|turn>model\n");
        }

        prompt
    }

    pub fn get_tools_for_api(&self) -> Vec<gemma_chat::ToolDefinition> {
        let sys = match &self.system_prompt { Some(s) => s.as_str(), None => return vec![] };
        let mut tools = Vec::new();
        let mut remaining = sys;
        while let Some(start) = remaining.find("<|tool>") {
            let after = &remaining[start + 7..];
            if let Some(end) = after.find("<tool|>") {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(after[..end].trim()) {
                    if let Some(obj) = v.as_object() {
                        let name = obj.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let desc = obj.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let params = obj.get("parameters").cloned().unwrap_or(json!({}));
                        tools.push(gemma_chat::ToolDefinition::new(name, desc, params));
                    }
                }
                remaining = &after[end + 7..];
            } else { break; }
        }
        tools
    }

    pub fn get_messages_for_api(&self) -> Vec<gemma_chat::Message> {
        let mut msgs = Vec::new();

        // ── 1. Latest files (background context, before system prompt) ──
        if !self.latest_files.is_empty() {
            let mut files_content = String::from("<latest_files>\n");
            for (path, _cached_file) in &self.latest_files {
                let abs = std::path::Path::new(&self.cwd).join(path);
                let body = match std::fs::read_to_string(&abs) {
                    Ok(c)  => format!("```\n{}\n```", sanitize_file_content(&c)),
                    Err(_) => format!("⚠ File `{}` was deleted or no longer exists on disk.", path),
                };
                files_content.push_str(&format!("File: `{}`\n{}\n", path, body));
            }
            files_content.push_str("</latest_files>");
            msgs.push(gemma_chat::Message::system(files_content));
        }

        // ── 2. System prompt ──
        if let Some(sys) = &self.system_prompt {
            let mut clean = sys.clone();
            loop {
                if let Some(s) = clean.find("<|tool>") {
                    if let Some(rel) = clean[s..].find("<tool|>") { clean.drain(s..s + rel + 7); }
                    else { clean.truncate(s); break; }
                } else { break; }
            }
            clean = clean.replace("<|think|>", "").replace("<|turn>", "").replace("<turn|>", "");
            let clean = clean.trim().to_string();
            if !clean.is_empty() {
                msgs.push(gemma_chat::Message::system(clean));
            }
        }

        // ── 3. Message history ──
        for msg in &self.messages {
            match msg.role.as_str() {
                "system" => msgs.push(gemma_chat::Message::system(msg.content.clone())),
                "user"   => msgs.push(gemma_chat::Message::user(msg.content.clone())),
                "assistant" => {
                    let clean = self.strip_thinking(&msg.content);
                    if let Some(calls) = &msg.tool_calls {
                        let gc: Vec<gemma_chat::AssistantToolCall> = calls.iter().map(|tc| gemma_chat::AssistantToolCall {
                            id: tc.id.clone(),
                            kind: "function".into(),
                            function: gemma_chat::FunctionCall {
                                name: tc.function.name.clone(),
                                arguments: serde_json::to_string(&tc.function.arguments).unwrap_or_default(),
                            },
                        }).collect();
                        msgs.push(gemma_chat::Message::assistant_with_tools(clean, gc, None));
                    } else {
                        msgs.push(gemma_chat::Message::assistant(clean));
                    }
                }
                "tool" => {
                    let tc_id = Self::extract_delimited(&msg.content, "tool_call_id:<|'|>", "<|'|>")
                        .unwrap_or("unknown".into());
                    let result = Self::extract_delimited(&msg.content, "result:<|'|>", "<|'|>")
                        .unwrap_or(msg.content.clone());
                    msgs.push(gemma_chat::Message::tool_result(tc_id, result));
                }
                _ => {}
            }
        }

        // ── 4. Active file (highest attention — right before generation) ──
        if !self.active_files.is_empty() {
            let mut files_content = String::from("<active_file>\n");
            for (path, _cached_file) in &self.active_files {
                let abs = std::path::Path::new(&self.cwd).join(path);
                let body = match std::fs::read_to_string(&abs) {
                    Ok(c)  => format!("```\n{}\n```", sanitize_file_content(&c)),
                    Err(_) => format!("⚠ File `{}` was deleted or no longer exists on disk.", path),
                };
                files_content.push_str(&format!("File: `{}`\n{}\n", path, body));
            }
            files_content.push_str("</active_file>");
            msgs.push(gemma_chat::Message::system(files_content));
        }

        msgs
    }

    fn strip_thinking(&self, content: &str) -> String {
        let mut c = content.to_string();
        for (start, end) in [("<|channel>thought", "<channel|>"), ("<think>", "</think>"), ("<thought>", "</thought>")] {
            loop {
                if let Some(s) = c.find(start) {
                    if let Some(rel) = c[s..].find(end) { c.replace_range(s..s + rel + end.len(), ""); }
                    else { c.truncate(s); break; }
                } else { break; }
            }
        }
        c.replace("<|channel>text\n", "").replace("<|channel>text", "").trim().to_string()
    }

    fn extract_delimited(content: &str, prefix: &str, suffix: &str) -> Option<String> {
        let pos = content.find(prefix)?;
        let after = &content[pos + prefix.len()..];
        let end = after.find(suffix)?;
        Some(after[..end].to_string())
    }

    pub fn get_token_count(&self) -> usize {
        estimate_tokens(&self.get_raw_prompt())
    }

    fn trim_context(&mut self) {
        let files_tokens = self.all_cached_token_total();
        let usable = self.max_tokens.saturating_sub(files_tokens);
        let msg_estimate: usize = self.messages.iter().map(|m| estimate_tokens(&m.content)).sum();
        if msg_estimate <= usable { return; }

        let total_msg_tokens: usize = self.messages.iter().map(|m| estimate_tokens(&m.content)).sum();
        if total_msg_tokens <= usable { return; }

        let mut need_to_drop = total_msg_tokens.saturating_sub(usable);
        let mut cut = 0usize;
        let n = self.messages.len();
        let mut i = 0;

        while i < n && need_to_drop > 0 {
            let msg = &self.messages[i];
            if msg.role == "system" { i += 1; continue; }

            if msg.role == "assistant" && msg.tool_calls.is_some() {
                let unit_start = i;
                let cost = estimate_tokens(&msg.content);
                i += 1;
                let mut unit_cost = cost;
                while i < n && self.messages[i].role == "tool" {
                    unit_cost += estimate_tokens(&self.messages[i].content);
                    i += 1;
                }
                cut = i;
                need_to_drop = need_to_drop.saturating_sub(unit_cost);
                let _ = unit_start;
            } else if msg.role == "tool" {
                let cost = estimate_tokens(&msg.content);
                i += 1;
                cut = i;
                need_to_drop = need_to_drop.saturating_sub(cost);
            } else {
                let cost = estimate_tokens(&msg.content);
                i += 1;
                cut = i;
                need_to_drop = need_to_drop.saturating_sub(cost);
            }
        }

        if cut > 0 {
            self.messages.drain(0..cut);
        }
    }
}

fn sanitize_file_content(content: &str) -> String {
    let mut s = content.to_string();
    for tag in &[
        "<turn|>", "<|turn>", "<|tool_call>", "<tool_call|>",
        "<|tool_response>", "<tool_response|>", "<|channel>", "<channel|>",
        "<thought>", "</thought>", "<think>", "</think>",
        "<|\"|>", "<|\\\\\">", "<|\\\">", "<|\">", "<|'>", "<|'|>",
    ] {
        s = s.replace(tag, "");
    }
    s
}
