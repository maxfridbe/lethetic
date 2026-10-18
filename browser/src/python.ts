import { emptyWorkspace, parseWorkspace } from "./workspace.js";
import type { Workspace } from "./workspace.js";
export type { Workspace } from "./workspace.js";
import type { Tool } from "./sdk.js";

export const PYTHON_TOOL_DEFINITION = {
    name: "python",
    description: "Run Python in a persistent Pyodide browser worker. Use print() for output. " +
      "Python execution, virtual file reads/writes, and browser-supported networking are already authorized; do not ask for per-call approval. Variables persist while the worker runs. Write session files under /home/pyodide (the initial working directory); files there sync with Files after each call and restore after reload or stop. Create CSV, HTML, SVG or PNG outputs for the user to preview or download in Files. Other directories are temporary. No shell or native files are available. " +
      "Packages bundled with Pyodide are loaded from imports. Stopping resets variables and discards unfinished file changes. Workspace limits: 100 files, 256 folders, 1 MiB per file, 2 MiB total; no symlinks.",
    parameters: { type: "object", properties: { code: { type: "string", description: "Python source" } },
      required: ["code"], additionalProperties: false },
};

export const PYODIDE_INDEX_URL = "https://cdn.jsdelivr.net/pyodide/v0.29.3/full/";

export interface PythonTool extends Tool {
  /** Terminate Python; the last completed workspace checkpoint survives. */
  reset(): void;
  snapshotWorkspace(): Workspace;
  setWorkspace(workspace: Workspace): void;
}

/** A persistent Python tool in a dedicated worker. Stop/timeout resets the worker.
 * Python has browser network access; this is not an untrusted-code security boundary.
 * No API keys, host tools, or conversation history are passed to the worker.
 */
export function createPythonTool(options: {
  indexURL?: string;
  timeoutMs?: number;
  workerFactory?: () => Worker;
  workspace?: Workspace;
  onWorkspace?: (workspace: Workspace) => void;
} = {}): PythonTool {
  const indexURL = new URL(options.indexURL ?? PYODIDE_INDEX_URL);
  if (!indexURL.pathname.endsWith("/")) throw new Error("Pyodide indexURL must end in /.");
  if (indexURL.protocol !== "https:" && !(indexURL.protocol === "http:" &&
      ["localhost", "127.0.0.1", "[::1]"].includes(indexURL.hostname))) {
    throw new Error("Pyodide needs an HTTPS URL or localhost.");
  }
  const timeoutMs = options.timeoutMs ?? 120_000;
  if (!Number.isFinite(timeoutMs) || timeoutMs < 1 || timeoutMs > 3_600_000) {
    throw new Error("Python timeout must be between 1 ms and one hour.");
  }
  let checkpoint = parseWorkspace(options.workspace ?? emptyWorkspace());
  let worker: Worker | null = null;
  let pending: ((reason: Error) => void) | null = null;
  let sequence = 0;
  let ready = false;
  const terminate = (): void => { worker?.terminate(); worker = null; ready = false; };
  const reset = (): void => {
    pending?.(new Error("Python worker was reset; variables were cleared; last completed files are kept."));
    terminate();
  };
  return {
    ...PYTHON_TOOL_DEFINITION,
    reset,
    snapshotWorkspace: () => structuredClone(checkpoint),
    setWorkspace(workspace): void {
      if (pending) throw new Error("Stop Python before editing its workspace.");
      checkpoint = parseWorkspace(workspace);
    },
    execute(arguments_, signal): Promise<string> {
      if (typeof arguments_["code"] !== "string" || !arguments_["code"].trim()) {
        return Promise.reject(new Error("Python requires nonempty code."));
      }
      if (arguments_["code"].length > 256_000) return Promise.reject(new Error("Python code is too large."));
      if (pending) return Promise.reject(new Error("Python is already running."));
      signal.throwIfAborted();
      worker ??= options.workerFactory?.() ?? new Worker(new URL("./python-worker.js?protocol=1", import.meta.url),
        { type: "module", name: "lethetic-python" });
      const activeWorker = worker;
      const id = ++sequence;
      return new Promise<string>((resolve, reject) => {
        const cleanup = (): void => {
          clearTimeout(timer);
          signal.removeEventListener("abort", abort);
          activeWorker.removeEventListener("message", message);
          activeWorker.removeEventListener("error", error);
          pending = null;
        };
        const fail = (reason: Error): void => { cleanup(); terminate(); reject(reason); };
        const abort = (): void => fail(new DOMException("Python stopped; session reset.", "AbortError"));
        const error = (): void => fail(new Error("Python worker could not load. Check Pyodide access and CSP."));
        const run = (): void => {
          try { activeWorker.postMessage({ id, protocol: 1, code: arguments_["code"], indexURL: indexURL.href, workspace: checkpoint }); }
          catch { fail(new Error("Could not start Python execution.")); }
        };
        const message = (event: MessageEvent<unknown>): void => {
          const data = event.data as { id?: unknown; protocol?: unknown; type?: unknown; output?: unknown; workspace?: unknown; workspaceError?: unknown } | null;
          if (!data || data.id !== id) return;
          if (data.protocol !== 1) {
            fail(new Error("Python worker is out of date. Reload this page to update it; saved session files are kept."));
            return;
          }
          if (!ready) {
            if (data.type !== "ready") return;
            ready = true; run(); return;
          }
          if (typeof data.output !== "string") return;
          let warning = typeof data.workspaceError === "string" ? data.workspaceError : "";
          if (data.workspace !== undefined) {
            try {
              checkpoint = parseWorkspace(data.workspace);
              options.onWorkspace?.(structuredClone(checkpoint));
            } catch (error) { warning = String(error); }
          } else if (!warning) warning = "Python did not return a workspace checkpoint.";
          cleanup();
          resolve(data.output + (warning ? `\n[Workspace not saved: ${warning}]\n` : ""));
        };
        const timer = setTimeout(() => fail(new Error("Python timed out; session reset.")), timeoutMs);
        pending = fail;
        signal.addEventListener("abort", abort, { once: true });
        activeWorker.addEventListener("message", message);
        activeWorker.addEventListener("error", error);
        try { if (ready) run(); else activeWorker.postMessage({ type: "hello", id, protocol: 1 }); }
        catch { fail(new Error("Could not start Python execution.")); }
      });
    },
  };
}
