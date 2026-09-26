use serde_json::{Value, json};

use crate::config::{Config, ConnectionKind, ToolProfile};
use crate::transport::{Message, Role, ToolDefinition};

fn python_guidance(config: &Config) -> Result<String, String> {
    crate::system_prompt::python_capability_guidance(config)
        .ok_or_else(|| "Python-only request is missing host capability guidance".to_string())
}

fn has_exact_guidance_suffix(content: &str, guidance: &str) -> bool {
    content.ends_with(guidance) && content.match_indices(guidance).count() == 1
}

fn messages_have_exact_guidance(messages: &[Message], guidance: &str) -> bool {
    let system = messages
        .iter()
        .filter(|message| message.role == Role::System)
        .collect::<Vec<_>>();
    let Some((last, preceding)) = system.split_last() else {
        return false;
    };
    preceding
        .iter()
        .all(|message| !message.content.text().contains(guidance))
        && has_exact_guidance_suffix(&last.content.text(), guidance)
}

fn openai_body_has_exact_guidance(body: &Value, guidance: &str) -> bool {
    let Some(messages) = body.get("messages").and_then(Value::as_array) else {
        return false;
    };
    let system = messages
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("system"))
        .collect::<Vec<_>>();
    let Some((last, preceding)) = system.split_last() else {
        return false;
    };
    preceding.iter().all(|message| {
        message
            .get("content")
            .and_then(Value::as_str)
            .is_none_or(|content| !content.contains(guidance))
    }) && last
        .get("content")
        .and_then(Value::as_str)
        .is_some_and(|content| has_exact_guidance_suffix(content, guidance))
}

fn canonical_python_tool() -> ToolDefinition {
    let tool = crate::tools::python::get_definition();
    ToolDefinition {
        name: tool.function.name,
        description: tool.function.description,
        input_schema: tool.function.parameters,
    }
}

pub(crate) fn native_tool_schema(schema: &Value) -> Value {
    let mut schema = schema.clone();
    if let Some(object) = schema.as_object_mut() {
        if let Some(properties) = object.get_mut("properties").and_then(Value::as_object_mut) {
            properties.remove("tool_call_id");
        }
        if let Some(required) = object.get_mut("required").and_then(Value::as_array_mut) {
            required.retain(|field| field.as_str() != Some("tool_call_id"));
        }
    }
    schema
}

fn canonical_openai_python_tool() -> Value {
    let tool = canonical_python_tool();
    json!({
        "type": "function",
        "function": {
            "name": tool.name,
            "description": tool.description,
            "parameters": tool.input_schema,
        }
    })
}

fn canonical_anthropic_python_tool() -> Value {
    let tool = canonical_python_tool();
    json!({
        "name": tool.name,
        "description": tool.description,
        "input_schema": native_tool_schema(&tool.input_schema),
    })
}

fn validate_common_agent_body(config: &Config, body: &Value) -> Result<(), String> {
    if body.get("model").and_then(Value::as_str) != Some(config.model.as_str()) {
        return Err("Agent request model does not match the active configuration".to_string());
    }
    if body.get("stream").and_then(Value::as_bool) != Some(true) {
        return Err("Agent request must use streaming transport".to_string());
    }
    if body.get("max_tokens").and_then(Value::as_u64)
        != Some(u64::from(config.request_output_tokens()))
    {
        return Err(
            "Agent request token limit does not match the active configuration".to_string(),
        );
    }
    Ok(())
}

fn reject_alternate_tool_fields(
    body: &Value,
    provider: &str,
    fields: &[&str],
) -> Result<(), String> {
    if let Some(field) = fields.iter().find(|field| body.get(**field).is_some()) {
        return Err(format!(
            "Python-only {provider} request contains forbidden alternate tool field `{field}`"
        ));
    }
    Ok(())
}

pub fn validate_agent_surface(
    config: &Config,
    messages: &[Message],
    tools: &[ToolDefinition],
) -> Result<(), String> {
    if config.tool_profile != ToolProfile::PythonOnly {
        return Ok(());
    }
    if let Some(error) = config.python_mode_validation_error() {
        return Err(format!("Invalid Python-only request policy: {error}"));
    }
    let expected = canonical_python_tool();
    if tools.len() != 1
        || tools[0].name != expected.name
        || tools[0].description != expected.description
        || tools[0].input_schema != expected.input_schema
    {
        return Err(
            "Python-only request must advertise exactly the canonical `python` tool".to_string(),
        );
    }
    let guidance = python_guidance(config)?;
    if !messages_have_exact_guidance(messages, &guidance) {
        return Err(
            "Python-only request is missing the exact final host capability guidance".to_string(),
        );
    }
    Ok(())
}

pub fn validate_final_agent_body(config: &Config, body: &Value) -> Result<(), String> {
    if config.tool_profile != ToolProfile::PythonOnly {
        return Ok(());
    }
    if let Some(error) = config.python_mode_validation_error() {
        return Err(format!("Invalid Python-only request policy: {error}"));
    }
    validate_common_agent_body(config, body)?;
    let guidance = python_guidance(config)?;
    match config.active_connection_kind() {
        ConnectionKind::OpenAiChatCompletions => {
            let tools = body
                .get("tools")
                .and_then(Value::as_array)
                .ok_or_else(|| "Python-only OpenAI request is missing tools".to_string())?;
            if tools.as_slice() != [canonical_openai_python_tool()] {
                return Err(
                    "Python-only OpenAI request must contain exactly the canonical `python` tool"
                        .to_string(),
                );
            }
            if body.get("parallel_tool_calls").and_then(Value::as_bool) != Some(false) {
                return Err("Python-only OpenAI request must disable parallel tools".to_string());
            }
            if body.get("tool_choice").is_some() {
                return Err(
                    "Python-only OpenAI request contains a noncanonical tool choice".to_string(),
                );
            }
            if body.get("stream_options") != Some(&json!({"include_usage": true})) {
                return Err(
                    "Python-only OpenAI request must retain exact streaming usage options"
                        .to_string(),
                );
            }
            reject_alternate_tool_fields(
                body,
                "OpenAI",
                &[
                    "functions",
                    "function_call",
                    "mcp_servers",
                    "max_completion_tokens",
                    "n",
                ],
            )?;
            if !openai_body_has_exact_guidance(body, &guidance) {
                return Err(
                    "Python-only OpenAI request is missing the exact final host capability guidance"
                        .to_string(),
                );
            }
        }
        ConnectionKind::ClaudeCodeProxy => {
            let tools = body
                .get("tools")
                .and_then(Value::as_array)
                .ok_or_else(|| "Python-only Anthropic request is missing tools".to_string())?;
            if tools.as_slice() != [canonical_anthropic_python_tool()] {
                return Err(
                    "Python-only Anthropic request must contain exactly the canonical `python` tool"
                        .to_string(),
                );
            }
            if body.get("tool_choice")
                != Some(&json!({
                    "type": "auto",
                    "disable_parallel_tool_use": true
                }))
            {
                return Err(
                    "Python-only Anthropic request must use the exact nonparallel automatic tool choice"
                        .to_string(),
                );
            }
            reject_alternate_tool_fields(
                body,
                "Anthropic",
                &[
                    "parallel_tool_calls",
                    "functions",
                    "function_call",
                    "mcp_servers",
                ],
            )?;
            if !body
                .get("system")
                .and_then(Value::as_str)
                .is_some_and(|system| has_exact_guidance_suffix(system, &guidance))
            {
                return Err(
                    "Python-only Anthropic request is missing the exact final host capability guidance"
                        .to_string(),
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        AccessMode, NetworkAccess, PackageAccess, PythonExecutionTarget, SandboxBackend,
    };
    use crate::tools::ToolSurface;

    fn config(kind: ConnectionKind) -> Config {
        let mut config = Config {
            server_url: "http://127.0.0.1:1/v1".to_string(),
            model: "policy-test-model".to_string(),
            context_size: 100_000,
            connection_kind: kind,
            tool_profile: ToolProfile::PythonOnly,
            ..Default::default()
        };
        config.python_runtime.target = Some(PythonExecutionTarget::Sandbox);
        config.python_runtime.sandbox.backend = Some(SandboxBackend::Podman);
        config.python_runtime.sandbox.network = Some(NetworkAccess::None);
        config.python_runtime.sandbox.workspace_access = Some(AccessMode::ReadWrite);
        config.python_runtime.sandbox.package_access = PackageAccess::Disabled;
        config
    }

    fn messages(config: &Config) -> Vec<Message> {
        let guidance = python_guidance(config).unwrap();
        vec![
            Message::system(format!("Synthetic host template\n\n{guidance}")),
            Message::user("Run one cell"),
        ]
    }

    fn tools(config: &Config) -> Vec<ToolDefinition> {
        crate::tools::get_api_tools(config, ToolSurface::Headless)
    }

    fn body_for(config: &Config, surface: ToolSurface) -> Value {
        let prepared = crate::transport::prepare_agent_request(
            config,
            &messages(config),
            &crate::tools::get_api_tools(config, surface),
            config.request_output_tokens(),
        )
        .unwrap();
        serde_json::from_slice(prepared.body_bytes()).unwrap()
    }

    fn body(config: &Config) -> Value {
        body_for(config, ToolSurface::Headless)
    }

    #[test]
    fn both_provider_and_surface_bodies_have_one_exact_nonparallel_python_tool() {
        for kind in [
            ConnectionKind::OpenAiChatCompletions,
            ConnectionKind::ClaudeCodeProxy,
        ] {
            let config = config(kind);
            for surface in [ToolSurface::Interactive, ToolSurface::Headless] {
                let body = body_for(&config, surface);
                validate_final_agent_body(&config, &body).unwrap();
                assert_eq!(body["tools"].as_array().unwrap().len(), 1);
                assert_eq!(body["stream"], true);
                assert_eq!(body["max_tokens"], config.request_output_tokens());
                match kind {
                    ConnectionKind::OpenAiChatCompletions => {
                        assert_eq!(body["tools"][0]["type"], "function");
                        assert_eq!(body["tools"][0]["function"]["name"], "python");
                        assert_eq!(body["parallel_tool_calls"], false);
                        assert!(body.get("tool_choice").is_none());
                    }
                    ConnectionKind::ClaudeCodeProxy => {
                        assert_eq!(body["tools"][0]["name"], "python");
                        assert_eq!(body["tool_choice"]["type"], "auto");
                        assert_eq!(body["tool_choice"]["disable_parallel_tool_use"], true);
                        assert!(
                            body["tools"][0]["input_schema"]["properties"]
                                .get("tool_call_id")
                                .is_none()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn surface_requires_canonical_tool_and_final_guidance_suffix() {
        let config = config(ConnectionKind::OpenAiChatCompletions);
        let mut wrong_tools = tools(&config);
        wrong_tools[0].description.push_str(" changed");
        assert!(validate_agent_surface(&config, &messages(&config), &wrong_tools).is_err());

        let guidance = python_guidance(&config).unwrap();
        let trailing = vec![Message::system(format!("{guidance}\ncontradictory tail"))];
        assert!(validate_agent_surface(&config, &trailing, &tools(&config)).is_err());
        let duplicate = vec![Message::system(format!("{guidance}\n\n{guidance}"))];
        assert!(validate_agent_surface(&config, &duplicate, &tools(&config)).is_err());
    }

    #[test]
    fn openai_final_body_rejects_tool_guidance_and_control_tampering() {
        let config = config(ConnectionKind::OpenAiChatCompletions);
        let canonical = body(&config);

        let mut tampered = canonical.clone();
        tampered["tools"][0]["function"]["name"] = json!("run_shell_command");
        assert!(validate_final_agent_body(&config, &tampered).is_err());

        let mut tampered = canonical.clone();
        tampered["parallel_tool_calls"] = json!(true);
        assert!(validate_final_agent_body(&config, &tampered).is_err());

        let mut tampered = canonical.clone();
        tampered["tool_choice"] = json!("none");
        assert!(validate_final_agent_body(&config, &tampered).is_err());

        let mut tampered = canonical.clone();
        tampered["functions"] = json!([{"name": "shell"}]);
        assert!(validate_final_agent_body(&config, &tampered).is_err());

        let mut tampered = canonical.clone();
        tampered["messages"][0]["content"]
            .as_str()
            .map(|content| format!("{content}\ntrailing text"))
            .map(|content| tampered["messages"][0]["content"] = json!(content));
        assert!(validate_final_agent_body(&config, &tampered).is_err());

        let mut tampered = canonical;
        tampered["stream"] = json!(false);
        assert!(validate_final_agent_body(&config, &tampered).is_err());
    }

    #[test]
    fn anthropic_final_body_rejects_tool_guidance_and_choice_tampering() {
        let config = config(ConnectionKind::ClaudeCodeProxy);
        let canonical = body(&config);

        let mut tampered = canonical.clone();
        tampered["tools"][0]["input_schema"]["properties"]["shell"] = json!({});
        assert!(validate_final_agent_body(&config, &tampered).is_err());

        let mut tampered = canonical.clone();
        tampered["tool_choice"]["type"] = json!("none");
        assert!(validate_final_agent_body(&config, &tampered).is_err());

        let mut tampered = canonical.clone();
        tampered["mcp_servers"] = json!([{"name": "other"}]);
        assert!(validate_final_agent_body(&config, &tampered).is_err());

        let mut tampered = canonical;
        tampered["system"] = json!(format!(
            "{}\ntrailing text",
            tampered["system"].as_str().unwrap()
        ));
        assert!(validate_final_agent_body(&config, &tampered).is_err());
    }

    #[tokio::test]
    async fn prepared_request_cannot_be_sent_under_a_different_provider_binding() {
        let config = config(ConnectionKind::OpenAiChatCompletions);
        let prepared = crate::transport::prepare_agent_request(
            &config,
            &messages(&config),
            &tools(&config),
            config.request_output_tokens(),
        )
        .unwrap();
        let mut switched = config.clone();
        switched.model = "different-model".to_string();

        let error = crate::transport::stream_prepared(&reqwest::Client::new(), &switched, prepared)
            .await
            .err()
            .expect("mismatched prepared request must fail before provider I/O");

        assert!(error.contains("does not match"), "{error}");
    }

    #[test]
    fn unresolved_python_policy_fails_before_body_preparation() {
        let mut config = config(ConnectionKind::OpenAiChatCompletions);
        config.python_runtime.target = None;
        let error = crate::transport::prepare_agent_request(
            &config,
            &[Message::system("no valid guidance")],
            &tools(&config),
            config.request_output_tokens(),
        )
        .unwrap_err();
        assert!(
            error.contains("Invalid Python-only request policy"),
            "{error}"
        );
    }
}
