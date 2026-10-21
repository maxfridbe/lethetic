//! C# uses the same attested rootless Podman worker and lifecycle as Python-only.
//! Python is only the internal IPC supervisor; the model sees only the C# tool.
use crate::{
    config::{Config, PythonPreset, PythonWorkspaceExposure},
    tool_runtime::ToolRuntime,
    tools::ToolExecution,
};
use tokio_util::sync::CancellationToken;

pub const IMAGE: &str = "localhost/lethetic-dotnet-runtime:dev";
const BRIDGE: &str = include_str!("../../runtime/dotnet/bridge.py");
const MAX_CODE: usize = 1024 * 1024;

/// This policy is deliberately fixed, regardless of stale Host/Full settings in
/// a resumed session. There is no host SDK path, network or extra-grant opt-in.
pub fn isolated_config(config: &Config) -> Config {
    let mut isolated = config.clone();
    isolated.apply_python_preset(PythonPreset::Isolated);
    isolated.python_runtime.python_executable = "python3".to_string();
    isolated.python_runtime.sandbox.podman_image = IMAGE.to_string();
    isolated.python_invocation.workspace_exposure = PythonWorkspaceExposure::SharedLaunchCwd;
    isolated
}

pub async fn execute(
    runtime: &ToolRuntime,
    config: &Config,
    code: &str,
    cwd: &str,
    cancellation: CancellationToken,
    progress: &tokio::sync::mpsc::UnboundedSender<crate::client::StreamEvent>,
) -> ToolExecution {
    if code.len() > MAX_CODE {
        return ToolExecution::error(
            "ERROR: C# cell exceeds the 1 MiB source limit; save large programs to files.",
            cwd,
        );
    }
    let policy = isolated_config(config);
    // JSON strings are also valid Python literals for these string-only values.
    let source = format!(
        "if '_lethetic_dotnet_bridge' not in globals():\n    _lethetic_dotnet_bridge = {{}}\n    exec({}, _lethetic_dotnet_bridge)\n_lethetic_dotnet_bridge['run']({})",
        serde_json::to_string(BRIDGE).expect("string serialization"),
        serde_json::to_string(code).expect("string serialization"),
    );
    let token = cancellation.child_token();
    let execution = runtime.execute_python(&policy, cwd, &source, token.clone(), Some(progress));
    tokio::pin!(execution);
    let result = tokio::select! {
        result = &mut execution => result,
        _ = tokio::time::sleep(std::time::Duration::from_secs(120)) => {
            token.cancel();
            execution.await
        }
    };
    let mut output = result.output;
    if result.is_error && (output.contains(IMAGE) || output.contains("Podman is unavailable")) {
        output.push_str("\n.NET runs only in isolated Podman; no host fallback. Build the runtime image with: podman build -t localhost/lethetic-dotnet-runtime:dev runtime/dotnet");
    }
    ToolExecution {
        output,
        cwd: result.cwd,
        is_error: result.is_error,
        provenance: result.provenance,
    }
}
