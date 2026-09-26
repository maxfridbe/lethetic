use crate::stream::StreamParser;
use crate::types::{Completion, Message, StreamEvent, ToolDefinition, Usage};
use futures_util::{Stream, StreamExt};
use reqwest::Client;
use serde_json::{Value, json};
use std::pin::Pin;
use std::time::Duration;

#[cfg(not(test))]
const TERMINAL_TAIL_TIMEOUT: Duration = Duration::from_secs(15);
#[cfg(test)]
const TERMINAL_TAIL_TIMEOUT: Duration = Duration::from_millis(250);

pub const RESERVED_EXTRA_BODY_FIELDS: &[&str] = &[
    "model",
    "messages",
    "system",
    "max_tokens",
    "max_completion_tokens",
    "stream",
    "stream_options",
    "tools",
    "tool_choice",
    "parallel_tool_calls",
    "functions",
    "function_call",
    "mcp_servers",
    "n",
    "thinking",
];

pub fn validate_extra_body(extra_body: Option<&Value>) -> Result<(), String> {
    let Some(extra) = extra_body else {
        return Ok(());
    };
    let object = extra
        .as_object()
        .ok_or_else(|| "extra_body must be a JSON object".to_string())?;
    if let Some(key) = object
        .keys()
        .find(|key| RESERVED_EXTRA_BODY_FIELDS.contains(&key.as_str()))
    {
        return Err(format!(
            "extra_body cannot override host-owned request field `{key}`"
        ));
    }
    Ok(())
}

/// Build the request body for `/v1/chat/completions`.
pub fn build_request(
    model: &str,
    messages: &[Message],
    tools: &[ToolDefinition],
    max_tokens: u32,
    thinking: Option<bool>,
    extra_body: Option<&serde_json::Value>,
) -> Result<Value, String> {
    validate_extra_body(extra_body)?;
    let mut body = json!({
        "model": model,
        "messages": messages,
        "max_tokens": max_tokens,
        "stream": true,
        "stream_options": {
            "include_usage": true
        },
        // llama.cpp-compatible servers only emit per-chunk `timings`
        // (prompt/predicted tokens-per-second) when this is set.
        "timings_per_token": true
    });
    if !tools.is_empty() {
        body["tools"] = json!(tools);
        body["parallel_tool_calls"] = json!(false);
    }
    if let Some(true) = thinking {
        body["thinking"] = json!({
            "type": "enabled"
        });
    }
    if let Some(extra_map) = extra_body.and_then(Value::as_object)
        && let Some(body_map) = body.as_object_mut()
    {
        for (key, value) in extra_map {
            body_map.insert(key.clone(), value.clone());
        }
    }
    Ok(body)
}

/// Stream a chat completion request while retaining ownership of the HTTP body.
pub async fn stream_chat(
    client: &Client,
    base_url: &str,
    model: &str,
    messages: &[Message],
    tools: &[ToolDefinition],
    max_tokens: u32,
    api_key: Option<&str>,
    thinking: Option<bool>,
    extra_body: Option<&serde_json::Value>,
) -> Result<Pin<Box<dyn Stream<Item = StreamEvent> + Send>>, String> {
    let body = build_request(model, messages, tools, max_tokens, thinking, extra_body)?;
    stream_chat_with_body(client, base_url, api_key, body).await
}

/// Stream one already-validated chat-completions request body.
pub async fn stream_chat_with_body(
    client: &Client,
    base_url: &str,
    api_key: Option<&str>,
    body: Value,
) -> Result<Pin<Box<dyn Stream<Item = StreamEvent> + Send>>, String> {
    let encoded = serde_json::to_vec(&body)
        .map_err(|error| format!("Could not encode chat request body: {error}"))?;
    stream_chat_with_encoded_body(client, base_url, api_key, encoded).await
}

/// Stream one already-validated, already-encoded chat-completions request body.
pub async fn stream_chat_with_encoded_body(
    client: &Client,
    base_url: &str,
    api_key: Option<&str>,
    body: Vec<u8>,
) -> Result<Pin<Box<dyn Stream<Item = StreamEvent> + Send>>, String> {
    let url = if base_url.contains("/chat/completions") {
        base_url.to_string()
    } else {
        format!("{}/chat/completions", base_url.trim_end_matches('/'))
    };

    let mut req = client
        .post(&url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body);
    if let Some(key) = api_key {
        req = req.bearer_auth(key);
    }
    let response = req
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(format!("Server {status}: {text}"));
    }

    let stream = async_stream::stream! {
        let mut parser = StreamParser::new();
        let mut buffer = Vec::new();
        let mut byte_stream = response.bytes_stream();
        let mut terminal_deadline = None;
        let mut body_error = None;

        loop {
            let next = match terminal_deadline {
                Some(deadline) => match tokio::time::timeout_at(deadline, byte_stream.next()).await {
                    Ok(next) => next,
                    Err(_) => {
                        parser.abort();
                        yield StreamEvent::Error(
                            "Provider did not send terminal usage/framing after its finish reason"
                                .to_string(),
                        );
                        return;
                    }
                },
                None => byte_stream.next().await,
            };
            let Some(chunk) = next else { break };
            match chunk {
                Err(error) => {
                    body_error = Some(format!("Stream error: {error}"));
                    break;
                }
                Ok(bytes) => {
                    buffer.extend_from_slice(&bytes);
                    while let Some(position) = buffer.iter().position(|byte| *byte == b'\n') {
                        let mut line = buffer.drain(..=position).collect::<Vec<_>>();
                        if line.last() == Some(&b'\n') {
                            line.pop();
                        }
                        if line.last() == Some(&b'\r') {
                            line.pop();
                        }
                        let line = match std::str::from_utf8(&line) {
                            Ok(line) => line,
                            Err(error) => {
                                parser.abort();
                                yield StreamEvent::Error(format!(
                                    "Stream contained invalid UTF-8: {error}"
                                ));
                                return;
                            }
                        };
                        for event in parser.process_line(line.trim()) {
                            let terminal = matches!(
                                event,
                                StreamEvent::Done { .. } | StreamEvent::Error(_)
                            );
                            yield event;
                            if terminal {
                                return;
                            }
                        }
                        if parser.awaiting_terminal() && terminal_deadline.is_none() {
                            terminal_deadline = Some(
                                tokio::time::Instant::now() + TERMINAL_TAIL_TIMEOUT,
                            );
                        }
                    }
                }
            }
        }

        if !buffer.is_empty() {
            let line = match std::str::from_utf8(&buffer) {
                Ok(line) => line.trim_end_matches(['\r', '\n']),
                Err(error) => {
                    parser.abort();
                    yield StreamEvent::Error(format!(
                        "Stream contained invalid trailing UTF-8: {error}"
                    ));
                    return;
                }
            };
            for event in parser.process_line(line.trim()) {
                if body_error.is_some()
                    && matches!(event, StreamEvent::ToolCallComplete { .. } | StreamEvent::Done { .. })
                {
                    continue;
                }
                let terminal = matches!(event, StreamEvent::Done { .. } | StreamEvent::Error(_));
                if body_error.is_none() || !matches!(event, StreamEvent::Error(_)) {
                    yield event;
                }
                if terminal && body_error.is_none() {
                    return;
                }
            }
        }

        if let Some(error) = body_error {
            parser.abort();
            yield StreamEvent::Error(error);
            return;
        }
        for event in parser.finish_clean() {
            let terminal = matches!(event, StreamEvent::Done { .. } | StreamEvent::Error(_));
            yield event;
            if terminal {
                return;
            }
        }
        yield StreamEvent::Error(
            "Provider stream ended without a terminal event".to_string(),
        );
    };

    Ok(Box::pin(stream))
}

/// Send a single non-streaming chat completion and return the assistant's text.
pub async fn complete(
    client: &Client,
    base_url: &str,
    model: &str,
    messages: &[Message],
    max_tokens: u32,
    api_key: Option<&str>,
    thinking: Option<bool>,
    extra_body: Option<&serde_json::Value>,
) -> Result<String, String> {
    complete_with_usage(
        client, base_url, model, messages, max_tokens, api_key, thinking, extra_body,
    )
    .await
    .map(|completion| completion.text)
}

/// Send a non-streaming chat completion and retain provider usage.
pub async fn complete_with_usage(
    client: &Client,
    base_url: &str,
    model: &str,
    messages: &[Message],
    max_tokens: u32,
    api_key: Option<&str>,
    thinking: Option<bool>,
    extra_body: Option<&serde_json::Value>,
) -> Result<Completion, String> {
    let url = if base_url.contains("/chat/completions") {
        base_url.to_string()
    } else {
        format!("{}/chat/completions", base_url.trim_end_matches('/'))
    };
    let mut body = build_request(model, messages, &[], max_tokens, thinking, extra_body)?;
    body["stream"] = serde_json::Value::Bool(false);
    if let Some(obj) = body.as_object_mut() {
        obj.remove("stream_options");
    }

    let mut req = client.post(&url).json(&body);
    if let Some(key) = api_key {
        req = req.bearer_auth(key);
    }
    let res = req.send().await.map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        return Err(format!("Server {status}: {text}"));
    }
    let json: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    let usage = json
        .get("usage")
        .cloned()
        .map(serde_json::from_value::<Usage>)
        .transpose()
        .map_err(|error| format!("Invalid usage response: {error}"))?;
    Ok(Completion {
        text: json["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string(),
        usage,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::AsyncReadExt;

    async fn read_http_request(socket: &mut tokio::net::TcpStream) {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        let header_end = loop {
            if let Some(position) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                break position + 4;
            }
            let read = socket.read(&mut buffer).await.unwrap();
            assert_ne!(read, 0, "client closed before sending request headers");
            request.extend_from_slice(&buffer[..read]);
        };
        let headers = std::str::from_utf8(&request[..header_end]).unwrap();
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        while request.len() - header_end < content_length {
            let read = socket.read(&mut buffer).await.unwrap();
            assert_ne!(read, 0, "client closed before sending request body");
            request.extend_from_slice(&buffer[..read]);
        }
    }

    #[test]
    fn build_request_no_tools() {
        let msgs = vec![Message::user("hello")];
        let body = build_request("model-x", &msgs, &[], 100, None, None).unwrap();
        assert_eq!(body["model"], "model-x");
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"]["include_usage"], true);
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn build_request_requests_timings_per_token() {
        let msgs = vec![Message::user("hello")];
        let body = build_request("model-x", &msgs, &[], 100, None, None).unwrap();
        assert_eq!(body["timings_per_token"], true);
    }

    #[test]
    fn build_request_extra_body_can_override_timings_per_token() {
        let msgs = vec![Message::user("hello")];
        let extra = json!({ "timings_per_token": false });
        let body = build_request("model-x", &msgs, &[], 100, None, Some(&extra)).unwrap();
        assert_eq!(body["timings_per_token"], false);
    }

    #[test]
    fn build_request_with_tools() {
        let msgs = vec![Message::user("ls the dir")];
        let tools = vec![ToolDefinition::new(
            "run_shell_command",
            "Run a bash command",
            json!({"type":"object","properties":{"command":{"type":"string"}},"required":["command"]}),
        )];
        let body = build_request("gemma", &msgs, &tools, 512, None, None).unwrap();
        let t = &body["tools"][0];
        assert_eq!(t["type"], "function");
        assert_eq!(t["function"]["name"], "run_shell_command");
        assert_eq!(body["parallel_tool_calls"], false);
    }

    #[tokio::test]
    async fn clean_eof_without_finish_reason_is_an_error() {
        use tokio::io::AsyncWriteExt;
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_http_request(&mut socket).await;
            let body = b"data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n";
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            socket.write_all(body).await.unwrap();
            socket.shutdown().await.unwrap();
        });

        let client = Client::new();
        let mut stream = stream_chat(
            &client,
            &format!("http://{address}/v1"),
            "model",
            &[Message::user("hello")],
            &[],
            32,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let mut saw_text = false;
        let mut saw_error = false;
        let mut saw_done = false;
        while let Some(event) =
            tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
                .await
                .unwrap()
        {
            saw_text |= matches!(event, StreamEvent::TextDelta(_));
            saw_error |= matches!(event, StreamEvent::Error(_));
            saw_done |= matches!(event, StreamEvent::Done { .. });
        }
        server.await.unwrap();
        assert!(saw_text);
        assert!(saw_error);
        assert!(!saw_done);
    }

    #[tokio::test]
    async fn rejected_finish_preserves_standard_delayed_usage() {
        use tokio::io::AsyncWriteExt;
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_http_request(&mut socket).await;
            let body = concat!(
                "data:{\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n\n",
                "data:{\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":9}}\n\n",
                "data:[DONE]\n\n"
            );
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            socket.write_all(body.as_bytes()).await.unwrap();
            socket.shutdown().await.unwrap();
        });

        let mut stream = stream_chat(
            &Client::new(),
            &format!("http://{address}/v1"),
            "model",
            &[Message::user("hello")],
            &[],
            32,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let mut events = Vec::new();
        while let Some(event) = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .unwrap()
        {
            events.push(event);
        }
        server.await.unwrap();

        let usage_position = events
            .iter()
            .position(|event| matches!(event, StreamEvent::UsageUpdate(usage) if usage.prompt_tokens == Some(12) && usage.completion_tokens == Some(9)))
            .unwrap();
        let error_position = events
            .iter()
            .position(|event| matches!(event, StreamEvent::Error(error) if error.contains("non-complete finish reason 'length'")))
            .unwrap();
        assert!(usage_position < error_position);
        assert!(!events.iter().any(|event| matches!(
            event,
            StreamEvent::Done { .. } | StreamEvent::ToolCallComplete { .. }
        )));
    }

    #[tokio::test]
    async fn truncated_tool_usage_tail_never_completes_the_tool() {
        use tokio::io::AsyncWriteExt;
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_http_request(&mut socket).await;
            let body = concat!(
                "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call-1\",\"function\":{\"name\":\"calculate\",\"arguments\":\"{\\\"expression\\\":\\\"2+2\\\"}\"}}]}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2}}\n\n"
            );
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len() + 100
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            socket.write_all(body.as_bytes()).await.unwrap();
            socket.shutdown().await.unwrap();
        });

        let mut stream = stream_chat(
            &Client::new(),
            &format!("http://{address}/v1"),
            "model",
            &[Message::user("hello")],
            &[],
            32,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let mut saw_usage = false;
        let mut saw_error = false;
        let mut saw_tool = false;
        let mut saw_done = false;
        while let Some(event) = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .unwrap()
        {
            saw_usage |= matches!(event, StreamEvent::UsageUpdate(_));
            saw_error |= matches!(event, StreamEvent::Error(_));
            saw_tool |= matches!(event, StreamEvent::ToolCallComplete { .. });
            saw_done |= matches!(event, StreamEvent::Done { .. });
        }
        server.await.unwrap();
        assert!(saw_usage);
        assert!(saw_error);
        assert!(!saw_tool);
        assert!(!saw_done);
    }

    #[tokio::test]
    async fn finish_reason_without_terminal_tail_times_out() {
        use tokio::io::AsyncWriteExt;
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_http_request(&mut socket).await;
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\n\r\n",
                )
                .await
                .unwrap();
            let body = b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2}}\n\n";
            socket
                .write_all(format!("{:x}\r\n", body.len()).as_bytes())
                .await
                .unwrap();
            socket.write_all(body).await.unwrap();
            socket.write_all(b"\r\n").await.unwrap();
            socket.flush().await.unwrap();
            let mut byte = [0_u8; 1];
            tokio::time::timeout(Duration::from_secs(2), socket.read(&mut byte))
                .await
                .unwrap()
                .unwrap()
                == 0
        });

        let mut stream = stream_chat(
            &Client::new(),
            &format!("http://{address}/v1"),
            "model",
            &[Message::user("hello")],
            &[],
            32,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let mut saw_usage = false;
        let mut error = None;
        while let Some(event) = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .unwrap()
        {
            match event {
                StreamEvent::UsageUpdate(_) => saw_usage = true,
                StreamEvent::Error(value) => error = Some(value),
                StreamEvent::Done { .. } => panic!("missing terminal tail emitted Done"),
                _ => {}
            }
        }
        assert!(saw_usage);
        assert!(error.unwrap().contains("terminal usage/framing"));
        assert!(server.await.unwrap());
    }

    #[tokio::test]
    async fn dropping_stream_closes_incomplete_http_body() {
        use tokio::io::AsyncWriteExt;
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_http_request(&mut socket).await;
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\n\r\n",
                )
                .await
                .unwrap();
            let body = b"data: {\"choices\":[{\"delta\":{\"content\":\"first\"}}]}\n\n";
            socket
                .write_all(format!("{:x}\r\n", body.len()).as_bytes())
                .await
                .unwrap();
            socket.write_all(body).await.unwrap();
            socket.write_all(b"\r\n").await.unwrap();
            socket.flush().await.unwrap();

            let mut byte = [0_u8; 1];
            tokio::time::timeout(std::time::Duration::from_secs(2), socket.read(&mut byte))
                .await
                .expect("dropping the response stream did not close the HTTP body")
                .unwrap()
                == 0
        });

        let client = Client::new();
        let mut stream = stream_chat(
            &client,
            &format!("http://{address}/v1"),
            "model",
            &[Message::user("hello")],
            &[],
            32,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(event, StreamEvent::TextDelta(ref text) if text == "first"));
        drop(stream);
        assert!(server.await.unwrap());
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
        let msgs = vec![Message::system("You are helpful"), Message::user("hi")];
        let body = build_request("m", &msgs, &[], 50, None, None).unwrap();
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["role"], "user");
    }

    #[test]
    fn build_request_with_thinking() {
        let msgs = vec![Message::user("hello")];
        let body = build_request("model-x", &msgs, &[], 100, Some(true), None).unwrap();
        assert_eq!(body["thinking"]["type"], "enabled");
    }

    #[test]
    fn build_request_with_extra_body() {
        let msgs = vec![Message::user("hello")];
        let extra = json!({
            "reasoning_effort": "high",
            "temperature": 0.5
        });
        let body = build_request("model-x", &msgs, &[], 100, None, Some(&extra)).unwrap();
        assert_eq!(body["reasoning_effort"], "high");
        assert_eq!(body["temperature"], 0.5);
        assert_eq!(body["max_tokens"], 100);
    }

    #[test]
    fn reserved_extra_body_fields_are_rejected() {
        let messages = vec![Message::user("hello")];
        for field in RESERVED_EXTRA_BODY_FIELDS {
            let extra = json!({(*field): "hostile"});
            let error =
                build_request("model-x", &messages, &[], 100, None, Some(&extra)).unwrap_err();
            assert!(error.contains(field), "{field}: {error}");
        }
        assert!(build_request("model-x", &messages, &[], 100, None, Some(&Value::Null)).is_err());
    }
}
