use super::*;

#[tokio::test]
async fn invalid_python_policy_is_rejected_before_session_creation() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config {
        tool_profile: ToolProfile::PythonOnly,
        ..Default::default()
    };

    let error = DurableHeadlessSession::open(temp.path(), &config, true, None)
        .await
        .err()
        .expect("invalid policy must be rejected");

    assert!(error.contains("Invalid Python-only policy"), "{error}");
    assert!(!temp.path().join(".lethetic").exists());
}

#[test]
fn retained_preflight_uses_notice_while_runtime_lock_is_held() {
    let temp = tempfile::tempdir().unwrap();
    let store = RuntimeStore::open_at(temp.path().join("state")).unwrap();
    let runtime_id = store.generate_runtime_id().unwrap();
    let lock = store.create_locked_runtime(&runtime_id).unwrap();
    assert!(store.try_lock_runtime(&runtime_id).unwrap().is_none());

    let workspace = temp.path().join("workspace");
    let container_id = "a".repeat(64);
    let notice = crate::python::PythonRuntimeNotice {
        container_id: container_id.clone(),
        container_name: format!("lethetic-python-{runtime_id}"),
        action: crate::python::RuntimeLaunchAction::Created,
        network: NetworkAccess::Nonlocal,
        mounted_cwd: workspace.clone(),
    };
    assert_eq!(
        retained_container_id_from_notice(Some(notice.clone()), &workspace).unwrap(),
        container_id
    );
    assert!(retained_container_id_from_notice(None, &workspace).is_err());

    let mut wrong_network = notice.clone();
    wrong_network.network = NetworkAccess::Full;
    assert!(retained_container_id_from_notice(Some(wrong_network), &workspace).is_err());
    assert!(retained_container_id_from_notice(Some(notice.clone()), temp.path()).is_err());
    let mut invalid_id = notice;
    invalid_id.container_id = "not-an-id".to_string();
    assert!(retained_container_id_from_notice(Some(invalid_id), &workspace).is_err());
    drop(lock);
}

fn message(role: &str, content: &str) -> crate::context::Message {
    crate::context::Message {
        role: role.to_string(),
        content: content.to_string(),
        tool_calls: None,
        provider_content: None,
        tool_result_is_error: false,
    }
}

fn tool_call_message(
    id: &str,
    provider_id: Option<&str>,
    name: &str,
    description: &str,
    content: &str,
) -> crate::context::Message {
    crate::context::Message {
        role: "assistant".to_string(),
        content: content.to_string(),
        tool_calls: Some(vec![crate::context::ToolCall {
            id: id.to_string(),
            provider_id: provider_id.map(str::to_string),
            function: crate::context::FunctionCall {
                name: name.to_string(),
                arguments: serde_json::json!({
                    "description": description,
                    "expression": "2 + 2"
                }),
            },
        }]),
        provider_content: provider_id.map(|provider_id| {
            vec![serde_json::json!({
                "type": "tool_use",
                "id": provider_id,
                "name": name,
                "input": {"expression": "2 + 2"}
            })]
        }),
        tool_result_is_error: false,
    }
}

fn tool_result_message(
    id: &str,
    name: &str,
    payload: &str,
    is_error: bool,
) -> crate::context::Message {
    crate::context::Message {
        role: "tool".to_string(),
        content: format!(
            "<|tool_response>response:{name}{{result:<|'|>{payload}<|'|>,tool_call_id:<|'|>{id}<|'|>}}<tool_response|><turn|>"
        ),
        tool_calls: None,
        provider_content: None,
        tool_result_is_error: is_error,
    }
}

fn tool_event(
    projection_id: &str,
    call_message: &crate::context::Message,
    ui: &str,
    is_error: bool,
) -> ToolTranscriptEvent {
    ToolTranscriptEvent {
        projection_id: projection_id.to_string(),
        call: call_message.tool_calls.as_ref().unwrap()[0].clone(),
        ui: ui.to_string(),
        is_error,
    }
}

fn started_request(request_id: &str) -> crate::accounting::ProviderRequestAccounting {
    crate::accounting::ProviderRequestAccounting {
        request_id: request_id.to_string(),
        connection_id: "proxy".to_string(),
        model: "test-model".to_string(),
        usage: Default::default(),
        usage_reported: false,
        estimated_cost: None,
        completed: false,
        in_flight: true,
    }
}

#[test]
fn streaming_checkpoint_upserts_one_partial_assistant_block() {
    let mut state = SessionState::default();
    state.blocks.push(RenderBlock::user("prompt"));
    state.messages = vec![message("user", "prompt")];
    let mut cursor = HeadlessCheckpointCursor::starting_after(1);
    let mut messages = vec![message("user", "prompt"), message("assistant", "par")];

    assert!(
        apply_transcript_checkpoint(
            &mut state,
            &mut cursor,
            &messages,
            TranscriptStage::Intermediate,
        )
        .unwrap()
    );
    let first_index = cursor.streaming_block_index.unwrap();
    assert_eq!(state.blocks[first_index].content, "par");
    let block_count = state.blocks.len();

    messages[1].content = "partial response".to_string();
    assert!(
        apply_transcript_checkpoint(
            &mut state,
            &mut cursor,
            &messages,
            TranscriptStage::Intermediate,
        )
        .unwrap()
    );
    assert_eq!(state.blocks.len(), block_count);
    assert_eq!(state.blocks[first_index].content, "partial response");
    assert_eq!(state.messages.last().unwrap().content, "partial response");

    assert!(
        apply_transcript_checkpoint(&mut state, &mut cursor, &messages, TranscriptStage::Final,)
            .unwrap()
    );
    assert!(cursor.streaming_block_index.is_none());
    assert_eq!(cursor.committed_messages, 2);
    assert_eq!(state.blocks.len(), block_count);
    assert!(
        !apply_transcript_checkpoint(&mut state, &mut cursor, &messages, TranscriptStage::Final,)
            .unwrap()
    );
}

#[test]
fn terminal_checkpoint_applies_usage_and_tool_transcript_together() {
    let logical_turn_id = "turn-one";
    let mut state = SessionState::default();
    let mut user = RenderBlock::user("prompt");
    user.logical_turn_id = Some(logical_turn_id.to_string());
    state.blocks.push(user);
    state.messages = vec![message("user", "prompt")];
    let mut cursor = HeadlessCheckpointCursor::starting_after(1);
    let started = started_request("request-one");
    assert!(
        apply_request_checkpoint(
            &mut state,
            logical_turn_id,
            &mut cursor,
            &crate::client::ProviderRequestCheckpoint {
                request: started.clone(),
                transcript: None,
            },
        )
        .unwrap()
    );

    let partial = vec![message("user", "prompt"), message("assistant", "working")];
    assert!(
        apply_transcript_checkpoint(
            &mut state,
            &mut cursor,
            &partial,
            TranscriptStage::Intermediate,
        )
        .unwrap()
    );
    let usage = crate::accounting::Usage {
        uncached_input_tokens: 11,
        output_tokens: 4,
        total_input_tokens: Some(11),
        breakdown_complete: true,
        ..Default::default()
    };
    let mut finished = started;
    finished.in_flight = false;
    finished.completed = true;
    finished.usage_reported = true;
    finished.usage = usage;
    let assistant = tool_call_message(
        "effective-call",
        Some("toolu_native"),
        "python",
        "Run Python",
        "working",
    );
    let terminal = crate::client::ProviderRequestCheckpoint {
        request: finished,
        transcript: Some(vec![message("user", "prompt"), assistant.clone()]),
    };

    assert!(apply_request_checkpoint(&mut state, logical_turn_id, &mut cursor, &terminal).unwrap());
    assert_eq!(state.messages.last(), Some(&assistant));
    assert_eq!(
        state.messages.last().unwrap().tool_calls.as_ref().unwrap()[0]
            .provider_id
            .as_deref(),
        Some("toolu_native")
    );
    assert_eq!(state.accounting.requests.len(), 1);
    assert!(!state.accounting.requests[0].in_flight);
    assert_eq!(state.accounting.requests[0].usage, usage);
    assert_eq!(state.blocks[0].usage, Some(usage));
    assert!(cursor.streaming_block_index.is_none());
    assert_eq!(state.blocks.last().unwrap().block_type, BlockType::ToolCall);
    assert_eq!(
        state.blocks.last().unwrap().title.as_deref(),
        Some("Run Python")
    );
}

#[test]
fn legacy_terminal_text_is_enriched_and_uses_typed_ui_result_once() {
    let logical_turn_id = "legacy-turn";
    let request_id = "legacy-request";
    let legacy_content = "I will calculate.\n<|tool_call>legacy payload<tool_call|>";
    let mut state = SessionState::default();
    let mut user = RenderBlock::user("prompt");
    user.logical_turn_id = Some(logical_turn_id.to_string());
    state.blocks.push(user);
    state.messages = vec![message("user", "prompt")];
    let mut cursor = HeadlessCheckpointCursor::starting_after(1);
    let started = started_request(request_id);
    apply_request_checkpoint(
        &mut state,
        logical_turn_id,
        &mut cursor,
        &crate::client::ProviderRequestCheckpoint {
            request: started.clone(),
            transcript: None,
        },
    )
    .unwrap();
    let mut finished = started;
    finished.in_flight = false;
    finished.completed = true;
    apply_request_checkpoint(
        &mut state,
        logical_turn_id,
        &mut cursor,
        &crate::client::ProviderRequestCheckpoint {
            request: finished,
            transcript: Some(vec![
                message("user", "prompt"),
                message("assistant", legacy_content),
            ]),
        },
    )
    .unwrap();
    assert_eq!(state.blocks.len(), 2);

    let structured = tool_call_message(
        "legacy-effective",
        None,
        "calculate",
        "Legacy calculation",
        legacy_content,
    );
    let structured_checkpoint = vec![message("user", "prompt"), structured.clone()];
    assert!(
        apply_transcript_and_tool_checkpoint(
            &mut state,
            &mut cursor,
            &structured_checkpoint,
            TranscriptStage::Intermediate,
            None,
            &[],
        )
        .unwrap()
    );
    assert_eq!(state.blocks.len(), 3);
    assert_eq!(state.blocks[2].block_type, BlockType::ToolCall);

    let mut result_checkpoint = structured_checkpoint;
    result_checkpoint.push(tool_result_message(
        "legacy-effective",
        "calculate",
        "MODEL CONTEXT RESULT",
        false,
    ));
    let event = tool_event("legacy-projection", &structured, "VISIBLE UI RESULT", false);
    assert!(
        apply_transcript_and_tool_checkpoint(
            &mut state,
            &mut cursor,
            &result_checkpoint,
            TranscriptStage::Intermediate,
            None,
            std::slice::from_ref(&event),
        )
        .unwrap()
    );
    assert_eq!(state.blocks.len(), 4);
    assert_eq!(state.blocks[3].block_type, BlockType::ToolResult);
    assert_eq!(state.blocks[3].content, "\nVISIBLE UI RESULT\n");
    assert!(!state.blocks[3].content.contains("MODEL CONTEXT RESULT"));

    assert!(
        !apply_transcript_and_tool_checkpoint(
            &mut state,
            &mut cursor,
            &result_checkpoint,
            TranscriptStage::Intermediate,
            None,
            &[event],
        )
        .unwrap()
    );
    assert_eq!(state.blocks.len(), 4);
}

#[test]
fn canonical_large_host_ui_survives_durable_projection() {
    let workspace = tempfile::tempdir().unwrap();
    let call_message = tool_call_message(
        "large-effective",
        Some("large-native"),
        "calculate",
        "Large result",
        "",
    );
    let presented = crate::tools::present_tool_execution_in(
        workspace.path(),
        "large-effective",
        crate::tools::ToolExecution {
            output: format!("HOST_HEAD{}HOST_TAIL", "x".repeat(30_000)),
            cwd: workspace.path().to_string_lossy().into_owned(),
            is_error: false,
            provenance: crate::tools::ToolOutputProvenance::OrdinaryHost,
        },
    );
    assert_ne!(presented.context, presented.ui);
    let expected_ui = presented.ui.clone();

    let mut state = SessionState {
        messages: vec![message("user", "prompt")],
        ..Default::default()
    };
    state.blocks.push(RenderBlock::user("prompt"));
    let mut cursor = HeadlessCheckpointCursor::starting_after(1);
    let call_checkpoint = vec![message("user", "prompt"), call_message.clone()];
    apply_transcript_checkpoint(
        &mut state,
        &mut cursor,
        &call_checkpoint,
        TranscriptStage::Final,
    )
    .unwrap();
    let mut result_checkpoint = call_checkpoint;
    result_checkpoint.push(tool_result_message(
        "large-effective",
        "calculate",
        &presented.context,
        false,
    ));
    let event = tool_event("large-projection", &call_message, &presented.ui, false);
    apply_transcript_and_tool_checkpoint(
        &mut state,
        &mut cursor,
        &result_checkpoint,
        TranscriptStage::Intermediate,
        None,
        &[event],
    )
    .unwrap();

    let resumed: SessionState =
        serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    assert_eq!(
        resumed.blocks.last().unwrap().content,
        format!("\n{expected_ui}\n")
    );
    assert!(
        !resumed
            .blocks
            .last()
            .unwrap()
            .content
            .contains("Use `summarize_content`")
    );
}

#[test]
fn finalized_request_rejects_stale_partial_without_regressing_tool_metadata() {
    let logical_turn_id = "turn-one";
    let request_id = "request-one";
    let mut state = SessionState::default();
    let mut user = RenderBlock::user("prompt");
    user.logical_turn_id = Some(logical_turn_id.to_string());
    state.blocks.push(user);
    state.messages = vec![message("user", "prompt")];
    let mut cursor = HeadlessCheckpointCursor::starting_after(1);
    let started = started_request(request_id);
    apply_request_checkpoint(
        &mut state,
        logical_turn_id,
        &mut cursor,
        &crate::client::ProviderRequestCheckpoint {
            request: started.clone(),
            transcript: None,
        },
    )
    .unwrap();
    let terminal_assistant = tool_call_message(
        "effective-call",
        Some("toolu_native"),
        "python",
        "Run Python",
        "working",
    );
    let mut finished = started;
    finished.in_flight = false;
    finished.completed = true;
    apply_request_checkpoint(
        &mut state,
        logical_turn_id,
        &mut cursor,
        &crate::client::ProviderRequestCheckpoint {
            request: finished,
            transcript: Some(vec![message("user", "prompt"), terminal_assistant.clone()]),
        },
    )
    .unwrap();

    let terminal_messages = state.messages.clone();
    let terminal_blocks = state.blocks.len();
    let stale_partial = vec![message("user", "prompt"), message("assistant", "work")];
    assert!(
        !apply_scoped_transcript_checkpoint(
            &mut state,
            &mut cursor,
            &stale_partial,
            TranscriptStage::Intermediate,
            Some(request_id),
        )
        .unwrap()
    );
    assert_eq!(state.messages, terminal_messages);
    assert_eq!(state.messages.last(), Some(&terminal_assistant));
    assert_eq!(state.blocks.len(), terminal_blocks);
    assert!(cursor.streaming_block_index.is_none());

    let mut next_partial = terminal_messages;
    next_partial.push(tool_result_message(
        "effective-call",
        "python",
        "Out[1]:\n4",
        false,
    ));
    next_partial.push(message("assistant", "next partial"));
    assert!(
        apply_scoped_transcript_checkpoint(
            &mut state,
            &mut cursor,
            &next_partial,
            TranscriptStage::Intermediate,
            Some("request-two"),
        )
        .unwrap()
    );
    assert_eq!(state.messages.last().unwrap().content, "next partial");
    assert!(cursor.streaming_block_index.is_some());
    assert_eq!(
        state.blocks[state.blocks.len() - 2].block_type,
        BlockType::ToolResult
    );
}

#[test]
fn projects_call_result_error_in_order_and_replay_is_idempotent() {
    let mut state = SessionState {
        messages: vec![message("user", "prompt")],
        ..Default::default()
    };
    state.blocks.push(RenderBlock::user("prompt"));
    let mut cursor = HeadlessCheckpointCursor::starting_after(1);

    let first_call = tool_call_message(
        "effective-one",
        Some("native-one"),
        "calculate",
        "First calculation",
        "",
    );
    let first_checkpoint = vec![message("user", "prompt"), first_call];
    assert!(
        apply_transcript_checkpoint(
            &mut state,
            &mut cursor,
            &first_checkpoint,
            TranscriptStage::Final,
        )
        .unwrap()
    );
    assert_eq!(state.blocks.last().unwrap().block_type, BlockType::ToolCall);

    let mut first_result = first_checkpoint;
    first_result.push(tool_result_message(
        "effective-one",
        "calculate",
        "4",
        false,
    ));
    assert!(
        apply_transcript_checkpoint(
            &mut state,
            &mut cursor,
            &first_result,
            TranscriptStage::Intermediate,
        )
        .unwrap()
    );
    let blocks_after_first = state.blocks.len();
    assert_eq!(
        state.blocks.last().unwrap().block_type,
        BlockType::ToolResult
    );
    assert_eq!(
        state.blocks.last().unwrap().title.as_deref(),
        Some("First calculation")
    );
    assert!(
        !apply_transcript_checkpoint(
            &mut state,
            &mut cursor,
            &first_result,
            TranscriptStage::Intermediate,
        )
        .unwrap()
    );
    assert_eq!(state.blocks.len(), blocks_after_first);

    let mut second_call = first_result;
    second_call.push(tool_call_message(
        "effective-two",
        Some("native-two"),
        "calculate",
        "Second calculation",
        "",
    ));
    apply_transcript_checkpoint(
        &mut state,
        &mut cursor,
        &second_call,
        TranscriptStage::Final,
    )
    .unwrap();
    second_call.push(tool_result_message(
        "effective-two",
        "calculate",
        "ERROR: division failed",
        true,
    ));
    apply_transcript_checkpoint(
        &mut state,
        &mut cursor,
        &second_call,
        TranscriptStage::Intermediate,
    )
    .unwrap();

    let tail = &state.blocks[state.blocks.len() - 4..];
    assert_eq!(tail[0].block_type, BlockType::ToolCall);
    assert_eq!(tail[1].block_type, BlockType::ToolResult);
    assert_eq!(tail[2].block_type, BlockType::ToolCall);
    assert_eq!(tail[3].block_type, BlockType::ToolError);
    assert_eq!(tail[3].success, Some(false));
    assert_eq!(tail[3].title.as_deref(), Some("Second calculation"));
}

#[test]
fn failed_save_rolls_back_projection_cursor_and_retries_once() {
    let mut state = SessionState {
        messages: vec![message("user", "prompt")],
        ..Default::default()
    };
    state.blocks.push(RenderBlock::user("prompt"));
    let mut cursor = HeadlessCheckpointCursor::starting_after(1);
    let original_cursor = cursor.clone();
    let checkpoint = vec![
        message("user", "prompt"),
        tool_call_message(
            "effective-call",
            Some("native-call"),
            "calculate",
            "Calculate",
            "",
        ),
    ];

    let error = commit_checkpoint(
        &mut state,
        &mut cursor,
        |candidate, candidate_cursor| {
            apply_transcript_checkpoint(
                candidate,
                candidate_cursor,
                &checkpoint,
                TranscriptStage::Final,
            )
        },
        |_| Err("injected save failure".to_string()),
    )
    .unwrap_err();
    assert_eq!(error, "injected save failure");
    assert_eq!(cursor, original_cursor);
    assert_eq!(state.messages.len(), 1);
    assert_eq!(state.blocks.len(), 1);

    assert!(
        commit_checkpoint(
            &mut state,
            &mut cursor,
            |candidate, candidate_cursor| {
                apply_transcript_checkpoint(
                    candidate,
                    candidate_cursor,
                    &checkpoint,
                    TranscriptStage::Final,
                )
            },
            |_| Ok(()),
        )
        .unwrap()
    );
    assert_eq!(state.blocks.len(), 2);
    assert!(
        !commit_checkpoint(
            &mut state,
            &mut cursor,
            |candidate, candidate_cursor| {
                apply_transcript_checkpoint(
                    candidate,
                    candidate_cursor,
                    &checkpoint,
                    TranscriptStage::Final,
                )
            },
            |_| panic!("idempotent replay must not save"),
        )
        .unwrap()
    );
    assert_eq!(state.blocks.len(), 2);

    let result_event = tool_event("result-projection", &checkpoint[1], "VISIBLE 4", false);
    let mut result_checkpoint = checkpoint.clone();
    result_checkpoint.push(tool_result_message(
        "effective-call",
        "calculate",
        "4",
        false,
    ));
    let committed_call_cursor = cursor.clone();
    let error = commit_checkpoint(
        &mut state,
        &mut cursor,
        |candidate, candidate_cursor| {
            apply_transcript_checkpoint(
                candidate,
                candidate_cursor,
                &result_checkpoint,
                TranscriptStage::Intermediate,
            )
        },
        |_| Err("injected result save failure".to_string()),
    )
    .unwrap_err();
    assert_eq!(error, "injected result save failure");
    assert_eq!(cursor, committed_call_cursor);
    assert_eq!(state.blocks.len(), 2);

    assert!(
        commit_checkpoint(
            &mut state,
            &mut cursor,
            |candidate, candidate_cursor| {
                apply_transcript_and_tool_checkpoint(
                    candidate,
                    candidate_cursor,
                    &result_checkpoint,
                    TranscriptStage::Intermediate,
                    None,
                    std::slice::from_ref(&result_event),
                )
            },
            |_| Ok(()),
        )
        .unwrap()
    );
    assert_eq!(state.blocks.len(), 3);
    assert_eq!(state.blocks[2].block_type, BlockType::ToolResult);
    assert_eq!(state.blocks[2].content, "\nVISIBLE 4\n");
    assert!(cursor.projected_tool_events.contains("result-projection"));
}

#[test]
fn resume_preserves_missing_history_and_projects_only_new_provenance() {
    let historical_call = tool_call_message(
        "old-effective",
        Some("old-native"),
        "calculate",
        "Old calculation",
        "",
    );
    let historical_result = tool_result_message("old-effective", "calculate", "old result", false);
    let mut state = SessionState {
        messages: vec![
            message("user", "old prompt"),
            historical_call,
            historical_result,
        ],
        ..Default::default()
    };
    // A legacy headless state may have no tool blocks. Do not reconstruct them without a
    // persisted projection cursor that proves their association.
    state.blocks.push(RenderBlock::user("old prompt"));
    let encoded = serde_json::to_vec(&state).unwrap();
    let mut state: SessionState = serde_json::from_slice(&encoded).unwrap();
    let mut cursor = HeadlessCheckpointCursor::starting_after(state.messages.len());
    let unchanged = state.messages.clone();
    assert!(
        !apply_transcript_checkpoint(&mut state, &mut cursor, &unchanged, TranscriptStage::Final,)
            .unwrap()
    );
    assert_eq!(state.blocks.len(), 1);

    state.messages.push(message("user", "new prompt"));
    state.blocks.push(RenderBlock::user("new prompt"));
    cursor = HeadlessCheckpointCursor::starting_after(state.messages.len());
    let mut forward = state.messages.clone();
    forward.push(tool_call_message(
        "new-effective",
        Some("new-native"),
        "calculate",
        "New calculation",
        "",
    ));
    forward.push(tool_result_message(
        "new-effective",
        "calculate",
        "new result",
        false,
    ));
    assert!(
        apply_transcript_checkpoint(&mut state, &mut cursor, &forward, TranscriptStage::Final,)
            .unwrap()
    );
    assert_eq!(state.blocks.len(), 4);
    assert_eq!(state.blocks[2].block_type, BlockType::ToolCall);
    assert_eq!(state.blocks[3].block_type, BlockType::ToolResult);

    let resumed: SessionState =
        serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    assert_eq!(resumed.blocks[2].block_type, BlockType::ToolCall);
    assert_eq!(resumed.blocks[3].block_type, BlockType::ToolResult);
    assert_eq!(
        resumed.messages[4].tool_calls.as_ref().unwrap()[0]
            .provider_id
            .as_deref(),
        Some("new-native")
    );
}

#[test]
fn typed_tool_event_survives_complete_transcript_omission_and_successor() {
    let call_message = tool_call_message(
        "trimmed-effective",
        Some("trimmed-native"),
        "calculate",
        "Trimmed calculation",
        "",
    );
    let call_checkpoint = vec![message("user", "prompt"), call_message.clone()];
    let mut state = SessionState {
        messages: vec![message("user", "prompt")],
        ..Default::default()
    };
    state.blocks.push(RenderBlock::user("prompt"));
    let mut cursor = HeadlessCheckpointCursor::starting_after(1);
    apply_transcript_checkpoint(
        &mut state,
        &mut cursor,
        &call_checkpoint,
        TranscriptStage::Final,
    )
    .unwrap();
    assert_eq!(state.blocks.last().unwrap().block_type, BlockType::ToolCall);

    // The execution event is deliberately independent from context retention. Simulate a
    // checkpoint in which no provider message survived, then continue from that snapshot.
    let omitted_messages = Vec::new();
    let event = tool_event(
        "trimmed-projection",
        &call_message,
        "VISIBLE TRIMMED RESULT",
        false,
    );
    assert!(
        apply_transcript_and_tool_checkpoint(
            &mut state,
            &mut cursor,
            &omitted_messages,
            TranscriptStage::Intermediate,
            None,
            std::slice::from_ref(&event),
        )
        .unwrap()
    );
    assert!(state.messages.is_empty());
    assert_eq!(state.blocks.len(), 3);
    assert_eq!(state.blocks[1].block_type, BlockType::ToolCall);
    assert_eq!(state.blocks[2].block_type, BlockType::ToolResult);
    assert_eq!(state.blocks[2].content, "\nVISIBLE TRIMMED RESULT\n");
    assert!(
        !apply_transcript_and_tool_checkpoint(
            &mut state,
            &mut cursor,
            &omitted_messages,
            TranscriptStage::Intermediate,
            None,
            &[event],
        )
        .unwrap()
    );

    let successor = vec![message("assistant", "successor response")];
    assert!(
        apply_transcript_and_tool_checkpoint(
            &mut state,
            &mut cursor,
            &successor,
            TranscriptStage::Final,
            None,
            &[],
        )
        .unwrap()
    );
    assert_eq!(state.blocks.len(), 4);
    assert_eq!(state.blocks[3].block_type, BlockType::Text);
    assert_eq!(state.blocks[3].content, "successor response");
}

#[test]
fn context_front_trim_rebases_without_replaying_visible_history() {
    let mut state = SessionState {
        messages: vec![
            message("user", "old prompt"),
            message("assistant", "old answer"),
            message("user", "current prompt"),
        ],
        blocks: vec![
            RenderBlock::user("old prompt"),
            RenderBlock::text("old answer"),
            RenderBlock::user("current prompt"),
        ],
        ..Default::default()
    };
    let mut cursor = HeadlessCheckpointCursor::starting_after(state.messages.len());
    let checkpoint = vec![
        message("user", "current prompt"),
        tool_call_message(
            "effective-call",
            Some("native-call"),
            "calculate",
            "Calculate",
            "",
        ),
        tool_result_message("effective-call", "calculate", "4", false),
    ];

    assert!(
        apply_transcript_checkpoint(&mut state, &mut cursor, &checkpoint, TranscriptStage::Final,)
            .unwrap()
    );

    assert_eq!(state.messages, checkpoint);
    assert_eq!(state.blocks.len(), 5);
    assert_eq!(state.blocks[3].block_type, BlockType::ToolCall);
    assert_eq!(state.blocks[4].block_type, BlockType::ToolResult);
}

#[test]
fn projected_tool_history_respects_shared_block_cap() {
    let mut state = SessionState {
        messages: vec![message("user", "prompt")],
        blocks: (0..200)
            .map(|index| RenderBlock::text(format!("old-{index}")))
            .collect(),
        ..Default::default()
    };
    let mut cursor = HeadlessCheckpointCursor::starting_after(1);
    let checkpoint = vec![
        message("user", "prompt"),
        tool_call_message(
            "effective-call",
            Some("native-call"),
            "calculate",
            "Calculate",
            "",
        ),
        tool_result_message("effective-call", "calculate", "4", false),
    ];

    apply_transcript_checkpoint(&mut state, &mut cursor, &checkpoint, TranscriptStage::Final)
        .unwrap();

    assert_eq!(state.blocks.len(), 200);
    assert_eq!(state.blocks[198].block_type, BlockType::ToolCall);
    assert_eq!(state.blocks[199].block_type, BlockType::ToolResult);
}
