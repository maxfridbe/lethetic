import { executionTimeout } from "./lifecycle.js";
import { emptyWorkspace, parseWorkspace } from "./workspace.js";
import type { Workspace } from "./workspace.js";
export type { Workspace } from "./workspace.js";
import { relayEndpoint, relayToken } from "./network-route.js";
import type { DotnetRelay } from "./network-route.js";
import type { Tool } from "./sdk.js";
import type { WireRequest, WireResponse, WireSite } from "./dotnet.js";

export const PYTHON_TOOL_DEFINITION = {
    name: "python",
    description: "Run Python in a persistent Pyodide browser worker. Use print() for output. " +
      "Python execution and virtual file reads/writes are authorized. Outbound networking requires this session's Network access permission; use await pyodide.http.pyfetch. The configured HTTP relay is used without direct fallback. Do not ask for per-call approval when enabled. Variables persist while the worker runs. Write session files under /home/pyodide (the initial working directory); files there sync with Files after each call and restore after reload or stop. Create CSV, HTML, SVG or PNG outputs for the user to preview or download in Files. Other directories are temporary. No shell or native files are available. " +
      "With Interactive sites enabled, lethetic_wire.listen(port, handler=async_handler, web_root='site') or listen(port, wsgi_app) hosts a site in the isolated Sites preview; return immediately. Request handlers return lethetic_wire.Response; request-created files sync to the session. Users can Run saved .py files from Files. " +
      "Packages bundled with Pyodide are loaded from imports. Stopping resets variables and discards unfinished file changes. Workspace limits: 100 files, 256 folders, 16 MiB per file, 64 MiB total; no symlinks.",
    parameters: { type: "object", properties: { code: { type: "string", description: "Python source" } },
      required: ["code"], additionalProperties: false },
};

export const PYODIDE_INDEX_URL = "https://cdn.jsdelivr.net/pyodide/v0.29.3/full/";

export interface PythonTool extends Tool {
  initialize(signal: AbortSignal): Promise<void>;
  execute(arguments_: Record<string, unknown>, signal: AbortSignal): Promise<string>;
  /** Terminate Python; the last completed workspace checkpoint survives. */
  sites(): WireSite[];
  requestSite(request: WireRequest, signal: AbortSignal): Promise<WireResponse>;
  reset(): void;
  snapshotWorkspace(): Workspace;
  setWorkspace(workspace: Workspace): void;
}

/** A persistent Python tool in a dedicated worker. Stop/timeout resets the worker.
 * Network access defaults off; this is not an untrusted-code security boundary.
 * No model API keys, host tools, or conversation history are passed to the worker.
 * The worker receives any configured relay credential.
 */
export function createPythonTool(options: {
  siteAccess?: boolean;
  networkAccess?: boolean;
  relay?: DotnetRelay | null;
  indexURL?: string;
  timeoutMs?: number;
  workerFactory?: () => Worker;
  workspace?: Workspace;
  onWorkspace?: (workspace: Workspace) => void;
} = {}): PythonTool {
  if (options.siteAccess !== undefined && typeof options.siteAccess !== "boolean") throw new Error("Interactive sites permission must be a boolean.");
  if (options.networkAccess !== undefined && typeof options.networkAccess !== "boolean") throw new Error("Network access must be a boolean.");
  const networkAccess = options.networkAccess ?? false;
  const relay = options.relay ? { endpoint: relayEndpoint(options.relay.endpoint), token: relayToken(options.relay.token) } : null;
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
  let sites: WireSite[] = [];
  type Reply = { output: string; response?: WireResponse; warning: string };
  const terminate = (): void => { worker?.terminate(); worker = null; ready = false; sites = []; };
  const reset = (): void => {
    pending?.(new Error("Python worker was reset; variables were cleared; last completed files are kept."));
    terminate();
  };
  function perform(code: string, signal: AbortSignal, siteRequest?: WireRequest): Promise<Reply> {
    if (pending) return Promise.reject(new Error("Python is already running."));
    signal.throwIfAborted();
    worker ??= options.workerFactory?.() ?? new Worker(new URL("./python-worker.js?protocol=1", import.meta.url),
      { type: "module", name: "lethetic-python" });
    const activeWorker = worker;
    const id = ++sequence;
    return new Promise<Reply>((resolve, reject) => {
      const cleanup = (): void => {
        cancelDeadline();
        signal.removeEventListener("abort", abort);
        activeWorker.removeEventListener("message", message);
        activeWorker.removeEventListener("error", error);
        pending = null;
      };
      const fail = (reason: Error): void => { cleanup(); terminate(); reject(reason); };
      const abort = (): void => fail(new DOMException("Python stopped; session reset.", "AbortError"));
      const error = (): void => fail(new Error("Python worker could not load. Check Pyodide access and CSP."));
      const run = (): void => {
        try { activeWorker.postMessage({ id, protocol: 1, code, indexURL: indexURL.href, workspace: checkpoint, siteAccess: options.siteAccess ?? false, networkAccess, relay, ...(siteRequest ? { siteRequest } : {}) }); }
        catch { fail(new Error("Could not start Python execution.")); }
      };
      const message = (event: MessageEvent<unknown>): void => {
        const data = event.data as { id?: unknown; protocol?: unknown; type?: unknown; output?: unknown; workspace?: unknown; workspaceError?: unknown; capabilities?: string[]; sites?: WireSite[]; response?: WireResponse } | null;
        if (!data || data.id !== id) return;
        if (data.protocol !== 1) {
          fail(new Error("Python worker is out of date. Reload this page to update it; saved session files are kept."));
          return;
        }
        if (!ready) {
          if (data.type !== "ready") return;
          if (!data.capabilities?.includes("sites") || !data.capabilities.includes("network-v1")) {
            fail(new Error("Python worker is out of date. Reload to update Python network permission, file and site support; saved files are kept.")); return;
          }
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
        sites = Array.isArray(data.sites) ? data.sites : [];
        cleanup();
        resolve({ output: data.output, response: data.response, warning });
      };
      const cancelDeadline = executionTimeout(() => fail(new Error("Python timed out; session reset.")), (siteRequest ? 20_000 : timeoutMs));
      pending = fail;
      signal.addEventListener("abort", abort, { once: true });
      activeWorker.addEventListener("message", message);
      activeWorker.addEventListener("error", error);
      try { if (ready) run(); else activeWorker.postMessage({ type: "hello", id, protocol: 1 }); }
      catch { fail(new Error("Could not start Python execution.")); }
    });
  }
  return {
    ...PYTHON_TOOL_DEFINITION,
    async initialize(signal): Promise<void> {
      const reply = await perform("", signal);
      if (reply.warning || reply.output.trim() !== "Python completed without output.") { reset(); throw new Error(`${reply.output}\n${reply.warning}`); }
    },
    reset,
    sites: () => structuredClone(sites),
    snapshotWorkspace: () => structuredClone(checkpoint),
    setWorkspace(workspace): void {
      if (pending) throw new Error("Stop Python before editing its workspace.");
      checkpoint = parseWorkspace(workspace);
    },
    async requestSite(request, signal): Promise<WireResponse> {
      if (!options.siteAccess) throw new Error("Interactive sites are disabled for this session.");
      if (!worker) throw new Error("Run the site's Python program first.");
      const reply = await perform("", signal, request);
      if (reply.warning) throw new Error(`Workspace not saved: ${reply.warning}`);
      if (!reply.response) throw new Error(reply.output || "Python site did not return a response.");
      return reply.response;
    },
    async execute(arguments_, signal): Promise<string> {
      if (typeof arguments_["code"] !== "string" || !arguments_["code"].trim()) throw new Error("Python requires nonempty code.");
      if (arguments_["code"].length > 256_000) throw new Error("Python code is too large.");
      const reply = await perform(arguments_["code"], signal);
      return reply.output + (reply.warning ? `\n[Workspace not saved: ${reply.warning}]\n` : "");
    },
  };
}
