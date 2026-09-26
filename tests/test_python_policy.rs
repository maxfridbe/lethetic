use lethetic::config::{
    AccessMode, Config, NetworkAccess, PythonExecutionTarget, PythonRuntimeConfig, SandboxBackend,
    ToolProfile,
};
use lethetic::python_policy::{
    PYTHON_POLICY_VERSION, PolicyRevision, PythonPolicyScope, PythonPolicySnapshot, load_policy,
    persist_policy, project_policy_path,
};

#[test]
fn python_policy_round_trip_is_complete_and_isolated() {
    let workspace = tempfile::tempdir().unwrap();
    let mut runtime = PythonRuntimeConfig {
        target: Some(PythonExecutionTarget::Sandbox),
        ..Default::default()
    };
    runtime.sandbox.backend = Some(SandboxBackend::Bubblewrap);
    runtime.sandbox.network = Some(NetworkAccess::None);
    runtime.sandbox.workspace_access = Some(AccessMode::ReadOnly);
    let snapshot = PythonPolicySnapshot {
        version: PYTHON_POLICY_VERSION,
        tool_profile: ToolProfile::PythonOnly,
        python_runtime: runtime,
    };
    let primary = workspace.path().join("config.yml");
    std::fs::write(&primary, "api_key: secret # unchanged\n").unwrap();

    persist_policy(
        PythonPolicyScope::Project,
        workspace.path(),
        &snapshot,
        Some(&PolicyRevision::Missing),
    )
    .unwrap();

    let loaded = load_policy(&project_policy_path(workspace.path()))
        .unwrap()
        .unwrap();
    assert_eq!(loaded.snapshot, snapshot);
    assert_eq!(
        std::fs::read_to_string(primary).unwrap(),
        "api_key: secret # unchanged\n"
    );
}

#[test]
fn legacy_config_defaults_to_general_mode() {
    let config: Config = serde_yaml::from_str(
        "server_url: http://localhost\nmodel: test\ncontext_size: 1\ntool_wrapper: null\n",
    )
    .unwrap();
    assert_eq!(config.tool_profile, ToolProfile::General);
    assert!(config.python_runtime.target.is_none());
}
