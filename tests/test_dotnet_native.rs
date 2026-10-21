//! Requires rootless Podman and the runtime/dotnet image; never uses a host SDK.
use lethetic::{
    config::{Config, ToolProfile},
    tool_runtime::ToolRuntime,
    tools::{self, ToolContext},
};
use tokio_util::sync::CancellationToken;

fn workspace() -> tempfile::TempDir {
    // Distrobox-forwarded Podman must see the same absolute workspace path.
    let parent =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/dotnet-isolation-tests");
    std::fs::create_dir_all(&parent).unwrap();
    tempfile::tempdir_in(parent).unwrap()
}

#[test]
fn runtime_lifecycle_futures_fit_on_normal_thread_stacks() {
    let runtime = ToolRuntime::headless(".");
    let config = Config::default();
    let (tx, _) = tokio::sync::mpsc::unbounded_channel();
    let sizes = [
        (
            "dotnet",
            std::mem::size_of_val(&lethetic::dotnet::execute(
                &runtime,
                &config,
                "",
                ".",
                CancellationToken::new(),
                &tx,
            )),
        ),
        (
            "python",
            std::mem::size_of_val(&runtime.execute_python(
                &config,
                ".",
                "",
                CancellationToken::new(),
                Some(&tx),
            )),
        ),
        ("reset", std::mem::size_of_val(&runtime.reset_checked())),
    ];
    assert!(
        sizes.iter().all(|(_, bytes)| *bytes < 16 * 1024),
        "{sizes:?}"
    );
}

async fn cell(
    runtime: &ToolRuntime,
    code: &str,
    cancel: CancellationToken,
) -> tools::ToolExecution {
    let config = Config {
        tool_profile: ToolProfile::DotnetOnly,
        ..Default::default()
    };
    let (tx, _) = tokio::sync::mpsc::unbounded_channel();
    Box::pin(tools::execute_with_runtime(
        runtime,
        "dotnet",
        &serde_json::json!({"code":code,"description":"test"}),
        ToolContext {
            cwd: runtime.workspace_root().to_str().unwrap(),
            cancellation_token: cancel,
            tx,
            client: &reqwest::Client::new(),
            config: &config,
        },
    ))
    .await
}

#[tokio::test]
#[ignore = "requires rootless Podman and the Lethetic .NET runtime image"]
async fn native_dotnet_persists_state_files_errors_and_resets() {
    let dir = workspace();
    let runtime = ToolRuntime::headless(dir.path());
    let result = cell(&runtime, "int answer = 40; await File.WriteAllTextAsync(\"answer.txt\", \"saved\"); Console.WriteLine(answer);", CancellationToken::new()).await;
    assert!(!result.is_error, "{}", result.output);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("answer.txt")).unwrap(),
        "saved"
    );
    let result = cell(&runtime, "answer + 2", CancellationToken::new()).await;
    assert!(!result.is_error, "{}", result.output);
    assert!(result.output.contains("42"), "{}", result.output);
    let result = cell(&runtime, "this will not compile", CancellationToken::new()).await;
    assert!(result.is_error);
    assert!(result.output.contains("CS"));
    assert!(
        !cell(&runtime, "answer", CancellationToken::new())
            .await
            .is_error
    );
    let result = cell(
        &runtime,
        "Console.Error.WriteLine(\"stderr-marker\"); throw new Exception(\"failure-marker\");",
        CancellationToken::new(),
    )
    .await;
    assert!(result.is_error);
    assert!(
        result.output.contains("stderr-marker") && result.output.contains("failure-marker"),
        "{}",
        result.output
    );
    let bounded = cell(
        &runtime,
        "Console.Out.Write(new string('x', 1_100_000).AsSpan());",
        CancellationToken::new(),
    )
    .await;
    assert!(
        !bounded.is_error,
        "{}",
        bounded.output.chars().take(200).collect::<String>()
    );
    assert!(bounded.output.contains("[output truncated]"));
    assert!(bounded.output.len() < 1_000_100);
    Box::pin(runtime.reset_checked()).await.unwrap();
    assert!(
        cell(&runtime, "answer", CancellationToken::new())
            .await
            .is_error
    );
}

#[tokio::test]
#[ignore = "requires rootless Podman and the Lethetic .NET runtime image"]
async fn cancellation_kills_runspace_and_next_call_starts_clean() {
    let dir = workspace();
    let runtime = ToolRuntime::headless(dir.path());
    let initial = cell(&runtime, "int previous = 1;", CancellationToken::new()).await;
    assert!(!initial.is_error, "{}", initial.output);
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        trigger.cancel();
    });
    let result = tokio::time::timeout(
        // Podman force-removal has a 10-second stop grace plus attested cleanup.
        std::time::Duration::from_secs(45),
        cell(&runtime, "while (true) {}", cancel),
    )
    .await
    .unwrap();
    assert!(
        result.is_error && result.output.to_lowercase().contains("cancel"),
        "{}",
        result.output
    );
    assert!(
        cell(&runtime, "previous", CancellationToken::new())
            .await
            .is_error
    );
    assert!(
        !cell(
            &runtime,
            "Console.WriteLine(\"recovered\");",
            CancellationToken::new()
        )
        .await
        .is_error
    );
}

#[test]
fn dotnet_surface_is_exclusive_and_prompt_is_native() {
    let config = Config {
        tool_profile: ToolProfile::DotnetOnly,
        ..Default::default()
    };
    for surface in [
        tools::ToolSurface::Interactive,
        tools::ToolSurface::Headless,
    ] {
        let offered = tools::get_tools_for_surface(&config, surface);
        assert_eq!(offered.len(), 1);
        assert_eq!(offered[0].function.name, "dotnet");
        assert!(!tools::is_tool_allowed(&config, surface, "python"));
        assert!(!tools::is_tool_allowed(
            &config,
            surface,
            "run_shell_command"
        ));
    }
    let prompt = lethetic::system_prompt::SystemPromptManager::resolve_prompt(
        "[TOOLS_DEFINITIONS]",
        ".",
        &config,
    );
    assert!(prompt.contains("native .NET") && prompt.contains("tasks.md"));
}

#[test]
fn native_python_capability_guidance_survives_cwd_projection() {
    use lethetic::{
        config::PythonExecutionTarget,
        context::ContextManager,
        system_prompt::{DEFAULT_PROMPT_TEMPLATE, SystemPromptManager},
        transport,
    };
    let mut config = Config {
        tool_profile: ToolProfile::PythonOnly,
        context_size: 100_000,
        ..Default::default()
    };
    config.python_runtime.target = Some(PythonExecutionTarget::Host);
    let prompt = SystemPromptManager::resolve_prompt(DEFAULT_PROMPT_TEMPLATE, "/tmp", &config);
    let mut context = ContextManager::new(100_000, Some(prompt));
    context.set_cwd("/tmp/another-directory".to_string());
    context.add_message("user", "Run one Python cell");
    let guidance = lethetic::system_prompt::python_capability_guidance(&config).unwrap();
    let projected = context.prepare_api_context_with_guidance(Some(&guidance));
    transport::request_policy::validate_agent_surface(
        &config,
        projected.messages(),
        &tools::get_api_tools(&config, tools::ToolSurface::Headless),
    )
    .unwrap();
    let system = projected
        .messages()
        .iter()
        .find(|m| m.role == transport::Role::System)
        .unwrap()
        .content
        .text();
    assert!(system.contains("/tmp/another-directory"));
    assert!(system.ends_with(&guidance));
    context.add_message("system", &guidance);
    let duplicated = context.prepare_api_context_with_guidance(Some(&guidance));
    assert!(
        transport::request_policy::validate_agent_surface(
            &config,
            duplicated.messages(),
            &tools::get_api_tools(&config, tools::ToolSurface::Headless)
        )
        .is_err()
    );
}

#[tokio::test]
#[ignore = "requires rootless Podman and the Lethetic .NET runtime image"]
async fn native_dotnet_can_build_and_run_a_saved_project() {
    let dir = workspace();
    let runtime = ToolRuntime::headless(dir.path());
    let code = r#"
Directory.CreateDirectory("program");
File.WriteAllText("program/Test.csproj", $"<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><OutputType>Exe</OutputType><TargetFramework>net{Environment.Version.Major}.0</TargetFramework><NuGetAudit>false</NuGetAudit></PropertyGroup></Project>");
File.WriteAllText("program/Program.cs", "System.Console.WriteLine(6 * 7);");
var start = new System.Diagnostics.ProcessStartInfo("dotnet") {
    RedirectStandardOutput = true, RedirectStandardError = true, UseShellExecute = false
};
start.ArgumentList.Add("run");
start.ArgumentList.Add("--project");
start.ArgumentList.Add("program/Test.csproj");
start.ArgumentList.Add("-p:UseSharedCompilation=false");
var program = System.Diagnostics.Process.Start(start)!;
var output = program.StandardOutput.ReadToEndAsync();
var errors = program.StandardError.ReadToEndAsync();
await program.WaitForExitAsync();
Console.WriteLine(await output);
Console.Error.WriteLine(await errors);
if (program.ExitCode != 0) throw new Exception("Project did not run");
program.Dispose();
"#;
    let result = cell(&runtime, code, CancellationToken::new()).await;
    assert!(!result.is_error, "{}", result.output);
    assert!(result.output.contains("42"), "{}", result.output);
    assert!(dir.path().join("program/Program.cs").is_file());
}

#[test]
fn dotnet_provider_requests_keep_the_exclusive_tool_policy() {
    use lethetic::{
        config::ConnectionKind,
        transport::{self, Message},
    };
    for kind in [
        ConnectionKind::OpenAiChatCompletions,
        ConnectionKind::ClaudeCodeProxy,
    ] {
        let config = Config {
            tool_profile: ToolProfile::DotnetOnly,
            connection_kind: kind,
            context_size: 100_000,
            ..Default::default()
        };
        let messages = [
            Message::system(lethetic::system_prompt::capability_guidance(&config).unwrap()),
            Message::user("Run C#"),
        ];
        for surface in [
            tools::ToolSurface::Headless,
            tools::ToolSurface::Interactive,
        ] {
            let prepared = transport::prepare_agent_request(
                &config,
                &messages,
                &tools::get_api_tools(&config, surface),
                config.request_output_tokens(),
            )
            .unwrap();
            let body: serde_json::Value = serde_json::from_slice(prepared.body_bytes()).unwrap();
            assert_eq!(body["tools"].as_array().unwrap().len(), 1);
            let mut tampered = body.clone();
            tampered["tools"] = serde_json::json!([]);
            assert!(
                transport::request_policy::validate_final_agent_body(&config, &tampered).is_err()
            );
            let mut tampered = body.clone();
            tampered["functions"] = serde_json::json!([{"name":"shell"}]);
            assert!(
                transport::request_policy::validate_final_agent_body(&config, &tampered).is_err()
            );
        }
    }
}

#[tokio::test]
#[ignore = "requires rootless Podman and the Lethetic .NET runtime image"]
async fn isolated_dotnet_cannot_reach_host_network() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        let mut received = 0;
        while !request[..received]
            .windows(4)
            .any(|part| part == b"\r\n\r\n")
        {
            assert!(received < request.len());
            let count = socket.read(&mut request[received..]).await.unwrap();
            assert!(count > 0, "request ended before HTTP headers");
            received += count;
        }
        assert!(request[..received].starts_with(b"GET / HTTP/1.1"));
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n42")
            .await
            .unwrap();
    });
    let dir = workspace();
    let runtime = ToolRuntime::headless(dir.path());
    let result = cell(&runtime, &format!(r#"
if (System.Net.NetworkInformation.NetworkInterface.GetAllNetworkInterfaces().Any(i => i.NetworkInterfaceType != System.Net.NetworkInformation.NetworkInterfaceType.Loopback)) throw new Exception("Unexpected network interface");
using (var http = new HttpClient {{ Timeout = TimeSpan.FromSeconds(1) }}) {{
    try {{ await http.GetStringAsync("http://{address}/"); throw new Exception("Host network was reachable"); }}
    catch (HttpRequestException) {{ Console.WriteLine("network-blocked"); }}
    catch (TaskCanceledException) {{ Console.WriteLine("network-blocked"); }}
}}
"#), CancellationToken::new()).await;
    server.abort();
    assert!(!result.is_error, "{}", result.output);
    assert!(
        result.output.contains("network-blocked"),
        "{}",
        result.output
    );
}

#[test]
fn dotnet_policy_cannot_inherit_host_permissions_or_network() {
    use lethetic::config::{
        AccessMode, NetworkAccess, PackageAccess, PythonExecutionTarget, PythonWorkspaceExposure,
        SandboxBackend,
    };
    let mut config = Config::default();
    config.python_runtime.target = Some(PythonExecutionTarget::Host);
    config.python_runtime.python_executable = "/tmp/untrusted-python".to_string();
    config.python_runtime.sandbox.network = Some(NetworkAccess::Full);
    config.python_runtime.sandbox.podman_image = "untrusted-image".to_string();
    config
        .python_runtime
        .sandbox
        .grants
        .push(lethetic::config::PathGrant {
            path: "/".into(),
            access: AccessMode::ReadWrite,
        });
    let isolated = lethetic::dotnet::isolated_config(&config);
    assert_eq!(
        isolated.python_runtime.target,
        Some(PythonExecutionTarget::Sandbox)
    );
    assert_eq!(
        isolated.python_runtime.sandbox.backend,
        Some(SandboxBackend::Podman)
    );
    assert_eq!(
        isolated.python_runtime.sandbox.network,
        Some(NetworkAccess::None)
    );
    assert_eq!(
        isolated.python_runtime.sandbox.workspace_access,
        Some(AccessMode::ReadWrite)
    );
    assert_eq!(
        isolated.python_runtime.sandbox.package_access,
        PackageAccess::Disabled
    );
    assert!(isolated.python_runtime.sandbox.grants.is_empty());
    assert_eq!(
        isolated.python_runtime.sandbox.podman_image,
        lethetic::dotnet::IMAGE
    );
    assert_eq!(
        isolated.python_invocation.workspace_exposure,
        PythonWorkspaceExposure::SharedLaunchCwd
    );
    assert_eq!(isolated.python_runtime.python_executable, "python3");
}

#[tokio::test]
#[ignore = "requires rootless Podman and the Lethetic .NET runtime image"]
async fn isolated_dotnet_hides_host_files_environment_and_control_state() {
    let outside = workspace();
    let secret = outside.path().join("host-secret");
    std::fs::write(&secret, "must-not-be-readable").unwrap();
    let dir = tempfile::tempdir_in(outside.path()).unwrap();
    std::fs::create_dir(dir.path().join(".lethetic")).unwrap();
    std::fs::write(dir.path().join(".lethetic/control-secret"), "hidden").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&secret, dir.path().join("escape-link")).unwrap();
    let runtime = ToolRuntime::headless(dir.path());
    let code = format!(
        r#"
foreach (var path in new[] {{ {}, "escape-link", ".lethetic/control-secret" }}) {{
    try {{ File.ReadAllText(path); throw new Exception("Host isolation failed: " + path); }}
    catch (IOException) {{ }}
    catch (UnauthorizedAccessException) {{ }}
}}
if (Environment.GetEnvironmentVariable("LETHETIC_DOTNET") != null) throw new Exception("Host environment leaked");
File.WriteAllText("allowed.txt", "workspace-access");
Console.WriteLine("isolation-passed");
"#,
        serde_json::to_string(secret.to_str().unwrap()).unwrap()
    );
    let result = cell(&runtime, &code, CancellationToken::new()).await;
    assert!(!result.is_error, "{}", result.output);
    assert!(result.output.contains("isolation-passed"));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("allowed.txt")).unwrap(),
        "workspace-access"
    );
    assert!(runtime.python_container_identity().is_some());
    Box::pin(runtime.reset_checked()).await.unwrap();
    assert!(runtime.python_container_identity().is_none());
}
