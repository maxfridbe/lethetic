use super::*;

fn runtime() -> BrowserRuntime {
    BrowserRuntime::new(
        "Be helpful",
        r#"[{"name":"python","description":"Python","parameters":{"type":"object"}}]"#,
    )
    .unwrap()
}

fn response(text: &str) -> String {
    json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":text}}]})
        .to_string()
}

fn tool_response(name: &str, arguments: &str) -> String {
    json!({"choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,
        "tool_calls":[{"id":"call-1","type":"function","function":{"name":name,"arguments":arguments}}]}}]}).to_string()
}

#[test]
fn completed_turns_are_preserved_and_credentials_are_absent() {
    let mut runtime = runtime();
    runtime.begin("hello").unwrap();
    let request: Value =
        serde_json::from_str(&runtime.request("openai", "model").unwrap()).unwrap();
    assert_eq!(request["messages"][0]["role"], "system");
    assert_eq!(request["tools"][0]["function"]["name"], "python");
    assert!(request.get("api_key").is_none());
    runtime.accept("openai", &response("hi")).unwrap();
    runtime.begin("second").unwrap();
    runtime.rollback();
    let history: Value = serde_json::from_str(&runtime.history()).unwrap();
    assert_eq!(history.as_array().unwrap().len(), 2);
    assert_eq!(history[1]["content"], "hi");
}

#[test]
fn tool_results_are_required_once_before_followup_request() {
    let mut runtime = runtime();
    runtime.begin("calculate").unwrap();
    runtime.request("openai", "model").unwrap();
    runtime
        .accept("openai", &tool_response("python", r#"{"code":"1+1"}"#))
        .unwrap();
    assert!(runtime.request("openai", "model").is_err());
    assert!(runtime.tool_result("other", "2").is_err());
    runtime.tool_result("call-1", "2").unwrap();
    assert!(runtime.tool_result("call-1", "2").is_err());
    let request: Value =
        serde_json::from_str(&runtime.request("openai", "model").unwrap()).unwrap();
    assert_eq!(request["messages"][3]["tool_call_id"], "call-1");
    runtime.accept("openai", &response("2")).unwrap();
    assert!(runtime.begin("next").is_ok());
}

#[test]
fn invalid_model_calls_never_enter_history() {
    for (name, arguments) in [("shell", "{}"), ("python", "[]"), ("python", "not json")] {
        let mut runtime = runtime();
        runtime.begin("hello").unwrap();
        runtime.request("openai", "model").unwrap();
        assert!(
            runtime
                .accept("openai", &tool_response(name, arguments))
                .is_err()
        );
        runtime.rollback();
        assert_eq!(runtime.history(), "[]");
    }
}

#[test]
fn invalid_transitions_and_duplicate_calls_are_rejected() {
    let mut runtime = runtime();
    assert!(runtime.request("openai", "model").is_err());
    assert!(runtime.accept("openai", &response("hi")).is_err());
    assert!(runtime.begin("  ").is_err());
    runtime.begin("hello").unwrap();
    assert!(runtime.begin("again").is_err());
    assert!(runtime.request("unsupported", "model").is_err());
    runtime.request("openai", "model").unwrap();
    assert!(runtime.request("openai", "model").is_err());
    let mut response: Value = serde_json::from_str(&tool_response("python", "{}")).unwrap();
    let calls = response["choices"][0]["message"]["tool_calls"]
        .as_array_mut()
        .unwrap();
    calls.push(calls[0].clone());
    assert!(runtime.accept("openai", &response.to_string()).is_err());
}

#[test]
fn truncated_and_filtered_responses_do_not_complete_turns() {
    for reason in ["length", "content_filter", "", "tool_calls"] {
        let mut runtime = runtime();
        runtime.begin("hello").unwrap();
        runtime.request("openai", "model").unwrap();
        let mut reply: Value = serde_json::from_str(&response("partial")).unwrap();
        reply["choices"][0]["finish_reason"] = json!(reason);
        assert!(runtime.accept("openai", &reply.to_string()).is_err());
    }
}

#[test]
fn anthropic_multiple_results_are_grouped_and_thinking_is_preserved() {
    let mut runtime = runtime();
    runtime.begin("calculate").unwrap();
    let request: Value =
        serde_json::from_str(&runtime.request("anthropic", "model").unwrap()).unwrap();
    assert_eq!(request["system"], "Be helpful");
    assert_eq!(request["tools"][0]["input_schema"]["type"], "object");
    let content = json!([
        {"type":"thinking","thinking":"reasoning","signature":"signed"},
        {"type":"tool_use","id":"a","name":"python","input":{"code":"1"}},
        {"type":"tool_use","id":"b","name":"python","input":{"code":"2"}}
    ]);
    runtime
        .accept(
            "anthropic",
            &json!({"role":"assistant","stop_reason":"tool_use","content":content}).to_string(),
        )
        .unwrap();
    runtime.tool_result("a", "1").unwrap();
    runtime.tool_result("b", "2").unwrap();
    let request: Value =
        serde_json::from_str(&runtime.request("anthropic", "model").unwrap()).unwrap();
    assert_eq!(request["messages"][1]["content"], content);
    assert_eq!(
        request["messages"][2]["content"].as_array().unwrap().len(),
        2
    );
    assert_eq!(request["messages"][2]["content"][1]["tool_use_id"], "b");
    runtime
        .accept(
            "anthropic",
            &json!({"role":"assistant","stop_reason":"end_turn",
        "content":[{"type":"text","text":"done"}]})
            .to_string(),
        )
        .unwrap();
}

#[test]
fn input_limits_and_tool_definitions_are_checked() {
    assert!(BrowserRuntime::new("", "not json").is_err());
    assert!(
        BrowserRuntime::new(
            "",
            r#"[{"name":"bad name","description":"","parameters":{}}]"#
        )
        .is_err()
    );
    let mut runtime = runtime();
    assert!(runtime.begin(&"x".repeat(MAX_BYTES + 1)).is_err());
    runtime.begin("ok").unwrap();
    runtime.clear();
    assert_eq!(runtime.history(), "[]");
    assert!(runtime.begin("new").is_ok());
}
