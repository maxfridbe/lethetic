#![cfg(target_os = "linux")]

use lethetic::config::{
    AccessMode, Config, NetworkAccess, PythonExecutionTarget, PythonRuntimeConfig,
    PythonWorkspaceExposure, SandboxBackend, ToolProfile,
};
use lethetic::tool_runtime::ToolRuntime;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

fn sandbox_config(
    backend: SandboxBackend,
    network: NetworkAccess,
    workspace_access: AccessMode,
) -> Config {
    let mut runtime = PythonRuntimeConfig {
        target: Some(PythonExecutionTarget::Sandbox),
        ..Default::default()
    };
    runtime.sandbox.backend = Some(backend);
    runtime.sandbox.network = Some(network);
    runtime.sandbox.workspace_access = Some(workspace_access);
    Config {
        tool_profile: ToolProfile::PythonOnly,
        python_runtime: runtime,
        ..Default::default()
    }
}

fn shared_podman_config(network: NetworkAccess) -> Config {
    let mut config = sandbox_config(SandboxBackend::Podman, network, AccessMode::ReadWrite);
    config.python_runtime.sandbox.podman_image =
        lethetic::config::DEFAULT_RETAINED_PODMAN_IMAGE.to_string();
    config.python_invocation.workspace_exposure = PythonWorkspaceExposure::SharedLaunchCwd;
    config
}

async fn execute_with_config(
    config: &Config,
    workspace: &TempDir,
    code: &str,
) -> (lethetic::tool_runtime::RuntimeExecution, ToolRuntime) {
    let runtime = ToolRuntime::interactive(workspace.path());
    let result = runtime
        .execute_python(
            config,
            workspace.path().to_str().unwrap(),
            code,
            CancellationToken::new(),
            None,
        )
        .await;
    (result, runtime)
}

async fn execute(
    backend: SandboxBackend,
    network: NetworkAccess,
    workspace_access: AccessMode,
    workspace: &TempDir,
    code: &str,
) -> lethetic::tool_runtime::RuntimeExecution {
    let config = sandbox_config(backend, network, workspace_access);
    let runtime = ToolRuntime::interactive(workspace.path());
    runtime
        .execute_python(
            &config,
            workspace.path().to_str().unwrap(),
            code,
            CancellationToken::new(),
            None,
        )
        .await
}

#[tokio::test]
#[ignore = "requires a working Bubblewrap runtime; never installs or falls back"]
async fn bubblewrap_enforces_workspace_read_only() {
    let workspace = TempDir::new().unwrap();
    let result = execute(
        SandboxBackend::Bubblewrap,
        NetworkAccess::None,
        AccessMode::ReadOnly,
        &workspace,
        "open('blocked.txt', 'w').write('no')",
    )
    .await;
    assert!(result.is_error, "{}", result.output);
    assert!(!workspace.path().join("blocked.txt").exists());
}

#[tokio::test]
#[ignore = "requires a working Bubblewrap runtime; never installs or falls back"]
async fn bubblewrap_allows_workspace_read_write() {
    let workspace = TempDir::new().unwrap();
    let result = execute(
        SandboxBackend::Bubblewrap,
        NetworkAccess::None,
        AccessMode::ReadWrite,
        &workspace,
        "open('allowed.txt', 'w').write('yes')",
    )
    .await;
    assert!(!result.is_error, "{}", result.output);
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("allowed.txt")).unwrap(),
        "yes"
    );
}

#[tokio::test]
#[ignore = "requires rootless Podman and the configured local image; never pulls"]
async fn podman_enforces_workspace_read_only() {
    let workspace = TempDir::new().unwrap();
    let result = execute(
        SandboxBackend::Podman,
        NetworkAccess::None,
        AccessMode::ReadOnly,
        &workspace,
        "open('blocked.txt', 'w').write('no')",
    )
    .await;
    assert!(result.is_error, "{}", result.output);
    assert!(!workspace.path().join("blocked.txt").exists());
}

#[tokio::test]
#[ignore = "requires rootless Podman and the configured local image; never pulls"]
async fn podman_allows_workspace_read_write() {
    let workspace = TempDir::new().unwrap();
    let result = execute(
        SandboxBackend::Podman,
        NetworkAccess::None,
        AccessMode::ReadWrite,
        &workspace,
        "open('allowed.txt', 'w').write('yes')",
    )
    .await;
    assert!(!result.is_error, "{}", result.output);
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("allowed.txt")).unwrap(),
        "yes"
    );
}

fn assert_exact_container_absent(container_id: &str) {
    let status = std::process::Command::new("podman")
        .args(["container", "exists", "--", container_id])
        .status()
        .expect("could not verify exact transient Podman container absence");
    assert_eq!(
        status.code(),
        Some(1),
        "exact transient container {container_id} still exists after reset"
    );
}

#[tokio::test]
#[ignore = "requires rootless Podman and an already-local image; never pulls"]
async fn literal_none_podman_shares_cwd_masks_control_state_and_cleans_exact_id() {
    let parent = TempDir::new().unwrap();
    let workspace = tempfile::Builder::new()
        .prefix("lethetic shared cwd ")
        .tempdir_in(parent.path())
        .unwrap();
    std::fs::create_dir(workspace.path().join(".lethetic")).unwrap();
    std::fs::write(
        workspace.path().join(".lethetic").join("host-marker"),
        "host-only",
    )
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let canonical = workspace.path().canonicalize().unwrap();
    let python_path = serde_json::to_string(canonical.to_str().unwrap()).unwrap();
    let code = format!(
        "import pathlib, socket\nroot = pathlib.Path({python_path})\nassert pathlib.Path.cwd() == root\ntry:\n    marker_visible = (root / '.lethetic' / 'host-marker').exists()\nexcept PermissionError:\n    pass\nelse:\n    assert not marker_visible\ntry:\n    list((root / '.lethetic').iterdir())\nexcept PermissionError:\n    pass\nelse:\n    raise AssertionError('masked .lethetic was readable')\ns = socket.socket()\ns.settimeout(2)\nassert s.connect_ex(('127.0.0.1', {port})) != 0\n(root / 'written by python.txt').write_text('ok', encoding='utf-8')"
    );
    let config = shared_podman_config(NetworkAccess::None);
    let (result, runtime) = execute_with_config(&config, &workspace, &code).await;
    assert!(!result.is_error, "{}", result.output);
    assert_eq!(result.cwd, canonical.to_string_lossy());
    let notice = runtime
        .take_python_runtime_notice()
        .expect("shared transient worker did not emit a runtime notice after hello");
    assert_eq!(
        notice.action,
        lethetic::python::RuntimeLaunchAction::Created
    );
    assert_eq!(notice.network, NetworkAccess::None);
    assert_eq!(notice.mounted_cwd, canonical);
    assert_eq!(notice.container_id.len(), 64);
    runtime.reset_checked().await.unwrap();
    assert_exact_container_absent(&notice.container_id);
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("written by python.txt")).unwrap(),
        "ok"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path().join(".lethetic").join("host-marker")).unwrap(),
        "host-only"
    );
    drop(listener);
}

#[tokio::test]
#[ignore = "requires rootless Podman and an already-local image; never pulls"]
async fn literal_permissive_podman_shares_cwd_and_reaches_host_loopback() {
    let parent = TempDir::new().unwrap();
    let workspace = tempfile::Builder::new()
        .prefix("lethetic permissive cwd ")
        .tempdir_in(parent.path())
        .unwrap();
    std::fs::create_dir(workspace.path().join(".lethetic")).unwrap();
    std::fs::write(
        workspace.path().join(".lethetic").join("host-marker"),
        "host-only",
    )
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let canonical = workspace.path().canonicalize().unwrap();
    let python_path = serde_json::to_string(canonical.to_str().unwrap()).unwrap();
    let code = format!(
        "import pathlib, socket\nroot = pathlib.Path({python_path})\nassert pathlib.Path.cwd() == root\ntry:\n    marker_visible = (root / '.lethetic' / 'host-marker').exists()\nexcept PermissionError:\n    pass\nelse:\n    assert not marker_visible\ns = socket.socket()\ns.settimeout(2)\nassert s.connect_ex(('127.0.0.1', {port})) == 0\n(root / 'permissive write.txt').write_text('ok', encoding='utf-8')"
    );
    let config = shared_podman_config(NetworkAccess::Full);
    let (result, runtime) = execute_with_config(&config, &workspace, &code).await;
    assert!(!result.is_error, "{}", result.output);
    let notice = runtime
        .take_python_runtime_notice()
        .expect("permissive worker did not emit a runtime notice after hello");
    assert_eq!(
        notice.action,
        lethetic::python::RuntimeLaunchAction::Created
    );
    assert_eq!(notice.network, NetworkAccess::Full);
    assert_eq!(notice.mounted_cwd, canonical);
    runtime.reset_checked().await.unwrap();
    assert_exact_container_absent(&notice.container_id);
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("permissive write.txt")).unwrap(),
        "ok"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path().join(".lethetic").join("host-marker")).unwrap(),
        "host-only"
    );
    drop(listener);
}

async fn network_probe(backend: SandboxBackend, access: NetworkAccess, should_connect: bool) {
    let workspace = TempDir::new().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let expected = if should_connect { "True" } else { "False" };
    let code = format!(
        "import socket\ns = socket.socket()\ns.settimeout(2)\nresult = s.connect_ex(('127.0.0.1', {port}))\nassert (result == 0) is {expected}"
    );
    let result = execute(backend, access, AccessMode::ReadOnly, &workspace, &code).await;
    assert!(!result.is_error, "{}", result.output);
    drop(listener);
}

#[tokio::test]
#[ignore = "requires a working Bubblewrap runtime; never installs or falls back"]
async fn bubblewrap_enforces_none_and_full_network() {
    network_probe(SandboxBackend::Bubblewrap, NetworkAccess::None, false).await;
    network_probe(SandboxBackend::Bubblewrap, NetworkAccess::Full, true).await;
}

#[tokio::test]
#[ignore = "requires rootless Podman and the configured local image; never pulls"]
async fn podman_enforces_none_and_full_network() {
    network_probe(SandboxBackend::Podman, NetworkAccess::None, false).await;
    network_probe(SandboxBackend::Podman, NetworkAccess::Full, true).await;
}
