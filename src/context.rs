use tiktoken_rs::cl100k_base;
use serde::{Deserialize, Serialize};
use once_cell::sync::Lazy;

static TOKENIZER: Lazy<tiktoken_rs::CoreBPE> = Lazy::new(|| cl100k_base().unwrap());

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
        });
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
            "<|tool_response|>response:{}{{result:<|\">{}<|\">,tool_call_id:<|\">{}<|\">}}<tool_response|><turn|>", 
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
                        if !msg.content.contains("<|channel>") {
                            prompt.push_str("<|channel>thought\n<channel|>");
                        }
                    }
                    prompt.push_str(&msg.content);
                    
                    if let Some(calls) = &msg.tool_calls {
                        for tc in calls {
                            prompt.push_str(&format!("<|tool_call>call:{}{{", tc.function.name));
                            if let Some(obj) = tc.function.arguments.as_object() {
                                let mut first = true;
                                for (k, v) in obj {
                                    if !first { prompt.push(','); }
                                    first = false;
                                    prompt.push_str(k);
                                    prompt.push(':');
                                    if let Some(s) = v.as_str() {
                                        prompt.push_str(&format!("<|\">{}<|\">", s));
                                    } else {
                                        prompt.push_str(&v.to_string());
                                    }
                                }
                            }
                            prompt.push_str("}<tool_call|>");
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
                        current_turn_role = "model".to_string();
                    }
                    prompt.push_str(&msg.content);
                    
                    // The tool response now ends with <turn|>, so the model turn is closed.
                    // Commented out as requested:
                    // current_turn_role = String::new();
                }
                _ => {}
            }
        }

        if current_turn_role != "model" {
            prompt.push_str("<|turn>model\n<|channel>thought\n<channel|>");
        }
        
        prompt
    }

    pub fn get_token_count(&self) -> usize {
        TOKENIZER.encode_with_special_tokens(&self.get_raw_prompt()).len()
    }

    fn trim_context(&mut self) {
        while self.get_token_count() > self.max_tokens && !self.messages.is_empty() {
            self.messages.remove(0);
        }
    }
}
