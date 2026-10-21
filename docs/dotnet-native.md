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
serves a controller over HTTPS; C# runs inside Podman on the CLI machine.

Build the runtime image once from the repository root:

```bash
podman build -t localhost/lethetic-dotnet-runtime:dev runtime/dotnet
```

Rootless **Podman** and this image are required. The image contains the .NET 11 SDK,
its Roslyn compiler, a precompiled persistent C# host, and Python as an internal
IPC supervisor. The model sees only the `dotnet` tool. No host .NET SDK, host Python
or Node.js is used. Image construction downloads dependencies; agent execution
uses the local image with networking disabled. Rebuild after runtime source changes.
Missing Podman or image is an error: there is **no host fallback**.

You can also choose **Ctrl+P → Agent Mode → .NET-only**, or choose **.NET only
(Podman isolated)** in the remote controller. The literal `--dotnet-only` flag
locks the mode for that process, including resumed sessions, and conflicts with
Python-only flags. A trusted user config can set `tool_profile: dotnet_only`;
project policy cannot silently select this mode.

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

Files live in the mounted launch workspace and persist on the host. Paths outside
that mount refer only to the container filesystem.
`Directory.SetCurrentDirectory` changes the subsequent tool cwd. With `--rc-files`,
files inside the shared launch directory are visible through the existing read-only
Files pane, subject to its usual disclosure rules.

Save reusable `.cs`/`.csproj` programs using `File.WriteAllText`. Build and run them
using `System.Diagnostics.Process` with `dotnet build app.csproj` and
`dotnet run --project app.csproj`; SDK 10+ also supports `dotnet run --file app.cs`.
Use `ProcessStartInfo.ArgumentList`, redirect both output streams, read them
concurrently, await process completion and check its exit code. Framework-only SDK
projects targeting `net11.0` with `<NuGetAudit>false</NuGetAudit>` build offline;
older targeting packs are not installed; downloading NuGet packages is disabled; project dependencies do not automatically
become references in the interactive runspace.

## Lifetime and permissions

This mode uses the **same isolated Podman launcher, attestation and cleanup as
`--python-only isolated`**:

- Rootless, non-root worker; read-only container root; all capabilities dropped;
  no-new-privileges and private temporary directories.
- Only the launch workspace mounted read/write, with its `.lethetic` control
  directory masked. No additional host path grants or host environment forwarding.
- Network disabled, including access to host services. Host SDK selections,
  inherited Python Host settings and network/package permissions cannot override it.
- Subprocesses, SDK compilation and saved programs all execute inside the container.

The CLI's model-provider requests remain outside this tool container and still
work. This is native .NET in Podman, not DotnetDide WASM: browser-only Wire,
virtual networking/debugger helpers and bundled media libraries are unavailable.
The injected prompt describes these boundaries and keeping `tasks.md` updated.

Cells accept up to **1 MiB of source** and have a **120-second deadline**. The C#
host captures up to **1,000,000 characters each** of stdout and stderr; the shared
worker additionally returns bounded output excerpts. Save large results to files.
Interactive stdin is closed. Cancellation/timeout uses the same container teardown
as Python-only, including child processes; the next call starts cleanly. Files
survive. Compiler errors leave earlier state available; runtime exceptions can
leave partial variable/file changes.

Session history and mode are saved normally. Live CLR objects are not serialized:
restart, session switch, mode change or cancellation loses variables without
replaying code or undoing files. The headless `--timeout-seconds` deadline also
applies to the whole agent turn.

## Verification

```bash
cargo test --test test_dotnet_native -- --include-ignored
cargo test --bin lethetic cli::tests
```

The container tests cover state across calls, workspace file output, compiler
errors, stderr, exceptions, offline project builds, reset, cancellation, blocked
networking, hidden host files/environment, and masked control files. CI builds
the Podman image and runs them with an invalid host SDK path. Regular Rust tests
check tool exclusivity, prompt guidance and fixed isolation policy without Podman.
