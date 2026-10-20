import type { DotnetRelay } from "./network-route.js";
import { emptyWorkspace, parseWorkspace } from "./workspace.js";
import type { Workspace } from "./workspace.js";
import type { Tool } from "./sdk.js";

export const DOTNET_TOOL_DEFINITION = {
  name: "dotnet",
  description: "Run C# in a persistent .NET 11 WASM worker with Roslyn compilation. " +
    "Use Console.WriteLine for output. System.IO files under /home/dotnetdide sync with this session's Files pane after every call. " +
    "Write reusable .cs programs and run them with await Dotnet.Run(\"file.cs\"); Dotnet.Build() compiles workspace files or a supported .csproj. " +
    "Create parent directories before writing; await Run and check run.Success, run.Build.Output, run.Error and run.ExitCode. " +
    "A .cs path builds only that file; a directory or .csproj builds multiple files. Builds emit bin/*.dll and bin/*.pdb. " +
    "Default usings: System, System.IO, System.Linq, System.Collections.Generic, System.Threading.Tasks. " +
    "Add explicit using directives for other available .NET libraries. Dotnetdide.Net provides async HTTP when the host enables Network access; browser CORS applies. " +
    "No shell, native OS or full MSBuild/NuGet project restore. StbImageSharp/StbImageWriteSharp, NAudio.Core and Concentus/Oggfile are bundled for images, WAV and Ogg Opus. Await DotnetDide.Nuget.Load(name, optionalVersion) in a separate call before using extra managed packages; requires NuGet package access. " +
    "DotnetDide.Wire hosts ASP.NET middleware and HttpContext routes on virtual ports when Interactive sites is enabled; await Wire.ListenAsync and return, then open Sites. " +
    "Variables reset on stop, reload or mode switch; completed files survive. " +
    "Workspace limits: 100 files, 256 folders, 1 MiB per file, 2 MiB total; no symlinks.",
  parameters: { type: "object", properties: { code: { type: "string", description: "C# script source, including top-level await" } },
    required: ["code"], additionalProperties: false },
};

export interface DotnetResult {
  success: boolean; stdout: string; stderr: string; value: string | null;
  diagnostics: { id: string; message: string; line: number; column: number; filePath?: string | null }[];
  error: { type: string; message: string } | null;
}
export interface WireSite { port: number; webRoot: string | null }
export interface WireRequest { port: number; method: string; path: string; headers?: Record<string, string[]>; body?: string }
export interface WireResponse { status: number; headers: Record<string, string[]>; body: string; error?: string | null }
export interface DotnetRuntime {
  runCSharpAsync(code: string, options: { signal: AbortSignal }): Promise<DotnetResult>;
  listSites?(): Promise<WireSite[]>;
  requestSite?(request: WireRequest, options: { signal: AbortSignal }): Promise<WireResponse>;
  importWorkspace(workspace: Workspace): Promise<void>;
  exportWorkspace(): Promise<unknown>;
  terminate(): void;
}
export type DotnetLoader = (options: { signal: AbortSignal; networkAccess: boolean; nugetAccess: boolean; siteAccess: boolean; relay?: DotnetRelay | null }) => Promise<DotnetRuntime>;
export interface DotnetTool extends Tool {
  sites(): WireSite[];
  requestSite(request: WireRequest, signal: AbortSignal): Promise<WireResponse>;
  reset(): void;
  snapshotWorkspace(): Workspace;
  setWorkspace(workspace: Workspace): void;
}
const loadRuntime: DotnetLoader = async options => {
  // The build downloads a checksum-pinned GitHub release, served from this origin.
  const url = new URL("../../runtimes/dotnetdide/dotnetdide.js", import.meta.url);
  const sdk = await import(url.href) as { loadDotnetdide: DotnetLoader };
  options.signal.throwIfAborted();
  return sdk.loadDotnetdide(options);
};

export function createDotnetTool(options: {
  networkAccess?: boolean; nugetAccess?: boolean; siteAccess?: boolean; relay?: DotnetRelay | null; workspace?: Workspace; timeoutMs?: number;
  onWorkspace?: (workspace: Workspace) => void; loadRuntime?: DotnetLoader;
} = {}): DotnetTool {
  let checkpoint = parseWorkspace(options.workspace ?? emptyWorkspace());
  let runtime: DotnetRuntime | null = null, installed = "";
  let operation: AbortController | null = null;
  const timeout = options.timeoutMs ?? 120_000;
  if (!Number.isFinite(timeout) || timeout < 1 || timeout > 3_600_000) throw new Error(".NET timeout must be between 1 ms and one hour.");
  let sites: WireSite[] = [];
  const terminate = (): void => { runtime?.terminate(); runtime = null; installed = ""; sites = []; };
  async function perform<T>(signal: AbortSignal, action: (runtime: DotnetRuntime, signal: AbortSignal) => Promise<T>, requestTimeout = timeout): Promise<{ result: T; warning: string }> {
    if (operation) throw new Error(".NET is already running.");
    signal.throwIfAborted();
    const controller = new AbortController(); operation = controller;
    const abort = (): void => controller.abort(signal.reason);
    const cancel = (): void => terminate();
    signal.addEventListener("abort", abort, { once: true });
    controller.signal.addEventListener("abort", cancel, { once: true });
    const timer = setTimeout(() => controller.abort(new Error(".NET timed out; completed session files are kept.")), requestTimeout);
    try {
      runtime ??= await (options.loadRuntime ?? loadRuntime)({ signal: controller.signal, networkAccess: options.networkAccess ?? false,
        nugetAccess: options.nugetAccess ?? false, siteAccess: options.siteAccess ?? false, ...(options.relay ? { relay: options.relay } : {}) });
      controller.signal.throwIfAborted();
      const current = JSON.stringify(checkpoint);
      if (installed !== current) { await runtime.importWorkspace(checkpoint); installed = current; }
      const result = await action(runtime, controller.signal);
      controller.signal.throwIfAborted();
      sites = await runtime.listSites?.() ?? [];
      let warning = "";
      try {
        const snapshot = parseWorkspace(await runtime.exportWorkspace());
        controller.signal.throwIfAborted();
        checkpoint = snapshot; installed = JSON.stringify(snapshot);
        options.onWorkspace?.(structuredClone(snapshot));
      } catch (error) { installed = ""; warning = `\n[Workspace not saved: ${String(error)}]`; }
      controller.signal.throwIfAborted();
      return { result, warning };
    } catch (error) { terminate(); throw error; }
    finally { clearTimeout(timer); signal.removeEventListener("abort", abort); controller.signal.removeEventListener("abort", cancel); operation = null; }
  }
  return {
    ...DOTNET_TOOL_DEFINITION,
    sites: () => structuredClone(sites),
    async requestSite(request, signal): Promise<WireResponse> {
      if (!options.siteAccess) throw new Error("Interactive sites are disabled for this session.");
      if (!runtime) throw new Error("Run the site's C# program first.");
      const { result, warning } = await perform(signal, async (worker, requestSignal) => {
        if (!worker.requestSite) throw new Error("This runtime does not support interactive sites.");
        return worker.requestSite(request, { signal: requestSignal });
      }, 20_000);
      if (warning) throw new Error(warning.trim());
      return result;
    },
    reset(): void { operation?.abort(new Error(".NET worker reset; completed session files are kept.")); terminate(); },
    snapshotWorkspace: () => structuredClone(checkpoint),
    setWorkspace(workspace): void {
      if (operation) throw new Error("Stop .NET before editing its workspace.");
      checkpoint = parseWorkspace(workspace);
    },
    async execute(args, signal): Promise<string> {
      const code = args["code"];
      if (typeof code !== "string" || !code.trim() || code.length > 256_000) throw new Error(".NET requires nonempty C# code up to 256,000 characters.");
      const { result, warning } = await perform(signal, (worker, signal) => worker.runCSharpAsync(code, { signal }));
      const output = [result.stdout, result.stderr, result.value,
        ...result.diagnostics.map(d => `${d.filePath ?? "submission.csx"}(${d.line},${d.column}): ${d.id}: ${d.message}`),
        result.error ? `${result.error.type}: ${result.error.message}` : ""].filter(Boolean).join("\n");
      return (output || (result.success ? ".NET completed without output." : ".NET compilation failed.")).slice(0, 65_536) +
        (output.length > 65_536 ? "\n[Output truncated at 64 KiB]" : "") + warning;
    },
  };
}
