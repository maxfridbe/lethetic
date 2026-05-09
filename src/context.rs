use serde::{Deserialize, Serialize};
use serde_json::json;

// Gemma 4 uses SentencePiece, not cl100k. A simple chars/4 heuristic (matching opencode's
// approach) is more accurate than GPT-4's tokenizer and avoids a heavy compile-time dep.
const CHARS_PER_TOKEN: usize = 4;

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
    // Truncate on a char boundary
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
}

// Files may not consume more than this fraction of the total context budget.
const LATEST_FILES_BUDGET_FRACTION: usize = 35; // 35%

pub struct ContextManager {
    max_tokens: usize,
    messages: Vec<Message>,
    system_prompt: Option<String>,
    cwd: String,
    pub latest_files: std::collections::HashMap<String, CachedFile>,
}

impl ContextManager {
    pub fn new(max_tokens: usize, system_prompt: Option<String>) -> Self {
        Self {
            max_tokens,
            messages: Vec::new(),
            system_prompt,
            cwd: ".".to_string(),
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

    pub fn update_latest_file(&mut self, path: String, content: String) {
        let tokens = estimate_tokens(&content);
        self.latest_files.insert(path, CachedFile {
            content,
            timestamp: std::time::Instant::now(),
            tokens,
        });
        self.evict_files_over_budget();
    }

    pub fn remove_latest_file(&mut self, path: &str) {
        self.latest_files.remove(path);
    }

    fn latest_files_token_total(&self) -> usize {
        self.latest_files.values().map(|f| f.tokens).sum()
    }

    // If the latest_files cache exceeds LATEST_FILES_BUDGET_FRACTION% of max_tokens,
    // evict the oldest files first until we're back under budget.
    fn evict_files_over_budget(&mut self) {
        let budget = self.max_tokens * LATEST_FILES_BUDGET_FRACTION / 100;
        if self.latest_files_token_total() <= budget {
            return;
        }
        // Collect paths sorted oldest→newest by timestamp
        let mut by_age: Vec<(std::time::Instant, String)> = self.latest_files
            .iter()
            .map(|(k, v)| (v.timestamp, k.clone()))
            .collect();
        by_age.sort_by_key(|(t, _)| *t);

        for (_, path) in by_age {
            if self.latest_files_token_total() <= budget {
                break;
            }
            self.latest_files.remove(&path);
        }
    }

    pub fn add_message(&mut self, role: &str, content: &str) {
        self.messages.push(Message {
            role: role.to_string(),
            content: content.to_string(),
            tool_calls: None,
        });
        self.trim_context();
    }

    pub fn add_message_raw(&mut self, msg: Message) {
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
    }

    pub fn get_messages(&self) -> Vec<Message> {
        self.messages.clone()
    }

    pub fn get_raw_prompt(&self) -> String {
        let mut prompt = String::from("<bos>");

        if let Some(sys) = &self.system_prompt {
            prompt.push_str("<|turn>system\n<|think|>\n");
            prompt.push_str(sys);
            prompt.push_str("<turn|>\n");
        }

        let mut current_turn_role = String::new();

        for msg in &self.messages {
            match msg.role.as_str() {
                "system" => {
                    if current_turn_role == "model" {
                        prompt.push_str("<turn|>\n");
                    }
                    prompt.push_str("<|turn>system\n<|think|>\n");
                    prompt.push_str(&msg.content);
                    prompt.push_str("<turn|>\n");
                    current_turn_role = String::new();
                }
                "user" => {
                    if current_turn_role == "model" {
                        prompt.push_str("<turn|>\n");
                    }
                    prompt.push_str("<|turn>user\n");
                    prompt.push_str(&msg.content);
                    prompt.push_str("<turn|>\n");
                    current_turn_role = String::new();
                }
                "assistant" => {
                    if current_turn_role != "model" {
                        prompt.push_str("<|turn>model\n");
                    }

                    let mut clean_content = msg.content.clone();

                    let thought_pairs = [
                        ("<|channel>thought", "<channel|>"),
                        ("<thought>", "</thought>"),
                        ("<think>", "</think>"),
                    ];

                    for (start_tag, end_tag) in thought_pairs {
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

                    clean_content = clean_content.replace("<|channel>text\n", "");
                    clean_content = clean_content.replace("<|channel>text", "");

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
                    if current_turn_role != "model" {
                        prompt.push_str("<|turn>model\n");
                    }
                    prompt.push_str(&msg.content);
                    current_turn_role = "model".to_string();
                }
                _ => {}
            }
        }

        if !self.latest_files.is_empty() {
            if current_turn_role == "model" {
                prompt.push_str("<turn|>\n");
            }
            prompt.push_str("<|turn>system\n<|think|>\n");
            prompt.push_str("<latest_files>\n");
            for (path, cached_file) in &self.latest_files {
                let mut safe_content = cached_file.content.clone();
                let tags_to_remove = [
                    "<turn|>", "<|turn>", "<|tool_call>", "<tool_call|>",
                    "<|tool_response>", "<tool_response|>", "<|channel>", "<channel|>",
                    "<thought>", "</thought>", "<think>", "</think>",
                    "<|\"|>", "<|\\\\\">", "<|\\\">", "<|\">", "<|'>", "<|'|>"
                ];
                for tag in tags_to_remove {
                    safe_content = safe_content.replace(tag, "");
                }
                prompt.push_str(&format!("File: `{}`\n```\n{}\n```\n", path, safe_content));
            }
            prompt.push_str("</latest_files>\n<turn|>\n");
            current_turn_role = String::new();
        }

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

        // Clean system prompt (strip <|tool>...<tool|> blocks and Gemma4 markers)
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
                    // Extract from stored format: <|tool_response>response:func{result:<|'|>RESULT<|'|>,tool_call_id:<|'|>ID<|'|>}...
                    let tc_id = Self::extract_delimited(&msg.content, "tool_call_id:<|'|>", "<|'|>")
                        .unwrap_or("unknown".into());
                    let result = Self::extract_delimited(&msg.content, "result:<|'|>", "<|'|>")
                        .unwrap_or(msg.content.clone());
                    msgs.push(gemma_chat::Message::tool_result(tc_id, result));
                }
                _ => {}
            }
        }
        if !self.latest_files.is_empty() {
            let mut files_content = String::from("<latest_files>\n");
            for (path, cached_file) in &self.latest_files {
                let mut safe_content = cached_file.content.clone();
                let tags_to_remove = [
                    "<turn|>", "<|turn>", "<|tool_call>", "<tool_call|>",
                    "<|tool_response>", "<tool_response|>", "<|channel>", "<channel|>",
                    "<thought>", "</thought>", "<think>", "</think>",
                    "<|\"|>", "<|\\\\\">", "<|\\\">", "<|\">", "<|'>", "<|'|>",
                ];
                for tag in tags_to_remove {
                    safe_content = safe_content.replace(tag, "");
                }
                files_content.push_str(&format!("File: `{}`\n```\n{}\n```\n", path, safe_content));
            }
            files_content.push_str("</latest_files>");
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
        let files_tokens = self.latest_files_token_total();
        // Usable budget for messages (headroom after latest_files)
        let usable = self.max_tokens.saturating_sub(files_tokens);

        // Fast path: estimate total message tokens cheaply before doing the full prompt build.
        let msg_estimate: usize = self.messages.iter().map(|m| estimate_tokens(&m.content)).sum();
        if msg_estimate <= usable {
            return;
        }

        // Identify the earliest index we can drop up to while keeping pairs intact
        // and never dropping system messages.
        //
        // Strategy: walk from the front, collecting "droppable units". A unit is:
        //   - a single non-system, non-tool message (user or assistant-without-tool-calls)
        //   - an (assistant-with-tool-calls) message AND all immediately following tool messages
        // We accumulate the cost of what we'd DROP, stopping when the remaining messages
        // fit within `usable`.
        let total_msg_tokens: usize = self.messages.iter().map(|m| estimate_tokens(&m.content)).sum();
        if total_msg_tokens <= usable {
            return;
        }

        let mut need_to_drop = total_msg_tokens.saturating_sub(usable);
        let mut cut = 0usize;
        let n = self.messages.len();
        let mut i = 0;

        while i < n && need_to_drop > 0 {
            let msg = &self.messages[i];

            // Never drop system messages
            if msg.role == "system" {
                i += 1;
                continue;
            }

            if msg.role == "assistant" && msg.tool_calls.is_some() {
                // Drop this assistant msg plus all immediately following tool msgs as a unit
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
                let _ = unit_start; // suppress warning
            } else if msg.role == "tool" {
                // Orphaned tool message (shouldn't normally happen) — drop it alone
                let cost = estimate_tokens(&msg.content);
                i += 1;
                cut = i;
                need_to_drop = need_to_drop.saturating_sub(cost);
            } else {
                // user or assistant-without-tool-calls — droppable unit
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
