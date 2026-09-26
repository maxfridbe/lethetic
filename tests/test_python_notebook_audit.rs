use lethetic::app::{App, handle_tool_call_with_provider};
use lethetic::client::StreamEvent;
use lethetic::config::{Config, PythonExecutionTarget, PythonRuntimeConfig, ToolProfile};
use lethetic::context::{FunctionCall, ToolCall};
use lethetic::tool_runtime::ToolRuntime;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

fn non_durable_app(config: &Config, workspace: std::path::PathBuf) -> App {
    let mut app = App::new(config);
    app.current_dir = workspace.to_string_lossy().into_owned();
    // A fresh cwd may create a session; the replacement runtime is not bound to it.
    app.current_session_dir = None;
    app.session_directory_binding = None;
    app.tool_runtime = ToolRuntime::interactive(workspace);
    app
}

#[test]
fn native_provider_id_is_the_notebook_audit_identity() {
    let workspace = tempfile::tempdir().unwrap();
    let workspace = workspace.path().canonicalize().unwrap();
    let config = Config {
        tool_profile: ToolProfile::PythonOnly,
        python_runtime: PythonRuntimeConfig {
            target: Some(PythonExecutionTarget::Host),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut app = non_durable_app(&config, workspace);
    let source = "value = {'exact': True}\nvalue\n";
    let call = ToolCall {
        id: "effective-openai-id".to_string(),
        provider_id: Some("toolu-provider-envelope".to_string()),
        function: FunctionCall {
            name: "python".to_string(),
            arguments: serde_json::json!({
                "code": source,
                "description": "preserve provider identity"
            }),
        },
    };
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<StreamEvent>();
    let mut cancellation = CancellationToken::new();

    handle_tool_call_with_provider(
        &mut app,
        vec![call],
        0,
        tx,
        &mut cancellation,
        "",
        true,
        None,
    );

    let path = app
        .tool_runtime
        .python_notebook_path()
        .expect("Python recognition must create the audit notebook");
    let notebook: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(
        notebook["cells"][0]["metadata"]["lethetic"]["tool_call_id"],
        "toolu-provider-envelope"
    );
    assert_eq!(notebook["cells"][0]["source"], source);
    let pending = app.pending_tool_call.as_ref().unwrap();
    assert_eq!(pending.id, "effective-openai-id");
    assert_eq!(
        pending.provider_id.as_deref(),
        Some("toolu-provider-envelope")
    );
    assert!(
        pending.function.arguments[lethetic::tools::INTERNAL_TOOL_CALL_ID_KEY].is_null(),
        "host-only audit IDs must not alter provider replay arguments"
    );
}

#[test]
fn malformed_python_call_is_a_terminal_denied_audit_cell() {
    let workspace = tempfile::tempdir().unwrap();
    let workspace = workspace.path().canonicalize().unwrap();
    let config = Config {
        tool_profile: ToolProfile::PythonOnly,
        python_runtime: PythonRuntimeConfig {
            target: Some(PythonExecutionTarget::Host),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut app = non_durable_app(&config, workspace);
    let call = ToolCall {
        id: "effective-malformed-id".to_string(),
        provider_id: Some("toolu-malformed".to_string()),
        function: FunctionCall {
            name: "python".to_string(),
            arguments: serde_json::json!({
                "code": 42,
                "description": "invalid code type"
            }),
        },
    };
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<StreamEvent>();
    let mut cancellation = CancellationToken::new();

    handle_tool_call_with_provider(
        &mut app,
        vec![call],
        0,
        tx,
        &mut cancellation,
        "",
        true,
        None,
    );

    let path = app.tool_runtime.python_notebook_path().unwrap();
    let notebook: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(
        notebook["cells"][0]["metadata"]["lethetic"]["status"],
        "denied"
    );
    assert_eq!(notebook["cells"][0]["source"], "");
    assert!(matches!(
        rx.try_recv().unwrap(),
        StreamEvent::PythonNotebookNotice(_)
    ));
    assert!(matches!(
        rx.try_recv().unwrap(),
        StreamEvent::ToolResult { is_error: true, .. }
    ));
}

#[test]
fn disallowed_python_dispatch_is_a_terminal_denied_audit_cell() {
    let workspace = tempfile::tempdir().unwrap();
    let workspace = workspace.path().canonicalize().unwrap();
    let config = Config::default();
    let mut app = non_durable_app(&config, workspace);
    let call = ToolCall {
        id: "effective-disallowed-id".to_string(),
        provider_id: Some("toolu-disallowed".to_string()),
        function: FunctionCall {
            name: "python".to_string(),
            arguments: serde_json::json!({
                "code": "1 + 1",
                "description": "not allowed in General mode"
            }),
        },
    };
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<StreamEvent>();
    let mut cancellation = CancellationToken::new();

    handle_tool_call_with_provider(
        &mut app,
        vec![call],
        0,
        tx,
        &mut cancellation,
        "",
        true,
        None,
    );

    let path = app.tool_runtime.python_notebook_path().unwrap();
    let notebook: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(
        notebook["cells"][0]["metadata"]["lethetic"]["status"],
        "denied"
    );
    assert!(app.tool_call_dispatched);
    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, StreamEvent::ToolResult { is_error: true, .. }))
    );
}

#[cfg(target_os = "linux")]
#[test]
fn durable_notebook_checkpoint_requires_an_active_session_lease() {
    let workspace = tempfile::tempdir().unwrap();
    let workspace = workspace.path().canonicalize().unwrap();
    let runtime = ToolRuntime::interactive(workspace.clone());
    let error = runtime
        .begin_python_audit_attempt(
            Some(&workspace),
            &Config::default(),
            "toolu-unleased",
            "pass",
            "must be leased",
            workspace.to_str().unwrap(),
        )
        .unwrap_err();
    assert!(error.contains("active session lease"), "{error}");
    assert!(!workspace.join("python.ipynb").exists());
}

#[test]
fn parallel_python_batch_is_rejected_and_every_call_is_audited() {
    let workspace = tempfile::tempdir().unwrap();
    let workspace = workspace.path().canonicalize().unwrap();
    let config = Config {
        context_size: 100_000,
        tool_profile: ToolProfile::PythonOnly,
        python_runtime: PythonRuntimeConfig {
            target: Some(PythonExecutionTarget::Host),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut app = non_durable_app(&config, workspace);
    let calls = [
        ("effective-one", "toolu-one", "first = 1"),
        ("effective-two", "toolu-two", "second = 2"),
    ]
    .into_iter()
    .map(|(id, provider_id, code)| ToolCall {
        id: id.to_string(),
        provider_id: Some(provider_id.to_string()),
        function: FunctionCall {
            name: "python".to_string(),
            arguments: serde_json::json!({
                "code": code,
                "description": format!("audit {id}")
            }),
        },
    })
    .collect::<Vec<_>>();
    app.context_manager
        .upsert_assistant_tool_call_with_provider("", calls.clone(), None);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<StreamEvent>();
    let mut cancellation = CancellationToken::new();

    let outcome = handle_tool_call_with_provider(
        &mut app,
        calls,
        0,
        tx,
        &mut cancellation,
        "Provider preamble",
        true,
        None,
    );

    assert_eq!(outcome, lethetic::app::AppEventOutcome::Continue);
    assert!(app.pending_tool_call.is_none());
    assert!(app.tool_call_pos.is_none());
    assert!(!app.show_approval_prompt);
    assert!(!app.is_asking_user);
    assert!(!app.is_processing);
    assert!(!app.is_executing_tool);
    assert!(!app.tool_call_dispatched);

    let messages = app.context_manager.get_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, "assistant");
    assert!(messages[0].tool_calls.is_none());
    assert!(messages[0].content.contains("Provider preamble"));
    assert!(messages[0].content.contains("entire batch was rejected"));

    while let Ok(event) = rx.try_recv() {
        assert!(
            matches!(event, StreamEvent::PythonNotebookNotice(_)),
            "parallel rejection must not continue with a tool result: {event:?}"
        );
    }

    let path = app.tool_runtime.python_notebook_path().unwrap();
    let notebook: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let cells = notebook["cells"].as_array().unwrap();
    assert_eq!(cells.len(), 2);
    assert_eq!(
        cells
            .iter()
            .map(|cell| cell["metadata"]["lethetic"]["tool_call_id"]
                .as_str()
                .unwrap())
            .collect::<Vec<_>>(),
        ["toolu-one", "toolu-two"]
    );
    assert!(
        cells
            .iter()
            .all(|cell| { cell["metadata"]["lethetic"]["status"] == serde_json::json!("denied") })
    );
}
