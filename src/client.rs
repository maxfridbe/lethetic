use serde::{Deserialize, Serialize};
use serde_json::json;
use reqwest::Client;
use std::fs;
use std::io::Write;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use futures_util::StreamExt;
use crate::context::{ContextManager, Message, ToolCall};
use crate::tools::get_standard_tools;
use crate::config::Config;

#[derive(Serialize, Deserialize, Debug)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub tools: Vec<crate::tools::Tool>,
    pub stream: bool,
    pub options: serde_json::Value,
}

#[derive(Deserialize, Debug)]
pub struct ChatResponse {
    pub message: ResponseMessage,
    pub done: bool,
    pub eval_count: Option<u32>,
    pub eval_duration: Option<u64>,
}

#[derive(Deserialize, Debug)]
pub struct ResponseMessage {
    pub role: String,
    pub content: String,
    pub tool_calls: Option<Vec<ToolCall>>,
}

#[derive(Debug, Clone)]
pub enum StreamEvent {
    Chunk(String),
    DebugLog(String),
    ToolCalls(Vec<ToolCall>),
    ToolResult(Option<String>, String),
    Done(Option<u32>, Option<u64>),
    Error(String),
}

pub async fn unload_model(client: &Client, server_url: &str, model: &str) {
    let _ = client.post(format!("{}/api/generate", server_url))
        .json(&json!({ "model": model, "keep_alive": 0 }))
        .send()
        .await;
}

pub fn trigger_llm_request(client: Client, config: Config, context_manager: &ContextManager, tx: mpsc::UnboundedSender<StreamEvent>, token: CancellationToken, is_debug: bool) {
    let messages = context_manager.get_messages();
    let req = ChatRequest {
        model: config.model.clone(),
        messages,
        tools: get_standard_tools(),
        stream: true,
        options: json!({ 
            "num_ctx": config.context_size
        }),
    };

    let log_tx = tx.clone();
    let server_url = config.server_url.clone();
    let ctx_len = context_manager.get_token_count();

    let req_id = chrono::Local::now().format("%Y%m%d_%H%M%S_%f").to_string();
    let sep = format!("\n//-------------{}-------------------------------------------\n", req_id);

    let prefix = if is_debug {
        let _ = fs::create_dir_all(".lethetic");
        ".lethetic/".to_string()
    } else {
        "".to_string()
    };

    if let Ok(full_req_json) = serde_json::to_string_pretty(&req) {
        let _ = fs::write(format!("{}last_context", prefix), &full_req_json);
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(format!("{}requests", prefix)) {
            let _ = write!(file, "{}{}", sep, full_req_json);
        }
    }
    let _ = fs::write(format!("{}last-response", prefix), "");
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(format!("{}responses", prefix)) {
        let _ = write!(file, "{}", sep);
    }

    let b_url = config.server_url.clone();
    let prefix_clone = prefix.clone();
    tokio::spawn(async move {
        let _ = log_tx.send(StreamEvent::DebugLog(format!("CALL_START|{}|{}", server_url, ctx_len)));
        let res_res = client.post(&b_url).json(&req).send().await;
        match res_res {
            Ok(res) => {
                let mut stream = res.bytes_stream();
                let mut buffer = String::new();
                while let Some(item) = tokio::select! {
                    _ = token.cancelled() => {
                        let _ = log_tx.send(StreamEvent::Done(None, None));
                        None
                    },
                    chunk = stream.next() => chunk,
                } {
                    if let Ok(bytes) = item {
                        if let Ok(chunk_str) = String::from_utf8(bytes.to_vec()) {
                            buffer.push_str(&chunk_str);
                            while let Some(pos) = buffer.find('\n') {
                                if token.is_cancelled() { return; }
                                let line = buffer.drain(..=pos).collect::<String>();
                                if line.trim().is_empty() { continue; }
                                
                                if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(format!("{}responses.jsonl", prefix_clone)) {
                                    let _ = write!(file, "{}", line);
                                }

                                match serde_json::from_str::<ChatResponse>(&line) {
                                    Ok(chat_res) => {
                                        if let Some(calls) = chat_res.message.tool_calls {
                                            if !calls.is_empty() { 
                                                if token.is_cancelled() { return; }
                                                let _ = log_tx.send(StreamEvent::DebugLog(format!("NATIVE_TOOL_CALLS|count:{}", calls.len())));
                                                let _ = log_tx.send(StreamEvent::ToolCalls(calls)); 
                                            }
                                        }
                                        if !chat_res.message.content.is_empty() {
                                            let content = chat_res.message.content.clone();
                                            if !token.is_cancelled() {
                                                // Log ONLY the content string to 'responses'
                                                if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(format!("{}responses", prefix_clone)) {
                                                    let _ = write!(file, "{}", content);
                                                }

                                                let _ = fs::OpenOptions::new().create(true).append(true).open(format!("{}last-response", prefix_clone))
                                                    .and_then(|mut f| write!(f, "{}", content));
                                                let _ = log_tx.send(StreamEvent::Chunk(content)); 
                                            }
                                        }
                                        if chat_res.done {
                                            if !token.is_cancelled() {
                                                let _ = log_tx.send(StreamEvent::Done(chat_res.eval_count, chat_res.eval_duration));
                                            }
                                            return;
                                        }
                                    }
                                    Err(e) => {
                                        if !token.is_cancelled() {
                                            let _ = log_tx.send(StreamEvent::DebugLog(format!("PARSE_ERR|{}|{}", e, line)));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => { let _ = log_tx.send(StreamEvent::Error(e.to_string())); }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn test_ollama_integration_from_last_context() {
        let config_content = fs::read_to_string("config.yml").expect("Need config.yml");
        let config: Config = serde_yaml::from_str(&config_content).expect("Valid config");

        let last_context_content = fs::read_to_string(".last_context").unwrap_or_else(|_| "{\"model\":\"\",\"messages\":[],\"tools\":[],\"stream\":true,\"options\":{}}".to_string());
        let req: ChatRequest = serde_json::from_str(&last_context_content).unwrap_or(ChatRequest {
            model: config.model.clone(),
            messages: vec![],
            tools: vec![],
            stream: true,
            options: json!({}),
        });

        let client = Client::new();
        let res = client.post(&config.server_url)
            .json(&req)
            .timeout(Duration::from_secs(30))
            .send()
            .await;

        match res {
            Ok(resp) => {
                assert!(resp.status().is_success(), "Ollama should return 200 OK");
            }
            Err(e) => println!("Integration Test: Skipping real connection check: {}", e),
        }
    }
}
