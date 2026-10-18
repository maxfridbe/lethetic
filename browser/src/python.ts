import type { Tool } from "./sdk.js";

export const PYODIDE_INDEX_URL = "https://cdn.jsdelivr.net/pyodide/v0.29.3/full/";

export interface PythonTool extends Tool {
  /** Terminate the worker and discard Python globals and its virtual filesystem. */
  reset(): void;
}

/** A persistent Python tool in a dedicated worker. Stop/timeout resets the worker.
 * Python has browser network access; this is not an untrusted-code security boundary.
 * No API keys, host tools, or conversation history are passed to the worker.
 */
export function createPythonTool(options: {
  indexURL?: string;
  timeoutMs?: number;
  workerFactory?: () => Worker;
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
  let worker: Worker | null = null;
  let pending: ((reason: Error) => void) | null = null;
  let sequence = 0;
  const terminate = (): void => { worker?.terminate(); worker = null; };
  const reset = (): void => {
    pending?.(new Error("Python worker was reset; variables and files were cleared."));
    terminate();
  };
  return {
    name: "python",
    description: "Run Python in a persistent Pyodide browser worker. Use print() for output. " +
      "Variables and virtual files persist within this chat. No shell or native files are available. " +
      "Packages bundled with Pyodide are loaded from imports. Stopping clears the Python session.",
    parameters: { type: "object", properties: { code: { type: "string", description: "Python source" } },
      required: ["code"], additionalProperties: false },
    reset,
    execute(arguments_, signal): Promise<string> {
      if (typeof arguments_["code"] !== "string" || !arguments_["code"].trim()) {
        return Promise.reject(new Error("Python requires nonempty code."));
      }
      if (arguments_["code"].length > 256_000) return Promise.reject(new Error("Python code is too large."));
      if (pending) return Promise.reject(new Error("Python is already running."));
      signal.throwIfAborted();
      worker ??= options.workerFactory?.() ?? new Worker(new URL("./python-worker.js", import.meta.url),
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
        const message = (event: MessageEvent<unknown>): void => {
          const data = event.data as { id?: unknown; output?: unknown } | null;
          if (!data || data.id !== id || typeof data.output !== "string") return;
          cleanup();
          resolve(data.output);
        };
        const timer = setTimeout(() => fail(new Error("Python timed out; session reset.")), timeoutMs);
        pending = fail;
        signal.addEventListener("abort", abort, { once: true });
        activeWorker.addEventListener("message", message);
        activeWorker.addEventListener("error", error);
        try { activeWorker.postMessage({ id, code: arguments_["code"], indexURL: indexURL.href }); }
        catch { fail(new Error("Could not start Python execution.")); }
      });
    },
  };
}
