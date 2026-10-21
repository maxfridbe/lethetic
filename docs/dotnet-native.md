# Native .NET-only mode

[Native reference](native-reference.md) · [Browser .NET WASM](../integration.md#net-wasm-mode)

Run Lethetic with exactly one model tool, `dotnet`, backed by a persistent native
C# runspace:

```bash
lethetic --dotnet-only
lethetic --dotnet-only --command "Write and run a C# program that summarizes these CSV files"
lethetic -rc 127.0.0.1 --rc-files --dotnet-only
```

`-rc` and `--rc` are equivalent. Remote control uses the same native session,
provider connection, approvals, tools and files as the terminal. The last example
serves a controller over HTTPS; C# runs on the CLI machine. It does not load browser
WASM or require DotnetDide, Pyodide, Node.js, containers or downloaded packages.

Install a **.NET SDK 8 or newer**, accessible as `dotnet` on PATH. Set
`LETHETIC_DOTNET=/absolute/path/to/dotnet` to choose another installation. An SDK is
required, not just the runtime. The embedded host builds in a private temporary
directory on first execution using the SDK's own Roslyn compiler and framework
references. The compiler's supported C# language version follows the installed SDK.

You can also choose **Ctrl+P → Agent Mode → .NET-only**, or choose **.NET only (host)**
in the remote controller. The literal `--dotnet-only` flag locks the mode for that
process, including resumed sessions, and conflicts with Python-only flags. A trusted
user config can set `tool_profile: dotnet_only`; project policy cannot silently opt
into unrestricted native execution.

## C# cells and files

Imports, variables and declarations persist across successful calls within the
current runspace. Top-level `await` works, and a final expression without a
semicolon is returned as the cell value. Compiler diagnostics, stdout, stderr and
runtime exceptions are returned to the model. Cells follow C# script syntax: use
`using (var resource = ...) { ... }` rather than top-level `using var` declarations.
Saved SDK programs use ordinary C# syntax.

Default namespaces are `System`, `System.IO`, `System.Linq`,
`System.Collections.Generic`, `System.Net.Http`, `System.Text`, `System.Text.Json`,
`System.Threading`, and `System.Threading.Tasks`. Other framework namespaces can be
imported explicitly.

```csharp
// First tool call:
int answer = 40;
await File.WriteAllTextAsync("tasks.md", "- [ ] Write the answer\n");

// A later tool call in the same runspace:
await File.WriteAllTextAsync("answer.txt", (answer + 2).ToString());
Console.WriteLine(await File.ReadAllTextAsync("answer.txt"));
await File.WriteAllTextAsync("tasks.md", "- [x] Write the answer\n");
```

Files are ordinary host files, initially relative to the launch workspace.
`Directory.SetCurrentDirectory` changes the subsequent tool cwd. With `--rc-files`,
files inside the shared launch directory are visible through the existing read-only
Files pane, subject to its usual disclosure rules.

Save reusable `.cs`/`.csproj` programs using `File.WriteAllText`. Build and run them
using `System.Diagnostics.Process` with `dotnet build app.csproj` and
`dotnet run --project app.csproj`; SDK 10+ also supports `dotnet run --file app.cs`.
Use `ProcessStartInfo.ArgumentList`, redirect both output streams, read them
concurrently, await process completion and check its exit code. Ordinary SDK
projects can restore NuGet packages; project dependencies do not automatically
become references in the interactive runspace.

## Lifetime and permissions

This mode runs with **the CLI user's host permissions**: real System.IO files,
HttpClient/networking, environment and subprocesses. It adds no Python-style
container sandbox or browser permission switches. Browser-only DotnetDide helpers
such as Wire, virtual networking/debugger data, and bundled media libraries are
not installed in this native runspace. The model receives these distinctions and
the default namespaces in its injected prompt, including guidance to keep tasks.md.

Cells accept up to **1 MiB of source**, have a **120-second deadline**, and capture
up to **1,000,000 characters each** of stdout and stderr. Interactive stdin is
closed. Cancellation/timeout kills the worker (and its process group on Unix);
the next call starts cleanly. Files survive. Compiler errors leave earlier state
available; runtime exceptions can leave partial variable/file changes.

Session history and mode are saved normally. Live CLR objects are not serialized:
restart, session switch, mode change or cancellation loses variables without
replaying code or undoing files. The headless `--timeout-seconds` deadline also
applies to the whole agent turn.

## Verification

```bash
cargo test --test test_dotnet_native -- --include-ignored
cargo test --bin lethetic cli::tests
```

The SDK tests cover state across calls, real file output, diagnostics, stderr,
exceptions, building/running a saved project, reset and cancellation of an infinite loop. CI explicitly installs
the .NET 11 SDK and runs them. Regular Rust test runs still check tool exclusivity
and native prompt guidance without requiring .NET on the build machine.
