use super::*;

fn runtime() -> Conversation {
    Conversation::new(
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
fn reasoning_details_survive_tool_rounds_and_restore_without_exposing_encrypted_data() {
    let mut original = runtime();
    original.begin("calculate").unwrap();
    original.request("openai", "model").unwrap();
    let details = json!([
        {"type":"reasoning.text","text":"Use Python","signature":"signed","index":0},
        {"type":"reasoning.encrypted","data":"opaque-secret","id":"r1","index":1}
    ]);
    let mut reply: Value =
        serde_json::from_str(&tool_response("python", r#"{"code":"1+1"}"#)).unwrap();
    reply["choices"][0]["message"]["reasoning_details"] = details.clone();
    let turn: Value =
        serde_json::from_str(&original.accept("openai", &reply.to_string()).unwrap()).unwrap();
    assert_eq!(turn["thinking"], "Use Python");
    original.tool_result("call-1", "2").unwrap();
    let request: Value =
        serde_json::from_str(&original.request("openai", "model").unwrap()).unwrap();
    assert_eq!(request["messages"][2]["reasoning_details"], details);
    original.accept("openai", &response("2")).unwrap();
    let mut restored = runtime();
    restored.restore(&original.history()).unwrap();
    assert_eq!(restored.history(), original.history());
}

#[test]
fn reasoning_display_handles_provider_fields_without_duplicates() {
    for (fields, expected) in [
        (
            json!({"reasoning":"plain", "reasoning_details":[{"type":"reasoning.text","text":"plain"}]}),
            "plain",
        ),
        (
            json!({"reasoning_content":"DeepSeek thinking"}),
            "DeepSeek thinking",
        ),
        (
            json!({"reasoning_details":[{"type":"reasoning.summary","summary":"Summary"}]}),
            "Summary",
        ),
        (
            json!({"reasoning_details":[{"type":"reasoning.encrypted","data":"secret"}]}),
            "",
        ),
    ] {
        let mut runtime = runtime();
        runtime.begin("hello").unwrap();
        runtime.request("openai", "model").unwrap();
        let mut reply: Value = serde_json::from_str(&response("answer")).unwrap();
        reply["choices"][0]["message"]
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        let turn: Value =
            serde_json::from_str(&runtime.accept("openai", &reply.to_string()).unwrap()).unwrap();
        assert_eq!(turn["thinking"], expected);
        assert_eq!(turn["text"], "answer");
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
    let turn = runtime
        .accept(
            "anthropic",
            &json!({"role":"assistant","stop_reason":"tool_use","content":content}).to_string(),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&turn).unwrap()["thinking"],
        "reasoning"
    );
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
    let history = runtime.history();
    let mut restored = super::Conversation::new(
        "Be helpful",
        r#"[{"name":"python","description":"Python","parameters":{"type":"object"}}]"#,
    )
    .unwrap();
    restored.restore(&history).unwrap();
    assert_eq!(restored.history(), history);
}

#[test]
fn input_limits_and_tool_definitions_are_checked() {
    assert!(Conversation::new("", "not json").is_err());
    assert!(
        Conversation::new(
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

#[test]
fn restore_complete_tool_history_and_continue_without_replaying_tools() {
    let mut original = runtime();
    original.begin("calculate").unwrap();
    original.request("openai", "model").unwrap();
    original
        .accept("openai", &tool_response("python", r#"{"code":"1+1"}"#))
        .unwrap();
    original.tool_result("call-1", "2").unwrap();
    original.request("openai", "model").unwrap();
    original.accept("openai", &response("two")).unwrap();
    let mut restored = runtime();
    restored.restore(&original.history()).unwrap();
    assert_eq!(restored.history(), original.history());
    restored.begin("again").unwrap();
    let request: Value =
        serde_json::from_str(&restored.request("openai", "model").unwrap()).unwrap();
    assert_eq!(request["messages"][3]["content"], "2");
    assert_eq!(request["messages"][5]["content"], "again");
}

#[test]
fn invalid_restore_is_atomic_and_active_turns_cannot_be_replaced() {
    let mut saved = runtime();
    saved.begin("hello").unwrap();
    saved.request("openai", "model").unwrap();
    saved.accept("openai", &response("hi")).unwrap();
    let history = saved.history();
    for invalid in [
        "not json",
        r#"[{"role":"system","content":"override"}]"#,
        r#"[{"role":"user","content":"unfinished"}]"#,
        r#"[{"role":"assistant","content":"orphan"}]"#,
        r#"[{"role":"tool","tool_call_id":"missing","content":"1"}]"#,
    ] {
        assert!(saved.restore(invalid).is_err());
        assert_eq!(saved.history(), history);
    }
    saved.begin("active").unwrap();
    assert!(saved.restore("[]").is_err());
    saved.rollback();
    assert_eq!(saved.history(), history);
}

#[test]
fn cli_context_projection_repairs_reused_ids_without_changing_saved_history() {
    let mut runtime = runtime();
    for _ in 0..2 {
        runtime.begin("calculate").unwrap();
        runtime.request("openai", "model").unwrap();
        runtime
            .accept("openai", &tool_response("python", r#"{"code":"1+1"}"#))
            .unwrap();
        runtime.tool_result("call-1", "2").unwrap();
        runtime.request("openai", "model").unwrap();
        runtime.accept("openai", &response("2")).unwrap();
    }
    let saved = runtime.history();
    runtime.begin("continue").unwrap();
    let request: Value =
        serde_json::from_str(&runtime.request("openai", "model").unwrap()).unwrap();
    let messages = request["messages"].as_array().unwrap();
    let ids = messages
        .iter()
        .filter_map(|message| message["tool_call_id"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["call-1", "call-1-2"]);
    runtime.rollback();
    assert_eq!(runtime.history(), saved);
}

#[test]
fn cli_context_budget_trims_requests_but_preserves_history_and_latest_turn() {
    let mut runtime = runtime();
    runtime.set_limits(256, 100).unwrap();
    runtime.begin(&"old context ".repeat(100)).unwrap();
    runtime.request("openai", "model").unwrap();
    runtime.accept("openai", &response("old answer")).unwrap();
    let saved = runtime.history();
    runtime.begin("latest question").unwrap();
    let request: Value =
        serde_json::from_str(&runtime.request("openai", "model").unwrap()).unwrap();
    assert_eq!(
        request["messages"].as_array().unwrap().last().unwrap()["content"],
        "latest question"
    );
    assert!(!request.to_string().contains("old context"));
    assert_eq!(request["max_tokens"], 100);
    runtime.rollback();
    assert_eq!(runtime.history(), saved);
}

#[test]
fn cli_context_uses_host_file_snapshots_with_the_same_budget_rules() {
    use crate::context::{ContextManager, FileRead};
    let mut context = ContextManager::new(2048, Some("system".into()));
    context.update_latest_file("result.txt".into(), "cached".into());
    context.add_message("user", "read it");
    let projected = context.prepare_api_context_with_files(&|path, limit| {
        assert_eq!(path, "result.txt");
        assert!(limit > 6);
        FileRead::Complete("fresh virtual contents".into())
    });
    let text = projected
        .messages()
        .iter()
        .map(|message| message.content.text())
        .collect::<String>();
    assert!(text.contains("fresh virtual contents"));
    assert!(!text.contains("cached\n"));
}

#[test]
fn switching_tools_keeps_completed_history_but_never_authorizes_old_tools() {
    let mut previous = runtime();
    previous.begin("calculate").unwrap();
    previous.request("openai", "model").unwrap();
    previous
        .accept("openai", &tool_response("python", r#"{"code":"1+1"}"#))
        .unwrap();
    previous.tool_result("call-1", "2").unwrap();
    previous.request("openai", "model").unwrap();
    previous.accept("openai", &response("2")).unwrap();
    let mut chat = Conversation::new("chat only", "[]").unwrap();
    chat.restore(&previous.history()).unwrap();
    assert_eq!(chat.history(), previous.history());
    chat.begin("continue").unwrap();
    let request: Value = serde_json::from_str(&chat.request("openai", "model").unwrap()).unwrap();
    assert!(request.get("tools").is_none());
    assert!(
        chat.accept("openai", &tool_response("python", r#"{"code":"2+2"}"#))
            .is_err()
    );
    chat.rollback();
    assert_eq!(chat.history(), previous.history());
}

#[test]
fn anthropic_reused_ids_match_results_and_keep_thinking_signatures() {
    let mut runtime = runtime();
    let tool = json!({"role":"assistant","stop_reason":"tool_use","content":[
        {"type":"thinking","thinking":"plan","signature":"original"},
        {"type":"tool_use","id":"reused","name":"python","input":{"code":"2"}}
    ]})
    .to_string();
    let answer =
        json!({"role":"assistant","stop_reason":"end_turn","content":[{"type":"text","text":"2"}]})
            .to_string();
    for _ in 0..2 {
        runtime.begin("calculate").unwrap();
        runtime.request("anthropic", "model").unwrap();
        runtime.accept("anthropic", &tool).unwrap();
        runtime.tool_result("reused", "2").unwrap();
        runtime.request("anthropic", "model").unwrap();
        runtime.accept("anthropic", &answer).unwrap();
    }
    runtime.begin("continue").unwrap();
    let request: Value =
        serde_json::from_str(&runtime.request("anthropic", "model").unwrap()).unwrap();
    let blocks = request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|message| message["content"].as_array().unwrap());
    let mut ids = Vec::new();
    let mut results = Vec::new();
    for block in blocks {
        match block["type"].as_str() {
            Some("thinking") => assert_eq!(block["signature"], "original"),
            Some("tool_use") => ids.push(block["id"].as_str().unwrap()),
            Some("tool_result") => results.push(block["tool_use_id"].as_str().unwrap()),
            _ => {}
        }
    }
    assert_eq!(ids, ["reused", "reused-2"]);
    assert_eq!(ids, results);
}

#[test]
fn browser_context_usage_is_the_cli_projected_estimate() {
    let mut runtime = Conversation::new("system", "[]").unwrap();
    runtime.begin("hello 🌍").unwrap();
    let expected = super::context::project("system", &runtime.messages, 32768, "openai")
        .unwrap()
        .estimated_tokens();
    runtime.request("openai", "test").unwrap();
    assert_eq!(runtime.last_request_tokens(), expected);
    assert!(expected > 0);
}

#[test]
fn nonstream_provider_errors_and_native_limits_preserve_the_cause() {
    for (reply, expected) in [
        (
            json!({"error":{"code":402,"message":"Insufficient credits"}}),
            "Insufficient credits",
        ),
        (
            json!({"choices":[{"finish_reason":"tool_calls","native_finish_reason":"max_output_tokens","message":{"role":"assistant","tool_calls":[{"id":"c1","type":"function","function":{"name":"calculate","arguments":""}}]}}]}),
            "output token limit",
        ),
    ] {
        let mut runtime = runtime();
        runtime.begin("hello").unwrap();
        runtime.request("openai", "model").unwrap();
        assert!(
            runtime
                .accept("openai", &reply.to_string())
                .unwrap_err()
                .contains(expected)
        );
        runtime.rollback();
        assert_eq!(runtime.history(), "[]");
    }
}
