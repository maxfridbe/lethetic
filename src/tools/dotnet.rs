use super::{FunctionDefinition, Tool};

pub const GUIDANCE: &str = r#"# Native .NET-only execution
Exactly one model tool is available: `dotnet`. This is a native .NET C# runspace, not browser WASM. The user expects you to write and execute C# programs to solve problems. Imports, declarations and variables persist between successful cells in this process. Default using namespaces: System, System.IO, System.Linq, System.Collections.Generic, System.Net.Http, System.Text, System.Text.Json, System.Threading, System.Threading.Tasks. Other installed runtime namespaces can be imported explicitly. Return a final expression without a semicolon to display its value, or use Console.WriteLine. Await asynchronous operations. Cells use C# script syntax: use `using (var resource = ...) { ... }` for disposal; top-level `using var` declarations are not supported in script cells. Regular SDK .cs programs support them.

Use File and Directory for real workspace files. Keep a tasks.md checklist updated, and save useful programs as .cs files or .csproj projects for the user. File.WriteAllText("hello.cs", "Console.WriteLine(\"hello\");") creates a source file. Compile projects with System.Diagnostics.Process running `dotnet build path/to/app.csproj`; run with `dotnet run --project path/to/app.csproj`. SDK 10+ can run a file with `dotnet run --file hello.cs`. Use ProcessStartInfo.ArgumentList for arguments, redirect both output streams, read them concurrently, await WaitForExitAsync and check ExitCode. NuGet packages can be added to ordinary SDK projects with `dotnet add package`. They are not automatically imported into this interactive runspace.

Execution uses the CLI user's permissions, including System.IO, HttpClient, networking, environment and subprocesses. There is no Lethetic filesystem/network sandbox for this native mode. Browser-only DotnetDide.Wire, virtual filesystem, media defaults and browser permission toggles are not available here. Never claim that they are. Console.ReadLine has no interactive input. Each cell has a 120-second deadline and a 1 MiB source limit; stdout and stderr are each bounded to 1,000,000 characters. Cancellation, timeout, session change or process restart discards variables; files remain. Saved session history does not replay C# code. After a runtime exception, earlier variable/file mutations may remain: inspect before retrying.
"#;

pub fn get_definition() -> Tool {
    Tool {
        tool_type: "function".into(),
        function: FunctionDefinition {
            name: "dotnet".into(),
            description: "Execute a C# cell in the persistent native .NET runspace. Use System.IO for workspace files, HttpClient for networking, and System.Diagnostics.Process for SDK builds/runs. Returns stdout, stderr, final expression, compiler diagnostics and exceptions. No interactive stdin. Variables persist until reset; files persist on disk.".into(),
            parameters: serde_json::json!({"type":"object","properties":{
                "code":{"type":"string","description":"C# script code to execute."},
                "description":{"type":"string","description":"Short description of this cell's purpose."}
            },"required":["code","description"]}),
        },
    }
}
