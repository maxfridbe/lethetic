// Runtime settings and relay credentials belong to this session; no model keys enter Python.
import { installPythonNetworkPolicy, pythonPackageDownloads, PYTHON_NETWORK_DENIED } from "./python-network.js";
import type { DotnetRelay } from "./network-route.js";
import { captureWorkspace, restoreWorkspace } from "./python-filesystem.js";
import type { PythonFS } from "./python-filesystem.js";
import { emptyWorkspace, parseWorkspace } from "./workspace.js";
import type { Workspace } from "./workspace.js";
import type { WireRequest, WireResponse, WireSite } from "./dotnet.js";
interface PythonRequest { id: number; code: string; indexURL: string; workspace?: Workspace; protocol?: number; type?: string; siteAccess?: boolean; networkAccess?: boolean; relay?: DotnetRelay | null; siteRequest?: WireRequest }
interface Pyodide {
  FS: PythonFS;
  runPythonAsync(code: string): Promise<unknown>;
  loadPackagesFromImports(code: string): Promise<void>;
  setStdout(options: { batched: (text: string) => void }): void;
  setStderr(options: { batched: (text: string) => void }): void;
  setStdin(options: { stdin: () => undefined }): void;
}
interface PyodideModule { loadPyodide(options: { indexURL: string; lockFileContents: string; packageBaseUrl: string }): Promise<Pyodide> }
const scope = globalThis as unknown as {
  onmessage: ((event: MessageEvent<PythonRequest>) => void) | null;
  postMessage(data: unknown): void;
};
let python: Promise<Pyodide> | null = null;
let busy = false;
let installedWorkspace = "";
let networkDenied = false;

scope.onmessage = (event): void => {
  const request = event.data;
  if (request.type === "hello") {
    scope.postMessage({ id: request.id, type: "ready", protocol: 1, capabilities: ["sites", "network-v1"] });
    return;
  }
  void execute(request);
};

async function execute({ id, code, indexURL, workspace, protocol, siteAccess = false, networkAccess = false, relay = null, siteRequest }: PythonRequest): Promise<void> {
  if (busy) { scope.postMessage({ id, protocol: 1, output: "Python is already running." }); return; }
  busy = true;
  networkDenied = false;
  let output = "";
  let runtime: Pyodide | undefined;
  let restored = false;
  let snapshot: Workspace | undefined;
  let workspaceError: string | undefined;
  let truncated = false;
  let sites: WireSite[] = [], response: WireResponse | undefined;
  const append = (text: string): void => {
    const line = text + "\n";
    const remaining = Math.max(0, 65_536 - output.length);
    output += line.slice(0, remaining);
    if (line.length > remaining) truncated = true;
  };
  try {
    if (protocol !== undefined && protocol !== 1) throw new Error("Unsupported Python worker protocol. Reload this page.");
    // Older open tabs omit workspace; preserve their existing files between calls.
    const legacy = protocol === undefined && workspace === undefined;
    const incoming = legacy ? null : parseWorkspace(workspace);
    python ??= (async () => {
      const lockResponse = await fetch(new URL("pyodide-lock.json", indexURL), { credentials: "omit", redirect: "error" });
      if (!lockResponse.ok) throw new Error("Could not load Pyodide package manifest. Reload this page.");
      const lockFileContents = await lockResponse.text();
      const downloads = pythonPackageDownloads(JSON.parse(lockFileContents), indexURL);
      for (const file of ["pyodide.asm.wasm", "python_stdlib.zip"]) downloads.add(new URL(file, indexURL).href);
      const source = await fetch(new URL("../../python/lethetic_wire.py", import.meta.url));
      if (!source.ok) throw new Error("Could not load Python site support. Reload this page.");
      const sourceText = await source.text();
      const relayModule: Pick<Parameters<typeof installPythonNetworkPolicy>[0], "relayFetch"> = relay
        ? await import(new URL("../../runtimes/dotnetdide/relay-fetch.js", import.meta.url).href) : {};
      installPythonNetworkPolicy({ networkAccess, relay, downloads, ...relayModule, onDenied: () => { networkDenied = true; } });
      const module: PyodideModule = await import(new URL("pyodide.mjs", indexURL).href);
      const loaded = await module.loadPyodide({ indexURL, lockFileContents, packageBaseUrl: indexURL });
      await loaded.runPythonAsync(`# lethetic-wire-bootstrap
import importlib.util as _wire_util, sys as _wire_sys
_wire_module = _wire_util.module_from_spec(_wire_util.spec_from_loader("lethetic_wire", loader=None))
_wire_sys.modules["lethetic_wire"] = _wire_module
exec(${JSON.stringify(sourceText)}, _wire_module.__dict__)`);
      return loaded;
    })();
    runtime = await python;
    if (incoming || !installedWorkspace) {
      const next = incoming ?? emptyWorkspace();
      const serialized = JSON.stringify(next);
      if (serialized !== installedWorkspace) {
        restoreWorkspace(runtime.FS, next); installedWorkspace = serialized;
      }
    }
    restored = true;
    runtime.setStdout({ batched: append });
    runtime.setStderr({ batched: append });
    runtime.setStdin({ stdin: () => undefined });
    await runtime.runPythonAsync(`# lethetic-wire-configure\n__import__("lethetic_wire")._configure(${siteAccess ? "True" : "False"})`);
    if (!siteRequest) await runtime.loadPackagesFromImports(code);
    const result = await runtime.runPythonAsync(siteRequest
      ? `# lethetic-wire-request\nawait __import__("lethetic_wire")._dispatch_json(${JSON.stringify(JSON.stringify(siteRequest))})`
      : code);
    if (siteRequest) response = JSON.parse(String(result)) as WireResponse;
    try { if (!siteRequest && result !== undefined && result !== null) append(String(result)); }
    finally {
      if (typeof result === "object" && result !== null && "destroy" in result &&
          typeof result.destroy === "function") result.destroy();
    }
  } catch (error) { append(String(error)); }
  finally {
    // Some libraries replace fetch's error (micropip reports "package not found").
    // Keep the real permission failure visible even if Python caught it.
    if (networkDenied && !output.includes(PYTHON_NETWORK_DENIED)) append(PYTHON_NETWORK_DENIED);
    if (runtime && restored) {
      try { sites = JSON.parse(String(await runtime.runPythonAsync('# lethetic-wire-sites\n__import__("lethetic_wire")._sites_json()'))) as WireSite[]; }
      catch (error) { append(`Could not list Python sites: ${String(error)}`); }
      try { snapshot = captureWorkspace(runtime.FS); installedWorkspace = JSON.stringify(snapshot); }
      catch (error) { workspaceError = String(error); }
    } else workspaceError = "Python workspace could not be restored; no files were changed in the saved session.";
    busy = false;
    scope.postMessage({ id, protocol: 1, workspace: snapshot, workspaceError, sites, response, output: (output || "Python completed without output.\n") +
      (truncated ? "\n[Output truncated at 64 KiB]\n" : "") });
  }
}

export {};
