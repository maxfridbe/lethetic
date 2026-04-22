use tiktoken_rs::cl100k_base;
use serde::{Deserialize, Serialize};
use once_cell::sync::Lazy;

static TOKENIZER: Lazy<tiktoken_rs::CoreBPE> = Lazy::new(|| cl100k_base().unwrap());

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Message {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub tool_type: Option<String>,
    pub function: FunctionCall,
    pub index: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: serde_json::Value,
}

pub struct ContextManager {
    max_tokens: usize,
    messages: Vec<Message>,
    system_prompt: Option<String>,
}

impl ContextManager {
    pub fn new(max_tokens: usize, system_prompt: Option<String>) -> Self {
        Self {
            max_tokens,
            messages: Vec::new(),
            system_prompt,
        }
    }

    pub fn update_system_prompt(&mut self, prompt: String) {
        self.system_prompt = Some(prompt);
    }

    pub fn add_message(&mut self, role: &str, content: &str) {
        self.messages.push(Message {
            role: role.to_string(),
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: None,
        });
        self.trim_context();
    }

    pub fn add_assistant_tool_call(&mut self, content: &str, tool_calls: Option<Vec<ToolCall>>) {
        self.messages.push(Message {
            role: "assistant".to_string(),
            content: content.to_string(),
            tool_calls,
            tool_call_id: None,
        });
        self.trim_context();
    }

    pub fn add_tool_result(&mut self, tool_call_id: String, content: &str) {
        self.messages.push(Message {
            role: "tool".to_string(),
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: Some(tool_call_id),
        });
        self.trim_context();
    }

    pub fn clear(&mut self) {
        self.messages.clear();
    }

    pub fn get_messages(&self) -> Vec<Message> {
        let mut all_messages = Vec::new();
        if let Some(sys) = &self.system_prompt {
            all_messages.push(Message {
                role: "system".to_string(),
                content: sys.clone(),
                tool_calls: None,
                tool_call_id: None,
            });
        }
        all_messages.extend(self.messages.clone());
        all_messages
    }

    pub fn get_token_count(&self) -> usize {
        let mut total = 0;
        if let Some(sys) = &self.system_prompt { total += TOKENIZER.encode_with_special_tokens(sys).len(); }
        for msg in &self.messages {
            total += TOKENIZER.encode_with_special_tokens(&msg.content).len();
            if let Some(_tool_calls) = &msg.tool_calls { total += 50; } // Overhead estimate
            if let Some(id) = &msg.tool_call_id { total += TOKENIZER.encode_with_special_tokens(id).len(); }
        }
        total
    }

    fn trim_context(&mut self) {
        while self.get_token_count() > self.max_tokens && !self.messages.is_empty() {
            self.messages.remove(0);
        }
    }
}
