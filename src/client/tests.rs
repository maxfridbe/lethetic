use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn identified_provider_reasoning_blocks_receive_one_synthetic_boundary() {
    let mut previous = None;
    let mut flattened = String::new();
    for (segment_index, text) in [(Some(4), "**segment A**"), (Some(7), "**segment B**")] {
        if let Some(separator) = reasoning_segment_separator(&mut previous, segment_index) {
            flattened.push_str(separator);
        }
        flattened.push_str(text);
    }
    assert_eq!(flattened, "**segment A**\n\n**segment B**");
    assert!(!flattened.contains("****"));
}

#[test]
fn repeated_or_unidentified_reasoning_deltas_do_not_gain_boundaries() {
    let mut previous = None;
    assert_eq!(reasoning_segment_separator(&mut previous, None), None);
    assert_eq!(reasoning_segment_separator(&mut previous, None), None);
    assert_eq!(reasoning_segment_separator(&mut previous, Some(2)), None);
    assert_eq!(reasoning_segment_separator(&mut previous, Some(2)), None);
    assert_eq!(
        reasoning_segment_separator(&mut previous, Some(3)),
        Some("\n\n")
    );
}

async fn read_http_request(socket: &mut tokio::net::TcpStream) -> Vec<u8> {
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
    request[header_end..header_end + content_length].to_vec()
}

#[tokio::test]
async fn persisted_agent_diagnostic_is_the_exact_openai_wire_body() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request_body = read_http_request(&mut socket).await;
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":1}}\n\n",
            "data: [DONE]\n\n"
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
        request_body
    });

    let mut config = Config {
        server_url: format!("http://{address}/v1"),
        model: "policy-test-model".to_string(),
        context_size: 100_000,
        connection_kind: ConnectionKind::OpenAiChatCompletions,
        tool_profile: crate::config::ToolProfile::PythonOnly,
        ..Default::default()
    };
    config.python_runtime.target = Some(crate::config::PythonExecutionTarget::Host);
    let guidance = crate::system_prompt::python_capability_guidance(&config).unwrap();
    let mut context = ContextManager::new(100_000, Some(format!("Template\n\n{guidance}")));
    context.add_message("user", "run one cell");
    let (tx, mut rx) = mpsc::unbounded_channel();
    let session = tempfile::tempdir().unwrap();
    trigger_llm_request_with_surface(
        Client::new(),
        config,
        &context,
        tx,
        CancellationToken::new(),
        false,
        Some(session.path().to_string_lossy().into_owned()),
        crate::tools::ToolSurface::Headless,
    )
    .unwrap();

    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap()
        {
            StreamEvent::Done { .. } => break,
            StreamEvent::Error(error) => panic!("provider request failed: {error}"),
            _ => {}
        }
    }
    let wire_body = server.await.unwrap();
    let diagnostic = std::fs::read(session.path().join("last_request.json")).unwrap();
    assert_eq!(diagnostic, wire_body);
    let body: serde_json::Value = serde_json::from_slice(&wire_body).unwrap();
    assert_eq!(body["tools"].as_array().unwrap().len(), 1);
    assert_eq!(body["tools"][0]["function"]["name"], "python");
    assert_eq!(body["parallel_tool_calls"], false);
}

#[tokio::test]
async fn persisted_agent_diagnostic_is_the_exact_anthropic_wire_body() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request_body = read_http_request(&mut socket).await;
        let body = [
            serde_json::json!({"type":"message_start","message":{"content":[],"usage":{"input_tokens":10,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}),
            serde_json::json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            serde_json::json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"ok"}}),
            serde_json::json!({"type":"content_block_stop","index":0}),
            serde_json::json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}),
            serde_json::json!({"type":"message_stop"}),
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
        request_body
    });

    let mut config = Config {
        server_url: format!("http://{address}/v1/messages"),
        model: "policy-test-model".to_string(),
        context_size: 100_000,
        connection_kind: ConnectionKind::ClaudeCodeProxy,
        tool_profile: crate::config::ToolProfile::PythonOnly,
        ..Default::default()
    };
    config.python_runtime.target = Some(crate::config::PythonExecutionTarget::Host);
    let guidance = crate::system_prompt::python_capability_guidance(&config).unwrap();
    let mut context = ContextManager::new(100_000, Some(format!("Template\n\n{guidance}")));
    context.add_message("user", "run one cell");
    let (tx, mut rx) = mpsc::unbounded_channel();
    let session = tempfile::tempdir().unwrap();
    trigger_llm_request_with_surface(
        Client::new(),
        config,
        &context,
        tx,
        CancellationToken::new(),
        false,
        Some(session.path().to_string_lossy().into_owned()),
        crate::tools::ToolSurface::Headless,
    )
    .unwrap();

    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap()
        {
            StreamEvent::Done { .. } => break,
            StreamEvent::Error(error) => panic!("provider request failed: {error}"),
            _ => {}
        }
    }
    let wire_body = server.await.unwrap();
    let diagnostic = std::fs::read(session.path().join("last_request.json")).unwrap();
    assert_eq!(diagnostic, wire_body);
    let body: serde_json::Value = serde_json::from_slice(&wire_body).unwrap();
    assert_eq!(body["tools"].as_array().unwrap().len(), 1);
    assert_eq!(body["tools"][0]["name"], "python");
    assert_eq!(body["tool_choice"]["disable_parallel_tool_use"], true);
}

#[tokio::test]
async fn prepared_request_reuses_one_fresh_file_snapshot_after_durable_start_seam() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request_body = read_http_request(&mut socket).await;
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":1}}\n\n",
            "data: [DONE]\n\n"
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
        request_body
    });

    let config = Config {
        server_url: format!("http://{address}/v1"),
        model: "snapshot-test-model".to_string(),
        context_size: 4_096,
        connection_kind: ConnectionKind::OpenAiChatCompletions,
        ..Default::default()
    };
    let workspace = tempfile::tempdir().unwrap();
    let file = workspace.path().join("changing.txt");
    std::fs::write(&file, "cached-small").unwrap();
    let mut context = ContextManager::new(4_096, None);
    context.set_cwd(workspace.path().to_string_lossy().into_owned());
    context.update_latest_file("changing.txt".to_string(), "cached-small".to_string());
    context.add_message("user", "inspect");
    std::fs::write(&file, "PREPARED-SNAPSHOT-V1").unwrap();
    let prepared =
        prepare_llm_request(&config, &context, crate::tools::ToolSurface::Interactive).unwrap();
    let request_id = prepared.accounting_start().request_id.clone();

    // This simulates the filesystem changing while the already prepared request's
    // accounting start is durably persisted.
    std::fs::write(&file, "MUTATED-AFTER-PREPARE-V2").unwrap();
    let session = tempfile::tempdir().unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let actual_id = trigger_prepared_llm_request_with_hook(
        Client::new(),
        prepared,
        tx,
        CancellationToken::new(),
        false,
        Some(session.path().to_string_lossy().into_owned()),
        None,
    )
    .unwrap();
    assert_eq!(actual_id, request_id);
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap()
        {
            StreamEvent::Done { .. } => break,
            StreamEvent::Error(error) => panic!("prepared request failed: {error}"),
            _ => {}
        }
    }

    let wire_body = server.await.unwrap();
    let encoded = String::from_utf8(wire_body.clone()).unwrap();
    assert!(encoded.contains("PREPARED-SNAPSHOT-V1"), "{encoded}");
    assert!(!encoded.contains("MUTATED-AFTER-PREPARE-V2"), "{encoded}");
    assert_eq!(
        std::fs::read(session.path().join("last_request.json")).unwrap(),
        wire_body
    );
}

#[test]
fn parent_settlement_route_survives_successful_private_send_then_receiver_drop() {
    let (private_tx, private_rx) = mpsc::unbounded_channel();
    let (parent_tx, mut parent_rx) = mpsc::unbounded_channel();
    let private_send_succeeded = send_with_settlement_fallback(
        &private_tx,
        Some(&parent_tx),
        StreamEvent::RequestSettlementFailed {
            request_id: "nested-race-request".to_string(),
            error: "simulated terminal checkpoint failure".to_string(),
            cancellation_requested: true,
        },
    );
    assert!(private_send_succeeded);

    // The private send was accepted into a queue that is now destroyed
    // without another receiver poll. Parent visibility must not depend on it.
    drop(private_rx);
    let event = parent_rx
        .try_recv()
        .expect("parent settlement route did not receive an independent event");
    assert!(matches!(
        event,
        StreamEvent::RequestSettlementFailed {
            request_id,
            cancellation_requested: true,
            ..
        } if request_id == "nested-race-request"
    ));
}

#[tokio::test]
async fn ready_provider_event_is_drained_before_cancellation() {
    let usage = crate::accounting::Usage {
        uncached_input_tokens: 7,
        output_tokens: 2,
        total_input_tokens: Some(7),
        breakdown_complete: true,
        ..Default::default()
    };
    let mut stream: transport::EventStream = Box::pin(futures_util::stream::iter([
        transport::StreamEvent::UsageUpdate(usage),
    ]));
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let (tx, _rx) = mpsc::unbounded_channel();

    match poll_provider_stream(&mut stream, &cancellation, &tx).await {
        ProviderStreamPoll::Event(Some(transport::StreamEvent::UsageUpdate(actual))) => {
            assert_eq!(actual, usage);
        }
        _ => panic!("a ready provider usage event must beat cancellation"),
    }
}

#[test]
fn request_accounting_prices_each_cache_aware_request() {
    let pricing = serde_yaml::from_str(
            "applies_to_models: [gpt-5.6-sol]\ncurrency: USD\nunit_tokens: 1000000\neffective_as_of: 2026-08-25\nvalid_through: 2026-11-21\nprovenance:\n  kind: api_equivalent_estimate\n  url: https://example.com/pricing\nrates:\n  uncached_input: 4.0\n  cached_read_input: 0.4\n  cache_creation_input: 5.0\n  output: 20.0\nlong_context:\n  threshold_input_tokens: 272000\n  applies_above_threshold: true\n  input_multiplier: 2.0\n  output_multiplier: 1.5\n",
        )
        .unwrap();
    let config = Config {
        model: "gpt-5.6-sol".to_string(),
        server_url: "http://proxy".to_string(),
        pricing: Some(pricing),
        ..Default::default()
    };
    let usage = crate::accounting::Usage {
        uncached_input_tokens: 10_000,
        cache_read_input_tokens: 90_000,
        output_tokens: 10_000,
        total_input_tokens: Some(100_000),
        breakdown_complete: true,
        ..Default::default()
    };

    let request = request_accounting(&config, "request-1", Some(usage), true);
    assert!(request.usage_reported);
    assert!(request.completed);
    assert_eq!(request.estimated_cost.unwrap().nanos, 276_000_000);

    let missing = request_accounting(&config, "request-2", None, false);
    assert!(!missing.usage_reported);
    assert!(!missing.completed);
    assert!(missing.estimated_cost.is_none());
    assert!(!missing.usage.breakdown_complete);
}

fn assert_nested_terminal_checkpoint_failure(
    mut rx: mpsc::UnboundedReceiver<StreamEvent>,
    expected_context: &str,
) {
    let mut started_id = None;
    let mut settlement = None;
    while let Ok(event) = rx.try_recv() {
        match event {
            StreamEvent::RequestStarted(request) => started_id = Some(request.request_id),
            StreamEvent::RequestSettlementFailed {
                request_id,
                error,
                cancellation_requested,
            } => settlement = Some((request_id, error, cancellation_requested)),
            StreamEvent::RequestFinished(request) => {
                panic!(
                    "failed nested checkpoint emitted RequestFinished for {}",
                    request.request_id
                )
            }
            _ => {}
        }
    }
    let started_id = started_id.expect("nested request start was not emitted");
    let (request_id, error, cancellation_requested) =
        settlement.expect("typed nested settlement failure was not emitted");
    assert_eq!(request_id, started_id);
    assert!(cancellation_requested);
    assert!(error.contains(expected_context), "{error}");
    assert!(error.contains("simulated nested checkpoint failure"));
}

fn fail_nested_terminal_checkpoint_hook() -> RequestAccountingHook {
    std::sync::Arc::new(|checkpoint| {
        if checkpoint.request.in_flight {
            Ok(())
        } else {
            Err("simulated nested checkpoint failure".to_string())
        }
    })
}

#[tokio::test]
async fn cancelled_summarization_checkpoint_failure_emits_exact_typed_settlement() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = Config {
        server_url: format!("http://{}/v1", listener.local_addr().unwrap()),
        model: "test-model".to_string(),
        context_size: 4096,
        connection_kind: ConnectionKind::OpenAiChatCompletions,
        ..Default::default()
    };
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let (tx, rx) = mpsc::unbounded_channel();

    let error = summarize_llm_accounted_with_hook(
        &Client::new(),
        &config,
        "content",
        "summarize",
        &tx,
        cancellation,
        Some(fail_nested_terminal_checkpoint_hook()),
    )
    .await
    .unwrap_err();

    assert!(
        error.contains("summarization terminal accounting checkpoint failed"),
        "{error}"
    );
    assert_nested_terminal_checkpoint_failure(
        rx,
        "summarization terminal accounting checkpoint failed",
    );
    drop(listener);
}

#[tokio::test]
async fn cancelled_single_response_checkpoint_failure_emits_exact_typed_settlement() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = Config {
        server_url: format!("http://{}/v1", listener.local_addr().unwrap()),
        model: "test-model".to_string(),
        context_size: 4096,
        connection_kind: ConnectionKind::OpenAiChatCompletions,
        ..Default::default()
    };
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let (tx, rx) = mpsc::unbounded_channel();

    let error = get_single_response_with_usage_and_hook(
        &Client::new(),
        &config,
        "inspect image".to_string(),
        None,
        Some(&tx),
        cancellation,
        Some(fail_nested_terminal_checkpoint_hook()),
    )
    .await
    .unwrap_err();

    assert!(
        error.contains("single-response terminal accounting checkpoint failed"),
        "{error}"
    );
    assert_nested_terminal_checkpoint_failure(
        rx,
        "single-response terminal accounting checkpoint failed",
    );
    drop(listener);
}

#[tokio::test]
async fn accounted_single_response_requires_parent_settlement_sender() {
    let error = get_single_response_with_usage_and_hook(
        &Client::new(),
        &Config::default(),
        "inspect image".to_string(),
        None,
        None,
        CancellationToken::new(),
        Some(fail_nested_terminal_checkpoint_hook()),
    )
    .await
    .unwrap_err();
    assert!(
        error.contains("require a settlement event sender"),
        "{error}"
    );
}

#[tokio::test]
async fn rejected_nested_completion_hooks_final_usage() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_http_request(&mut socket).await;
        let body = serde_json::json!({
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

    let config = Config {
        server_url: format!("http://{address}/v1/messages"),
        model: "gpt-5.6-sol".to_string(),
        connection_kind: ConnectionKind::ClaudeCodeProxy,
        ..Default::default()
    };
    let recorded = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let hook_records = recorded.clone();
    let hook: RequestStartedHook = std::sync::Arc::new(move |checkpoint| {
        hook_records
            .lock()
            .unwrap()
            .push(checkpoint.request.clone());
        Ok(())
    });
    let (tx, _rx) = mpsc::unbounded_channel();
    let error = summarize_llm_accounted_with_hook(
        &Client::new(),
        &config,
        "content",
        "summarize",
        &tx,
        CancellationToken::new(),
        Some(hook),
    )
    .await
    .unwrap_err();
    server.await.unwrap();

    assert!(error.contains("max_tokens"));
    let recorded = recorded.lock().unwrap();
    assert_eq!(recorded.len(), 2);
    assert_eq!(recorded[0].request_id, recorded[1].request_id);
    assert!(recorded[0].in_flight);
    assert!(!recorded[1].in_flight);
    assert!(!recorded[1].completed);
    assert!(recorded[1].usage_reported);
    assert_eq!(recorded[1].usage.total_input_tokens, Some(20));
    assert_eq!(recorded[1].usage.output_tokens, 9);
}

#[tokio::test]
async fn failed_terminal_checkpoint_blocks_tool_authorization() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_http_request(&mut socket).await;
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"provider-call-1\",\"function\":{\"name\":\"todowrite\",\"arguments\":\"{\\\"todos\\\":[]}\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":2}}\n\n",
            "data: [DONE]\n\n"
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
    let config = Config {
        server_url: format!("http://{address}/v1"),
        model: "test-model".to_string(),
        context_size: 4096,
        connection_kind: ConnectionKind::OpenAiChatCompletions,
        ..Default::default()
    };
    let mut context = ContextManager::new(4096, None);
    context.add_message("user", "make a todo");
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let calls_for_hook = calls.clone();
    let hook: RequestAccountingHook = std::sync::Arc::new(move |checkpoint| {
        calls_for_hook.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if checkpoint.request.in_flight {
            Ok(())
        } else {
            Err("simulated durable storage failure".to_string())
        }
    });
    let (tx, mut rx) = mpsc::unbounded_channel();
    trigger_llm_request_with_surface_and_start_hook(
        Client::new(),
        config,
        &context,
        tx,
        CancellationToken::new(),
        false,
        None,
        crate::tools::ToolSurface::Headless,
        Some(hook),
    )
    .unwrap();

    loop {
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        match event {
            StreamEvent::RequestSettlementFailed {
                request_id: _,
                error,
                cancellation_requested,
            } => {
                assert!(!cancellation_requested);
                assert!(error.contains("terminal provider response checkpoint failed"));
                assert!(error.contains("simulated durable storage failure"));
                break;
            }
            StreamEvent::Error(error) => {
                panic!("checkpoint failure used untyped error event: {error}")
            }
            StreamEvent::RequestFinished(_)
            | StreamEvent::ToolCalls { .. }
            | StreamEvent::Done { .. } => {
                panic!("unjournaled terminal response reached authorization events")
            }
            _ => {}
        }
    }
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    server.await.unwrap();
}

#[tokio::test]
async fn failed_cancellation_checkpoint_emits_typed_terminal_settlement() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = Config {
        server_url: format!("http://{}/v1", listener.local_addr().unwrap()),
        model: "test-model".to_string(),
        context_size: 4096,
        connection_kind: ConnectionKind::OpenAiChatCompletions,
        ..Default::default()
    };
    let context = ContextManager::new(4096, None);
    let started = prepare_provider_request(&config);
    let expected_request_id = started.request_id.clone();
    let checkpoint_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let calls_for_hook = checkpoint_calls.clone();
    let hook: RequestAccountingHook = std::sync::Arc::new(move |_| {
        calls_for_hook.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err("simulated cancellation checkpoint failure".to_string())
    });
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let (tx, mut rx) = mpsc::unbounded_channel();

    let request_id = trigger_llm_request_with_surface_and_prepared_start_and_hook(
        Client::new(),
        config,
        &context,
        tx,
        cancellation,
        false,
        None,
        crate::tools::ToolSurface::Headless,
        started,
        Some(hook),
    )
    .unwrap();
    assert_eq!(request_id, expected_request_id);

    loop {
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("cancelled request settlement timed out")
            .expect("cancelled request event channel closed");
        match event {
            StreamEvent::RequestSettlementFailed {
                request_id,
                error,
                cancellation_requested,
            } => {
                assert_eq!(request_id, expected_request_id);
                assert!(cancellation_requested);
                assert!(error.contains("provider cancellation accounting checkpoint failed"));
                assert!(error.contains("simulated cancellation checkpoint failure"));
                break;
            }
            StreamEvent::RequestFinished(_) | StreamEvent::RequestCancelled { .. } => {
                panic!("failed cancellation checkpoint reported a successful settlement")
            }
            StreamEvent::Error(error) => {
                panic!("cancellation checkpoint failure used untyped error event: {error}")
            }
            _ => {}
        }
    }
    assert_eq!(
        checkpoint_calls.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    drop(listener);
}

#[tokio::test]
async fn tool_call_waits_for_delayed_terminal_usage() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (finish_sent_tx, finish_sent_rx) = tokio::sync::oneshot::channel();
    let (release_usage_tx, release_usage_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_http_request(&mut socket).await;
        let before_usage = concat!(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"provider-call-1\",\"function\":{\"name\":\"todowrite\",\"arguments\":\"{\\\"todos\\\":[]}\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n"
        );
        let usage = concat!(
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":3,\"uncached_input_tokens\":20,\"cache_read_input_tokens\":80,\"cache_creation_input_tokens\":0}}\n\n",
            "data: [DONE]\n\n"
        );
        socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        before_usage.len() + usage.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        socket.write_all(before_usage.as_bytes()).await.unwrap();
        socket.flush().await.unwrap();
        finish_sent_tx.send(()).unwrap();
        release_usage_rx.await.unwrap();
        socket.write_all(usage.as_bytes()).await.unwrap();
        socket.shutdown().await.unwrap();
    });

    let config = Config {
        server_url: format!("http://{address}/v1"),
        model: "test-model".to_string(),
        context_size: 4096,
        connection_kind: ConnectionKind::OpenAiChatCompletions,
        ..Default::default()
    };
    let mut context = ContextManager::new(4096, None);
    context.add_message("user", "make a todo");
    let (tx, mut rx) = mpsc::unbounded_channel();
    let session = tempfile::tempdir().unwrap();
    let checkpoints = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let checkpoints_for_hook = checkpoints.clone();
    let hook: RequestAccountingHook = std::sync::Arc::new(move |checkpoint| {
        checkpoints_for_hook
            .lock()
            .unwrap()
            .push(checkpoint.clone());
        Ok(())
    });
    let request_id = trigger_llm_request_with_surface_and_start_hook(
        Client::new(),
        config,
        &context,
        tx,
        CancellationToken::new(),
        false,
        Some(session.path().to_string_lossy().into_owned()),
        crate::tools::ToolSurface::Headless,
        Some(hook),
    )
    .unwrap();

    finish_sent_rx.await.unwrap();
    let mut saw_prepare = false;
    let mut started_count = 0;
    while !saw_prepare {
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("client did not process the pre-usage tool chunk")
            .expect("client channel closed before delayed usage");
        match event {
            StreamEvent::RequestStarted(accounting) => {
                assert_eq!(accounting.request_id, request_id);
                started_count += 1;
            }
            StreamEvent::PreparingToolCall(name) => {
                saw_prepare = true;
                assert_eq!(name, "todowrite");
            }
            StreamEvent::ToolCalls { .. }
            | StreamEvent::Done { .. }
            | StreamEvent::RequestFinished(_) => {
                panic!("provider request became terminal before delayed usage")
            }
            StreamEvent::Error(error) => panic!("unexpected stream error: {error}"),
            _ => {}
        }
    }
    assert_eq!(started_count, 1);
    release_usage_tx.send(()).unwrap();

    let mut terminal_order = Vec::new();
    let mut finished = None;
    loop {
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        match event {
            StreamEvent::RequestFinished(accounting) => {
                assert_eq!(accounting.request_id, request_id);
                finished = Some(accounting);
                terminal_order.push("finished");
            }
            StreamEvent::ToolCalls { calls, .. } => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].id, "provider-call-1");
                terminal_order.push("tools");
            }
            StreamEvent::Done {
                request_id: done_request_id,
                ..
            } => {
                assert_eq!(done_request_id, request_id);
                terminal_order.push("done");
                break;
            }
            StreamEvent::Error(error) => panic!("unexpected stream error: {error}"),
            _ => {}
        }
    }
    assert_eq!(terminal_order, ["finished", "tools", "done"]);
    let finished = finished.unwrap();
    assert!(finished.completed);
    assert!(finished.usage_reported);
    assert_eq!(finished.usage.total_input_tokens, Some(100));
    assert_eq!(finished.usage.output_tokens, 3);
    let checkpoints = checkpoints.lock().unwrap();
    assert_eq!(checkpoints.len(), 2);
    assert!(checkpoints[0].request.in_flight);
    assert!(checkpoints[0].transcript.is_none());
    assert!(checkpoints[1].request.completed);
    assert_eq!(checkpoints[1].request.usage.total_input_tokens, Some(100));
    let transcript = checkpoints[1].transcript.as_ref().unwrap();
    assert_eq!(transcript.len(), 2);
    assert_eq!(transcript[0].role, "user");
    let assistant = &transcript[1];
    assert_eq!(assistant.role, "assistant");
    assert_eq!(assistant.content, "");
    assert_eq!(assistant.tool_calls.as_ref().unwrap().len(), 1);
    assert_eq!(
        assistant.tool_calls.as_ref().unwrap()[0].id,
        "provider-call-1"
    );
    drop(checkpoints);
    server.await.unwrap();
}

#[tokio::test]
async fn dropping_event_receiver_closes_provider_body() {
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
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"waiting\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":2}}\n\n"
        )
        .as_bytes();
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
            .expect("dropping the event receiver left the provider body open")
            .unwrap()
            == 0
    });

    let config = Config {
        server_url: format!("http://{address}/v1"),
        model: "test-model".to_string(),
        context_size: 4096,
        connection_kind: ConnectionKind::OpenAiChatCompletions,
        ..Default::default()
    };
    let mut context = ContextManager::new(4096, None);
    context.add_message("user", "wait");
    let (tx, mut rx) = mpsc::unbounded_channel();
    let (fallback_tx, mut fallback_rx) = mpsc::unbounded_channel();
    let session = tempfile::tempdir().unwrap();
    let checkpoints = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let checkpoints_for_hook = checkpoints.clone();
    let hook: RequestAccountingHook = std::sync::Arc::new(move |checkpoint| {
        checkpoints_for_hook
            .lock()
            .unwrap()
            .push(checkpoint.clone());
        Ok(())
    });
    let request_id = trigger_llm_request_with_surface_and_start_hook_and_settlement_fallback(
        Client::new(),
        config,
        &context,
        tx,
        Some(fallback_tx),
        CancellationToken::new(),
        false,
        Some(session.path().to_string_lossy().into_owned()),
        crate::tools::ToolSurface::Headless,
        Some(hook),
    )
    .unwrap();

    loop {
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        if let StreamEvent::UsageUpdate { usage, .. } = event {
            assert_eq!(usage.total_input_tokens, Some(10));
            assert_eq!(usage.output_tokens, 2);
            break;
        }
    }
    drop(rx);
    assert!(server.await.unwrap());
    let (accounting, saw_start) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        let mut saw_start = false;
        loop {
            match fallback_rx
                .recv()
                .await
                .expect("parent settlement fallback closed unexpectedly")
            {
                StreamEvent::RequestStarted(started) => {
                    assert_eq!(started.request_id, request_id);
                    saw_start = true;
                }
                StreamEvent::RequestFinished(accounting) => break (accounting, saw_start),
                other => {
                    panic!("receiver closure emitted unexpected fallback event: {other:?}")
                }
            }
        }
    })
    .await
    .expect("receiver closure did not use the parent settlement fallback");
    assert!(
        saw_start,
        "terminal settlement arrived before parent-visible start"
    );
    assert_eq!(accounting.request_id, request_id);
    assert!(!accounting.in_flight);
    assert!(!accounting.completed);
    assert!(accounting.usage_reported);
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if checkpoints.lock().unwrap().len() == 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("receiver closure was not terminally checkpointed");
    let checkpoints = checkpoints.lock().unwrap();
    assert_eq!(checkpoints.len(), 2);
    assert_eq!(
        checkpoints[0].request.request_id,
        checkpoints[1].request.request_id
    );
    assert!(checkpoints[0].request.in_flight);
    assert!(!checkpoints[1].request.in_flight);
    assert!(!checkpoints[1].request.completed);
    assert!(checkpoints[1].request.usage_reported);
    assert_eq!(checkpoints[1].request.usage.total_input_tokens, Some(10));
    assert_eq!(checkpoints[1].request.usage.output_tokens, 2);
}

#[tokio::test]
async fn pre_header_receiver_closure_routes_terminal_checkpoint_failure_to_fallback() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (request_received_tx, request_received_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_http_request(&mut socket).await;
        request_received_tx.send(()).unwrap();
        let mut byte = [0_u8; 1];
        tokio::time::timeout(std::time::Duration::from_secs(2), socket.read(&mut byte))
            .await
            .expect("dropping the receiver before headers left the request open")
            .unwrap()
            == 0
    });

    let config = Config {
        server_url: format!("http://{address}/v1"),
        model: "test-model".to_string(),
        context_size: 4096,
        connection_kind: ConnectionKind::OpenAiChatCompletions,
        ..Default::default()
    };
    let mut context = ContextManager::new(4096, None);
    context.add_message("user", "wait for headers");
    let (tx, rx) = mpsc::unbounded_channel();
    let (fallback_tx, mut fallback_rx) = mpsc::unbounded_channel();
    let session = tempfile::tempdir().unwrap();
    let checkpoints = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let checkpoints_for_hook = checkpoints.clone();
    let hook: RequestAccountingHook = std::sync::Arc::new(move |checkpoint| {
        checkpoints_for_hook
            .lock()
            .unwrap()
            .push(checkpoint.clone());
        if checkpoint.request.in_flight {
            Ok(())
        } else {
            Err("simulated receiver-closure checkpoint failure".to_string())
        }
    });
    let request_id = trigger_llm_request_with_surface_and_start_hook_and_settlement_fallback(
        Client::new(),
        config,
        &context,
        tx,
        Some(fallback_tx),
        CancellationToken::new(),
        false,
        Some(session.path().to_string_lossy().into_owned()),
        crate::tools::ToolSurface::Headless,
        Some(hook),
    )
    .unwrap();

    request_received_rx.await.unwrap();
    drop(rx);
    assert!(server.await.unwrap());
    let (settled_id, error, cancellation_requested, saw_start) =
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            let mut saw_start = false;
            loop {
                match fallback_rx
                    .recv()
                    .await
                    .expect("settlement fallback closed unexpectedly")
                {
                    StreamEvent::RequestStarted(started) => {
                        assert_eq!(started.request_id, request_id);
                        saw_start = true;
                    }
                    StreamEvent::RequestSettlementFailed {
                        request_id,
                        error,
                        cancellation_requested,
                    } => break (request_id, error, cancellation_requested, saw_start),
                    other => {
                        panic!("receiver closure emitted unexpected fallback event: {other:?}")
                    }
                }
            }
        })
        .await
        .expect("pre-header receiver closure did not use the settlement fallback");
    assert_eq!(settled_id, request_id);
    assert!(
        saw_start,
        "terminal settlement arrived before parent-visible start"
    );
    assert!(!cancellation_requested);
    assert!(
        error.contains("provider receiver-closure accounting checkpoint failed"),
        "{error}"
    );
    assert!(
        error.contains("simulated receiver-closure checkpoint failure"),
        "{error}"
    );
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if checkpoints.lock().unwrap().len() == 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("pre-header receiver closure was not terminally checkpointed");
    let checkpoints = checkpoints.lock().unwrap();
    assert_eq!(checkpoints.len(), 2);
    assert_eq!(
        checkpoints[0].request.request_id,
        checkpoints[1].request.request_id
    );
    assert!(checkpoints[0].request.in_flight);
    assert!(!checkpoints[1].request.in_flight);
    assert!(!checkpoints[1].request.completed);
    assert!(!checkpoints[1].request.usage_reported);
}

#[tokio::test]
async fn cancelled_reasoning_is_not_replayed_as_empty_native_assistant() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        read_http_request(&mut first).await;
        first
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\n\r\n",
            )
            .await
            .unwrap();
        let first_events = [
            serde_json::json!({"type":"message_start","message":{"content":[],"usage":{"input_tokens":3,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}),
            serde_json::json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}),
            serde_json::json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"cancelled private reasoning"}}),
        ]
        .into_iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>();
        first
            .write_all(format!("{:x}\r\n", first_events.len()).as_bytes())
            .await
            .unwrap();
        first.write_all(first_events.as_bytes()).await.unwrap();
        first.write_all(b"\r\n").await.unwrap();
        first.flush().await.unwrap();
        let mut closed = [0_u8; 1];
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(2), first.read(&mut closed))
                .await
                .expect("cancelled first request did not close")
                .unwrap(),
            0
        );

        let (mut second, _) = listener.accept().await.unwrap();
        let second_body = read_http_request(&mut second).await;
        let response = [
            serde_json::json!({"type":"message_start","message":{"content":[],"usage":{"input_tokens":5,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}),
            serde_json::json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            serde_json::json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"continued"}}),
            serde_json::json!({"type":"content_block_stop","index":0}),
            serde_json::json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}),
            serde_json::json!({"type":"message_stop"}),
        ]
        .into_iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>();
        second
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        second.write_all(response.as_bytes()).await.unwrap();
        second.shutdown().await.unwrap();
        second_body
    });

    let config = Config {
        server_url: format!("http://{address}/v1/messages"),
        model: "native-test-model".to_string(),
        context_size: 4_096,
        connection_kind: ConnectionKind::ClaudeCodeProxy,
        ..Default::default()
    };
    let mut context = ContextManager::new(4_096, None);
    context.add_message("user", "first request");
    let session = tempfile::tempdir().unwrap();
    let session_dir = session.path().to_string_lossy().into_owned();
    let first_cancel = CancellationToken::new();
    let (first_tx, mut first_rx) = mpsc::unbounded_channel();
    trigger_llm_request_with_surface(
        Client::new(),
        config.clone(),
        &context,
        first_tx,
        first_cancel.clone(),
        false,
        Some(session_dir.clone()),
        crate::tools::ToolSurface::Interactive,
    )
    .unwrap();

    let mut partial = String::new();
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(2), first_rx.recv())
            .await
            .unwrap()
            .unwrap()
        {
            StreamEvent::Chunk(chunk) => {
                partial.push_str(&chunk);
                if partial.contains("cancelled private reasoning") {
                    first_cancel.cancel();
                }
            }
            StreamEvent::RequestCancelled { .. } => break,
            StreamEvent::Done { .. } => panic!("cancelled reasoning request completed"),
            StreamEvent::Error(error) => panic!("cancelled reasoning request failed: {error}"),
            _ => {}
        }
    }

    context.add_assistant_message(&partial, None);
    context.add_message("user", "continue after cancellation");
    assert_eq!(context.get_messages()[1].content, partial);
    let (second_tx, mut second_rx) = mpsc::unbounded_channel();
    trigger_llm_request_with_surface(
        Client::new(),
        config,
        &context,
        second_tx,
        CancellationToken::new(),
        false,
        Some(session_dir.clone()),
        crate::tools::ToolSurface::Interactive,
    )
    .unwrap();
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(2), second_rx.recv())
            .await
            .unwrap()
            .unwrap()
        {
            StreamEvent::Done { .. } => break,
            StreamEvent::Error(error) => panic!("follow-up request failed: {error}"),
            _ => {}
        }
    }

    let body: serde_json::Value = serde_json::from_slice(&server.await.unwrap()).unwrap();
    let wire_messages = body["messages"].as_array().unwrap();
    assert!(wire_messages.iter().all(|message| {
        message["role"] != "assistant"
            || message["content"]
                .as_array()
                .is_none_or(|content| !content.is_empty())
    }));
    assert!(!body.to_string().contains("cancelled private reasoning"));
    assert_eq!(context.get_messages()[1].content, partial);
}

#[tokio::test]
async fn cancellation_closes_body_and_emits_one_terminal_sequence() {
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
        let body = b"data: {\"choices\":[{\"delta\":{\"content\":\"waiting\"}}]}\n\n";
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
            .expect("cancelled provider body remained open")
            .unwrap()
            == 0
    });

    let config = Config {
        server_url: format!("http://{address}/v1"),
        model: "test-model".to_string(),
        context_size: 4096,
        connection_kind: ConnectionKind::OpenAiChatCompletions,
        ..Default::default()
    };
    let mut context = ContextManager::new(4096, None);
    context.add_message("user", "wait");
    let (tx, mut rx) = mpsc::unbounded_channel();
    let session = tempfile::tempdir().unwrap();
    let cancellation = CancellationToken::new();
    let request_id = trigger_llm_request_with_surface(
        Client::new(),
        config,
        &context,
        tx,
        cancellation.clone(),
        false,
        Some(session.path().to_string_lossy().into_owned()),
        crate::tools::ToolSurface::Headless,
    )
    .unwrap();

    let mut sequence = Vec::new();
    loop {
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        match event {
            StreamEvent::RequestStarted(accounting) => {
                assert_eq!(accounting.request_id, request_id);
                sequence.push("started");
            }
            StreamEvent::Chunk(text) if text == "waiting" => {
                cancellation.cancel();
            }
            StreamEvent::RequestFinished(accounting) => {
                assert_eq!(accounting.request_id, request_id);
                assert!(!accounting.completed);
                sequence.push("finished");
            }
            StreamEvent::RequestCancelled {
                request_id: cancelled_id,
            } => {
                assert_eq!(cancelled_id, request_id);
                sequence.push("cancelled");
                break;
            }
            StreamEvent::Done { .. } => panic!("cancelled request emitted Done"),
            StreamEvent::Error(error) => {
                panic!("cancelled request emitted provider error: {error}")
            }
            _ => {}
        }
    }
    assert_eq!(sequence, ["started", "finished", "cancelled"]);
    assert!(server.await.unwrap());
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn premature_eof_emits_finished_then_error_only() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
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

    let config = Config {
        server_url: format!("http://{address}/v1"),
        model: "test-model".to_string(),
        context_size: 4096,
        connection_kind: ConnectionKind::OpenAiChatCompletions,
        ..Default::default()
    };
    let mut context = ContextManager::new(4096, None);
    context.add_message("user", "fail cleanly");
    let (tx, mut rx) = mpsc::unbounded_channel();
    let session = tempfile::tempdir().unwrap();
    let request_id = trigger_llm_request_with_surface(
        Client::new(),
        config,
        &context,
        tx,
        CancellationToken::new(),
        false,
        Some(session.path().to_string_lossy().into_owned()),
        crate::tools::ToolSurface::Headless,
    )
    .unwrap();

    let mut sequence = Vec::new();
    loop {
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        match event {
            StreamEvent::RequestStarted(accounting) => {
                assert_eq!(accounting.request_id, request_id);
                sequence.push("started");
            }
            StreamEvent::RequestFinished(accounting) => {
                assert_eq!(accounting.request_id, request_id);
                assert!(!accounting.completed);
                sequence.push("finished");
            }
            StreamEvent::Error(error) => {
                assert!(error.contains("closed the reply before finishing"), "{error}");
                sequence.push("error");
                break;
            }
            StreamEvent::Done { .. } => panic!("premature EOF emitted Done"),
            StreamEvent::RequestCancelled { .. } => {
                panic!("premature EOF emitted cancellation")
            }
            _ => {}
        }
    }
    assert_eq!(sequence, ["started", "finished", "error"]);
    server.await.unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .is_none()
    );
}

#[test]
fn preparation_never_drops_the_active_user_prompt_to_fit_fixed_context() {
    let config = Config {
        context_size: 10,
        ..Default::default()
    };
    let mut context = ContextManager::new(10, Some("s".repeat(36)));
    context.add_message("user", "12345678");

    let error =
        prepare_llm_request(&config, &context, crate::tools::ToolSurface::Interactive).unwrap_err();

    assert!(error.contains("exceeds the configured input token budget"));
    assert_eq!(context.get_messages().len(), 1);
    assert_eq!(context.get_messages()[0].content, "12345678");
    let prepared = context.prepare_api_context();
    assert!(prepared.estimated_tokens() > context.input_token_budget());
    assert!(
        prepared
            .messages()
            .iter()
            .any(|message| message.role == crate::transport::Role::User)
    );
}

#[test]
fn irreducible_system_context_is_rejected_before_request_preparation() {
    let config = Config {
        context_size: 10,
        ..Default::default()
    };
    let mut context = ContextManager::new(10, Some("x".repeat(1_000)));
    context.add_message("user", "not sent");

    let error =
        prepare_llm_request(&config, &context, crate::tools::ToolSurface::Interactive).unwrap_err();

    assert!(error.contains("exceeds the configured input token budget"));
}

#[test]
fn generated_request_ids_are_unique() {
    assert_ne!(next_request_id(), next_request_id());
}

#[test]
fn native_tool_calls_keep_provider_assigned_ids() {
    let arguments = serde_json::json!({"tool_call_id": "model-supplied"});
    assert_eq!(
        effective_tool_call_id(
            ConnectionKind::ClaudeCodeProxy,
            "toolu_provider",
            &arguments,
        ),
        "toolu_provider"
    );
    assert_eq!(
        effective_tool_call_id(
            ConnectionKind::OpenAiChatCompletions,
            "server-generated",
            &arguments,
        ),
        "model-supplied"
    );
}
