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

#[derive(Clone, Debug)]
pub enum StreamEvent {
    Chunk(String),
    ToolCalls(Vec<ToolCall>),
    ToolResult(Option<String>, String, String, String), // (id, func_name, result, current_dir)
    ToolProgress(String),
    LoadProgress(f32, String),
    SessionLoaded(String, Vec<crate::app::RenderBlock>, Vec<crate::context::Message>),
    Done { completion_tokens: Option<u32>, prompt_tokens: Option<u32>, tg_per_s: Option<f64>, pp_per_s: Option<f64> },
    Error(String),
    DebugLog(String),
    TokenUpdate(u32, f64), // (count, ms)
}

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

    let req_body = gemma_chat::build_request(&model, &messages, &tools, 24576);

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

        let mut event_stream = match gemma_chat::stream_chat(&client, &base, &model, &messages, &tools, 24576).await {
            Ok(s) => s,
            Err(e) => {
                let _ = log_tx_spawn.send(StreamEvent::Error(e));
                let _ = log_tx_spawn.send(StreamEvent::Done { completion_tokens: None, prompt_tokens: None, tg_per_s: None, pp_per_s: None });
                return;
            }
        };

        let mut in_thought_mode = true;

        loop {
            let ev = tokio::select! {
                e = event_stream.next() => e,
                _ = token_spawn.cancelled() => None,
            };
            let Some(ev) = ev else { break; };

            match ev {
                gemma_chat::StreamEvent::ReasoningDelta(text) => {
                    let ms = request_start.elapsed().as_millis();
                    append_token(&prefix_clone, serde_json::json!({"c": text, "t": ms, "kind": "reasoning"}));
                    let _ = log_tx_spawn.send(StreamEvent::Chunk(text));
                }
                gemma_chat::StreamEvent::TextDelta(text) => {
                    if in_thought_mode {
                        in_thought_mode = false;
                        let ms = request_start.elapsed().as_millis();
                        append_token(&prefix_clone, serde_json::json!({"c": "</think>\n", "t": ms, "kind": "synthetic"}));
                        let _ = log_tx_spawn.send(StreamEvent::Chunk("</think>\n".to_string()));
                    }
                    let ms = request_start.elapsed().as_millis();
                    append_token(&prefix_clone, serde_json::json!({"c": text, "t": ms, "kind": "text"}));
                    let _ = log_tx_spawn.send(StreamEvent::Chunk(text));
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
    gemma_chat::complete(client, &base_url(&config.server_url), &config.model, &messages, 4096).await
}

pub async fn get_single_response(client: &Client, config: &Config, prompt: String, _images: Option<Vec<String>>, tx: Option<&mpsc::UnboundedSender<StreamEvent>>) -> Result<String, String> {
    let base = base_url(&config.server_url);
    if let Some(log_tx) = tx {
        let _ = log_tx.send(StreamEvent::DebugLog(format!("SINGLE_CALL_START|{}", base)));
    }
    let messages = vec![gemma_chat::Message::user(prompt)];
    gemma_chat::complete(client, &base, &config.model, &messages, 4096).await
}
