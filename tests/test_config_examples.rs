use lethetic::config::{Config, ConnectionKind};

#[test]
fn tracked_config_contains_native_proxy_example_without_selecting_it() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config.yml");
    let text = std::fs::read_to_string(path).unwrap();
    let config: Config = serde_yaml::from_str(&text).unwrap();
    config.validate().unwrap();
    let proxy = config
        .model_servers
        .iter()
        .find(|server| server.connection_id() == "local-claude-code-proxy")
        .expect("tracked config should contain the local proxy example");
    assert_eq!(proxy.kind, ConnectionKind::ClaudeCodeProxy);
    assert_eq!(proxy.url, "http://127.0.0.1:18765");
    assert_eq!(proxy.model, "gpt-5.6-sol");
    assert_eq!(proxy.context_size, Some(262_144));
    let limits = proxy
        .context_limits
        .as_ref()
        .expect("the Sol proxy example should include exact context limits");
    assert!(limits.applies_to("gpt-5.6-sol"));
    assert_eq!(limits.total_context_tokens, 1_050_000);
    assert_eq!(limits.maximum_input_tokens, 922_000);
    assert_eq!(limits.maximum_output_tokens, 128_000);
    assert_eq!(limits.request_output_tokens, 24_576);
    assert_eq!(limits.lethetic_input_budget_tokens, 900_000);
    let pricing = proxy
        .pricing
        .as_ref()
        .expect("the Sol proxy example should include API-equivalent pricing");
    assert!(pricing.applies_to("gpt-5.6-sol"));
    assert_eq!(pricing.rates.uncached_input, 4.0);
    assert_eq!(pricing.rates.cached_read_input, 0.4);
    assert_eq!(pricing.rates.cache_creation_input, 5.0);
    assert_eq!(pricing.rates.output, 20.0);
    assert_eq!(pricing.valid_through.as_deref(), Some("2026-11-21"));
    assert_ne!(
        config.active_connection_id(),
        Some("local-claude-code-proxy"),
        "the repository default connection must remain unchanged"
    );
}
