use super::*;
use crate::config::ConnectionKind;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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

fn config() -> Config {
    Config {
        server_url: "http://127.0.0.1:18765/v1/messages".to_string(),
        model: "gpt-5.6-sol".to_string(),
        context_size: 372_000,
        connection_kind: ConnectionKind::ClaudeCodeProxy,
        api_key: Some("local-placeholder".to_string()),
        thinking: Some(true),
        extra_body: Some(json!({
            "output_config": {"effort": "max"}
        })),
        ..Default::default()
    }
}

fn tool() -> ToolDefinition {
    ToolDefinition {
        name: "calculate".to_string(),
        description: "Calculate an expression".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "expression": {"type": "string"},
                "tool_call_id": {"type": "string"}
            },
            "required": ["expression", "tool_call_id"]
        }),
    }
}

#[test]
fn normalizes_proxy_urls() {
    assert_eq!(
        messages_url("http://localhost:18765"),
        "http://localhost:18765/v1/messages"
    );
    assert_eq!(
        messages_url("http://localhost:18765/v1"),
        "http://localhost:18765/v1/messages"
    );
    assert_eq!(
        messages_url("http://localhost:18765/v1/messages"),
        "http://localhost:18765/v1/messages"
    );
}

#[test]
fn builds_native_request_and_removes_synthetic_tool_id() {
    let body = build_request(
        &config(),
        &[Message::system("system"), Message::user("hello")],
        &[tool()],
        100,
        true,
    )
    .unwrap();
    assert_eq!(body["model"], "gpt-5.6-sol");
    assert_eq!(body["system"], "system");
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(body["thinking"]["type"], "adaptive");
    assert_eq!(body["output_config"]["effort"], "max");
    assert_eq!(body["tool_choice"]["disable_parallel_tool_use"], true);
    assert!(
        body["tools"][0]["input_schema"]["properties"]
            .get("tool_call_id")
            .is_none()
    );
    assert_eq!(
        body["tools"][0]["input_schema"]["required"],
        json!(["expression"])
    );
}

#[test]
fn empty_provider_blocks_fall_back_to_portable_text_and_tools() {
    let messages = vec![
        Message::assistant_with_tools("portable text", Vec::new(), Some(Vec::new())),
        Message::assistant_with_tools(
            "",
            vec![ToolCall {
                id: "toolu_portable".to_string(),
                name: "calculate".to_string(),
                arguments: json!({"expression": "2+2"}),
            }],
            Some(Vec::new()),
        ),
        Message::tool_result("toolu_portable", "4"),
    ];

    let body = build_request(&config(), &messages, &[tool()], 100, true).unwrap();

    assert_eq!(body["messages"][0]["content"][0]["text"], "portable text");
    assert_eq!(
        body["messages"][1]["content"][0],
        json!({
            "type": "tool_use",
            "id": "toolu_portable",
            "name": "calculate",
            "input": {"expression": "2+2"}
        })
    );
    assert_eq!(
        body["messages"][2]["content"][0]["tool_use_id"],
        "toolu_portable"
    );
}

#[test]
fn omits_only_empty_portable_assistant_projection() {
    let valid_native = vec![json!({
        "type": "thinking",
        "thinking": "reason",
        "signature": "signed"
    })];
    let messages = vec![
        Message::user("first"),
        Message::assistant_with_tools("", Vec::new(), Some(Vec::new())),
        Message::user("continue"),
        Message::assistant_with_tools("", Vec::new(), Some(valid_native.clone())),
    ];

    let body = build_request(&config(), &messages, &[], 100, true).unwrap();
    let wire = body["messages"].as_array().unwrap();

    assert_eq!(wire.len(), 3);
    assert_eq!(wire[0]["role"], "user");
    assert_eq!(wire[1]["role"], "user");
    assert_eq!(wire[2]["content"], Value::Array(valid_native));
}

#[test]
fn preserves_provider_assistant_blocks_verbatim() {
    let blocks = vec![
        json!({"type": "thinking", "thinking": "x", "signature": "sig"}),
        json!({"type": "tool_use", "id": "toolu_1", "name": "calculate", "input": {"expression": "2+2"}}),
    ];
    let messages = vec![
        Message::assistant_with_tools(
            "ignored",
            vec![ToolCall {
                id: "different".to_string(),
                name: "calculate".to_string(),
                arguments: json!({}),
            }],
            Some(blocks.clone()),
        ),
        Message::tool_result_with_status("toolu_1", "division failed", true),
    ];
    let body = build_request(&config(), &messages, &[tool()], 100, true).unwrap();
    assert_eq!(body["messages"][0]["content"], Value::Array(blocks));
    assert_eq!(body["messages"][1]["content"][0]["tool_use_id"], "toolu_1");
    assert_eq!(body["messages"][1]["content"][0]["is_error"], true);
}

#[test]
fn converts_png_to_anthropic_source_block() {
    let body = build_request(
        &config(),
        &[Message::user_with_pngs("inspect", &["abc".to_string()])],
        &[],
        100,
        false,
    )
    .unwrap();
    assert_eq!(
        body["messages"][0]["content"][0]["source"]["media_type"],
        "image/png"
    );
    assert_eq!(body["messages"][0]["content"][0]["source"]["data"], "abc");
}

#[test]
fn decodes_split_utf8_and_crlf_sse_lines() {
    let line = "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"hé\"}}\r\n";
    let bytes = line.as_bytes();
    let split = line.find('é').unwrap() + 1;
    let mut decoder = SseDecoder::default();
    assert!(decoder.push(&bytes[..split]).is_empty());
    let decoded = decoder.push(&bytes[split..]);
    assert_eq!(decoded.len(), 1);
    assert_eq!(decoded[0].as_ref().unwrap(), line[6..].trim());
}

#[test]
fn preserves_cached_input_usage_with_checked_arithmetic() {
    let usage = anthropic_usage(&json!({
        "input_tokens": 12,
        "cache_creation_input_tokens": 3,
        "cache_read_input_tokens": 5,
        "output_tokens": 9
    }))
    .unwrap();
    assert_eq!(usage.uncached_input_tokens, 12);
    assert_eq!(usage.cache_creation_input_tokens, 3);
    assert_eq!(usage.cache_read_input_tokens, 5);
    assert_eq!(usage.output_tokens, 9);
    assert_eq!(usage.total_input_tokens, Some(20));
    assert!(usage.breakdown_complete);

    let overflow = anthropic_usage(&json!({
        "input_tokens": u64::MAX,
        "cache_creation_input_tokens": 0,
        "cache_read_input_tokens": 1,
        "output_tokens": 1
    }))
    .unwrap();
    assert_eq!(overflow.uncached_input_tokens, u64::MAX);
    assert_eq!(overflow.total_input_tokens, None);
    assert!(!overflow.breakdown_complete);

    let partial = anthropic_usage(&json!({"input_tokens": 4})).unwrap();
    assert_eq!(partial.total_input_tokens, Some(4));
    assert!(!partial.breakdown_complete);
    assert_eq!(anthropic_usage(&json!({})), None);
}

#[test]
fn non_streaming_text_requires_a_complete_stop_reason() {
    for reason in ["max_tokens", "refusal", "pause_turn", "tool_use"] {
        let response =
            json!({"stop_reason": reason, "content": [{"type":"text","text":"partial"}]});
        let error = validate_text_completion_stop(&response).unwrap_err();
        assert!(error.contains(reason));
        assert!(error.contains("not accepted"));
    }
    assert!(validate_text_completion_stop(&json!({"stop_reason":"end_turn"})).is_ok());
    assert!(validate_text_completion_stop(&json!({"stop_reason":"stop_sequence"})).is_ok());
    assert!(validate_text_completion_stop(&json!({})).is_err());
}

#[tokio::test]
async fn rejected_nonstreaming_completion_retains_billable_usage() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_http_request(&mut socket).await;
        let body = json!({
            "content": [{"type":"text","text":"partial"}],
            "stop_reason": "max_tokens",
            "usage": {
                "input_tokens": 12,
                "cache_creation_input_tokens": 3,
                "cache_read_input_tokens": 5,
                "output_tokens": 9
            }
        })
        .to_string();
        socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        socket.write_all(body.as_bytes()).await.unwrap();
        socket.shutdown().await.unwrap();
    });

    let mut test_config = config();
    test_config.server_url = format!("http://{address}/v1/messages");
    let error =
        complete_with_usage_or_error(&Client::new(), &test_config, &[Message::user("hello")], 32)
            .await
            .unwrap_err();
    server.await.unwrap();

    assert!(error.message.contains("max_tokens"));
    let usage = error.usage.unwrap();
    assert_eq!(usage.total_input_tokens, Some(20));
    assert_eq!(usage.output_tokens, 9);
    assert!(usage.breakdown_complete);
}

#[tokio::test]
async fn clean_eof_without_message_stop_preserves_usage_and_errors() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_http_request(&mut socket).await;
        let body = format!(
            "data: {}\n\n",
            json!({
                "type":"message_start",
                "message":{"content":[],"usage":{
                    "input_tokens":12,
                    "cache_creation_input_tokens":3,
                    "cache_read_input_tokens":5
                }}
            })
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

    let mut test_config = config();
    test_config.server_url = format!("http://{address}/v1/messages");
    let mut events = stream(
        &Client::new(),
        &test_config,
        &[Message::user("hello")],
        &[],
        32,
    )
    .await
    .unwrap();
    let mut usage = None;
    let mut error = None;
    let mut saw_done = false;
    while let Some(event) = tokio::time::timeout(std::time::Duration::from_secs(2), events.next())
        .await
        .unwrap()
    {
        match event {
            StreamEvent::UsageUpdate(value) => usage = Some(value),
            StreamEvent::Error(value) => error = Some(value),
            StreamEvent::Done { .. } => saw_done = true,
            _ => {}
        }
    }
    server.await.unwrap();
    let usage = usage.unwrap();
    assert_eq!(usage.total_input_tokens, Some(20));
    assert_eq!(usage.output_tokens, 0);
    assert!(error.unwrap().contains("before message_stop"));
    assert!(!saw_done);
}

#[tokio::test]
async fn body_framing_error_does_not_discard_known_usage() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_http_request(&mut socket).await;
        let body = format!(
            "data: {}",
            json!({
                "type":"message_start",
                "message":{"content":[],"usage":{
                    "input_tokens":7,
                    "cache_creation_input_tokens":0,
                    "cache_read_input_tokens":2
                }}
            })
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

    let mut test_config = config();
    test_config.server_url = format!("http://{address}/v1/messages");
    let mut events = stream(
        &Client::new(),
        &test_config,
        &[Message::user("hello")],
        &[],
        32,
    )
    .await
    .unwrap();
    let mut usage = None;
    let mut error = None;
    while let Some(event) = tokio::time::timeout(std::time::Duration::from_secs(2), events.next())
        .await
        .unwrap()
    {
        match event {
            StreamEvent::UsageUpdate(value) => usage = Some(value),
            StreamEvent::Error(value) => error = Some(value),
            StreamEvent::Done { .. } => panic!("framing failure emitted Done"),
            _ => {}
        }
    }
    server.await.unwrap();
    assert_eq!(usage.unwrap().total_input_tokens, Some(9));
    assert!(error.unwrap().contains("Stream error"));
}

#[tokio::test]
async fn dropping_anthropic_stream_closes_incomplete_body() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
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
        let body = format!(
            "data: {}\n\ndata: {}\n\n",
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"first"}})
        );
        socket
            .write_all(format!("{:x}\r\n", body.len()).as_bytes())
            .await
            .unwrap();
        socket.write_all(body.as_bytes()).await.unwrap();
        socket.write_all(b"\r\n").await.unwrap();
        socket.flush().await.unwrap();

        let mut byte = [0_u8; 1];
        tokio::time::timeout(std::time::Duration::from_secs(2), socket.read(&mut byte))
            .await
            .expect("dropping Anthropic stream did not close its HTTP body")
            .unwrap()
            == 0
    });

    let mut test_config = config();
    test_config.server_url = format!("http://{address}/v1/messages");
    let mut events = stream(
        &Client::new(),
        &test_config,
        &[Message::user("hello")],
        &[],
        32,
    )
    .await
    .unwrap();
    let event = tokio::time::timeout(std::time::Duration::from_secs(2), events.next())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(event, StreamEvent::TextDelta(ref text) if text == "first"));
    drop(events);
    assert!(server.await.unwrap());
}

#[tokio::test]
async fn stop_reason_without_message_stop_times_out_and_closes_body() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
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
        let body = format!(
            "data: {}\n\ndata: {}\n\n",
            json!({"type":"message_start","message":{"content":[],"usage":{"input_tokens":5,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}),
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}})
        );
        socket
            .write_all(format!("{:x}\r\n", body.len()).as_bytes())
            .await
            .unwrap();
        socket.write_all(body.as_bytes()).await.unwrap();
        socket.write_all(b"\r\n").await.unwrap();
        socket.flush().await.unwrap();
        let mut byte = [0_u8; 1];
        tokio::time::timeout(std::time::Duration::from_secs(2), socket.read(&mut byte))
            .await
            .unwrap()
            .unwrap()
            == 0
    });

    let mut test_config = config();
    test_config.server_url = format!("http://{address}/v1/messages");
    let mut events = stream(
        &Client::new(),
        &test_config,
        &[Message::user("hello")],
        &[],
        32,
    )
    .await
    .unwrap();
    let mut latest_usage = None;
    let mut error = None;
    while let Some(event) = tokio::time::timeout(std::time::Duration::from_secs(2), events.next())
        .await
        .unwrap()
    {
        match event {
            StreamEvent::UsageUpdate(usage) => latest_usage = Some(usage),
            StreamEvent::Error(value) => error = Some(value),
            StreamEvent::Done { .. } => panic!("missing message_stop emitted Done"),
            _ => {}
        }
    }
    let usage = latest_usage.unwrap();
    assert_eq!(usage.total_input_tokens, Some(5));
    assert_eq!(usage.output_tokens, 2);
    assert!(error.unwrap().contains("message_stop"));
    assert!(server.await.unwrap());
}

#[tokio::test]
async fn deferred_parser_error_times_out_without_stop_reason() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
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
        let body = format!(
            "data: {}\n\n",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{}"}})
        );
        socket
            .write_all(format!("{:x}\r\n", body.len()).as_bytes())
            .await
            .unwrap();
        socket.write_all(body.as_bytes()).await.unwrap();
        socket.write_all(b"\r\n").await.unwrap();
        socket.flush().await.unwrap();
        let mut byte = [0_u8; 1];
        tokio::time::timeout(std::time::Duration::from_secs(2), socket.read(&mut byte))
            .await
            .expect("client did not close a deferred-error body")
            .unwrap()
            == 0
    });

    let mut test_config = config();
    test_config.server_url = format!("http://{address}/v1/messages");
    let mut events = stream(
        &Client::new(),
        &test_config,
        &[Message::user("hello")],
        &[tool()],
        32,
    )
    .await
    .unwrap();
    let mut error = None;
    while let Some(event) = tokio::time::timeout(std::time::Duration::from_secs(2), events.next())
        .await
        .unwrap()
    {
        match event {
            StreamEvent::Error(value) => error = Some(value),
            StreamEvent::ToolCalls { .. } | StreamEvent::Done { .. } => {
                panic!("deferred parser error authorized a terminal completion")
            }
            _ => {}
        }
    }
    assert!(error.unwrap().contains("before starting content block"));
    assert!(server.await.unwrap());
}

#[test]
fn parses_thinking_signature_text_usage_and_tools() {
    let mut parser = AnthropicStreamParser::default();
    let events = [
            json!({"type":"message_start","message":{"content":[],"usage":{"input_tokens":12,"cache_creation_input_tokens":3,"cache_read_input_tokens":5}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"reason"}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"signed"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"hello"}}),
            json!({"type":"content_block_stop","index":1}),
            json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_1","name":"calculate","input":{}}}),
            json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"expression\":"}}),
            json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"\"2+2\"}"}}),
            json!({"type":"content_block_stop","index":2}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":9}}),
            json!({"type":"message_stop"}),
        ]
        .into_iter()
        .flat_map(|event| parser.process_data(&event.to_string()))
        .collect::<Vec<_>>();

    assert!(events.iter().any(|event| {
        matches!(
            event,
            StreamEvent::ReasoningDelta {
                text,
                segment_index: Some(0)
            } if text == "reason"
        )
    }));
    assert!(
        events
            .iter()
            .any(|event| { matches!(event, StreamEvent::TextDelta(text) if text == "hello") })
    );
    let (calls, provider_content) = events
        .iter()
        .find_map(|event| match event {
            StreamEvent::ToolCalls {
                calls,
                provider_content,
            } => Some((calls, provider_content)),
            _ => None,
        })
        .unwrap();
    assert_eq!(calls[0].id, "toolu_1");
    assert_eq!(calls[0].arguments["expression"], "2+2");
    let blocks = provider_content.as_ref().unwrap();
    assert_eq!(blocks[0]["signature"], "signed");
    assert_eq!(blocks[2]["input"]["expression"], "2+2");
    assert!(events.iter().any(|event| {
        matches!(event, StreamEvent::Done {
                completion_tokens: Some(9),
                prompt_tokens: Some(20),
                stop_reason: Some(reason),
                ..
            } if reason == "tool_use")
    }));
    let usage = events
        .iter()
        .find_map(|event| match event {
            StreamEvent::Done {
                usage: Some(usage), ..
            } => Some(usage),
            _ => None,
        })
        .unwrap();
    assert_eq!(usage.uncached_input_tokens, 12);
    assert_eq!(usage.cache_creation_input_tokens, 3);
    assert_eq!(usage.cache_read_input_tokens, 5);
    assert_eq!(usage.output_tokens, 9);
    assert!(usage.breakdown_complete);
}

#[test]
fn distinct_thinking_indices_survive_streaming_and_provider_replay() {
    let mut parser = AnthropicStreamParser::default();
    let events = [
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"**segment A**"}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"signature-a"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"thinking","thinking":"","signature":""}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"thinking_delta","thinking":"**segment B**"}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"signature_delta","signature":"signature-b"}}),
            json!({"type":"content_block_stop","index":1}),
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}),
            json!({"type":"message_stop"}),
        ]
        .into_iter()
        .flat_map(|event| parser.process_data(&event.to_string()))
        .collect::<Vec<_>>();

    let streamed = events
        .iter()
        .filter_map(|event| match event {
            StreamEvent::ReasoningDelta {
                text,
                segment_index,
            } => Some((*segment_index, text.as_str())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        streamed,
        vec![(Some(0), "**segment A**"), (Some(1), "**segment B**")]
    );
    let blocks = events
        .iter()
        .find_map(|event| match event {
            StreamEvent::Done {
                provider_content: Some(blocks),
                ..
            } => Some(blocks),
            _ => None,
        })
        .unwrap();
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0]["thinking"], "**segment A**");
    assert_eq!(blocks[0]["signature"], "signature-a");
    assert_eq!(blocks[1]["thinking"], "**segment B**");
    assert_eq!(blocks[1]["signature"], "signature-b");
}

#[test]
fn preserves_redacted_thinking_blocks() {
    let mut parser = AnthropicStreamParser::default();
    parser.process_data(
        &json!({
            "type":"content_block_start",
            "index":0,
            "content_block":{"type":"redacted_thinking","data":"opaque"}
        })
        .to_string(),
    );
    parser.process_data(&json!({"type":"content_block_stop","index":0}).to_string());
    parser.process_data(
            &json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}).to_string(),
        );
    let events = parser.process_data(&json!({"type":"message_stop"}).to_string());
    let blocks = events
        .iter()
        .find_map(|event| match event {
            StreamEvent::Done {
                provider_content: Some(blocks),
                ..
            } => Some(blocks),
            _ => None,
        })
        .unwrap();
    assert_eq!(blocks[0]["data"], "opaque");
}

#[test]
fn rejects_native_tools_without_tool_use_stop_reason() {
    for reason in [
        Some("max_tokens"),
        Some("refusal"),
        Some("pause_turn"),
        Some("end_turn"),
        None,
    ] {
        let mut parser = AnthropicStreamParser::default();
        parser.process_data(
                &json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_unsafe","name":"calculate","input":{}}}).to_string(),
            );
        parser.process_data(
                &json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"expression\":\"2+2\"}"}}).to_string(),
            );
        parser.process_data(&json!({"type":"content_block_stop","index":0}).to_string());
        if let Some(reason) = reason {
            parser.process_data(
                    &json!({"type":"message_delta","delta":{"stop_reason":reason},"usage":{"output_tokens":9}}).to_string(),
                );
        }

        let events = parser.process_data(&json!({"type":"message_stop"}).to_string());
        assert!(
            events.iter().any(|event| matches!(
                event,
                StreamEvent::Error(error) if error.contains("was not accepted")
                    || error.contains("refusing to execute")
            )),
            "expected a refusal for stop reason {reason:?}: {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, StreamEvent::ToolCalls { .. }))
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, StreamEvent::Done { .. }))
        );
    }
}

#[test]
fn passes_every_native_tool_call_to_the_host_in_block_order() {
    let mut parser = AnthropicStreamParser::default();
    for (index, id) in [(0, "toolu_1"), (1, "toolu_2")] {
        parser.process_data(
                &json!({"type":"content_block_start","index":index,"content_block":{"type":"tool_use","id":id,"name":"calculate","input":{}}}).to_string(),
            );
        parser.process_data(
                &json!({"type":"content_block_delta","index":index,"delta":{"type":"input_json_delta","partial_json":"{\"expression\":\"2+2\"}"}}).to_string(),
            );
        parser.process_data(&json!({"type":"content_block_stop","index":index}).to_string());
    }
    parser.process_data(
            &json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":9}}).to_string(),
        );
    let events = parser.process_data(&json!({"type":"message_stop"}).to_string());
    let ids: Vec<String> = events
        .iter()
        .find_map(|event| match event {
            StreamEvent::ToolCalls { calls, .. } => {
                Some(calls.iter().map(|call| call.id.clone()).collect())
            }
            _ => None,
        })
        .expect("tool calls event");
    assert_eq!(ids, ["toolu_1", "toolu_2"]);
    assert!(!events.iter().any(|event| matches!(event, StreamEvent::Error(_))));
}

#[test]
fn batched_tool_results_share_one_user_turn() {
    let assistant = Message::assistant_with_tools(
        "",
        vec![
            crate::transport::ToolCall {
                id: "toolu_1".to_string(),
                name: "calculate".to_string(),
                arguments: json!({"expression": "1+1"}),
            },
            crate::transport::ToolCall {
                id: "toolu_2".to_string(),
                name: "calculate".to_string(),
                arguments: json!({"expression": "2+2"}),
            },
        ],
        None,
    );
    let messages = vec![
        Message::user("go"),
        assistant,
        Message::tool_result_with_status("toolu_1", "2", false),
        Message::tool_result_with_status("toolu_2", "4", false),
        Message::user("next"),
    ];
    let (_, conversation) = native_messages(&messages);
    assert_eq!(conversation.len(), 4, "{conversation:#?}");
    let results = conversation[2]["content"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[1]["tool_use_id"], "toolu_2");
    assert_eq!(conversation[3]["content"][0]["text"], "next");
}

#[test]
fn rejects_partial_or_refused_native_text_turns() {
    for reason in [
        Some("max_tokens"),
        Some("refusal"),
        Some("pause_turn"),
        None,
    ] {
        let mut parser = AnthropicStreamParser::default();
        parser.process_data(
                &json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}).to_string(),
            );
        parser.process_data(
                &json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"partial"}}).to_string(),
            );
        parser.process_data(&json!({"type":"content_block_stop","index":0}).to_string());
        if let Some(reason) = reason {
            parser.process_data(
                    &json!({"type":"message_delta","delta":{"stop_reason":reason},"usage":{"output_tokens":1}}).to_string(),
                );
        }

        let events = parser.process_data(&json!({"type":"message_stop"}).to_string());
        assert!(events.iter().any(|event| matches!(
            event,
            StreamEvent::Error(error) if error.contains("was not accepted")
        )));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, StreamEvent::Done { .. }))
        );
    }
}

#[test]
fn rejected_turn_still_emits_final_billable_usage() {
    let mut parser = AnthropicStreamParser::default();
    let events = [
            json!({"type":"message_start","message":{"content":[],"usage":{"input_tokens":12,"cache_creation_input_tokens":3,"cache_read_input_tokens":5}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"partial"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":9}}),
            json!({"type":"message_stop"}),
        ]
        .into_iter()
        .flat_map(|event| parser.process_data(&event.to_string()))
        .collect::<Vec<_>>();

    let usage = events
        .iter()
        .rev()
        .find_map(|event| match event {
            StreamEvent::UsageUpdate(usage) => Some(usage),
            _ => None,
        })
        .unwrap();
    assert_eq!(usage.total_input_tokens, Some(20));
    assert_eq!(usage.output_tokens, 9);
    assert!(usage.breakdown_complete);
    assert!(events.iter().any(|event| matches!(
        event,
        StreamEvent::Error(error) if error.contains("max_tokens")
    )));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, StreamEvent::Done { .. }))
    );
}

#[tokio::test]
async fn buffered_malformed_tool_preserves_terminal_usage_before_error() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_http_request(&mut socket).await;
        let body = [
                json!({"type":"message_start","message":{"content":[],"usage":{"input_tokens":12,"cache_creation_input_tokens":3,"cache_read_input_tokens":5}}}),
                json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"calculate","input":{}}}),
                json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{"}}),
                json!({"type":"content_block_stop","index":0}),
                json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":9}}),
                json!({"type":"message_stop"}),
            ]
            .into_iter()
            .map(|event| format!("data: {event}\n\n"))
            .collect::<String>();
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

    let mut test_config = config();
    test_config.server_url = format!("http://{address}/v1/messages");
    let mut events = stream(
        &Client::new(),
        &test_config,
        &[Message::user("calculate")],
        &[tool()],
        128,
    )
    .await
    .unwrap();
    let mut received = Vec::new();
    while let Some(event) = tokio::time::timeout(std::time::Duration::from_secs(2), events.next())
        .await
        .unwrap()
    {
        received.push(event);
    }
    server.await.unwrap();

    let usage_position = received
            .iter()
            .position(|event| matches!(event, StreamEvent::UsageUpdate(usage) if usage.total_input_tokens == Some(20) && usage.output_tokens == 9))
            .unwrap();
    let error_position = received
            .iter()
            .position(|event| matches!(event, StreamEvent::Error(error) if error.contains("Invalid tool input")))
            .unwrap();
    assert!(usage_position < error_position);
    assert!(!received.iter().any(|event| matches!(
        event,
        StreamEvent::ToolCalls { .. } | StreamEvent::Done { .. }
    )));
}

#[test]
fn malformed_tool_json_preserves_later_usage_before_error() {
    let mut parser = AnthropicStreamParser::default();
    parser.process_data(
            &json!({"type":"message_start","message":{"content":[],"usage":{"input_tokens":12,"cache_creation_input_tokens":3,"cache_read_input_tokens":5}}}).to_string(),
        );
    parser.process_data(
            &json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"calculate","input":{}}}).to_string(),
        );
    parser.process_data(
            &json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{"}}).to_string(),
        );
    let stop_events =
        parser.process_data(&json!({"type":"content_block_stop","index":0}).to_string());
    assert!(
        !stop_events
            .iter()
            .any(|event| matches!(event, StreamEvent::Error(_)))
    );
    let events = [
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":9}}),
            json!({"type":"message_stop"}),
        ]
        .into_iter()
        .flat_map(|event| parser.process_data(&event.to_string()))
        .collect::<Vec<_>>();

    let usage_position = events
        .iter()
        .position(
            |event| matches!(event, StreamEvent::UsageUpdate(usage) if usage.output_tokens == 9),
        )
        .unwrap();
    let error_position = events
            .iter()
            .position(|event| matches!(event, StreamEvent::Error(error) if error.contains("Invalid tool input")))
            .unwrap();
    assert!(usage_position < error_position);
    assert!(!events.iter().any(|event| matches!(
        event,
        StreamEvent::ToolCalls { .. } | StreamEvent::Done { .. }
    )));
}

#[test]
fn fresh_tool_block_after_stop_reason_is_rejected() {
    let mut parser = AnthropicStreamParser::default();
    let events = [
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":4}}),
            json!({"type":"content_block_start","index":7,"content_block":{"type":"tool_use","id":"toolu_late","name":"run_shell_command","input":{}}}),
            json!({"type":"content_block_delta","index":7,"delta":{"type":"input_json_delta","partial_json":"{\"command\":\"id\"}"}}),
            json!({"type":"content_block_stop","index":7}),
            json!({"type":"message_stop"}),
        ]
        .into_iter()
        .flat_map(|event| parser.process_data(&event.to_string()))
        .collect::<Vec<_>>();

    assert!(events.iter().any(|event| matches!(
        event,
        StreamEvent::Error(error) if error.contains("after its stop reason")
    )));
    assert!(!events.iter().any(|event| matches!(
        event,
        StreamEvent::ToolCalls { .. } | StreamEvent::Done { .. }
    )));
}

#[test]
fn stopped_tool_index_cannot_be_reused_before_stop_reason() {
    let mut parser = AnthropicStreamParser::default();
    let events = [
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_a","name":"calculate","input":{}}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"expression\":\"2+2\"}"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_b","name":"run_shell_command","input":{}}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"command\":\"id\"}"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":4}}),
            json!({"type":"message_stop"}),
        ]
        .into_iter()
        .flat_map(|event| parser.process_data(&event.to_string()))
        .collect::<Vec<_>>();

    assert!(events.iter().any(|event| matches!(
        event,
        StreamEvent::Error(error) if error.contains("reused content block index")
    )));
    assert!(!events.iter().any(|event| matches!(
        event,
        StreamEvent::ToolCalls { .. } | StreamEvent::Done { .. }
    )));
}

#[test]
fn out_of_order_tool_delta_never_authorizes_a_tool() {
    let mut parser = AnthropicStreamParser::default();
    let events = [
            json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"expression\":\"2+2\"}"}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"calculate","input":{}}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":4}}),
            json!({"type":"message_stop"}),
        ]
        .into_iter()
        .flat_map(|event| parser.process_data(&event.to_string()))
        .collect::<Vec<_>>();

    assert!(events.iter().any(|event| matches!(
        event,
        StreamEvent::Error(error) if error.contains("before starting content block")
    )));
    assert!(!events.iter().any(|event| matches!(
        event,
        StreamEvent::ToolCalls { .. } | StreamEvent::Done { .. }
    )));
}
