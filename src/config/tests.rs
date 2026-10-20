use super::*;
use serde_json::json;

fn server(id: &str, name: &str, url: &str, model: &str) -> ModelServer {
    ModelServer {
        id: Some(id.to_string()),
        name: name.to_string(),
        kind: ConnectionKind::OpenAiChatCompletions,
        url: url.to_string(),
        model: model.to_string(),
        parser: "qwen3".to_string(),
        api_key: None,
        context_size: None,
        context_limits: None,
        input_cost_per_1m: None,
        output_cost_per_1m: None,
        pricing: None,
        thinking: None,
        extra_body: None,
        context_mode: None,
        theme: None,
        discover_models: true,
        provider_retries: None,
        model_pricing: Vec::new(),
        models: Vec::new(),
    }
}

fn sol_context_limits() -> ModelContextLimits {
    ModelContextLimits {
        applies_to_models: vec!["gpt-5.6-sol".to_string()],
        total_context_tokens: 1_050_000,
        maximum_input_tokens: 922_000,
        maximum_output_tokens: 128_000,
        request_output_tokens: 24_576,
        lethetic_input_budget_tokens: 900_000,
    }
}

#[test]
fn test_merge_matching_server_settings() {
    let mut model_server = server(
        "test-server",
        "Test Server",
        "http://example.com/api",
        "my-model",
    );
    model_server.api_key = Some("secret-key".to_string());
    model_server.context_size = Some(8192);
    model_server.input_cost_per_1m = Some(1.5);
    model_server.output_cost_per_1m = Some(2.5);
    model_server.thinking = Some(true);
    model_server.extra_body = Some(json!({"reasoning_effort": "high"}));
    model_server.theme = Some("Monokai".to_string());

    let mut config = Config {
        server_url: "http://example.com/api".to_string(),
        model: "my-model".to_string(),
        context_size: 0,
        model_servers: vec![model_server],
        ..Default::default()
    };

    config.merge_matching_server_settings();

    assert_eq!(config.active_server.as_deref(), Some("test-server"));
    assert_eq!(config.api_key.as_deref(), Some("secret-key"));
    assert_eq!(config.input_cost_per_1m, Some(1.5));
    assert_eq!(config.output_cost_per_1m, Some(2.5));
    assert_eq!(config.thinking, Some(true));
    assert_eq!(config.context_size, 8192);
    assert_eq!(
        config.extra_body.as_ref().unwrap()["reasoning_effort"],
        "high"
    );
    assert_eq!(config.theme.as_deref(), Some("Monokai"));
}

#[test]
fn test_legacy_config_defaults_to_openai() {
    let config: Config = serde_yaml::from_str(
        "server_url: http://x\nmodel: m\ncontext_size: 1\ntool_wrapper: null\n",
    )
    .unwrap();
    assert_eq!(
        config.active_connection_kind(),
        ConnectionKind::OpenAiChatCompletions
    );
    assert!(config.active_server.is_none());
}

#[test]
fn test_explicit_proxy_kind_deserializes() {
    let model_server: ModelServer = serde_yaml::from_str(
            "id: proxy\nname: Proxy\nkind: claude_code_proxy\nurl: http://127.0.0.1:18765\nmodel: gpt-5.6-sol\n",
        )
        .unwrap();
    assert_eq!(model_server.kind, ConnectionKind::ClaudeCodeProxy);
    assert_eq!(model_server.connection_id(), "proxy");
}

#[test]
fn extra_body_rejects_reserved_fields_for_active_and_server_configs() {
    for field in gemma_chat::RESERVED_EXTRA_BODY_FIELDS {
        let mut config = Config {
            extra_body: Some(json!({(*field): "override"})),
            ..Default::default()
        };
        let error = config.validate().unwrap_err();
        assert!(error.contains("active config"), "{field}: {error}");
        assert!(error.contains(field), "{field}: {error}");

        config.extra_body = None;
        let mut model_server = server("guarded", "Guarded", "http://same", "model");
        model_server.extra_body = Some(json!({(*field): "override"}));
        config.model_servers = vec![model_server];
        let error = config.validate().unwrap_err();
        assert!(error.contains("model server 'guarded'"), "{field}: {error}");
        assert!(error.contains(field), "{field}: {error}");
    }
}

#[test]
fn extra_body_rejects_non_objects_and_allows_output_config() {
    let mut model_server = server("allowed", "Allowed", "http://same", "model");
    model_server.extra_body = Some(json!({"output_config": {"effort": "max"}}));
    let config = Config {
        extra_body: Some(json!({"output_config": {"format": "text"}})),
        model_servers: vec![model_server],
        ..Default::default()
    };
    config.validate().unwrap();

    let invalid = Config {
        extra_body: Some(json!("not-an-object")),
        ..Default::default()
    };
    let error = invalid.validate().unwrap_err();
    assert!(error.contains("active config"));
    assert!(error.contains("JSON object"));
}

#[test]
fn test_activate_model_uses_stable_id_for_duplicate_urls() {
    let mut proxy = server("proxy", "Proxy", "http://same", "gpt-5.6-sol");
    proxy.kind = ConnectionKind::ClaudeCodeProxy;
    proxy.context_size = Some(372_000);
    proxy.thinking = Some(true);
    let openai = server("openai", "OpenAI", "http://same", "other");
    let mut config = Config {
        server_url: "http://same".to_string(),
        model: "other".to_string(),
        context_size: 1,
        model_servers: vec![openai, proxy],
        ..Default::default()
    };

    config.activate_model("proxy", "gpt-5.6-sol-fast").unwrap();

    assert_eq!(config.active_server.as_deref(), Some("proxy"));
    assert_eq!(config.connection_kind, ConnectionKind::ClaudeCodeProxy);
    assert_eq!(config.model, "gpt-5.6-sol-fast");
    assert_eq!(config.context_size, 372_000);
    assert_eq!(config.thinking, Some(true));
}

#[test]
fn test_structured_context_limits_apply_only_to_exact_models() {
    let mut proxy = server("proxy", "Proxy", "http://same", "gpt-5.6-sol");
    proxy.kind = ConnectionKind::ClaudeCodeProxy;
    proxy.context_size = Some(123_456);
    proxy.context_limits = Some(sol_context_limits());
    let mut config = Config {
        model_servers: vec![proxy],
        ..Default::default()
    };

    config.activate_model("proxy", "gpt-5.6-sol").unwrap();
    assert_eq!(config.input_token_budget(), 900_000);
    assert_eq!(config.context_size, 900_000);
    assert_eq!(config.request_output_tokens(), 24_576);
    assert_eq!(config.maximum_output_tokens(), 128_000);
    assert!(config.active_context_limits().is_some());

    config
        .activate_model("proxy", "discovered-unknown")
        .unwrap();
    assert_eq!(config.input_token_budget(), 123_456);
    assert_eq!(config.context_size, 123_456);
    assert_eq!(
        config.request_output_tokens(),
        REQUEST_OUTPUT_TOKENS_LEGACY_DEFAULT
    );
    assert_eq!(config.maximum_output_tokens(), u32::MAX);
    assert!(config.active_context_limits().is_none());
}

#[test]
fn test_structured_context_limits_validate_relationships() {
    let mut limits = sol_context_limits();
    limits.validate().unwrap();

    limits.lethetic_input_budget_tokens = limits.maximum_input_tokens + 1;
    assert!(
        limits
            .validate()
            .unwrap_err()
            .contains("cannot exceed maximum_input_tokens")
    );

    let mut limits = sol_context_limits();
    limits.request_output_tokens = limits.maximum_output_tokens + 1;
    assert!(
        limits
            .validate()
            .unwrap_err()
            .contains("cannot exceed maximum_output_tokens")
    );

    let mut limits = sol_context_limits();
    limits.applies_to_models.push("gpt-5.6-sol".to_string());
    assert!(limits.validate().unwrap_err().contains("duplicate model"));
}

#[test]
fn test_activation_clears_previous_connection_secrets_and_context() {
    let mut proxy = server("proxy", "Proxy", "http://proxy", "proxy-model");
    proxy.kind = ConnectionKind::ClaudeCodeProxy;
    proxy.parser = "default".to_string();
    proxy.api_key = Some("proxy-secret".to_string());
    proxy.context_size = Some(372_000);
    proxy.thinking = Some(true);
    proxy.extra_body = Some(json!({"private": "proxy-only"}));
    let openai = server("openai", "OpenAI", "http://openai", "openai-model");
    let mut config = Config {
        model_servers: vec![proxy, openai],
        ..Default::default()
    };

    config.activate_model("proxy", "proxy-model").unwrap();
    config
        .activate_model("openai", "other-openai-model")
        .unwrap();

    assert_eq!(config.active_server.as_deref(), Some("openai"));
    assert_eq!(
        config.connection_kind,
        ConnectionKind::OpenAiChatCompletions
    );
    assert_eq!(config.server_url, "http://openai");
    assert_eq!(config.model, "other-openai-model");
    assert_eq!(config.active_parser(), "qwen3");
    assert_eq!(config.api_key, None);
    assert_eq!(config.context_size, CONTEXT_SIZE_LEGACY_DEFAULT);
    assert_eq!(config.thinking, None);
    assert_eq!(config.extra_body, None);
}

#[test]
fn test_ambiguous_legacy_url_requires_active_server() {
    let first = server("first", "First", "http://same", "one");
    let second = server("second", "Second", "http://same", "two");
    let mut config = Config {
        server_url: "http://same".to_string(),
        model: "unlisted-model".to_string(),
        model_servers: vec![first, second],
        ..Default::default()
    };

    let error = config.validate().unwrap_err();
    assert!(error.contains("legacy server_url selection is ambiguous"));
    assert!(error.contains("set active_server explicitly"));

    config.active_server = Some("second".to_string());
    config.validate().unwrap();
    assert_eq!(config.active_connection_id(), Some("second"));
}

#[test]
fn test_load_overlays_config_local_by_id_and_legacy_name() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("config.yml");
    std::fs::write(
        &base,
        "\
server_url: http://base/v1
model: base-model
context_size: 1000
tool_wrapper: null
model_servers:
  - id: azure
    name: Azure
    model: deepseek
  - name: Local
    url: http://local/v1
    model: gemma
",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("config.local.yml"),
        "\
api_key: top-level-key
model_servers:
  - id: azure
    name: Renamed Azure
    url: http://azure/v1
    api_key: azure-secret
  - name: Local
    api_key: local-secret
  - id: extra
    name: Extra
    url: http://extra/v1
    model: extra-model
",
    )
    .unwrap();

    let config = Config::load(&base).unwrap();
    assert_eq!(config.api_key.as_deref(), Some("top-level-key"));
    assert_eq!(config.model, "base-model");
    assert_eq!(config.model_servers.len(), 3);
    let azure = config
        .model_servers
        .iter()
        .find(|server| server.connection_id() == "azure")
        .unwrap();
    assert_eq!(azure.name, "Renamed Azure");
    assert_eq!(azure.api_key.as_deref(), Some("azure-secret"));
    assert_eq!(azure.url, "http://azure/v1");
    assert_eq!(azure.model, "deepseek");
    let local = config
        .model_servers
        .iter()
        .find(|server| server.name == "Local")
        .unwrap();
    assert_eq!(local.api_key.as_deref(), Some("local-secret"));
}

#[test]
fn legacy_startup_preserves_local_root_model_key_and_model_scoped_limits() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("config.yml");
    std::fs::write(
        &base,
        "\
server_url: http://shared/v1
model: server-default
context_size: 1000
model_servers:
  - id: shared
    name: Shared
    url: http://shared/v1
    model: server-default
    api_key: server-secret
    context_size: 2048
    context_limits:
      applies_to_models: [selected-model]
      total_context_tokens: 12000
      maximum_input_tokens: 10000
      maximum_output_tokens: 2000
      request_output_tokens: 1000
      lethetic_input_budget_tokens: 9000
    pricing:
      applies_to_models: [selected-model]
      effective_as_of: 2026-09-01
      provenance:
        kind: test
        url: https://example.invalid/pricing
      rates:
        uncached_input: 1.0
        cached_read_input: 0.1
        cache_creation_input: 1.25
        output: 5.0
  - id: other
    name: Other
    url: http://other/v1
    model: other-model
",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("config.local.yml"),
        "model: selected-model\napi_key: root-local-secret\n",
    )
    .unwrap();

    let mut config = Config::load(&base).unwrap();
    config.merge_matching_server_settings();

    assert_eq!(config.active_server.as_deref(), Some("shared"));
    assert_eq!(config.model, "selected-model");
    assert_eq!(config.api_key.as_deref(), Some("root-local-secret"));
    assert_eq!(config.context_size, 9000);
    assert_eq!(config.input_token_budget(), 9000);
    assert_eq!(config.request_output_tokens(), 1000);
    assert!(config.pricing.is_some());

    config.activate_model("other", "other-model").unwrap();
    assert_eq!(config.api_key, None);
    config.activate_model("shared", "selected-model").unwrap();
    assert_eq!(config.api_key.as_deref(), Some("root-local-secret"));
}

#[test]
fn legacy_startup_inherits_local_server_key_only_when_root_key_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("config.yml");
    std::fs::write(
            &base,
            "server_url: http://shared/v1\nmodel: selected-model\ncontext_size: 1000\nmodel_servers:\n  - id: shared\n    name: Shared\n    url: http://shared/v1\n    model: server-default\n",
        )
        .unwrap();
    std::fs::write(
        dir.path().join("config.local.yml"),
        "model_servers:\n  - id: shared\n    api_key: server-local-secret\n",
    )
    .unwrap();

    let mut config = Config::load(&base).unwrap();
    config.merge_matching_server_settings();

    assert_eq!(config.model, "selected-model");
    assert_eq!(config.api_key.as_deref(), Some("server-local-secret"));

    config.activate_model("shared", "discovered-model").unwrap();
    assert_eq!(config.model, "discovered-model");
    assert_eq!(config.api_key.as_deref(), Some("server-local-secret"));
    config.activate_current_model("another-model").unwrap();
    assert_eq!(config.api_key.as_deref(), Some("server-local-secret"));
}

#[test]
fn explicit_startup_activation_uses_server_model_and_key() {
    let mut selected = server("selected", "Selected", "http://selected", "server-model");
    selected.api_key = Some("server-key".to_string());
    let mut config = Config {
        active_server: Some("selected".to_string()),
        server_url: "http://legacy".to_string(),
        model: "legacy-model".to_string(),
        api_key: Some("legacy-key".to_string()),
        model_servers: vec![selected],
        ..Default::default()
    };

    config.merge_matching_server_settings();

    assert_eq!(config.server_url, "http://selected");
    assert_eq!(config.model, "server-model");
    assert_eq!(config.api_key.as_deref(), Some("server-key"));
}

#[test]
fn config_local_overlay_cannot_inject_reserved_request_fields() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("config.yml");
    let local = dir.path().join("config.local.yml");
    std::fs::write(
            &base,
            "server_url: http://x\nmodel: m\ncontext_size: 1000\nmodel_servers:\n  - id: guarded\n    name: Guarded\n    url: http://guarded\n    model: guarded-model\n",
        )
        .unwrap();

    std::fs::write(&local, "extra_body:\n  tools: []\n").unwrap();
    let error = Config::load(&base).unwrap_err();
    assert!(error.contains("active config"), "{error}");
    assert!(error.contains("tools"), "{error}");

    std::fs::write(
        &local,
        "model_servers:\n  - id: guarded\n    extra_body:\n      functions: []\n",
    )
    .unwrap();
    let error = Config::load(&base).unwrap_err();
    assert!(error.contains("model server 'guarded'"), "{error}");
    assert!(error.contains("functions"), "{error}");
}

#[test]
fn test_load_rejects_duplicate_effective_ids() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("config.yml");
    std::fs::write(
        &base,
        "\
server_url: http://x
model: m
context_size: 1
tool_wrapper: null
model_servers:
  - id: duplicate
    name: One
    model: one
  - id: duplicate
    name: Two
    model: two
",
    )
    .unwrap();

    let error = Config::load(&base).unwrap_err();
    assert!(error.contains("duplicate model server id 'duplicate'"));
}

#[test]
fn test_load_without_local_file() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("config.yml");
    std::fs::write(
        &base,
        "server_url: http://x\nmodel: m\ncontext_size: 1\ntool_wrapper: null\n",
    )
    .unwrap();
    let config = Config::load(&base).unwrap();
    assert_eq!(config.model, "m");
}

#[test]
fn test_legacy_config_defaults_to_general_with_unresolved_python() {
    let config: Config = serde_yaml::from_str(
        "server_url: http://x\nmodel: m\ncontext_size: 1\ntool_wrapper: null\n",
    )
    .unwrap();
    assert_eq!(config.tool_profile, ToolProfile::General);
    assert_eq!(config.python_runtime.target, None);
    assert_eq!(config.python_runtime.python_executable, "python3");
    assert_eq!(
        config.python_runtime.sandbox.podman_image,
        "docker.io/library/python:3.13-slim"
    );
    assert_eq!(
        config.python_runtime.sandbox.package_access,
        PackageAccess::Disabled
    );
}

#[test]
fn test_explicit_python_sandbox_deserializes_and_validates() {
    let config: Config = serde_yaml::from_str(
        "\
server_url: http://x
model: m
context_size: 1
tool_wrapper: null
tool_profile: python_only
python_runtime:
  target: sandbox
  python_executable: /usr/bin/python3
  sandbox:
    backend: bubblewrap
    network: none
    workspace_access: read_only
    grants:
      - path: /data
        access: read_write
",
    )
    .unwrap();
    assert_eq!(config.tool_profile, ToolProfile::PythonOnly);
    assert_eq!(
        config.python_runtime.target,
        Some(PythonExecutionTarget::Sandbox)
    );
    assert!(config.python_sandbox_is_complete());
    assert!(config.python_mode_validation_error().is_none());
    assert_eq!(config.python_runtime.sandbox.grants.len(), 1);
}

#[test]
fn test_incomplete_python_sandbox_reports_specific_error() {
    let mut config = Config {
        tool_profile: ToolProfile::PythonOnly,
        python_runtime: PythonRuntimeConfig {
            target: Some(PythonExecutionTarget::Sandbox),
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(
        config.python_mode_validation_error().as_deref(),
        Some("Python sandbox backend is not configured")
    );
    config.python_runtime.sandbox.backend = Some(SandboxBackend::Podman);
    assert_eq!(
        config.python_mode_validation_error().as_deref(),
        Some("Python sandbox network access is not configured")
    );
}

#[test]
fn test_nonlocal_runtime_requires_exact_fail_closed_policy() {
    let mut runtime = PythonRuntimeConfig {
        target: Some(PythonExecutionTarget::Sandbox),
        ..Default::default()
    };
    runtime.sandbox.backend = Some(SandboxBackend::Podman);
    runtime.sandbox.network = Some(NetworkAccess::Nonlocal);
    runtime.sandbox.workspace_access = Some(AccessMode::ReadWrite);
    runtime.sandbox.package_access = PackageAccess::Session;
    assert!(runtime.sandbox_is_complete());
    assert_eq!(runtime.validation_error(), None);
    assert!(is_exact_retained_nonlocal_python_policy(
        ToolProfile::PythonOnly,
        &runtime
    ));
    assert!(!is_exact_retained_nonlocal_python_policy(
        ToolProfile::General,
        &runtime
    ));
    let mut stale_host = runtime.clone();
    stale_host.target = Some(PythonExecutionTarget::Host);
    assert_eq!(stale_host.validation_error(), None);
    assert!(!is_exact_retained_nonlocal_python_policy(
        ToolProfile::PythonOnly,
        &stale_host
    ));

    runtime.sandbox.grants.push(PathGrant {
        path: PathBuf::from("/tmp/extra"),
        access: AccessMode::ReadOnly,
    });
    assert_eq!(
        runtime.validation_error().as_deref(),
        Some("Nonlocal package mode does not allow additional path grants")
    );
    assert!(!is_exact_retained_nonlocal_python_policy(
        ToolProfile::PythonOnly,
        &runtime
    ));
    runtime.sandbox.grants.clear();
    runtime.sandbox.package_access = PackageAccess::Disabled;
    assert_eq!(
        runtime.validation_error().as_deref(),
        Some("Nonlocal network access requires session package access")
    );
    runtime.sandbox.package_access = PackageAccess::Session;
    runtime.sandbox.backend = Some(SandboxBackend::Bubblewrap);
    assert_eq!(
        runtime.validation_error().as_deref(),
        Some("Nonlocal network access is supported only by the Podman backend")
    );
}

#[test]
fn test_project_config_cannot_enable_nonlocal_package_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yml");
    std::fs::write(
            &path,
            "server_url: http://x\nmodel: m\ncontext_size: 1\ntool_wrapper: null\n\
             tool_profile: python_only\npython_runtime:\n  target: sandbox\n  sandbox:\n    backend: podman\n    network: nonlocal\n    workspace_access: read_write\n    package_access: session\n    podman_image: local/runtime@sha256:abc\n",
        )
        .unwrap();

    let error = Config::load(&path).unwrap_err();
    assert!(
        error.contains("retained Nonlocal package runtime"),
        "{error}"
    );

    let general = std::fs::read_to_string(&path)
        .unwrap()
        .replace("tool_profile: python_only", "tool_profile: general");
    std::fs::write(&path, general).unwrap();
    let error = Config::load(&path).unwrap_err();
    assert!(
        error.contains("retained Nonlocal package runtime"),
        "{error}"
    );
}

#[test]
fn test_structured_pricing_activates_only_for_exact_model() {
    let mut config: Config = serde_yaml::from_str(
            "server_url: http://proxy\nmodel: gpt-5.6-sol\ncontext_size: 372000\ntool_wrapper: null\n\
             model_servers:\n  - id: proxy\n    name: Proxy\n    kind: claude_code_proxy\n    url: http://proxy\n    model: gpt-5.6-sol\n    pricing:\n      applies_to_models: [gpt-5.6-sol]\n      currency: USD\n      unit_tokens: 1000000\n      effective_as_of: 2026-08-25\n      valid_through: 2026-11-21\n      provenance:\n        kind: api_equivalent_estimate\n        url: https://developers.openai.com/api/docs/models/gpt-5.6-sol\n        note: Subscription billing may differ.\n      rates:\n        uncached_input: 4.0\n        cached_read_input: 0.4\n        cache_creation_input: 5.0\n        output: 20.0\n      long_context:\n        threshold_input_tokens: 272000\n        applies_above_threshold: true\n        input_multiplier: 2.0\n        output_multiplier: 1.5\n",
        )
        .unwrap();
    config.validate().unwrap();

    config.activate_model("proxy", "gpt-5.6-sol").unwrap();
    assert!(config.pricing.is_some());
    config.activate_model("proxy", "gpt-5.6-sol-fast").unwrap();
    assert!(config.pricing.is_none());
}

#[test]
fn test_invalid_legacy_and_structured_rates_are_rejected() {
    let mut config = Config {
        input_cost_per_1m: Some(f64::INFINITY),
        ..Default::default()
    };
    assert!(
        config
            .validate()
            .unwrap_err()
            .contains("finite and nonnegative")
    );

    config.input_cost_per_1m = None;
    let mut model_server = server("priced", "Priced", "http://x", "m");
    model_server.pricing = Some(
            serde_yaml::from_str(
                "applies_to_models: [m]\ncurrency: USD\nunit_tokens: 1000000\neffective_as_of: 2026-08-25\nprovenance:\n  kind: estimate\n  url: https://example.com/pricing\nrates:\n  uncached_input: -1.0\n  cached_read_input: 0.0\n  cache_creation_input: 0.0\n  output: 1.0\n",
            )
            .unwrap(),
        );
    config.model_servers = vec![model_server];
    let error = config.validate().unwrap_err();
    assert!(error.contains("pricing.rates.uncached_input"), "{error}");
}

#[test]
fn test_activate_model_preserves_python_policy() {
    let mut config = Config {
        tool_profile: ToolProfile::PythonOnly,
        python_runtime: PythonRuntimeConfig {
            target: Some(PythonExecutionTarget::Host),
            ..Default::default()
        },
        model_servers: vec![server("next", "Next", "http://next", "model")],
        ..Default::default()
    };
    let expected_runtime = config.python_runtime.clone();
    let expected_fingerprint = config.python_policy_fingerprint();

    config.activate_model("next", "new-model").unwrap();

    assert_eq!(config.tool_profile, ToolProfile::PythonOnly);
    assert_eq!(config.python_runtime, expected_runtime);
    assert_eq!(config.python_policy_fingerprint(), expected_fingerprint);
}

#[test]
fn test_explicit_overlay_id_wins_before_legacy_name_fallback() {
    let mut base: serde_yaml::Value = serde_yaml::from_str(
            "model_servers:\n  - name: Shared\n    url: http://legacy\n    model: old\n  - id: exact\n    name: Shared\n    url: http://exact\n    model: new\n",
        )
        .unwrap();
    let overlay: serde_yaml::Value = serde_yaml::from_str(
        "model_servers:\n  - id: exact\n    name: Shared\n    api_key: exact-secret\n",
    )
    .unwrap();

    merge_yaml(&mut base, overlay).unwrap();

    assert!(base["model_servers"][0].get("api_key").is_none());
    assert_eq!(
        base["model_servers"][1]["api_key"].as_str(),
        Some("exact-secret")
    );
}

#[test]
fn test_ambiguous_legacy_overlay_name_is_rejected() {
    let mut base: serde_yaml::Value = serde_yaml::from_str(
        "model_servers:\n  - id: one\n    name: Shared\n  - id: two\n    name: Shared\n",
    )
    .unwrap();
    let overlay: serde_yaml::Value =
        serde_yaml::from_str("model_servers:\n  - name: Shared\n    api_key: secret\n").unwrap();

    let error = merge_yaml(&mut base, overlay).unwrap_err();
    assert!(error.contains("ambiguous"));
    assert!(error.contains("explicit id"));
}

#[test]
fn test_non_model_sequences_replace_in_overlay() {
    let mut base: serde_yaml::Value =
        serde_yaml::from_str("extra_body:\n  stop: [one, two]\nmodel_servers: []\n").unwrap();
    let overlay: serde_yaml::Value =
        serde_yaml::from_str("extra_body:\n  stop: []\nmodel_servers: []\n").unwrap();
    merge_yaml(&mut base, overlay).unwrap();
    assert_eq!(
        base["extra_body"]["stop"],
        serde_yaml::Value::Sequence(Vec::new())
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_project_config_host_runtime_fails_before_headless_readiness() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("executed");
    let payload = dir.path().join("payload");
    std::fs::write(
        &payload,
        format!("#!/bin/sh\n: > '{}'\nexit 1\n", marker.display()),
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&payload).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&payload, permissions).unwrap();
    let config_path = dir.path().join("config.yml");
    std::fs::write(
            &config_path,
            format!(
                "server_url: http://x\nmodel: m\ncontext_size: 1\ntool_wrapper: null\n\
                 tool_profile: python_only\npython_runtime:\n  target: host\n  python_executable: '{}'\n",
                payload.display()
            ),
        )
        .unwrap();

    let load_error = match Config::load(&config_path) {
        Err(error) => error,
        Ok(config) => {
            // Mirrors headless readiness. The vulnerable loader reaches
            // this branch and executes the project-selected program.
            let runtime = crate::tool_runtime::ToolRuntime::headless(dir.path());
            let _ = runtime.ensure_ready(&config, dir.path()).await;
            "project Host config unexpectedly loaded".to_string()
        }
    };

    assert!(load_error.contains("project"), "{load_error}");
    assert!(load_error.contains("Host"), "{load_error}");
    assert!(!marker.exists(), "project executable was launched");
}

#[test]
fn test_project_sandbox_config_cannot_override_python_executable() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.yml");
    std::fs::write(
            &config_path,
            "server_url: http://x\nmodel: m\ncontext_size: 1\ntool_wrapper: null\n\
             tool_profile: python_only\npython_runtime:\n  target: sandbox\n  python_executable: /project/payload\n  sandbox:\n    backend: bubblewrap\n    network: none\n    workspace_access: read_only\n",
        )
        .unwrap();

    let config = Config::load(&config_path).unwrap();

    assert_eq!(config.python_runtime.python_executable, "python3");
    assert_eq!(
        config.python_runtime.target,
        Some(PythonExecutionTarget::Sandbox)
    );
}

#[test]
fn test_general_project_config_allows_stale_host_draft_but_sanitizes_executable() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.yml");
    std::fs::write(
            &config_path,
            "server_url: http://x\nmodel: m\ncontext_size: 1\ntool_wrapper: null\n\
             tool_profile: general\npython_runtime:\n  target: host\n  python_executable: /project/payload\n",
        )
        .unwrap();

    let config = Config::load(&config_path).unwrap();

    assert_eq!(config.tool_profile, ToolProfile::General);
    assert_eq!(
        config.python_runtime.target,
        Some(PythonExecutionTarget::Host)
    );
    assert_eq!(config.python_runtime.python_executable, "python3");
}
