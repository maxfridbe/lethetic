use super::*;

const SESSION_ID: &str = "11111111-2222-4333-8444-555555555555";

fn request(command: WebCommand) -> ICommandRequest {
    ICommandRequest {
        id: "request-1".to_string(),
        expected_revision: 7,
        command,
    }
}

#[test]
fn request_shape_is_flat_and_rejects_unknown_fields() {
    let value = serde_json::to_value(request(WebCommand::SendPrompt {
        session_id: SESSION_ID.to_string(),
        prompt: "hello".to_string(),
    }))
    .unwrap();
    assert_eq!(value["id"], "request-1");
    assert_eq!(value["expected_revision"], 7);
    assert_eq!(value["type"], "send_prompt");
    assert_eq!(value["session_id"], SESSION_ID);
    assert_eq!(value["prompt"], "hello");
    assert!(value.get("command").is_none());

    let mut unknown = value;
    unknown["path"] = serde_json::json!("/private/session");
    let error = serde_json::from_value::<ICommandRequest>(unknown).unwrap_err();
    assert!(error.to_string().contains("unknown field `path`"));
}

#[test]
fn request_deserialization_enforces_bounds_and_exact_ids() {
    let oversized = serde_json::json!({
        "id": "request-1",
        "expected_revision": 0,
        "type": "send_prompt",
        "session_id": SESSION_ID,
        "prompt": "x".repeat(MAX_PROMPT_BYTES + 1),
    });
    assert!(serde_json::from_value::<ICommandRequest>(oversized).is_err());

    let unsafe_revision = serde_json::json!({
        "id": "request-1",
        "expected_revision": MAX_SAFE_JAVASCRIPT_INTEGER + 1,
        "type": "stop",
        "session_id": SESSION_ID,
        "cancel_id": "cancel-1",
    });
    assert!(serde_json::from_value::<ICommandRequest>(unsafe_revision).is_err());

    let path_session = serde_json::json!({
        "id": "request-1",
        "expected_revision": 0,
        "type": "resume_session",
        "session_id": "/tmp/session",
    });
    assert!(serde_json::from_value::<ICommandRequest>(path_session).is_err());
}

#[test]
fn stop_requires_an_exact_bounded_cancellation_instance() {
    let missing = serde_json::json!({
        "id": "request-stop",
        "expected_revision": 0,
        "type": "stop",
        "session_id": SESSION_ID,
    });
    assert!(serde_json::from_value::<ICommandRequest>(missing).is_err());

    let valid = serde_json::json!({
        "id": "request-stop",
        "expected_revision": 0,
        "type": "stop",
        "session_id": SESSION_ID,
        "cancel_id": "cancel-123",
    });
    assert!(serde_json::from_value::<ICommandRequest>(valid).is_ok());

    let oversized = serde_json::json!({
        "id": "request-stop",
        "expected_revision": 0,
        "type": "stop",
        "session_id": SESSION_ID,
        "cancel_id": "x".repeat(MAX_CANCEL_ID_BYTES + 1),
    });
    assert!(serde_json::from_value::<ICommandRequest>(oversized).is_err());
}

#[test]
fn history_content_outcome_enforces_session_entry_and_prompt_bounds() {
    let valid = ICommandResponse {
        id: "history-response".to_string(),
        result: CommandResult::Ok {
            revision: 1,
            outcome: CommandOutcome::HistoryEntrySelected {
                session_id: SESSION_ID.to_string(),
                entry_id: "history-1".to_string(),
                editor_content: "界".repeat(MAX_PROMPT_BYTES / 3),
            },
        },
    };
    assert!(valid.validate().is_ok());

    let mut oversized = valid;
    let CommandResult::Ok {
        outcome: CommandOutcome::HistoryEntrySelected { editor_content, .. },
        ..
    } = &mut oversized.result
    else {
        unreachable!();
    };
    editor_content.push_str("界");
    assert!(oversized.validate().is_err());
}

#[test]
fn affirmative_tool_decisions_require_an_exact_hidden_content_acknowledgment_field() {
    let missing = serde_json::json!({
        "id": "request-1",
        "expected_revision": 7,
        "type": "approve_tool_once",
        "session_id": SESSION_ID,
        "approval_id": "approval-1",
        "tool_call_id": "tool-1",
    });
    assert!(serde_json::from_value::<ICommandRequest>(missing).is_err());

    let complete = serde_json::json!({
        "id": "request-1",
        "expected_revision": 7,
        "type": "approve_tool_always",
        "session_id": SESSION_ID,
        "approval_id": "approval-1",
        "tool_call_id": "tool-1",
        "acknowledge_hidden_content": true,
    });
    let decoded = serde_json::from_value::<ICommandRequest>(complete).unwrap();
    assert!(matches!(
        decoded.command,
        WebCommand::ApproveToolAlways {
            acknowledge_hidden_content: true,
            ..
        }
    ));

    let deny_with_acknowledgment = serde_json::json!({
        "id": "request-1",
        "expected_revision": 7,
        "type": "deny_tool",
        "session_id": SESSION_ID,
        "approval_id": "approval-1",
        "tool_call_id": "tool-1",
        "acknowledge_hidden_content": true,
    });
    assert!(serde_json::from_value::<ICommandRequest>(deny_with_acknowledgment).is_err());

    let duplicate = format!(
        r#"{{"id":"request-1","expected_revision":7,"type":"approve_tool_once","session_id":"{SESSION_ID}","approval_id":"approval-1","tool_call_id":"tool-1","acknowledge_hidden_content":false,"acknowledge_hidden_content":true}}"#
    );
    assert!(serde_json::from_str::<ICommandRequest>(&duplicate).is_err());
}

#[test]
fn request_validation_includes_the_encoded_message_envelope() {
    let escaped_prompt = request(WebCommand::SendPrompt {
        session_id: SESSION_ID.to_string(),
        prompt: "\"".repeat(MAX_PROMPT_BYTES),
    });
    assert!(serde_json::to_vec(&escaped_prompt).unwrap().len() > MAX_COMMAND_MESSAGE_BYTES);
    assert!(
        escaped_prompt
            .validate()
            .unwrap_err()
            .contains("encoded UTF-8 bytes")
    );

    let maximum_content = request(WebCommand::SaveSystemPrompt {
        session_id: SESSION_ID.to_string(),
        name: "prompt".to_string(),
        content: "x".repeat(MAX_SYSTEM_PROMPT_BYTES),
        confirmed_overwrite: false,
        confirmation_id: None,
    });
    assert!(serde_json::to_vec(&maximum_content).unwrap().len() > MAX_COMMAND_MESSAGE_BYTES);
    assert!(maximum_content.validate().is_err());
}

#[test]
fn every_command_variant_has_a_stable_flat_type() {
    let commands = vec![
        WebCommand::InvokeCommand {
            command_id: CommandId::Hotkeys,
        },
        WebCommand::SendPrompt {
            session_id: SESSION_ID.to_string(),
            prompt: "hello".to_string(),
        },
        WebCommand::Stop {
            session_id: SESSION_ID.to_string(),
            cancel_id: "cancel-1".to_string(),
        },
        WebCommand::ApproveToolOnce {
            session_id: SESSION_ID.to_string(),
            approval_id: "approval-1".to_string(),
            tool_call_id: "tool-1".to_string(),
            acknowledge_hidden_content: false,
        },
        WebCommand::ApproveToolAlways {
            session_id: SESSION_ID.to_string(),
            approval_id: "approval-1".to_string(),
            tool_call_id: "tool-1".to_string(),
            acknowledge_hidden_content: false,
        },
        WebCommand::DenyTool {
            session_id: SESSION_ID.to_string(),
            approval_id: "approval-1".to_string(),
            tool_call_id: "tool-1".to_string(),
        },
        WebCommand::RenameSession {
            session_id: SESSION_ID.to_string(),
            name: Some("Demo".to_string()),
        },
        WebCommand::AnswerUser {
            session_id: SESSION_ID.to_string(),
            tool_call_id: "tool-1".to_string(),
            form_id: "form-1".to_string(),
            answers: vec![UserAnswer {
                question_id: "question-1".to_string(),
                selected_option_ids: vec!["choice-1".to_string()],
                other_text: None,
            }],
        },
        WebCommand::SelectTheme {
            theme_id: "theme-1".to_string(),
        },
        WebCommand::SelectModel {
            model_id: "model-1".to_string(),
        },
        WebCommand::NewSession,
        WebCommand::ResumeSession {
            session_id: SESSION_ID.to_string(),
        },
        WebCommand::DeleteSession {
            session_id: SESSION_ID.to_string(),
            confirmed: true,
            confirmation_id: Some("confirmation-1".to_string()),
        },
        WebCommand::WipeSessions {
            confirmed: true,
            confirmation_id: Some("confirmation-1".to_string()),
        },
        WebCommand::SelectHistoryEntry {
            session_id: SESSION_ID.to_string(),
            entry_id: "history-1".to_string(),
        },
        WebCommand::SelectLatestFile {
            session_id: SESSION_ID.to_string(),
            file_id: "file-1".to_string(),
        },
        WebCommand::SelectSystemPrompt {
            session_id: SESSION_ID.to_string(),
            prompt_id: "prompt-1".to_string(),
        },
        WebCommand::SaveSystemPrompt {
            session_id: SESSION_ID.to_string(),
            name: "default".to_string(),
            content: "You are helpful.".to_string(),
            confirmed_overwrite: false,
            confirmation_id: None,
        },
        WebCommand::SetLoopDetection {
            session_id: SESSION_ID.to_string(),
            mode_id: "ngram".to_string(),
        },
        WebCommand::SetAgentMode {
            session_id: SESSION_ID.to_string(),
            mode_id: "general".to_string(),
        },
        WebCommand::RunLspAction {
            session_id: SESSION_ID.to_string(),
            server_id: "rust-analyzer".to_string(),
            action: LspAction::Enable,
        },
        WebCommand::ClearContext {
            session_id: SESSION_ID.to_string(),
            confirmed: true,
            confirmation_id: Some("confirmation-1".to_string()),
        },
        WebCommand::DeletePythonRuntime {
            session_id: SESSION_ID.to_string(),
            confirmed: true,
            confirmation_id: Some("confirmation-1".to_string()),
        },
        WebCommand::DismissOverlay,
        WebCommand::RequestSnapshot,
        WebCommand::Quit {
            confirmed: true,
            confirmation_id: Some("confirmation-1".to_string()),
        },
    ];

    let mut types = std::collections::BTreeSet::new();
    for (index, command) in commands.into_iter().enumerate() {
        let expected = command.wire_type();
        let serialized = serde_json::to_value(request(command)).unwrap();
        assert_eq!(serialized["type"], expected);
        let decoded: ICommandRequest = serde_json::from_value(serialized).unwrap();
        assert!(types.insert(decoded.command.wire_type()));
        assert_eq!(decoded.id, "request-1");
        assert_eq!(decoded.expected_revision, 7, "fixture {index}");
    }
    assert_eq!(types.len(), 26);
}

#[test]
fn protocol_v6_rejects_older_hello_versions() {
    let hello = IProtocolHello::new(
        1,
        0,
        ProtocolCapabilities {
            state_patches: true,
            request_replay: true,
            session_names: true,
            exact_tool_approval: true,
            read_only_files: false,
        },
    )
    .unwrap();
    assert_eq!(hello.protocol_version, 6);
    assert_eq!(hello.minimum_protocol_version, 6);

    let current = serde_json::to_value(hello).unwrap();
    let mut missing_capability = current.clone();
    missing_capability["capabilities"]
        .as_object_mut()
        .unwrap()
        .remove("read_only_files");
    assert!(serde_json::from_value::<IProtocolHello>(missing_capability).is_err());
    for (protocol_version, minimum_protocol_version) in [(5, 5), (6, 5)] {
        let mut value = current.clone();
        value["protocol_version"] = serde_json::json!(protocol_version);
        value["minimum_protocol_version"] = serde_json::json!(minimum_protocol_version);
        assert!(serde_json::from_value::<IProtocolHello>(value).is_err());
    }
}

#[test]
fn cli_locked_policy_source_has_exact_wire_spelling() {
    assert_eq!(
        serde_json::to_value(PythonPolicySourceView::CliLocked).unwrap(),
        serde_json::json!("cli_locked")
    );
}

#[test]
fn destructive_commands_require_dedicated_confirmation_ids() {
    let generic = serde_json::json!({
        "id": "request-1",
        "expected_revision": 7,
        "type": "invoke_command",
        "command_id": "clear-context",
    });
    assert!(serde_json::from_value::<ICommandRequest>(generic).is_err());

    let begin = serde_json::json!({
        "id": "request-1",
        "expected_revision": 7,
        "type": "clear_context",
        "session_id": SESSION_ID,
        "confirmed": false,
        "confirmation_id": null,
    });
    assert!(serde_json::from_value::<ICommandRequest>(begin).is_ok());

    let missing_id = serde_json::json!({
        "id": "request-1",
        "expected_revision": 7,
        "type": "clear_context",
        "session_id": SESSION_ID,
        "confirmed": true,
        "confirmation_id": null,
    });
    assert!(serde_json::from_value::<ICommandRequest>(missing_id).is_err());

    let confirmed = serde_json::json!({
        "id": "request-1",
        "expected_revision": 7,
        "type": "clear_context",
        "session_id": SESSION_ID,
        "confirmed": true,
        "confirmation_id": "confirmation-1",
    });
    assert!(serde_json::from_value::<ICommandRequest>(confirmed).is_ok());
}

#[test]
fn dismiss_overlay_is_an_exact_empty_typed_command() {
    let value = serde_json::to_value(request(WebCommand::DismissOverlay)).unwrap();
    assert_eq!(value["type"], "dismiss_overlay");
    assert_eq!(value.as_object().unwrap().len(), 3);
    assert!(serde_json::from_value::<ICommandRequest>(value.clone()).is_ok());

    let mut unknown = value;
    unknown["panel"] = serde_json::json!("confirmation");
    let error = serde_json::from_value::<ICommandRequest>(unknown).unwrap_err();
    assert!(error.to_string().contains("unknown field `panel`"));
}

#[test]
fn generated_request_is_a_discriminated_flattened_union() {
    let declarations = typescript_declarations();
    assert!(
        declarations.contains(
            "export type ICommandRequest = { id: string, expected_revision: number, } & ("
        )
    );
    assert!(declarations.contains("\"type\": \"approve_tool_once\""));
    assert!(declarations.contains("\"type\": \"dismiss_overlay\""));
    assert!(declarations.contains("export type CommandId = \"hotkeys\""));
}
