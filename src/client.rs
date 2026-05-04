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

#[derive(Clone, Debug)]
pub enum StreamEvent {
    Chunk(String),
    ToolCalls(Vec<ToolCall>),
    ToolResult(Option<String>, String, String, String), // (id, func_name, result, current_dir)
    ToolProgress(String),
    LoadProgress(f32, String),
    SessionLoaded(String, Vec<crate::app::RenderBlock>, Vec<crate::context::Message>),
    Done(Option<u32>, Option<u64>),
    Error(String),
    DebugLog(String),
    TokenUpdate(u32, f64), // (count, ms)
}

pub fn trigger_llm_request(client: Client, config: Config, context_manager: &ContextManager, tx: mpsc::UnboundedSender<StreamEvent>, token: CancellationToken, _is_debug: bool, session_dir: Option<String>) {
    let raw_prompt = context_manager.get_raw_prompt();
    
    let req_body = json!({
        "model": config.model.clone(),
        "input": raw_prompt.clone(),
        "stream": true,
        "max_tokens": 16384,
    });

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

    let b_url = config.server_url.clone();

    let prefix_clone = prefix.clone();
    
    let log_tx_spawn = log_tx.clone();
    let client_spawn = client.clone();
    let b_url_spawn = b_url.clone();
    let req_body_spawn = req_body.clone();
    let token_spawn = token.clone();
    let server_url_spawn = server_url.clone();

    tokio::spawn(async move {
        let _ = log_tx_spawn.send(StreamEvent::DebugLog(format!("CALL_START|{}|{}", server_url_spawn, ctx_len)));
        let res_res = client_spawn.post(&b_url_spawn).json(&req_body_spawn).send().await;
        match res_res {
            Ok(res) => {
                if !res.status().is_success() {
                    let status = res.status();
                    let body = res.text().await.unwrap_or_else(|_| "Failed to read error body".to_string());
                    let _ = log_tx_spawn.send(StreamEvent::Error(format!("Server returned {}: {}", status, body)));
                    let _ = log_tx_spawn.send(StreamEvent::Done(None, None));
                    return;
                }

                let mut stream = res.bytes_stream();
                let mut buffer = String::new();
                let mut current_event = String::new();
                let mut in_thought_mode = true;

                while let Some(item) = tokio::select! {
                    i = stream.next() => i,
                    _ = token_spawn.cancelled() => None,
                } {
                    if let Ok(bytes) = item {
                        if let Ok(chunk_str) = String::from_utf8(bytes.to_vec()) {
                            buffer.push_str(&chunk_str);
                            while let Some(pos) = buffer.find('\n') {
                                let line = buffer.drain(..=pos).collect::<String>();
                                let trimmed = line.trim();
                                if trimmed.is_empty() { continue; }
                                
                                if trimmed.starts_with("event: ") {
                                    current_event = trimmed[7..].to_string();
                                } else if trimmed.starts_with("data: ") {
                                    let json_str = &trimmed[6..];
                                    if json_str == "[DONE]" { break; }

                                    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(format!("{}/responses.jsonl", prefix_clone)) {
                                        let _ = write!(file, "{}", json_str);
                                    }

                                    let val: serde_json::Value = match serde_json::from_str(json_str) {
                                        Ok(v) => v,
                                        Err(_) => continue,
                                    };

                                    let mut delta_text = None;
                                    let mut is_reasoning = false;
                                    let mut is_done = false;

                                    if current_event == "response.reasoning_text.delta" {
                                        delta_text = val["delta"].as_str().map(|s| s.to_string());
                                        is_reasoning = true;
                                    } else if current_event == "response.output_text.delta" {
                                        delta_text = val["delta"].as_str().map(|s| s.to_string());
                                    } else if current_event == "response.completed" {
                                        is_done = true;
                                    } else if let Some(content) = val["content"].as_str() {
                                        delta_text = Some(content.to_string());
                                        if val["stop"].as_bool() == Some(true) {
                                            is_done = true;
                                        }
                                    } else if let Some(choices) = val["choices"].as_array() {
                                        if !choices.is_empty() {
                                            if let Some(delta) = choices[0]["delta"]["content"].as_str() {
                                                delta_text = Some(delta.to_string());
                                            } else if let Some(delta) = choices[0]["delta"]["reasoning_content"].as_str() {
                                                delta_text = Some(delta.to_string());
                                                is_reasoning = true;
                                            }
                                        }
                                        if val["choices"][0]["finish_reason"].is_string() && val["choices"][0]["finish_reason"].as_str() != Some("null") {
                                            is_done = true;
                                        }
                                    } else if val["stop"].as_bool() == Some(true) {
                                        is_done = true;
                                    }

                                    if let Some(delta) = delta_text {
                                        if is_reasoning {
                                            let _ = log_tx_spawn.send(StreamEvent::Chunk(delta));
                                        } else {
                                            if in_thought_mode {
                                                in_thought_mode = false;
                                                let _ = log_tx_spawn.send(StreamEvent::Chunk("</think>\n".to_string()));
                                            }
                                            let _ = log_tx_spawn.send(StreamEvent::Chunk(delta.clone()));
                                            if !token_spawn.is_cancelled() {
                                                if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(format!("{}/responses", prefix_clone)) {
                                                    let _ = write!(file, "{}", delta);
                                                }
                                                if let Ok(mut file) = std::fs::OpenOptions::new().create(true).open(format!("{}/last-response", prefix_clone)) {
                                                    let _ = write!(file, "{}", delta);
                                                }
                                            }
                                        }
                                    }

                                    if is_done {
                                        let mut eval_count = None;
                                        if let Some(usage) = val["response"]["usage"].as_object() {
                                            eval_count = usage.get("output_tokens").and_then(|v| v.as_u64()).map(|v| v as u32);
                                        } else if let Some(tokens_predicted) = val["tokens_predicted"].as_u64() {
                                            eval_count = Some(tokens_predicted as u32);
                                        } else if let Some(usage) = val["usage"].as_object() {
                                            eval_count = usage.get("completion_tokens").and_then(|v| v.as_u64()).map(|v| v as u32);
                                        }
                                        let _ = log_tx_spawn.send(StreamEvent::Done(eval_count, None));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                let _ = log_tx_spawn.send(StreamEvent::Error(format!("NETWORK_ERROR: {}", e)));
            }
        }
    });
}

pub async fn summarize_llm(client: &reqwest::Client, config: &Config, context: &str, prompt: &str) -> Result<String, String> {
    let truncated_context = crate::context::truncate_to_tokens(context, 160000);
    let raw_prompt = format!("{}\n\nContext to summarize:\n{}", prompt, truncated_context);
    
    let req_body = json!({
        "model": config.model.clone(),
        "input": raw_prompt,
        "stream": false,
    });
    
    let b_url = config.server_url.clone();

    let res = client.post(&b_url).json(&req_body).send().await.map_err(|e| e.to_string())?;
    
    if !res.status().is_success() {
        return Err(format!("Server returned error: {}", res.status()));
    }
    
    let json: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    
    // Non-streaming response format for /v1/responses might be different
    // Let's assume it has choices or a direct output
    let text = json["output"][0]["content"][0]["text"].as_str()
        .or_else(|| json["response"]["output"][0]["content"][0]["text"].as_str())
        .or_else(|| json["choices"][0]["message"]["content"].as_str())
        .unwrap_or("")
        .to_string();
        
    Ok(text)
}


pub async fn get_single_response(client: &Client, config: &Config, prompt: String, images: Option<Vec<String>>, tx: Option<&mpsc::UnboundedSender<StreamEvent>>) -> Result<String, String> {
    let b_url = config.server_url.clone();

    let req_body = json!({
        "model": config.model.clone(),
        "input": prompt,
        "stream": false,
        "images": images
    });

    if let Some(log_tx) = tx {
        let _ = log_tx.send(StreamEvent::DebugLog(format!("SINGLE_CALL_START|{}", b_url)));
    }

    let res = client.post(&b_url).json(&req_body).send().await.map_err(|e| e.to_string())?;
    
    if !res.status().is_success() {
        return Err(format!("Server returned error: {}", res.status()));
    }
    
    let json: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    
    let text = json["output"][0]["content"][0]["text"].as_str()
        .or_else(|| json["response"]["output"][0]["content"][0]["text"].as_str())
        .or_else(|| json["choices"][0]["message"]["content"].as_str())
        .unwrap_or("")
        .to_string();
        
    Ok(text)
}
