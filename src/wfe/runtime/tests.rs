use super::super::actor::ConnectionId;
use super::*;
use crate::commands::CommandId;
use crate::config::Config;

fn request(id: &str, revision: u64, command: WebCommand) -> ICommandRequest {
    ICommandRequest {
        id: id.to_string(),
        expected_revision: revision,
        command,
    }
}

#[tokio::test]
async fn lifecycle_capacity_is_bounded_and_independent_of_commands() {
    let app = App::new(&Config::default());
    let (frontend, mut runtime) = WfeRuntime::new(&app, Vec::new()).unwrap();
    let event = WfeConnectionEvent::Connected(WfeConnectedEvent {
        peer_ip: "127.0.0.1".parse().unwrap(),
        connection_ordinal: 1,
        active_clients: 1,
        authentication_mode: ControllerAuthenticationMode::TokenRequired,
        initial_sequence: 0,
        initial_revision: 0,
        round_trip_time: Some(Duration::from_millis(1)),
    });
    for _ in 0..WFE_CONNECTION_LIFECYCLE_CAPACITY {
        frontend.connection_events.try_send(event.clone()).unwrap();
    }
    assert!(matches!(
        frontend.connection_events.try_send(event),
        Err(mpsc::error::TrySendError::Full(_))
    ));

    let response = frontend
        .commands
        .try_submit(
            ConnectionId::new("client-independent".to_string()).unwrap(),
            request("request-independent", 0, WebCommand::RequestSnapshot),
        )
        .unwrap();
    let envelope = runtime.recv().await.unwrap();
    assert_eq!(envelope.request.id, "request-independent");
    drop(envelope);
    assert!(response.await.is_err());
    assert!(matches!(
        runtime.recv_connection_event().await,
        Some(WfeConnectionEvent::Connected(_))
    ));
}

#[tokio::test]
async fn stale_revision_and_request_id_conflict_fail_closed() {
    let app = App::new(&Config::default());
    let (frontend, mut runtime) = WfeRuntime::new(&app, Vec::new()).unwrap();
    let connection = ConnectionId::new("client-1".to_string()).unwrap();
    let response = frontend
        .commands
        .try_submit(
            connection.clone(),
            request(
                "request-1",
                1,
                WebCommand::InvokeCommand {
                    command_id: CommandId::Hotkeys,
                },
            ),
        )
        .unwrap();
    let envelope = runtime.recv().await.unwrap();
    assert!(runtime.admit(&app, envelope).is_none());
    assert!(matches!(
        response.await.unwrap().result,
        CommandResult::Error {
            error: CommandError {
                code: CommandErrorCode::StaleRevision,
                ..
            }
        }
    ));

    let conflict = frontend
        .commands
        .try_submit(
            connection,
            request(
                "request-1",
                0,
                WebCommand::InvokeCommand {
                    command_id: CommandId::Themes,
                },
            ),
        )
        .unwrap();
    let envelope = runtime.recv().await.unwrap();
    assert!(runtime.admit(&app, envelope).is_none());
    assert!(matches!(
        conflict.await.unwrap().result,
        CommandResult::Error {
            error: CommandError {
                code: CommandErrorCode::RequestIdConflict,
                ..
            }
        }
    ));
}

#[tokio::test]
async fn exact_live_stop_bypasses_revision_but_stale_targets_do_not() {
    let mut app = App::new(&Config::default());
    app.show_session_manager = false;
    app.add_logical_turn_user_segment("turn one".to_string());
    app.is_processing = true;
    let cancel_id = app.active_cancellation_id().unwrap().to_string();
    let session_id = app.session_id.clone();
    let (frontend, mut runtime) = WfeRuntime::new(&app, Vec::new()).unwrap();

    app.show_hotkeys = true;
    assert!(runtime.publish(&app).unwrap());
    assert_eq!(runtime.revision(), 1);
    let exact_response = frontend
        .commands
        .try_submit(
            ConnectionId::new("client-exact-stop".to_string()).unwrap(),
            request(
                "exact-stale-revision-stop",
                0,
                WebCommand::Stop {
                    session_id: session_id.clone(),
                    cancel_id: cancel_id.clone(),
                },
            ),
        )
        .unwrap();
    let envelope = runtime.recv().await.unwrap();
    let admitted = runtime
        .admit(&app, envelope)
        .expect("exact live stop must bypass presentation revision");
    runtime
        .complete(admitted, Ok(CommandOutcome::Applied))
        .unwrap();
    assert!(matches!(
        exact_response.await.unwrap().result,
        CommandResult::Ok { .. }
    ));

    let wrong_at_current = frontend
        .commands
        .try_submit(
            ConnectionId::new("client-wrong-stop".to_string()).unwrap(),
            request(
                "wrong-current-stop",
                runtime.revision(),
                WebCommand::Stop {
                    session_id: session_id.clone(),
                    cancel_id: "cancel-wrong".to_string(),
                },
            ),
        )
        .unwrap();
    let envelope = runtime.recv().await.unwrap();
    assert!(runtime.admit(&app, envelope).is_none());
    assert!(matches!(
        wrong_at_current.await.unwrap().result,
        CommandResult::Error {
            error: CommandError {
                code: CommandErrorCode::NotFound,
                ..
            }
        }
    ));

    app.is_processing = false;
    assert_eq!(app.active_cancellation_id(), Some(cancel_id.as_str()));
    assert!(app.live_cancellation_id().is_none());
    let no_longer_live = frontend
        .commands
        .try_submit(
            ConnectionId::new("client-idle-stop".to_string()).unwrap(),
            request(
                "stale-idle-stop",
                0,
                WebCommand::Stop {
                    session_id: session_id.clone(),
                    cancel_id: cancel_id.clone(),
                },
            ),
        )
        .unwrap();
    let envelope = runtime.recv().await.unwrap();
    assert!(runtime.admit(&app, envelope).is_none());
    assert!(matches!(
        no_longer_live.await.unwrap().result,
        CommandResult::Error {
            error: CommandError {
                code: CommandErrorCode::StaleRevision,
                ..
            }
        }
    ));

    app.settle_logical_turn();
    app.add_logical_turn_user_segment("turn two".to_string());
    app.is_processing = true;
    assert_ne!(app.active_cancellation_id(), Some(cancel_id.as_str()));
    let successor = frontend
        .commands
        .try_submit(
            ConnectionId::new("client-successor-stop".to_string()).unwrap(),
            request(
                "old-stop-for-successor",
                runtime.revision(),
                WebCommand::Stop {
                    session_id,
                    cancel_id,
                },
            ),
        )
        .unwrap();
    let envelope = runtime.recv().await.unwrap();
    assert!(runtime.admit(&app, envelope).is_none());
    assert!(matches!(
        successor.await.unwrap().result,
        CommandResult::Error {
            error: CommandError {
                code: CommandErrorCode::NotFound,
                ..
            }
        }
    ));
}

#[tokio::test]
async fn affirmative_approval_keeps_the_presentation_revision_fence() {
    let mut app = App::new(&Config::default());
    app.show_session_manager = false;
    app.add_logical_turn_user_segment("approval turn".to_string());
    app.show_approval_prompt = true;
    app.pending_tool_call = Some(crate::context::ToolCall {
        id: "approval-call".to_string(),
        provider_id: None,
        function: crate::context::FunctionCall {
            name: "calculate".to_string(),
            arguments: serde_json::json!({"expression":"1+1"}),
        },
    });
    let (frontend, mut runtime) = WfeRuntime::new(&app, Vec::new()).unwrap();
    let approval_id = runtime.pending_approval_id().unwrap().to_string();
    app.show_debug = !app.show_debug;
    assert!(runtime.publish(&app).unwrap());

    let response = frontend
        .commands
        .try_submit(
            ConnectionId::new("client-stale-approval".to_string()).unwrap(),
            request(
                "stale-approval",
                0,
                WebCommand::ApproveToolOnce {
                    session_id: app.session_id.clone(),
                    approval_id,
                    tool_call_id: "approval-call".to_string(),
                    acknowledge_hidden_content: false,
                },
            ),
        )
        .unwrap();
    let envelope = runtime.recv().await.unwrap();
    assert!(runtime.admit(&app, envelope).is_none());
    assert!(matches!(
        response.await.unwrap().result,
        CommandResult::Error {
            error: CommandError {
                code: CommandErrorCode::StaleRevision,
                ..
            }
        }
    ));
}

#[tokio::test]
async fn generic_invoke_rejects_every_confirm_behavior_command() {
    let app = App::new(&Config::default());
    let original_session_id = app.session_id.clone();
    let (frontend, mut runtime) = WfeRuntime::new(&app, Vec::new()).unwrap();

    for (index, command_id) in [
        CommandId::ClearContext,
        CommandId::DeletePythonRuntime,
        CommandId::Quit,
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(
            command_id.spec().behavior,
            crate::commands::CommandBehavior::Confirm
        );
        let response = frontend
            .commands
            .try_submit(
                ConnectionId::new(format!("client-confirm-{index}")).unwrap(),
                request(
                    &format!("generic-confirm-{index}"),
                    runtime.revision(),
                    WebCommand::InvokeCommand { command_id },
                ),
            )
            .unwrap();
        let envelope = runtime.recv().await.unwrap();
        assert!(runtime.admit(&app, envelope).is_none());
        assert!(matches!(
            response.await.unwrap().result,
            CommandResult::Error {
                error: CommandError {
                    code: CommandErrorCode::BadRequest,
                    ..
                }
            }
        ));
        assert_eq!(runtime.revision(), 0);
        assert_eq!(app.session_id, original_session_id);
    }
}

#[test]
fn history_recall_returns_only_complete_lossless_bounded_originals() {
    let mut app = App::new(&Config::default());
    app.input = "server draft".to_string();
    app.backbuffer = "server backbuffer".to_string();
    let long_original = "界".repeat(2_000);
    let redacted = "read ghp_HiStOrYT0kenAbCdEfGhIjKlMnOpQrStUv12".to_string();
    let oversized = "x".repeat(MAX_PROMPT_BYTES + 1);
    app.history = vec![oversized.clone(), redacted.clone(), long_original.clone()];
    let (_, runtime) = WfeRuntime::new(&app, Vec::new()).unwrap();
    let entries = history_entry_map(&app);
    let id_for = |expected: &str| {
        entries
            .iter()
            .find_map(|(id, value)| (value == expected).then(|| id.clone()))
            .unwrap()
    };

    let recalled = runtime
        .lossless_history_entry(&app, &id_for(&long_original))
        .unwrap();
    assert_eq!(recalled, long_original);
    assert_eq!(app.input, "server draft");
    assert_eq!(app.backbuffer, "server backbuffer");

    for unavailable in [&redacted, &oversized] {
        let error = runtime
            .lossless_history_entry(&app, &id_for(unavailable))
            .unwrap_err();
        assert_eq!(error.code, CommandErrorCode::BadRequest);
        assert_eq!(app.input, "server draft");
        assert_eq!(app.backbuffer, "server backbuffer");
    }
}

#[test]
fn projected_model_choices_share_the_resolver_bound() {
    let mut app = App::new(&Config::default());
    app.available_models = (0..=MAX_WEB_MODELS)
        .map(|index| crate::client::ModelChoice {
            display: format!("model-{index}"),
            connection_id: "connection".to_string(),
            kind: crate::config::ConnectionKind::OpenAiChatCompletions,
            url: "https://127.0.0.1:1".to_string(),
            model_id: format!("model-{index}"),
            available: true,
        })
        .collect();
    let snapshot = project_with_state(&app, &PresentationState::default());
    let choices = model_choice_map(&app);
    assert_eq!(snapshot.models.len(), MAX_WEB_MODELS);
    assert_eq!(choices.len(), MAX_WEB_MODELS);
    assert!(choices.contains_key(&snapshot.models[MAX_WEB_MODELS - 1].model_id));
}

#[test]
fn pending_approval_clears_cached_nonblocking_panel_data() {
    let mut app = App::new(&Config::default());
    app.show_session_manager = false;
    let (frontend, mut runtime) = WfeRuntime::new(&app, Vec::new()).unwrap();
    runtime.open_loop_modes(&app);
    assert!(runtime.publish(&app).unwrap());
    assert_eq!(
        frontend.mirror.latest_snapshot().state.overlay.active_panel,
        Some(PanelId::LoopDetection)
    );

    app.show_approval_prompt = true;
    app.pending_tool_call = Some(crate::context::ToolCall {
        id: "blocking-call".to_string(),
        provider_id: Some("provider-private".to_string()),
        function: crate::context::FunctionCall {
            name: "python".to_string(),
            arguments: serde_json::json!({
                "code": "1 + 1",
                "description": "Review code"
            }),
        },
    });
    assert!(runtime.publish(&app).unwrap());
    let snapshot = frontend.mirror.latest_snapshot();
    assert!(snapshot.state.pending_approval.is_some());
    assert_eq!(
        snapshot.state.overlay.active_panel,
        Some(PanelId::ToolApproval)
    );
    assert!(snapshot.state.overlay.data.is_none());
}

#[tokio::test]
async fn exact_approval_identity_is_required() {
    let mut app = App::new(&Config::default());
    app.show_session_manager = false;
    app.show_approval_prompt = true;
    app.pending_tool_call = Some(crate::context::ToolCall {
        id: "tool-call-1".to_string(),
        provider_id: Some("provider-private".to_string()),
        function: crate::context::FunctionCall {
            name: "python".to_string(),
            arguments: serde_json::json!({"code": "1 + 1", "description": "demo"}),
        },
    });
    let (frontend, mut runtime) = WfeRuntime::new(&app, Vec::new()).unwrap();
    let approval_id = runtime.pending_approval_id().unwrap().to_string();
    let response = frontend
        .commands
        .try_submit(
            ConnectionId::new("client-1".to_string()).unwrap(),
            request(
                "request-1",
                0,
                WebCommand::ApproveToolOnce {
                    session_id: app.session_id.clone(),
                    approval_id,
                    tool_call_id: "wrong-call".to_string(),
                    acknowledge_hidden_content: false,
                },
            ),
        )
        .unwrap();
    let envelope = runtime.recv().await.unwrap();
    assert!(runtime.admit(&app, envelope).is_none());
    assert!(matches!(
        response.await.unwrap().result,
        CommandResult::Error {
            error: CommandError {
                code: CommandErrorCode::ToolCallMismatch,
                ..
            }
        }
    ));
}

#[tokio::test]
async fn hidden_approval_requires_acknowledgment_but_deny_remains_immediate() {
    let mut app = App::new(&Config::default());
    app.show_session_manager = false;
    app.show_approval_prompt = true;
    let original_secret = "ghp_ApPrOvAlT0kenAbCdEfGhIjKlMnOpQrStUv12";
    app.pending_tool_call = Some(crate::context::ToolCall {
        id: "tool-call-lossy".to_string(),
        provider_id: Some("provider-private".to_string()),
        function: crate::context::FunctionCall {
            name: "python".to_string(),
            arguments: serde_json::json!({
                "code": format!("open('{original_secret}')"),
                "description": "Review exact code"
            }),
        },
    });
    let (frontend, mut runtime) = WfeRuntime::new(&app, Vec::new()).unwrap();
    let approval = frontend
        .mirror
        .latest_snapshot()
        .state
        .pending_approval
        .clone()
        .unwrap();
    assert!(approval.preview_redacted);
    assert!(!approval.preview_truncated);
    assert!(!approval.preview.contains(original_secret));
    assert!(!approval.can_view_original);
    assert_eq!(
        approval.allowed_decisions,
        vec![
            ApprovalDecision::ApproveOnce,
            ApprovalDecision::ApproveAlways,
            ApprovalDecision::Deny,
        ]
    );

    let approval_id = approval.approval_id.clone();
    let approve_response = frontend
        .commands
        .try_submit(
            ConnectionId::new("client-approve".to_string()).unwrap(),
            request(
                "crafted-approve",
                0,
                WebCommand::ApproveToolOnce {
                    session_id: app.session_id.clone(),
                    approval_id: approval_id.clone(),
                    tool_call_id: "tool-call-lossy".to_string(),
                    acknowledge_hidden_content: false,
                },
            ),
        )
        .unwrap();
    let envelope = runtime.recv().await.unwrap();
    assert!(runtime.admit(&app, envelope).is_none());
    assert!(matches!(
        approve_response.await.unwrap().result,
        CommandResult::Error {
            error: CommandError {
                code: CommandErrorCode::BadRequest,
                ..
            }
        }
    ));

    let deny_response = frontend
        .commands
        .try_submit(
            ConnectionId::new("client-deny".to_string()).unwrap(),
            request(
                "deny-lossy",
                0,
                WebCommand::DenyTool {
                    session_id: app.session_id.clone(),
                    approval_id,
                    tool_call_id: "tool-call-lossy".to_string(),
                },
            ),
        )
        .unwrap();
    let envelope = runtime.recv().await.unwrap();
    let admitted = runtime.admit(&app, envelope).unwrap();
    runtime
        .complete(
            admitted,
            Ok(CommandOutcome::ToolDecisionRecorded {
                tool_call_id: "tool-call-lossy".to_string(),
            }),
        )
        .unwrap();
    assert!(matches!(
        deny_response.await.unwrap().result,
        CommandResult::Ok {
            outcome: CommandOutcome::ToolDecisionRecorded { .. },
            ..
        }
    ));
}

#[tokio::test]
async fn acknowledged_redacted_approval_is_admitted() {
    let mut app = App::new(&Config::default());
    app.show_session_manager = false;
    app.show_approval_prompt = true;
    app.pending_tool_call = Some(crate::context::ToolCall {
        id: "tool-call-redacted".to_string(),
        provider_id: Some("provider-private".to_string()),
        function: crate::context::FunctionCall {
            name: "python".to_string(),
            arguments: serde_json::json!({
                "code": "open('ghp_ApPrOvAlT0kenAbCdEfGhIjKlMnOpQrStUv12')",
                "description": "Review exact code"
            }),
        },
    });
    let (frontend, mut runtime) = WfeRuntime::new(&app, Vec::new()).unwrap();
    let approval = frontend
        .mirror
        .latest_snapshot()
        .state
        .pending_approval
        .clone()
        .unwrap();
    assert!(approval.preview_redacted);
    assert!(!approval.preview_truncated);

    let response = frontend
        .commands
        .try_submit(
            ConnectionId::new("client-acknowledged".to_string()).unwrap(),
            request(
                "acknowledged-redaction",
                0,
                WebCommand::ApproveToolOnce {
                    session_id: app.session_id.clone(),
                    approval_id: approval.approval_id,
                    tool_call_id: "tool-call-redacted".to_string(),
                    acknowledge_hidden_content: true,
                },
            ),
        )
        .unwrap();
    let envelope = runtime.recv().await.unwrap();
    let admitted = runtime.admit(&app, envelope).unwrap();
    runtime
        .complete(
            admitted,
            Ok(CommandOutcome::ToolDecisionRecorded {
                tool_call_id: "tool-call-redacted".to_string(),
            }),
        )
        .unwrap();
    assert!(matches!(
        response.await.unwrap().result,
        CommandResult::Ok {
            outcome: CommandOutcome::ToolDecisionRecorded { .. },
            ..
        }
    ));
}

#[tokio::test]
async fn acknowledged_truncated_approval_is_admitted() {
    let mut app = App::new(&Config::default());
    app.show_session_manager = false;
    app.show_approval_prompt = true;
    app.pending_tool_call = Some(crate::context::ToolCall {
        id: "tool-call-truncated".to_string(),
        provider_id: Some("provider-private".to_string()),
        function: crate::context::FunctionCall {
            name: "python".to_string(),
            arguments: serde_json::json!({
                "code": "safe text ".repeat(
                    crate::wfe::presentation::MAX_WEB_APPROVAL_PREVIEW_BYTES
                ),
                "description": "Review long code"
            }),
        },
    });
    let (frontend, mut runtime) = WfeRuntime::new(&app, Vec::new()).unwrap();
    let approval = frontend
        .mirror
        .latest_snapshot()
        .state
        .pending_approval
        .clone()
        .unwrap();
    assert!(!approval.preview_redacted);
    assert!(approval.preview_truncated);

    let response = frontend
        .commands
        .try_submit(
            ConnectionId::new("client-acknowledged".to_string()).unwrap(),
            request(
                "acknowledged-truncation",
                0,
                WebCommand::ApproveToolAlways {
                    session_id: app.session_id.clone(),
                    approval_id: approval.approval_id,
                    tool_call_id: "tool-call-truncated".to_string(),
                    acknowledge_hidden_content: true,
                },
            ),
        )
        .unwrap();
    let envelope = runtime.recv().await.unwrap();
    let admitted = runtime.admit(&app, envelope).unwrap();
    runtime
        .complete(
            admitted,
            Ok(CommandOutcome::ToolDecisionRecorded {
                tool_call_id: "tool-call-truncated".to_string(),
            }),
        )
        .unwrap();
    assert!(matches!(
        response.await.unwrap().result,
        CommandResult::Ok {
            outcome: CommandOutcome::ToolDecisionRecorded { .. },
            ..
        }
    ));
}

#[tokio::test]
async fn complete_approval_rejects_spurious_hidden_content_acknowledgment() {
    let mut app = App::new(&Config::default());
    app.show_session_manager = false;
    app.show_approval_prompt = true;
    app.pending_tool_call = Some(crate::context::ToolCall {
        id: "tool-call-complete".to_string(),
        provider_id: Some("provider-private".to_string()),
        function: crate::context::FunctionCall {
            name: "python".to_string(),
            arguments: serde_json::json!({
                "code": "1 + 1",
                "description": "Review code"
            }),
        },
    });
    let (frontend, mut runtime) = WfeRuntime::new(&app, Vec::new()).unwrap();
    let approval = frontend
        .mirror
        .latest_snapshot()
        .state
        .pending_approval
        .clone()
        .unwrap();
    assert!(!approval.preview_redacted);
    assert!(!approval.preview_truncated);

    let response = frontend
        .commands
        .try_submit(
            ConnectionId::new("client-spurious".to_string()).unwrap(),
            request(
                "spurious-acknowledgment",
                0,
                WebCommand::ApproveToolOnce {
                    session_id: app.session_id.clone(),
                    approval_id: approval.approval_id,
                    tool_call_id: "tool-call-complete".to_string(),
                    acknowledge_hidden_content: true,
                },
            ),
        )
        .unwrap();
    let envelope = runtime.recv().await.unwrap();
    assert!(runtime.admit(&app, envelope).is_none());
    assert!(matches!(
        response.await.unwrap().result,
        CommandResult::Error {
            error: CommandError {
                code: CommandErrorCode::BadRequest,
                ..
            }
        }
    ));
}

#[tokio::test]
async fn lossy_system_prompt_projection_rejects_a_crafted_remote_save() {
    let mut app = App::new(&Config::default());
    app.show_session_manager = false;
    app.show_prompt_editor = true;
    let original_path = "/etc/private/system-prompt";
    app.system_prompt = format!("Read {original_path} before every answer");
    let (frontend, mut runtime) = WfeRuntime::new(&app, Vec::new()).unwrap();
    let snapshot = frontend.mirror.latest_snapshot();
    let PanelDataView::SystemPrompts {
        editor_content,
        content_truncated,
        ..
    } = snapshot.state.overlay.data.clone().unwrap()
    else {
        panic!("expected system prompt editor projection");
    };
    assert!(content_truncated);
    assert!(!editor_content.unwrap().contains(original_path));

    let response = frontend
        .commands
        .try_submit(
            ConnectionId::new("client-save".to_string()).unwrap(),
            request(
                "crafted-save",
                0,
                WebCommand::SaveSystemPrompt {
                    session_id: app.session_id.clone(),
                    name: "remote-copy".to_string(),
                    content: "[REDACTED]".to_string(),
                    confirmed_overwrite: false,
                    confirmation_id: None,
                },
            ),
        )
        .unwrap();
    let envelope = runtime.recv().await.unwrap();
    assert!(runtime.admit(&app, envelope).is_none());
    assert!(matches!(
        response.await.unwrap().result,
        CommandResult::Error {
            error: CommandError {
                code: CommandErrorCode::BadRequest,
                ..
            }
        }
    ));
}

#[tokio::test]
async fn answer_requires_the_exact_ephemeral_question_identity() {
    let mut app = App::new(&Config::default());
    app.show_session_manager = false;
    app.is_asking_user = true;
    app.pending_tool_call = Some(crate::context::ToolCall {
        id: "question-call-1".to_string(),
        provider_id: Some("provider-private".to_string()),
        function: crate::context::FunctionCall {
            name: "ask_the_user".to_string(),
            arguments: serde_json::json!({"question": "Continue?"}),
        },
    });
    let (frontend, mut runtime) = WfeRuntime::new(&app, Vec::new()).unwrap();
    let form_id = runtime.pending_form_id().unwrap().to_string();
    let response = frontend
        .commands
        .try_submit(
            ConnectionId::new("client-1".to_string()).unwrap(),
            request(
                "bad-answer",
                0,
                WebCommand::AnswerUser {
                    session_id: app.session_id.clone(),
                    tool_call_id: "question-call-1".to_string(),
                    form_id: form_id.clone(),
                    answers: vec![UserAnswer {
                        question_id: "wrong-question".to_string(),
                        selected_option_ids: Vec::new(),
                        other_text: Some("yes".to_string()),
                    }],
                },
            ),
        )
        .unwrap();
    let envelope = runtime.recv().await.unwrap();
    assert!(runtime.admit(&app, envelope).is_none());
    assert!(matches!(
        response.await.unwrap().result,
        CommandResult::Error {
            error: CommandError {
                code: CommandErrorCode::BadRequest,
                ..
            }
        }
    ));

    let question_id = opaque_choice_id("question", &[&form_id, "question"]);
    let response = frontend
        .commands
        .try_submit(
            ConnectionId::new("client-2".to_string()).unwrap(),
            request(
                "good-answer",
                0,
                WebCommand::AnswerUser {
                    session_id: app.session_id.clone(),
                    tool_call_id: "question-call-1".to_string(),
                    form_id,
                    answers: vec![UserAnswer {
                        question_id,
                        selected_option_ids: Vec::new(),
                        other_text: Some("yes".to_string()),
                    }],
                },
            ),
        )
        .unwrap();
    let envelope = runtime.recv().await.unwrap();
    let admitted = runtime.admit(&app, envelope).unwrap();
    runtime
        .complete(admitted, Ok(CommandOutcome::Applied))
        .unwrap();
    assert!(matches!(
        response.await.unwrap().result,
        CommandResult::Ok {
            outcome: CommandOutcome::Applied,
            ..
        }
    ));
}

#[tokio::test]
async fn lossy_question_is_visible_but_cannot_be_answered_remotely() {
    let mut app = App::new(&Config::default());
    app.show_session_manager = false;
    app.is_asking_user = true;
    let original_secret = "ghp_QuEsTiOnT0kenAbCdEfGhIjKlMnOpQrStUv12";
    app.pending_tool_call = Some(crate::context::ToolCall {
        id: "question-call-lossy".to_string(),
        provider_id: Some("provider-private".to_string()),
        function: crate::context::FunctionCall {
            name: "ask_the_user".to_string(),
            arguments: serde_json::json!({
                "question": format!("Should I read {original_secret}?")
            }),
        },
    });
    let (frontend, mut runtime) = WfeRuntime::new(&app, Vec::new()).unwrap();
    let question = frontend
        .mirror
        .latest_snapshot()
        .state
        .pending_question
        .clone()
        .unwrap();
    assert!(question.content_truncated);
    assert_eq!(
        question.questions[0].prompt,
        crate::wfe::presentation::QUESTION_PREVIEW_UNAVAILABLE
    );
    assert!(!question.questions[0].prompt.contains(original_secret));
    assert!(!question.questions[0].allows_other);

    let response = frontend
        .commands
        .try_submit(
            ConnectionId::new("client-lossy-question".to_string()).unwrap(),
            request(
                "lossy-answer",
                runtime.revision(),
                WebCommand::AnswerUser {
                    session_id: app.session_id.clone(),
                    tool_call_id: question.tool_call_id,
                    form_id: question.form_id,
                    answers: vec![UserAnswer {
                        question_id: question.questions[0].question_id.clone(),
                        selected_option_ids: Vec::new(),
                        other_text: Some("yes".to_string()),
                    }],
                },
            ),
        )
        .unwrap();
    let envelope = runtime.recv().await.unwrap();
    assert!(runtime.admit(&app, envelope).is_none());
    assert!(matches!(
        response.await.unwrap().result,
        CommandResult::Error {
            error: CommandError {
                code: CommandErrorCode::BadRequest,
                ..
            }
        }
    ));
}

#[tokio::test]
async fn stale_confirmation_id_cannot_confirm_a_replaced_action_instance() {
    let app = App::new(&Config::default());
    let (frontend, mut runtime) = WfeRuntime::new(&app, Vec::new()).unwrap();
    let unconfirmed = WebCommand::ClearContext {
        session_id: app.session_id.clone(),
        confirmed: false,
        confirmation_id: None,
    };
    let first_id = runtime
        .begin_confirmation_for_command(&unconfirmed, "Clear context?", "First confirmation")
        .unwrap();
    runtime.publish(&app).unwrap();
    runtime.clear_confirmation();
    let second_id = runtime
        .begin_confirmation_for_command(&unconfirmed, "Clear context?", "Second confirmation")
        .unwrap();
    assert_ne!(first_id, second_id);
    runtime.publish(&app).unwrap();

    let response = frontend
        .commands
        .try_submit(
            ConnectionId::new("client-1".to_string()).unwrap(),
            request(
                "stale-confirmation",
                runtime.revision(),
                WebCommand::ClearContext {
                    session_id: app.session_id.clone(),
                    confirmed: true,
                    confirmation_id: Some(first_id),
                },
            ),
        )
        .unwrap();
    let envelope = runtime.recv().await.unwrap();
    assert!(runtime.admit(&app, envelope).is_none());
    assert!(matches!(
        response.await.unwrap().result,
        CommandResult::Error {
            error: CommandError {
                code: CommandErrorCode::ConfirmationRequired,
                ..
            }
        }
    ));
}

#[tokio::test]
async fn noncurrent_delete_confirmation_is_presented_and_exact_target_bound() {
    const OTHER_SESSION: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";

    let mut app = App::new(&Config::default());
    app.show_session_manager = false;
    let (frontend, mut runtime) = WfeRuntime::new(&app, Vec::new()).unwrap();
    let unconfirmed = WebCommand::DeleteSession {
        session_id: OTHER_SESSION.to_string(),
        confirmed: false,
        confirmation_id: None,
    };
    let confirmation_id = runtime
        .begin_confirmation_for_command(
            &unconfirmed,
            "Delete session?",
            "Delete the exact selected session.",
        )
        .unwrap();
    runtime.publish(&app).unwrap();
    let snapshot = frontend.mirror.latest_snapshot();
    let PanelDataView::Confirmation { confirmation } = snapshot.state.overlay.data.clone().unwrap()
    else {
        panic!("expected confirmation panel data");
    };
    assert_eq!(confirmation.session_id.as_deref(), Some(OTHER_SESSION));
    assert_eq!(confirmation.confirmation_id, confirmation_id);

    let mismatched = frontend
        .commands
        .try_submit(
            ConnectionId::new("client-wrong-target".to_string()).unwrap(),
            request(
                "wrong-delete-target",
                runtime.revision(),
                WebCommand::DeleteSession {
                    session_id: app.session_id.clone(),
                    confirmed: true,
                    confirmation_id: Some(confirmation_id.clone()),
                },
            ),
        )
        .unwrap();
    let envelope = runtime.recv().await.unwrap();
    assert!(runtime.admit(&app, envelope).is_none());
    assert!(matches!(
        mismatched.await.unwrap().result,
        CommandResult::Error {
            error: CommandError {
                code: CommandErrorCode::ConfirmationRequired,
                ..
            }
        }
    ));

    let exact = frontend
        .commands
        .try_submit(
            ConnectionId::new("client-exact-target".to_string()).unwrap(),
            request(
                "exact-delete-target",
                runtime.revision(),
                WebCommand::DeleteSession {
                    session_id: OTHER_SESSION.to_string(),
                    confirmed: true,
                    confirmation_id: Some(confirmation_id),
                },
            ),
        )
        .unwrap();
    let envelope = runtime.recv().await.unwrap();
    let admitted = runtime.admit(&app, envelope).unwrap();
    runtime
        .complete(
            admitted,
            Ok(CommandOutcome::SessionDeleted {
                session_id: OTHER_SESSION.to_string(),
            }),
        )
        .unwrap();
    assert!(matches!(
        exact.await.unwrap().result,
        CommandResult::Ok {
            outcome: CommandOutcome::SessionDeleted { ref session_id },
            ..
        } if session_id == OTHER_SESSION
    ));
}

#[tokio::test]
async fn confirmation_id_is_bound_to_the_exact_destructive_payload() {
    let mut app = App::new(&Config::default());
    app.show_session_manager = false;
    app.show_prompt_editor = true;
    app.system_prompt = "ok".to_string();
    let (frontend, mut runtime) = WfeRuntime::new(&app, Vec::new()).unwrap();
    let unconfirmed = WebCommand::SaveSystemPrompt {
        session_id: app.session_id.clone(),
        name: "demo".to_string(),
        content: "original content".to_string(),
        confirmed_overwrite: false,
        confirmation_id: None,
    };
    let confirmation_id = runtime
        .begin_confirmation_for_command(&unconfirmed, "Overwrite prompt?", "Confirm exact content")
        .unwrap();
    runtime.publish(&app).unwrap();

    let response = frontend
        .commands
        .try_submit(
            ConnectionId::new("client-1".to_string()).unwrap(),
            request(
                "changed-payload",
                runtime.revision(),
                WebCommand::SaveSystemPrompt {
                    session_id: app.session_id.clone(),
                    name: "demo".to_string(),
                    content: "different content".to_string(),
                    confirmed_overwrite: true,
                    confirmation_id: Some(confirmation_id.clone()),
                },
            ),
        )
        .unwrap();
    let envelope = runtime.recv().await.unwrap();
    assert!(runtime.admit(&app, envelope).is_none());
    assert!(matches!(
        response.await.unwrap().result,
        CommandResult::Error {
            error: CommandError {
                code: CommandErrorCode::ConfirmationRequired,
                ..
            }
        }
    ));

    let response = frontend
        .commands
        .try_submit(
            ConnectionId::new("client-2".to_string()).unwrap(),
            request(
                "exact-payload",
                runtime.revision(),
                WebCommand::SaveSystemPrompt {
                    session_id: app.session_id.clone(),
                    name: " demo ".to_string(),
                    content: "original content".to_string(),
                    confirmed_overwrite: true,
                    confirmation_id: Some(confirmation_id),
                },
            ),
        )
        .unwrap();
    let envelope = runtime.recv().await.unwrap();
    let admitted = runtime.admit(&app, envelope).unwrap();
    runtime
        .complete(admitted, Ok(CommandOutcome::Applied))
        .unwrap();
    assert!(matches!(
        response.await.unwrap().result,
        CommandResult::Ok {
            outcome: CommandOutcome::Applied,
            ..
        }
    ));
}

#[tokio::test]
async fn late_session_transition_failure_replays_save_failed_after_resynchronization() {
    const REPLACEMENT_SESSION: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    let mut app = App::new(&Config::default());
    app.show_session_manager = false;
    let (frontend, mut runtime) = WfeRuntime::new(&app, Vec::new()).unwrap();
    let transition = request("new-session-late-failure", 0, WebCommand::NewSession);
    let first_response = frontend
        .commands
        .try_submit(
            ConnectionId::new("client-transition".to_string()).unwrap(),
            transition.clone(),
        )
        .unwrap();
    let envelope = runtime.recv().await.unwrap();
    let admitted = runtime.admit(&app, envelope).unwrap();

    // Model a failure reported after the creation path has already installed a
    // replacement identity. The changed mirror must not be treated as success.
    app.session_id = REPLACEMENT_SESSION.to_string();
    assert!(runtime.publish(&app).unwrap());
    runtime
        .complete(
            admitted,
            Err(RuntimeFailure::new(
                CommandErrorCode::SaveFailed,
                "A new session could not be created safely",
                false,
            )),
        )
        .unwrap();
    let first_response = first_response.await.unwrap();
    assert!(matches!(
        &first_response.result,
        CommandResult::Error {
            error: CommandError {
                code: CommandErrorCode::SaveFailed,
                current_revision: Some(1),
                retryable: false,
                ..
            }
        }
    ));
    assert_eq!(
        frontend.mirror.latest_snapshot().state.session.session_id,
        REPLACEMENT_SESSION
    );

    let replay = frontend
        .commands
        .try_submit(
            ConnectionId::new("client-transition-replay".to_string()).unwrap(),
            transition,
        )
        .unwrap();
    let envelope = runtime.recv().await.unwrap();
    assert!(runtime.admit(&app, envelope).is_none());
    assert_eq!(replay.await.unwrap(), first_response);
}

#[tokio::test]
async fn identical_in_flight_requests_share_one_execution_and_response() {
    let app = App::new(&Config::default());
    let (frontend, mut runtime) = WfeRuntime::new(&app, Vec::new()).unwrap();
    let command = request(
        "shared-request",
        0,
        WebCommand::InvokeCommand {
            command_id: CommandId::Hotkeys,
        },
    );
    let first_response = frontend
        .commands
        .try_submit(
            ConnectionId::new("client-1".to_string()).unwrap(),
            command.clone(),
        )
        .unwrap();
    let second_response = frontend
        .commands
        .try_submit(ConnectionId::new("client-2".to_string()).unwrap(), command)
        .unwrap();

    let first_envelope = runtime.recv().await.unwrap();
    let first = runtime.admit(&app, first_envelope).unwrap();
    assert_eq!(first.connection_id.as_str(), "client-1");
    let duplicate = runtime.recv().await.unwrap();
    assert!(runtime.admit(&app, duplicate).is_none());
    runtime
        .complete(first, Ok(CommandOutcome::Applied))
        .unwrap();

    let first = first_response.await.unwrap();
    let second = second_response.await.unwrap();
    assert_eq!(first, second);
    assert!(matches!(
        first.result,
        CommandResult::Ok {
            outcome: CommandOutcome::Applied,
            ..
        }
    ));
}

#[tokio::test]
async fn stale_client_can_always_request_a_full_snapshot() {
    let mut app = App::new(&Config::default());
    let (frontend, mut runtime) = WfeRuntime::new(&app, Vec::new()).unwrap();
    app.show_hotkeys = true;
    runtime.publish(&app).unwrap();
    assert_eq!(runtime.revision(), 1);

    let response = frontend
        .commands
        .try_submit(
            ConnectionId::new("client-1".to_string()).unwrap(),
            request("snapshot", 0, WebCommand::RequestSnapshot),
        )
        .unwrap();
    let envelope = runtime.recv().await.unwrap();
    let admitted = runtime.admit(&app, envelope).unwrap();
    runtime
        .complete(admitted, Ok(CommandOutcome::SnapshotQueued))
        .unwrap();
    assert!(matches!(
        response.await.unwrap().result,
        CommandResult::Ok {
            revision: 1,
            outcome: CommandOutcome::SnapshotQueued
        }
    ));
}

#[tokio::test]
async fn dismiss_overlay_cancels_confirmation_and_replays_at_the_applied_revision() {
    const OTHER_SESSION: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";

    let mut app = App::new(&Config::default());
    app.show_session_manager = false;
    app.show_hotkeys = true;
    app.show_debug = true;
    let (frontend, mut runtime) = WfeRuntime::new(&app, Vec::new()).unwrap();
    let confirmation_id = runtime
        .begin_confirmation_for_command(
            &WebCommand::DeleteSession {
                session_id: OTHER_SESSION.to_string(),
                confirmed: false,
                confirmation_id: None,
            },
            "Delete session?",
            "Delete the exact selected session.",
        )
        .unwrap();
    assert!(runtime.publish(&app).unwrap());
    let confirmation_revision = runtime.revision();
    assert_eq!(
        frontend.mirror.latest_snapshot().state.overlay.active_panel,
        Some(PanelId::Confirmation)
    );

    let dismiss_request = request(
        "dismiss-overlay",
        confirmation_revision,
        WebCommand::DismissOverlay,
    );
    let first_response = frontend
        .commands
        .try_submit(
            ConnectionId::new("client-dismiss".to_string()).unwrap(),
            dismiss_request.clone(),
        )
        .unwrap();
    let envelope = runtime.recv().await.unwrap();
    let admitted = runtime.admit(&app, envelope).unwrap();
    runtime.dismiss_overlay(&mut app);
    assert!(app.show_debug, "overlay dismissal closed the debugger pane");
    assert!(runtime.publish(&app).unwrap());
    let dismissed_revision = runtime.revision();
    runtime
        .complete(admitted, Ok(CommandOutcome::Applied))
        .unwrap();
    let first_response = first_response.await.unwrap();
    assert!(matches!(
        &first_response.result,
        CommandResult::Ok {
            revision,
            outcome: CommandOutcome::Applied,
        } if *revision == dismissed_revision
    ));
    assert_eq!(
        frontend.mirror.latest_snapshot().state.overlay,
        OverlayView {
            active_panel: None,
            data: None,
        }
    );
    assert!(frontend.mirror.latest_snapshot().state.debugger.open);

    let replayed = frontend
        .commands
        .try_submit(
            ConnectionId::new("client-replay".to_string()).unwrap(),
            dismiss_request,
        )
        .unwrap();
    let envelope = runtime.recv().await.unwrap();
    assert!(runtime.admit(&app, envelope).is_none());
    assert_eq!(replayed.await.unwrap(), first_response);
    assert_eq!(runtime.revision(), dismissed_revision);

    let stale = frontend
        .commands
        .try_submit(
            ConnectionId::new("client-stale".to_string()).unwrap(),
            request(
                "stale-dismiss",
                confirmation_revision,
                WebCommand::DismissOverlay,
            ),
        )
        .unwrap();
    let envelope = runtime.recv().await.unwrap();
    assert!(runtime.admit(&app, envelope).is_none());
    assert!(matches!(
        stale.await.unwrap().result,
        CommandResult::Error {
            error: CommandError {
                code: CommandErrorCode::StaleRevision,
                ..
            }
        }
    ));

    let stale_confirmation = frontend
        .commands
        .try_submit(
            ConnectionId::new("client-confirm".to_string()).unwrap(),
            request(
                "dismissed-confirmation",
                dismissed_revision,
                WebCommand::DeleteSession {
                    session_id: OTHER_SESSION.to_string(),
                    confirmed: true,
                    confirmation_id: Some(confirmation_id),
                },
            ),
        )
        .unwrap();
    let envelope = runtime.recv().await.unwrap();
    assert!(runtime.admit(&app, envelope).is_none());
    assert!(matches!(
        stale_confirmation.await.unwrap().result,
        CommandResult::Error {
            error: CommandError {
                code: CommandErrorCode::ConfirmationRequired,
                ..
            }
        }
    ));
}

#[test]
fn web_hotkeys_do_not_advertise_tui_mouse_capture() {
    let mut app = App::new(&Config::default());
    app.show_session_manager = false;
    app.show_hotkeys = true;
    let (frontend, _) = WfeRuntime::new(&app, Vec::new()).unwrap();
    let snapshot = frontend.mirror.latest_snapshot();
    let PanelDataView::Hotkeys { shortcuts } = snapshot.state.overlay.data.clone().unwrap() else {
        panic!("expected hotkeys panel data");
    };
    assert!(shortcuts.iter().all(|shortcut| shortcut.keys != "F10"));
    assert!(shortcuts.iter().all(|shortcut| {
        !shortcut
            .label
            .to_ascii_lowercase()
            .contains("mouse capture")
    }));
}

#[test]
fn volatile_sampling_does_not_advance_runtime_revision() {
    let mut app = App::new(&Config::default());
    let (_, mut runtime) = WfeRuntime::new(&app, Vec::new()).unwrap();
    app.memory_usage = 1024;
    app.tokens_per_s = 25.0;
    app.git_status = "dirty".to_string();
    assert!(!runtime.publish(&app).unwrap());
    assert_eq!(runtime.revision(), 0);
}

#[test]
fn operational_diagnostics_refresh_the_snapshot_without_staling_commands() {
    let app = App::new(&Config::default());
    let (frontend, mut runtime) = WfeRuntime::new(&app, Vec::new()).unwrap();
    let before = frontend.mirror.latest_snapshot();

    runtime.record_operational(OperationalEvent::Provider(
        crate::wfe::diagnostics::ProviderPhase::Failed,
    ));
    assert!(!runtime.publish(&app).unwrap());

    let after = frontend.mirror.latest_snapshot();
    assert_eq!(after.sequence, before.sequence);
    assert_eq!(after.revision, before.revision);
    assert_ne!(after.state.debugger.entries, before.state.debugger.entries);
    assert_eq!(
        after.state.debugger.entries[0].message,
        "Provider request failed."
    );
}

#[test]
fn a_dialog_closed_in_the_terminal_closes_in_the_browser() {
    let mut app = App::new(&Config::default());
    app.show_session_manager = false;
    let (frontend, mut runtime) = WfeRuntime::new(&app, Vec::new()).unwrap();
    let panel = |frontend: &WfeFrontendHandle| frontend.mirror.latest_snapshot().state.overlay.active_panel;

    for open in [
        |app: &mut App| app.show_hotkeys = true,
        |app: &mut App| app.show_history = true,
        |app: &mut App| app.show_latest_files = true,
        |app: &mut App| app.show_prompt_manager = true,
    ] {
        open(&mut app);
        runtime.publish(&app).unwrap();
        assert!(panel(&frontend).is_some(), "terminal dialog is mirrored");

        app.show_hotkeys = false;
        app.show_history = false;
        app.show_latest_files = false;
        app.show_prompt_manager = false;
        runtime.publish(&app).unwrap();
        assert_eq!(panel(&frontend), None, "closed with the terminal dialog");
    }

    // A panel the browser opened itself is not tied to a terminal flag.
    runtime.open_loop_modes(&app);
    runtime.publish(&app).unwrap();
    assert_eq!(panel(&frontend), Some(PanelId::LoopDetection));
}

#[test]
fn the_skills_menu_is_mirrored_with_resolvable_ids() {
    let mut app = App::new(&Config::default());
    app.show_session_manager = false;
    let skill = crate::skills::Skill {
        name: "greeting".to_string(),
        description: "How to greet".to_string(),
        dir: std::path::PathBuf::from("/tmp/greeting"),
        source: crate::skills::SkillSource::LetheticProject,
        enabled: true,
    };
    app.skills_panel = Some(crate::app::SkillsPanel {
        rows: vec![
            crate::app::SkillRow::Installed(skill),
            crate::app::SkillRow::Catalog {
                entry: &crate::skills::catalog::ENTRIES[0],
                installed: false,
            },
        ],
        selected: 0,
        installing: None,
        message: None,
    });
    let (frontend, mut runtime) = WfeRuntime::new(&app, Vec::new()).unwrap();
    runtime.publish(&app).unwrap();
    let overlay = frontend.mirror.latest_snapshot().state.overlay.clone();
    assert_eq!(overlay.active_panel, Some(PanelId::Skills));
    let Some(PanelDataView::Skills { skills, catalog, .. }) = overlay.data else {
        panic!("skills panel data");
    };
    assert_eq!(skills[0].name, "greeting");
    assert!(skills[0].enabled);
    assert_eq!(
        skill_choice_map(&app).get(&skills[0].skill_id).map(String::as_str),
        Some("greeting")
    );
    assert_eq!(
        skill_catalog_map().get(&catalog[0].entry_id).map(String::as_str),
        Some(crate::skills::catalog::ENTRIES[0].name)
    );
    assert!(catalog[0].url.starts_with("https://github.com/anthropics/skills/tree/"));

    app.skills_panel = None;
    runtime.publish(&app).unwrap();
    assert_eq!(frontend.mirror.latest_snapshot().state.overlay.active_panel, None);
}
