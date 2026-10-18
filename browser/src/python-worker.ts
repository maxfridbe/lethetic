// Only source, a runtime URL and this session's workspace cross the worker boundary.
import { captureWorkspace, restoreWorkspace } from "./python-filesystem.js";
import type { PythonFS } from "./python-filesystem.js";
import { parseWorkspace } from "./workspace.js";
import type { Workspace } from "./workspace.js";
interface PythonRequest { id: number; code: string; indexURL: string; workspace: Workspace }
interface Pyodide {
  FS: PythonFS;
  runPythonAsync(code: string): Promise<unknown>;
  loadPackagesFromImports(code: string): Promise<void>;
  setStdout(options: { batched: (text: string) => void }): void;
  setStderr(options: { batched: (text: string) => void }): void;
  setStdin(options: { stdin: () => undefined }): void;
}
interface PyodideModule { loadPyodide(options: { indexURL: string }): Promise<Pyodide> }
const scope = globalThis as unknown as {
  onmessage: ((event: MessageEvent<PythonRequest>) => void) | null;
  postMessage(data: unknown): void;
};
let python: Promise<Pyodide> | null = null;
let busy = false;
let installedWorkspace = "";

scope.onmessage = (event): void => { void execute(event.data); };

async function execute({ id, code, indexURL, workspace }: PythonRequest): Promise<void> {
  if (busy) { scope.postMessage({ id, output: "Python is already running." }); return; }
  busy = true;
  let output = "";
  let runtime: Pyodide | undefined;
  let restored = false;
  let snapshot: Workspace | undefined;
  let workspaceError: string | undefined;
  let truncated = false;
  const append = (text: string): void => {
    const line = text + "\n";
    const remaining = Math.max(0, 65_536 - output.length);
    output += line.slice(0, remaining);
    if (line.length > remaining) truncated = true;
  };
  try {
    python ??= (async () => {
      const module: PyodideModule = await import(new URL("pyodide.mjs", indexURL).href);
      return module.loadPyodide({ indexURL });
    })().catch((error: unknown) => { python = null; throw error; });
    runtime = await python;
    const incoming = parseWorkspace(workspace);
    const serialized = JSON.stringify(incoming);
    if (serialized !== installedWorkspace) {
      restoreWorkspace(runtime.FS, incoming); installedWorkspace = serialized;
    }
    restored = true;
    runtime.setStdout({ batched: append });
    runtime.setStderr({ batched: append });
    runtime.setStdin({ stdin: () => undefined });
    await runtime.loadPackagesFromImports(code);
    const result = await runtime.runPythonAsync(code);
    try { if (result !== undefined && result !== null) append(String(result)); }
    finally {
      if (typeof result === "object" && result !== null && "destroy" in result &&
          typeof result.destroy === "function") result.destroy();
    }
  } catch (error) { append(String(error)); }
  finally {
    if (runtime && restored) {
      try { snapshot = captureWorkspace(runtime.FS); installedWorkspace = JSON.stringify(snapshot); }
      catch (error) { workspaceError = String(error); }
    } else workspaceError = "Python workspace could not be restored; no files were changed in the saved session.";
    busy = false;
    scope.postMessage({ id, workspace: snapshot, workspaceError, output: (output || "Python completed without output.\n") +
      (truncated ? "\n[Output truncated at 64 KiB]\n" : "") });
  }
}

export {};
