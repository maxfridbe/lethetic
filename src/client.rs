use serde::{Deserialize, Serialize};
use serde_json::json;
use reqwest::Client;
use std::fs;
use std::io::Write;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use futures_util::StreamExt;
use crate::context::{ContextManager, ToolCall};
use crate::config::Config;

#[derive(Serialize, Deserialize, Debug)]
pub struct GenerateRequest {
    pub model: String,
    pub prompt: String,
    pub raw: bool,
    pub stream: bool,
    pub options: serde_json::Value,
}

#[derive(Deserialize, Debug)]
pub struct GenerateResponse {
    #[serde(alias = "content")]
    pub response: String,
    #[serde(alias = "stop")]
    pub done: bool,
    pub eval_count: Option<u32>,
    pub eval_duration: Option<u64>,
}

#[derive(Debug, Clone)]
pub enum StreamEvent {
    Chunk(String),
    DebugLog(String),
    ToolCalls(Vec<ToolCall>),
    ToolResult(Option<String>, String, String), // id, function_name, result
    Done(Option<u32>, Option<u64>),
    Error(String),
}

pub fn trigger_llm_request(client: Client, config: Config, context_manager: &ContextManager, tx: mpsc::UnboundedSender<StreamEvent>, token: CancellationToken, is_debug: bool) {
    let raw_prompt = context_manager.get_raw_prompt();
    let mut req_body = json!({
        "model": config.model.clone(),
        "prompt": raw_prompt.clone(),
        "raw": true,
        "stream": true,
        "temperature": 1.0,
        "stop": ["<turn|>", "<eos>", "<tool_response|>", "<|tool_response|>", "<tool_call|>", "<|tool_call|>"],
        "num_ctx": config.context_size,
    });

    // If it's Ollama, move some things into options
    if config.server_url.contains("/api/chat") {
        req_body = json!({
            "model": config.model.clone(),
            "prompt": raw_prompt.clone(),
            "raw": true,
            "stream": true,
            "options": {
                "num_ctx": config.context_size,
                "stop": ["<turn|>", "<eos>", "<tool_response|>", "<|tool_response|>", "<tool_call|>", "<|tool_call|>"]
            }
        });
    }

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

    if let Ok(full_req_json) = serde_json::to_string_pretty(&req_body) {
        let _ = fs::write(format!("{}last_context", prefix), &full_req_json);
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(format!("{}requests", prefix)) {
            let _ = write!(file, "{}{}", sep, full_req_json);
        }
    }
    let _ = fs::write(format!("{}last-response", prefix), "");
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(format!("{}responses.jsonl", prefix)) {
        let _ = write!(file, "{}", sep);
    }

    let b_url = config.server_url.replace("/api/chat", "/api/generate");
    let prefix_clone = prefix.clone();
    tokio::spawn(async move {
        let _ = log_tx.send(StreamEvent::DebugLog(format!("CALL_START|{}|{}", server_url, ctx_len)));
        let res_res = client.post(&b_url).json(&req_body).send().await;
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
                                let trimmed = line.trim();
                                if trimmed.is_empty() { continue; }
                                
                                let json_str = if trimmed.starts_with("data: ") {
                                    &trimmed[6..]
                                } else {
                                    trimmed
                                };

                                if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(format!("{}responses.jsonl", prefix_clone)) {
                                    let _ = write!(file, "{}", json_str);
                                }

                                match serde_json::from_str::<GenerateResponse>(json_str) {
                                    Ok(gen_res) => {
                                        if !gen_res.response.is_empty() {
                                            let content = gen_res.response.clone();
                                            if !token.is_cancelled() {
                                                if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(format!("{}responses", prefix_clone)) {
                                                    let _ = write!(file, "{}", content);
                                                }
                                                let _ = fs::OpenOptions::new().create(true).append(true).open(format!("{}last-response", prefix_clone))
                                                    .and_then(|mut f| write!(f, "{}", content));
                                                let _ = log_tx.send(StreamEvent::Chunk(content)); 
                                            }
                                        }
                                        if gen_res.done {
                                            if !token.is_cancelled() {
                                                let _ = log_tx.send(StreamEvent::Done(gen_res.eval_count, gen_res.eval_duration));
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
