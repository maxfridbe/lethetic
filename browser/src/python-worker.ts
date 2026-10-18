// Kept separate from the application: only Python source and a runtime URL cross the boundary.
interface Pyodide {
  runPythonAsync(code: string): Promise<unknown>;
  loadPackagesFromImports(code: string): Promise<void>;
  setStdout(options: { batched: (text: string) => void }): void;
  setStderr(options: { batched: (text: string) => void }): void;
  setStdin(options: { stdin: () => undefined }): void;
}
interface PyodideModule { loadPyodide(options: { indexURL: string }): Promise<Pyodide> }
const scope = globalThis as unknown as {
  onmessage: ((event: MessageEvent<{ id: number; code: string; indexURL: string }>) => void) | null;
  postMessage(data: unknown): void;
};
let python: Promise<Pyodide> | null = null;
let busy = false;

scope.onmessage = (event): void => { void execute(event.data); };

async function execute({ id, code, indexURL }: { id: number; code: string; indexURL: string }): Promise<void> {
  if (busy) { scope.postMessage({ id, output: "Python is already running." }); return; }
  busy = true;
  let output = "";
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
    const runtime = await python;
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
    busy = false;
    scope.postMessage({ id, output: (output || "Python completed without output.\n") +
      (truncated ? "\n[Output truncated at 64 KiB]\n" : "") });
  }
}

export {};
