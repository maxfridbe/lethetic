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

export const DOTNET_INSTRUCTIONS = `dotnet-wasm mode
The user expects you to write, compile and run C# programs to solve their problems. Your dotnet tool runs .NET 11 and Roslyn inside a browser worker. Use System.IO to inspect uploaded files and create reusable .cs programs and results under /home/dotnetdide (the initial working directory). These files sync into this session after each tool call and the user can preview or download them in Files. Python and .NET modes share relative session file paths.

Default script usings: System, System.IO, System.Linq, System.Collections.Generic, System.Threading.Tasks. Use using (...) blocks for disposable script resources; top-level using var declarations require a regular compiled .cs program. Add explicit using directives for other .NET namespaces, e.g. System.Text.Json, System.Net.Http and System.IO.Compression. Variables persist until stop, reload, mode/permission changes or session switch; files survive. Session limits: 100 files, 256 folders, 1 MiB per file, 2 MiB total. Use Console.WriteLine to report output and inspect compiler diagnostics before claiming success.

Write File.WriteAllText("Program.cs", source), then await Dotnet.Run("Program.cs"). Dotnet.Build() builds workspace sources; Dotnet.Build("app.csproj") and await Dotnet.Run("app.csproj") support a limited SDK-style project subset, not full MSBuild. Inspect the returned Success, Output or Build.Output and ExitCode. No NuGet restore, package/project references, native binaries or shell. Script variables and script using directives are not inherited by separately compiled programs.

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
