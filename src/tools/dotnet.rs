use super::{FunctionDefinition, Tool};

pub const GUIDANCE: &str = r#"# Isolated .NET-only execution
Exactly one model tool is available: `dotnet`. This is a native .NET C# runspace inside an isolated rootless Podman container, not browser WASM or host execution. The user expects you to write and execute C# programs to solve problems. Imports, declarations and variables persist between successful cells in this process. Default using namespaces: System, System.IO, System.Linq, System.Collections.Generic, System.Net.Http, System.Text, System.Text.Json, System.Threading, System.Threading.Tasks. Other installed runtime namespaces can be imported explicitly. Return a final expression without a semicolon to display its value, or use Console.WriteLine. Await asynchronous operations. Cells use C# script syntax: use `using (var resource = ...) { ... }` for disposal; top-level `using var` declarations are not supported in script cells. Regular SDK .cs programs support them.

Use File and Directory for the mounted workspace files. Keep a tasks.md checklist updated, and save useful programs as .cs files or .csproj projects for the user. File.WriteAllText("hello.cs", "Console.WriteLine(\"hello\");") creates a source file. Compile projects with System.Diagnostics.Process running `dotnet build path/to/app.csproj`; run with `dotnet run --project path/to/app.csproj`. SDK 10+ can run a file with `dotnet run --file hello.cs`. Use ProcessStartInfo.ArgumentList for arguments, redirect both output streams, read them concurrently, await WaitForExitAsync and check ExitCode. Networking and package downloads are disabled. The container includes .NET 11 only: use <TargetFramework>net11.0</TargetFramework> and <NuGetAudit>false</NuGetAudit> in saved projects. Do not target net8.0/net9.0/net10.0: their targeting packs are not installed and cannot be downloaded. Framework-only net11.0 SDK builds work offline inside the container.

Execution uses the exact isolated Python-only Podman policy: no network, read-only container root, dropped capabilities, no-new-privileges, non-root worker, private temporary directories, and only the launch workspace mounted read/write. The workspace .lethetic directory is masked. Host environment, home, credentials and files outside that mount are not available. System.Diagnostics.Process and dotnet build/run execute inside the container; a host SDK is neither used nor required. Do not try to bypass isolation or claim networking is available. Browser-only DotnetDide.Wire, virtual filesystem, media defaults and browser permission toggles are not available here. Never claim that they are. Console.ReadLine has no interactive input. Each cell has a 120-second deadline and a 1 MiB source limit; stdout and stderr are each bounded to 1,000,000 characters, with further bounded excerpts from the shared worker. Save large results to files. Cancellation, timeout, session change or process restart discards variables; files remain. Saved session history does not replay C# code. After a runtime exception, earlier variable/file mutations may remain: inspect before retrying.
"#;

pub fn get_definition() -> Tool {
    Tool {
        tool_type: "function".into(),
        function: FunctionDefinition {
            name: "dotnet".into(),
            description: "Execute a C# cell in the persistent .NET runspace inside rootless Podman (network disabled, launch workspace read/write, no host access). Use System.IO for workspace files and System.Diagnostics.Process for offline SDK builds/runs inside the container. Returns stdout, stderr, final expression, compiler diagnostics and exceptions. No interactive stdin. Variables persist until reset; files persist on disk.".into(),
            parameters: serde_json::json!({"type":"object","properties":{
                "code":{"type":"string","description":"C# script code to execute."},
                "description":{"type":"string","description":"Short description of this cell's purpose."}
            },"required":["code","description"]}),
        },
    }
}
