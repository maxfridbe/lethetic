use tiktoken_rs::cl100k_base;
use serde::{Deserialize, Serialize};
use serde_json::json;
use once_cell::sync::Lazy;
use std::cell::Cell;

static TOKENIZER: Lazy<tiktoken_rs::CoreBPE> = Lazy::new(|| cl100k_base().unwrap());

pub fn truncate_to_tokens(text: &str, max_tokens: usize) -> String {
    let tokens = TOKENIZER.encode_with_special_tokens(text);
    if tokens.len() <= max_tokens {
        return text.to_string();
    }
    TOKENIZER.decode(&tokens[..max_tokens]).unwrap_or_else(|_| text[..max_tokens.min(text.len())].to_string())
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

pub struct ContextManager {
    max_tokens: usize,
    messages: Vec<Message>,
    system_prompt: Option<String>,
    cwd: String,
    cached_token_count: Cell<Option<usize>>,
    pub latest_files: std::collections::HashMap<String, CachedFile>,
}

impl ContextManager {
    pub fn new(max_tokens: usize, system_prompt: Option<String>) -> Self {
        Self {
            max_tokens,
            messages: Vec::new(),
            system_prompt,
            cwd: ".".to_string(),
            cached_token_count: Cell::new(None),
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
        let tokens = TOKENIZER.encode_with_special_tokens(&content).len();
        self.latest_files.insert(path, CachedFile {
            content,
            timestamp: std::time::Instant::now(),
            tokens,
        });
        self.cached_token_count.set(None);
    }

    pub fn remove_latest_file(&mut self, path: &str) {
        self.latest_files.remove(path);
        self.cached_token_count.set(None);
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
            "<|tool_response|>response:{}{{result:<|\"|>{}<|\"|>,tool_call_id:<|\"|>{}<|\"|>}}<tool_response|><turn|>",
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
                            let call_json = json!({
                                "name": tc.function.name,
                                "arguments": tc.function.arguments
                            });
                            prompt.push_str(&format!("<|tool_call>{}{}", serde_json::to_string(&call_json).unwrap_or_default(), "<tool_call|>"));
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
                    "<|\"|>", "<|\\\\\">", "<|\\\">", "<|\">", "<|'>"
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

    pub fn get_token_count(&self) -> usize {
        if let Some(count) = self.cached_token_count.get() {
            return count;
        }
        let count = TOKENIZER.encode_with_special_tokens(&self.get_raw_prompt()).len();
        self.cached_token_count.set(Some(count));
        count
    }

    fn trim_context(&mut self) {
        self.cached_token_count.set(None);
        while self.get_token_count() > self.max_tokens && !self.messages.is_empty() {
            self.messages.remove(0);
            self.cached_token_count.set(None);
        }
    }
}
