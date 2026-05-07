use futures_util::StreamExt;
use reqwest::Client;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use crate::types::{Message, StreamEvent, ToolDefinition};
use crate::stream::StreamParser;

/// Build the request body for `/v1/chat/completions`.
pub fn build_request(
    model: &str,
    messages: &[Message],
    tools: &[ToolDefinition],
    max_tokens: u32,
) -> Value {
    let mut body = json!({
        "model": model,
        "messages": messages,
        "max_tokens": max_tokens,
        "stream": true,
    });
    if !tools.is_empty() {
        body["tools"] = json!(tools);
    }
    body
}

/// Stream a chat completion request, yielding `StreamEvent`s via a channel-backed stream.
pub async fn stream_chat(
    client: &Client,
    base_url: &str,
    model: &str,
    messages: &[Message],
    tools: &[ToolDefinition],
    max_tokens: u32,
) -> Result<UnboundedReceiverStream<StreamEvent>, String> {
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let body = build_request(model, messages, tools, max_tokens);

    let response = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(format!("Server {status}: {text}"));
    }

    let (tx, rx) = mpsc::unbounded_channel();
    let mut byte_stream = response.bytes_stream();

    tokio::spawn(async move {
        let mut parser = StreamParser::new();
        let mut buffer = String::new();
        let mut done_sent = false;

        while let Some(chunk) = byte_stream.next().await {
            match chunk {
                Err(e) => {
                    let _ = tx.send(StreamEvent::Error(format!("Stream error: {e}")));
                    break;
                }
                Ok(bytes) => {
                    match std::str::from_utf8(&bytes) {
                        Err(_) => continue,
                        Ok(s) => buffer.push_str(s),
                    }
                    while let Some(pos) = buffer.find('\n') {
                        let line: String = buffer.drain(..=pos).collect();
                        for event in parser.process_line(line.trim()) {
                            if matches!(event, StreamEvent::Done { .. }) {
                                done_sent = true;
                            }
                            let _ = tx.send(event);
                        }
                    }
                }
            }
        }

        for event in parser.flush() {
            let _ = tx.send(event);
        }
        if !done_sent {
            let _ = tx.send(StreamEvent::Done { completion_tokens: None, prompt_tokens: None, tg_per_s: None, pp_per_s: None });
        }
    });

    Ok(UnboundedReceiverStream::new(rx))
}

/// Send a single non-streaming chat completion and return the assistant's text.
pub async fn complete(
    client: &Client,
    base_url: &str,
    model: &str,
    messages: &[Message],
    max_tokens: u32,
) -> Result<String, String> {
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let mut body = build_request(model, messages, &[], max_tokens);
    body["stream"] = serde_json::Value::Bool(false);

    let res = client.post(&url).json(&body).send().await.map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        return Err(format!("Server {status}: {text}"));
    }
    let json: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    Ok(json["choices"][0]["message"]["content"].as_str().unwrap_or("").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn build_request_no_tools() {
        let msgs = vec![Message::user("hello")];
        let body = build_request("model-x", &msgs, &[], 100);
        assert_eq!(body["model"], "model-x");
        assert_eq!(body["stream"], true);
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn build_request_with_tools() {
        let msgs = vec![Message::user("ls the dir")];
        let tools = vec![ToolDefinition::new(
            "run_shell_command",
            "Run a bash command",
            json!({"type":"object","properties":{"command":{"type":"string"}},"required":["command"]}),
        )];
        let body = build_request("gemma", &msgs, &tools, 512);
        let t = &body["tools"][0];
        assert_eq!(t["type"], "function");
        assert_eq!(t["function"]["name"], "run_shell_command");
    }

    #[test]
    fn message_serialization() {
        let m = Message::tool_result("call-1", "EXIT_CODE: 0\nresult");
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["role"], "tool");
        assert_eq!(v["tool_call_id"], "call-1");
        assert_eq!(v["content"], "EXIT_CODE: 0\nresult");
    }

    #[test]
    fn build_request_messages_serialized() {
        let msgs = vec![
            Message::system("You are helpful"),
            Message::user("hi"),
        ];
        let body = build_request("m", &msgs, &[], 50);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["role"], "user");
    }
}
