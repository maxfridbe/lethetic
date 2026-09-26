use lethetic::app::App;
use lethetic::config::Config;
use lethetic::wfe::contracts::{
    CommandError, CommandErrorCode, CommandOutcome, CommandResult, DiagnosticCode,
    DiagnosticSeverity, DiagnosticView, ICommandResponse, IProtocolHello, IServerMessage,
    IStatePatch, IStateSnapshot, MAX_SAFE_JAVASCRIPT_INTEGER, StateChange,
};
use lethetic::wfe::presentation::{ProjectionContext, project_app};

#[test]
fn checked_in_typescript_contracts_match_rust_generation() {
    let expected = lethetic::wfe::generated_typescript_source().unwrap();
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("web/src/generated/contracts.ts");
    let actual = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "could not read generated contracts at {}: {error}",
            path.display()
        )
    });
    assert_eq!(
        actual.replace("\r\n", "\n"),
        expected,
        "generated contracts are stale; run `cargo run --bin generate_web_contracts`"
    );
}

#[test]
fn state_patch_contains_only_changed_top_level_sections() {
    let app = App::new(&Config::default());
    let previous = project_app(&app, ProjectionContext::default());
    let mut current = previous.clone();
    current.debugger.entries.push(DiagnosticView {
        code: DiagnosticCode::ConnectionInterrupted,
        severity: DiagnosticSeverity::Warning,
        message: "Connection interrupted".to_string(),
    });

    let patch = IStatePatch::between(8, 6, 7, &previous, &current).unwrap();
    assert_eq!(patch.sequence, 8);
    assert_eq!(patch.base_revision, 6);
    assert_eq!(patch.revision, 7);
    assert_eq!(patch.changes.len(), 1);
    assert!(matches!(
        &patch.changes[0],
        StateChange::Debugger { value } if value.entries.len() == 1
    ));
    assert!(IStatePatch::between(8, 7, 7, &previous, &current).is_err());
    assert!(IStatePatch::between(8, 6, 7, &previous, &previous).is_err());

    let mut invalid = serde_json::to_value(&patch).unwrap();
    invalid["revision"] = serde_json::json!(6);
    assert!(serde_json::from_value::<IStatePatch>(invalid).is_err());

    let mut duplicate = serde_json::to_value(&patch).unwrap();
    let first = duplicate["changes"][0].clone();
    duplicate["changes"].as_array_mut().unwrap().push(first);
    assert!(serde_json::from_value::<IStatePatch>(duplicate).is_err());
}

#[test]
fn response_and_server_message_shapes_are_stable_and_exact() {
    let response = ICommandResponse {
        id: "request-1".to_string(),
        result: CommandResult::Ok {
            revision: 4,
            outcome: CommandOutcome::Applied,
        },
    };
    let value = serde_json::to_value(&response).unwrap();
    assert_eq!(value["id"], "request-1");
    assert_eq!(value["result"]["status"], "ok");
    assert_eq!(value["result"]["revision"], 4);
    assert_eq!(value["result"]["outcome"]["type"], "applied");
    assert_eq!(value.as_object().unwrap().len(), 2);

    let mut unknown = value;
    unknown["debug"] = serde_json::json!("private");
    assert!(serde_json::from_value::<ICommandResponse>(unknown).is_err());

    let hello = IProtocolHello::new(3, 2, Default::default()).unwrap();
    let message = IServerMessage::Hello { hello };
    let message = serde_json::to_value(message).unwrap();
    assert_eq!(message["type"], "hello");
    assert_eq!(message["hello"]["server_name"], "lethetic");
}

#[test]
fn v6_history_response_is_full_and_session_scoped() {
    const SESSION_ID: &str = "11111111-2222-4333-8444-555555555555";
    let original = "界".repeat(2_000);
    let response = ICommandResponse {
        id: "history-response".to_string(),
        result: CommandResult::Ok {
            revision: 4,
            outcome: CommandOutcome::HistoryEntrySelected {
                session_id: SESSION_ID.to_string(),
                entry_id: "history-1".to_string(),
                editor_content: original.clone(),
            },
        },
    };
    response.validate().unwrap();
    let value = serde_json::to_value(&response).unwrap();
    assert_eq!(value["result"]["outcome"]["type"], "history_entry_selected");
    assert_eq!(value["result"]["outcome"]["session_id"], SESSION_ID);
    assert_eq!(value["result"]["outcome"]["entry_id"], "history-1");
    assert_eq!(value["result"]["outcome"]["editor_content"], original);
    assert_eq!(
        serde_json::from_value::<ICommandResponse>(value).unwrap(),
        response
    );
}

#[test]
fn every_server_message_variant_roundtrips_with_exact_tags() {
    let app = App::new(&Config::default());
    let previous = project_app(&app, ProjectionContext::default());
    let mut current = previous.clone();
    current.debugger.entries.push(DiagnosticView {
        code: DiagnosticCode::Unknown,
        severity: DiagnosticSeverity::Info,
        message: "status changed".to_string(),
    });
    let messages = vec![
        IServerMessage::Hello {
            hello: IProtocolHello::new(1, 0, Default::default()).unwrap(),
        },
        IServerMessage::CommandResponse {
            response: ICommandResponse {
                id: "request-ok".to_string(),
                result: CommandResult::Ok {
                    revision: 1,
                    outcome: CommandOutcome::Applied,
                },
            },
        },
        IServerMessage::CommandResponse {
            response: ICommandResponse {
                id: "request-error".to_string(),
                result: CommandResult::Error {
                    error: CommandError {
                        code: CommandErrorCode::StaleRevision,
                        message: "State revision changed".to_string(),
                        current_revision: Some(1),
                        retryable: true,
                    },
                },
            },
        },
        IServerMessage::StateSnapshot {
            snapshot: Box::new(IStateSnapshot::new(1, 0, previous.clone()).unwrap()),
        },
        IServerMessage::StatePatch {
            patch: IStatePatch::between(2, 0, 1, &previous, &current).unwrap(),
        },
    ];
    let expected_types = [
        "hello",
        "command_response",
        "command_response",
        "state_snapshot",
        "state_patch",
    ];
    for (message, expected_type) in messages.into_iter().zip(expected_types) {
        let value = serde_json::to_value(&message).unwrap();
        assert_eq!(value["type"], expected_type);
        let decoded: IServerMessage = serde_json::from_value(value).unwrap();
        assert_eq!(decoded, message);
    }
}

#[test]
fn snapshot_constructor_rejects_unsafe_javascript_integers() {
    let app = App::new(&Config::default());
    let state = project_app(&app, ProjectionContext::default());
    let snapshot = IStateSnapshot::new(MAX_SAFE_JAVASCRIPT_INTEGER, 0, state.clone()).unwrap();
    assert_eq!(snapshot.protocol_version, 6);
    let mut legacy = serde_json::to_value(&snapshot).unwrap();
    legacy["protocol_version"] = serde_json::json!(5);
    assert!(serde_json::from_value::<IStateSnapshot>(legacy).is_err());
    assert!(IStateSnapshot::new(MAX_SAFE_JAVASCRIPT_INTEGER + 1, 0, state.clone()).is_err());
    assert!(IStateSnapshot::new(0, MAX_SAFE_JAVASCRIPT_INTEGER + 1, state.clone()).is_err());
    assert!(IProtocolHello::new(MAX_SAFE_JAVASCRIPT_INTEGER + 1, 0, Default::default(),).is_err());
    let response = serde_json::json!({
        "id": "request-1",
        "result": {
            "status": "ok",
            "revision": MAX_SAFE_JAVASCRIPT_INTEGER + 1,
            "outcome": { "type": "applied" }
        }
    });
    assert!(serde_json::from_value::<ICommandResponse>(response).is_err());
}
