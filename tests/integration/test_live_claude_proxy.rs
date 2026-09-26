use lethetic::config::{Config, PythonExecutionTarget, PythonRuntimeConfig, ToolProfile};
use lethetic::transport::{self, Message};
use serial_test::serial;
use std::time::Duration;

fn proxy_config() -> Result<Config, String> {
    let mut config = Config::load("config.yml")?;
    let proxy = config
        .model_servers
        .iter()
        .find(|server| server.connection_id() == "local-claude-code-proxy")
        .ok_or_else(|| "local-claude-code-proxy is missing from config.yml".to_string())?
        .clone();
    config.activate_model(proxy.connection_id(), &proxy.model)?;
    Ok(config)
}

#[tokio::test]
#[serial(llm)]
#[ignore = "requires the separately running, authorized claude-code-proxy on 127.0.0.1:18765"]
async fn test_claude_proxy_text_smoke() {
    let config = proxy_config().unwrap();
    let client = reqwest::Client::new();
    let response = tokio::time::timeout(
        Duration::from_secs(180),
        transport::complete(
            &client,
            &config,
            &[Message::user("Reply with exactly: PROXY_TEXT_OK")],
            256,
        ),
    )
    .await
    .expect("proxy text request timed out")
    .expect("proxy text request failed");
    assert!(
        response.contains("PROXY_TEXT_OK"),
        "unexpected proxy response: {response}"
    );
}

#[tokio::test]
#[serial(llm)]
#[ignore = "requires claude-code-proxy plus python3; performs a harmless persistent Python tool roundtrip"]
async fn test_claude_proxy_python_state_roundtrip() {
    let mut config = proxy_config().unwrap();
    config.tool_profile = ToolProfile::PythonOnly;
    config.python_runtime = PythonRuntimeConfig {
        target: Some(PythonExecutionTarget::Host),
        ..Default::default()
    };
    let client = reqwest::Client::new();
    let prompt = "Use the python tool in two sequential turns. In the first cell set `proxy_state_probe = 19`. After that tool result, call python again with `proxy_state_probe * 2`. After the second result, reply with exactly `PYTHON_STATE=38`. Call at most one tool per turn.";
    let response = tokio::time::timeout(
        Duration::from_secs(240),
        lethetic::headless::run_agent(prompt.to_string(), &client, &config, false, None),
    )
    .await
    .expect("proxy Python roundtrip timed out")
    .expect("proxy Python roundtrip failed");
    assert!(
        response.contains("PYTHON_STATE=38"),
        "unexpected proxy Python response: {response}"
    );
}
