# Work in C#

[All modes](../../README.md#how-to-guides) · [Native .NET reference](../dotnet-native.md)

.NET mode gives the model one execution tool, `dotnet`, backed by a persistent C#
runspace. Browser and native modes have different runtimes and capabilities.

## Browser DotnetDide

1. [Connect in the browser](browser.md), then select **.NET 11 · WASM** in
   **Connections → Runtime & security**.
2. Ask the model to write and execute C#. Files under `/home/dotnetdide` synchronize
   with the session's **Files** pane.
3. Save a `.cs` or supported `.csproj` and use its **▶ Run** button. Programs can
   also call `await Dotnet.Run("Program.cs")` or `Dotnet.Build()`; inspect the build
   result and compilation output.
4. Preview or download generated files. Bundled image, WAV, and Ogg Opus libraries
   support media generation without adding packages.

For additional compatible managed libraries, enable **NuGet package access** and
call `await DotnetDide.Nuget.Load("name", "version")` before using the package in a
later submission. Browser downloads are cached in Cache Storage.

Enable **Interactive sites** for `DotnetDide.Wire` / ASP.NET handlers, run the saved
program, then open **Sites**. Enable **Network access** separately for outbound
HTTP. CORS applies to direct requests; an optional HTTP relay can route through
upstream SOCKS5, including a Tor client's SOCKS port. See the
[relay setup](https://github.com/maxfridbe/dotnetdide/blob/master/docs/networking.md).

To debug a running application, call `Dotnet.GetDebugInfo()` in a later submission
for prior C# and iframe console logs, virtual HTTP requests, and loaded assemblies.
Save the JSON to a session file for inspection.

DotnetDide is bundled from a checksum-pinned release. Its browser build API does
not provide full native MSBuild or unrestricted OS access. See the
[.NET browser reference](../../integration.md#net-wasm-mode),
[site example](../../integration.md#run-saved-programs-and-interactive-sites), and
[media and package guide](../../integration.md#bundled-media-and-optional-nuget-packages).

## Native .NET

1. Complete [native setup](native.md) and configure rootless Podman.
2. Build the runtime image from the Lethetic repository:

   ```bash
   podman build -t localhost/lethetic-dotnet-runtime:dev runtime/dotnet
   ```

3. Run from the project you want to work on:

   ```bash
   lethetic --dotnet-only

   # Or use the same profile for a headless task.
   lethetic --dotnet-only --command "Write and test a C# program to summarize these CSV files"
   ```

The image contains the .NET SDK, Roslyn runspace, and an internal Python IPC
supervisor. Execution uses the same isolated Podman infrastructure as Python-only:
the launch workspace is writable, `.lethetic` is masked, and networking is off.
There is no host SDK fallback. Image construction can download dependencies;
agent execution cannot.

Native programs can invoke the SDK inside the container to build and run saved
projects. Browser-specific `Dotnet.Run` and `DotnetDide.Wire` APIs are not the native
SDK interface. See [native C# cells and files](../dotnet-native.md#c-cells-and-files)
for offline project requirements, default namespaces, cancellation, and reset behavior.
