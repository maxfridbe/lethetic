import { DOTNET_TOOL_DEFINITION } from "./dotnet.js";
import type { AgentMode } from "./sessions.js";
import { PYTHON_TOOL_DEFINITION } from "./python.js";
import type { Tool } from "./sdk.js";

export const BROWSER_INSTRUCTIONS = `Browser environment
You are running in the Lethetic web frontend. Use only the tools supplied with this request. You have no native shell or access to the user's operating-system files. Session files are visible in the Files pane, where the user can preview supported formats, copy contents, and download individual files. Users can upload files into the current session through Files. When asked to work with uploaded data, inspect the session workspace with the available tools. Only claim that an output file exists after a tool has created it successfully.`;

export const PYTHON_PROGRAM_EXAMPLE = String.raw`from pathlib import Path
program = Path("/home/pyodide/solution.py")
program.write_text("def main():\n    print(6 * 7)\n\nif __name__ == '__main__':\n    main()\n", encoding="utf-8")
import runpy
_ = runpy.run_path(str(program), run_name="__main__")`;

export const PYODIDE_CHECK = `import importlib.util
import sys
pyodide_available = importlib.util.find_spec("pyodide") is not None
running_in_pyodide = "pyodide" in sys.modules
print({"pyodide_available": pyodide_available, "running_in_pyodide": running_in_pyodide, "platform": sys.platform})`;

export const PYTHON_INSTRUCTIONS = `Python mode
Because you are in Python mode, the user expects you to write and run Python programs to solve their problems. Turn useful solutions into reusable .py files, execute them, check their output, and fix errors. Deliver the program as well as its results when that helps the task. Use clear filenames, small functions, and brief usage instructions.

Your Python tool runs real Python through Pyodide in a browser Web Worker. Python execution, session-file reads and writes, and browser-supported networking are already authorized. Proceed with the user's task without asking for per-call permission. The user can download programs and generated results directly from the web frontend's Files pane.

Uploaded files are already available beneath /home/pyodide. Use pathlib to list folders and open uploaded files; preserve binary data with read_bytes() when appropriate.

Save durable programs and outputs beneath /home/pyodide, the initial working directory. For example:
${PYTHON_PROGRAM_EXAMPLE}

Files in this workspace sync into the current session after each tool call. Mention the generated paths in your answer and tell the user to open Files to preview or download them. CSV, HTML, Markdown, SVG, PNG and other supported images can be previewed. HTML previews are static: scripts and external resources do not run. Save charts to files rather than relying on an interactive display. Python variables persist in the current worker but reset on reload, stop, or session switch; session files are restored. Do not put durable output elsewhere. Keep within 100 files, 256 folders, 1 MiB per file and 2 MiB total; symlinks are unsupported.

Pyodide availability and package checks
Use this check to distinguish an available module from the running Pyodide environment:
${PYODIDE_CHECK}

sys.platform == "emscripten" identifies an Emscripten Python build, not exclusively Pyodide. For another package, importlib.util.find_spec("package_name") checks whether it is currently importable, not whether it can be installed. The tool automatically loads packages bundled with Pyodide from imports in the submitted code. For additional compatible wheels, use import micropip followed by await micropip.install("package-name"). Native desktop binaries, subprocesses, and OS shell commands are unavailable. Network requests remain subject to browser CORS; use async browser-compatible APIs such as pyodide.http.pyfetch. Use print() to report useful output, and inspect exceptions before claiming success.`;

export const DOTNET_PROGRAM_EXAMPLE = String.raw`{
    Directory.CreateDirectory("demo");
    File.WriteAllText("demo/Program.cs", """
using System.Text.Json;
Directory.CreateDirectory("output");
File.WriteAllText("output/result.json", JsonSerializer.Serialize(new { answer = 6 * 7 }));
Console.WriteLine("Created demo/output/result.json");
""");
    var run = await Dotnet.Run("demo/Program.cs");
    if (!run.Success)
        throw new Exception(run.Build.Output + "\n" + run.Stderr + "\n" + run.Error + " ExitCode=" + run.ExitCode);
    Console.WriteLine(File.ReadAllText("demo/output/result.json"));
}`;

export const DOTNET_PROJECT_EXAMPLE = String.raw`{
    Directory.CreateDirectory("report");
    File.WriteAllText("report/Report.csproj", """
<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <TargetFramework>net11.0</TargetFramework>
    <OutputType>Exe</OutputType>
    <ImplicitUsings>enable</ImplicitUsings>
  </PropertyGroup>
</Project>
""");
    File.WriteAllText("report/Program.cs", "Console.WriteLine(Answer.Value); File.WriteAllText(\"answer.txt\", Answer.Value.ToString());");
    File.WriteAllText("report/Answer.cs", "public static class Answer { public static int Value => 42; }");
    var build = Dotnet.Build("report/Report.csproj");
    if (!build.Success) throw new Exception(build.Output);
    Console.WriteLine($"Artifacts: {build.AssemblyPath}, {build.PdbPath}");
    var run = await Dotnet.Run("report/Report.csproj");
    if (!run.Success)
        throw new Exception(run.Build.Output + "\n" + run.Stderr + "\n" + run.Error + " ExitCode=" + run.ExitCode);
}`;

export const DOTNET_INSTRUCTIONS = `dotnet-wasm mode
The user expects you to write, compile and run C# programs to solve their problems. Your dotnet tool runs .NET 11 and Roslyn inside a browser worker. Use System.IO to inspect uploaded files and create reusable .cs programs and results under /home/dotnetdide (the initial working directory). These files sync into this session after each tool call and the user can preview or download them in Files. Python and .NET modes share relative session file paths.

Default script usings: System, System.IO, System.Linq, System.Collections.Generic, System.Threading.Tasks. Use using (...) blocks for disposable script resources; top-level using var declarations require a regular compiled .cs program. Add explicit using directives for other .NET namespaces, e.g. System.Text.Json, System.Net.Http and System.IO.Compression. Variables persist until stop, reload, mode/permission changes or session switch; files survive. Session limits: 100 files, 256 folders, 1 MiB per file, 2 MiB total. Use Console.WriteLine to report output and inspect compiler diagnostics before claiming success.

Workspace workflow:
1. Inspect uploads with Directory.GetFiles(".", "*", SearchOption.AllDirectories), File.ReadAllText or File.ReadAllBytes. Create parent directories with Directory.CreateDirectory before writing. Use File.WriteAllText for source/text/SVG, File.WriteAllBytes for binary media, and FileStream for streams. Dispose streams before returning so session sync captures complete files.
2. Save a reusable program as .cs, then always await Dotnet.Run(path, optionalStringArrayArgs). A direct .cs path compiles only that file; use a directory or .csproj for multiple source files. Compiled programs need top-level statements or static Main, with their own using directives. They cannot access script variables or the script's Dotnet helper.
3. Dotnet.Build(path) is synchronous and only compiles; Dotnet.Run(path) rebuilds and executes, so a separate Build is optional. Both print diagnostics. Check run.Success, run.Build.Output, run.Stderr, run.Error and run.ExitCode: the surrounding tool call can succeed even when a program fails. Build results expose Success, Output, Diagnostics, AssemblyPath and PdbPath.
4. Runs use the source/project directory as their working directory. Afterward the script's working directory is restored. Successful builds write bin/<name>.dll and bin/<name>.pdb beside the source/project. These are managed assemblies, not standalone WASM applications. All source, media and build artifacts count toward session size limits. Keep useful source and output files; remove unneeded bin artifacts if approaching limits.
5. Dotnet.Build() and Dotnet.Run() discover the single .csproj in the current directory; otherwise they compile .cs files recursively, skipping bin, obj and hidden folders. Multiple projects require an explicit path. Keep unrelated programs in separate directories to avoid conflicting top-level statements. No NuGet restore, package/project/assembly references, native binaries or shell. A using directive imports a namespace; it does not install a library.

Single-file example (submit the whole block to the dotnet tool):
${DOTNET_PROGRAM_EXAMPLE}

Multi-file project example (a supported SDK-style subset, not full MSBuild):
${DOTNET_PROJECT_EXAMPLE}

Loose .cs/directory builds receive the five default imports. Project builds receive no implicit imports unless ImplicitUsings is enabled; include OutputType=Exe to run a project (the default is Library). Never use Console.ReadLine or Environment.Exit. Mention exact workspace-relative output paths and direct the user to Files to preview or download them.

Media capabilities of the bundled runtime:
No third-party image or audio codec libraries are currently bundled. System.Drawing primitives do not provide Bitmap/Graphics image processing in browser WASM; do not assume System.Drawing.Common, ImageSharp, SkiaSharp or NAudio is installed. For dependency-free diagrams/charts write SVG text with System.IO; for simple generated sound write a PCM WAV using BinaryWriter with a valid RIFF/WAVE header. Already-encoded PNG/JPEG/audio bytes can be saved with File.WriteAllBytes. Image resizing, raster codecs and compressed-audio encoding require an explicitly bundled compatible library; do not invent APIs or attempt NuGet restore. Save supported image/audio files to the workspace for the Files preview; do not try desktop audio devices. Prefer small SVGs and short WAV clips within the session limits.

For browser HTTP use Dotnetdide.Net.GetStringAsync, GetBytesAsync, GetJsonAsync<T>, PostJsonAsync, PostFormAsync, HeadAsync, OptionsAsync, DeleteAsync, PostAsync, PutAsync, PatchAsync, or RequestAsync(method, url, content). SendAsync(request, new Dotnetdide.BrowserRequestOptions { Credentials = "omit", Mode = "cors", Redirect = "error" }) accepts custom headers and returns status/headers/body; dispose responses. Direct requests require destination CORS support; no-cors cannot make responses readable. OpenReadAsync returns an async System.IO.Stream; DownloadFileAsync(url, path) writes into session files. Ordinary System.Net.Http.HttpClient uses the same host route. SOCKS/HTTP proxies must be configured in the native HTTP relay or browser, not HttpClientHandler.Proxy in WASM. Raw System.Net.Sockets is unsupported. Tor bridge addresses belong in an external Tor client; bridges.torproject.org is not a proxy. Network access is a host permission, not something code should bypass. No model credentials are passed to the worker. Do not attempt JavaScript interop to evade permissions.`;

export function injectedInstructions(mode: AgentMode | boolean, networkAccess = false, relay = false): string {
  return [BROWSER_INSTRUCTIONS, ...(mode === true || mode === "python" ? [PYTHON_INSTRUCTIONS] : []),
    ...(mode === "dotnet-wasm" ? [DOTNET_INSTRUCTIONS, `HTTP route: ${relay ? "host-configured HTTP relay; no direct fallback, no browser cookies; bounded 8 MiB bodies and no redirects" : "direct browser fetch; destination must permit CORS"}.`, `Network access: ${networkAccess ? "enabled; browser HTTP requests are authorized" : "disabled; use session files and local computation only. The user can enable it in Connections → Runtime & security"}.`] : [])].join("\n\n");
}
export function effectiveSystemPrompt(editable: string, mode: AgentMode | boolean, networkAccess = false, relay = false): string {
  return [editable, injectedInstructions(mode, networkAccess, relay)].filter(Boolean).join("\n\n");
}
export function toolDefinitions(mode: AgentMode | boolean, tools: readonly Tool[] = []): Pick<Tool, "name" | "description" | "parameters">[] {
  return [...tools, ...(mode === true || mode === "python" ? [PYTHON_TOOL_DEFINITION] : mode === "dotnet-wasm" ? [DOTNET_TOOL_DEFINITION] : [])]
    .map(({ name, description, parameters }) => ({ name, description, parameters }));
}
