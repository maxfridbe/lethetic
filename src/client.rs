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

#[derive(Serialize, Deserialize, Debug, Clone)]
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
    pub tokens_predicted: Option<u32>,
    pub timings: Option<Timings>,
}

#[derive(Deserialize, Debug)]
pub struct Timings {
    pub predicted_ms: Option<f64>,
}

#[derive(Clone, Debug)]
pub enum StreamEvent {
    Chunk(String),
    ToolCalls(Vec<ToolCall>),
    ToolResult(Option<String>, String, String, String), // (id, func_name, result, current_dir)
    Done(Option<u32>, Option<u64>),
    Error(String),
    DebugLog(String),
    TokenUpdate(u32, f64), // (count, ms)
}

pub fn trigger_llm_request(client: Client, config: Config, context_manager: &ContextManager, tx: mpsc::UnboundedSender<StreamEvent>, token: CancellationToken, _is_debug: bool, session_dir: Option<String>) {
    let raw_prompt = context_manager.get_raw_prompt();
    
    let mut req_body = json!({
        "model": config.model.clone(),
        "prompt": raw_prompt.clone(),
        "raw": true,
        "stream": true,
        "temperature": 1.0,
        "stop": ["<turn|>", "<eos>", "<tool_response|>", "<|tool_response|>"],
        "num_ctx": config.context_size,
    });

    if config.server_url.contains("/api/chat") {
        req_body = json!({
            "model": config.model.clone(),
            "prompt": raw_prompt.clone(),
            "raw": true,
            "stream": true,
            "options": {
                "num_ctx": config.context_size,
                "stop": ["<turn|>", "<eos>", "<tool_response|>", "<|tool_response|>"]
            }
        });
    }

    let log_tx = tx.clone();
    let server_url = config.server_url.clone();
    let ctx_len = context_manager.get_token_count();

    let req_id = chrono::Local::now().format("%Y%m%d_%H%M%S_%f").to_string();
    let sep = format!("\n//-------------{}-------------------------------------------\n", req_id);

    let prefix = session_dir.clone().unwrap_or_else(|| ".lethetic/".to_string());
    let _ = fs::create_dir_all(&prefix);

    if let Ok(full_req_json) = serde_json::to_string_pretty(&req_body) {
        let _ = fs::write(format!("{}/last_context", prefix), &full_req_json);
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(format!("{}/requests", prefix)) {
            let _ = write!(file, "{}{}", sep, full_req_json);
        }
    }
    let _ = fs::write(format!("{}/last-response", prefix), "");
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(format!("{}/responses.jsonl", prefix)) {
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
                    i = stream.next() => i,
                    _ = token.cancelled() => None,
                } {
                    if let Ok(bytes) = item {
                        if let Ok(chunk_str) = String::from_utf8(bytes.to_vec()) {
                            buffer.push_str(&chunk_str);
                            while let Some(pos) = buffer.find('\n') {
                                let line = buffer.drain(..=pos).collect::<String>();
                                let trimmed = line.trim();
                                if trimmed.is_empty() { continue; }
                                
                                let json_str = if trimmed.starts_with("data: ") {
                                    &trimmed[6..]
                                } else {
                                    trimmed
                                };

                                if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(format!("{}/responses.jsonl", prefix_clone)) {
                                    let _ = write!(file, "{}", json_str);
                                }

                                match serde_json::from_str::<GenerateResponse>(json_str) {
                                    Ok(gen_res) => {
                                        if !gen_res.response.is_empty() {
                                            let _ = log_tx.send(StreamEvent::Chunk(gen_res.response.clone()));
                                            if !token.is_cancelled() {
                                                if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(format!("{}/responses", prefix_clone)) {
                                                    let _ = write!(file, "{}", gen_res.response);
                                                }
                                                if let Ok(mut file) = std::fs::OpenOptions::new().create(true).open(format!("{}/last-response", prefix_clone)) {
                                                    let _ = write!(file, "{}", gen_res.response);
                                                }
                                            }
                                        }

                                        // Update tokens predicted and speed info
                                        if let (Some(count), Some(timings)) = (gen_res.tokens_predicted, gen_res.timings) {
                                            if let Some(ms) = timings.predicted_ms {
                                                let _ = log_tx.send(StreamEvent::TokenUpdate(count, ms));
                                            }
                                        }

                                        if gen_res.done {
                                            let _ = log_tx.send(StreamEvent::Done(gen_res.eval_count, gen_res.eval_duration));
                                            return;
                                        }
                                    }
                                    Err(_) => {
                                        // Potential tool call or malformed JSON
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
