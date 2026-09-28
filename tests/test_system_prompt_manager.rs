use lethetic::config::{
    AccessMode, Config, NetworkAccess, PackageAccess, PythonExecutionTarget, PythonRuntimeConfig,
    SandboxBackend, ToolProfile,
};
use lethetic::system_prompt::SystemPromptManager;

#[test]
fn test_system_prompt_manager_lifecycle() {
    // For now, let's just test `resolve_prompt` to ensure the placeholder is replaced.
    let template = "Hello\n[TOOLS_DEFINITIONS]\nGoodbye";
    let config = Config {
        server_url: "".to_string(),
        model: "".to_string(),
        context_size: 0,
        tool_wrapper: None,
        tool_profile: Default::default(),
        python_runtime: Default::default(),
        python_invocation: Default::default(),
        active_server: None,
        connection_kind: Default::default(),
        api_key: None,
        estimate_cost: None,
        pricing: None,
        input_cost_per_1m: None,
        output_cost_per_1m: None,
        enable_image_processing_tool: false,
        background_tasks: Default::default(),
        theme: None,
        model_servers: Vec::new(),
        thinking: None,
        extra_body: None,
        context_mode: None,
    };
    let resolved = SystemPromptManager::resolve_prompt(template, "/mock/cwd", &config);

    assert!(resolved.contains("Hello"));
    assert!(resolved.contains("Goodbye"));
    assert!(!resolved.contains("[TOOLS_DEFINITIONS]"));

    // Check if some expected tool declarations are present in new JSON format
    assert!(resolved.contains("<|tool>"));
    assert!(resolved.contains("\"name\": \"read_file\""));
    assert!(resolved.contains("\"name\": \"run_shell_command\""));
}

#[test]
fn test_python_only_prompt_has_exact_tool_surface() {
    let config = Config {
        tool_profile: lethetic::config::ToolProfile::PythonOnly,
        python_runtime: lethetic::config::PythonRuntimeConfig {
            target: Some(lethetic::config::PythonExecutionTarget::Host),
            ..Default::default()
        },
        ..Default::default()
    };
    let resolved = SystemPromptManager::resolve_prompt(
        lethetic::system_prompt::DEFAULT_PROMPT_TEMPLATE,
        "/mock/cwd",
        &config,
    );
    let names = lethetic::tools::get_api_tools(&config, lethetic::tools::ToolSurface::Interactive)
        .into_iter()
        .map(|tool| tool.name)
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["python"]);
    assert!(resolved.contains("# Python-only mode"));
    assert!(resolved.contains("\"name\": \"python\""));
    assert!(resolved.contains("import lethetic_todo"));
    assert!(!resolved.contains("\"name\": \"todowrite\""));
    assert!(!resolved.contains("\"name\": \"run_shell_command\""));
    assert!(!resolved.contains("\"name\": \"task\""));
}

fn sandbox_python_config(
    backend: SandboxBackend,
    network: NetworkAccess,
    package_access: PackageAccess,
) -> Config {
    let mut config = Config {
        tool_profile: ToolProfile::PythonOnly,
        python_runtime: PythonRuntimeConfig {
            target: Some(PythonExecutionTarget::Sandbox),
            ..Default::default()
        },
        ..Default::default()
    };
    config.python_runtime.sandbox.backend = Some(backend);
    config.python_runtime.sandbox.network = Some(network);
    config.python_runtime.sandbox.workspace_access = Some(AccessMode::ReadWrite);
    config.python_runtime.sandbox.package_access = package_access;
    config
}

#[test]
fn python_capability_prompt_matrix_advertises_only_exact_available_policy() {
    let unresolved = Config {
        tool_profile: ToolProfile::PythonOnly,
        ..Default::default()
    };
    let host = Config {
        tool_profile: ToolProfile::PythonOnly,
        python_runtime: PythonRuntimeConfig {
            target: Some(PythonExecutionTarget::Host),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut stale_host = host.clone();
    stale_host.python_runtime.sandbox.network = Some(NetworkAccess::Nonlocal);
    stale_host.python_runtime.sandbox.package_access = PackageAccess::Session;

    let cases = [
        ("unresolved", unresolved, "unresolved or invalid", false),
        ("host", host, "Execution target: Host", false),
        (
            "host-stale-nonlocal",
            stale_host,
            "Execution target: Host",
            false,
        ),
        (
            "bubblewrap-none",
            sandbox_python_config(
                SandboxBackend::Bubblewrap,
                NetworkAccess::None,
                PackageAccess::Disabled,
            ),
            "Network policy: None",
            false,
        ),
        (
            "bubblewrap-full",
            sandbox_python_config(
                SandboxBackend::Bubblewrap,
                NetworkAccess::Full,
                PackageAccess::Disabled,
            ),
            "Bubblewrap backend",
            false,
        ),
        (
            "podman-none",
            sandbox_python_config(
                SandboxBackend::Podman,
                NetworkAccess::None,
                PackageAccess::Disabled,
            ),
            "transient container",
            false,
        ),
        (
            "podman-full",
            sandbox_python_config(
                SandboxBackend::Podman,
                NetworkAccess::Full,
                PackageAccess::Disabled,
            ),
            "Network policy: Full",
            false,
        ),
        (
            "retained-nonlocal",
            sandbox_python_config(
                SandboxBackend::Podman,
                NetworkAccess::Nonlocal,
                PackageAccess::Session,
            ),
            "retained Podman sandbox",
            true,
        ),
    ];

    for (name, config, expected, package_helper) in cases {
        let guidance = lethetic::system_prompt::python_capability_guidance(&config)
            .unwrap_or_else(|| panic!("missing Python guidance for {name}"));
        assert!(
            guidance.contains("Exactly one model tool is available: `python`"),
            "{name}"
        );
        assert!(guidance.contains("not a second model tool"), "{name}");
        assert!(guidance.contains(expected), "{name}: {guidance}");
        assert_eq!(
            guidance.contains("lethetic-pkg refresh"),
            package_helper,
            "{name}"
        );
        assert!(!guidance.contains("lethetic-python-"), "{name}");
    }
}
