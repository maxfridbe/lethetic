import { validateConnection } from "./connection.js";
import { readUsage } from "./usage.js";
import type { ProviderUsage } from "./usage.js";
export type { ProviderUsage } from "./usage.js";
import type { Connection } from "./connection.js";
export type { Connection } from "./connection.js";
export { createPythonTool } from "./python.js";
export type { PythonTool, Workspace } from "./python.js";

export interface ToolCall { id: string; name: string; arguments: Record<string, unknown> }
export interface Tool {
  name: string;
  description: string;
  parameters: Record<string, unknown>;
  /** The host is responsible for authorizing tool effects. Honor the AbortSignal. */
  execute(arguments_: Record<string, unknown>, signal: AbortSignal): Promise<string> | string;
}
export type RuntimeEvent =
  | { type: "request"; round: number }
  | { type: "response"; usage: ProviderUsage; durationMs: number }
  | { type: "assistant"; text: string }
  | { type: "tool_start"; call: ToolCall }
  | { type: "tool_end"; call: ToolCall; output: string };

export interface RuntimeOptions {
  connection: Connection;
  systemPrompt?: string;
  tools?: readonly Tool[];
  onEvent?: (event: RuntimeEvent) => void;
  /** Defaults to 12 model requests per user turn. */
  maxRounds?: number;
  /** Timeout for the entire turn, including tools; defaults to 180 seconds. */
  timeoutMs?: number;
  /** Optional injected fetch for an embedding host or tests. */
  fetch?: typeof globalThis.fetch;
  /** Optional bytes for hosts that load WASM themselves (including Node). */
  wasm?: BufferSource;
}

interface WasmRuntime {
  begin(prompt: string): void;
  request(provider: string, model: string): string;
  accept(provider: string, response: string): string;
  tool_result(id: string, output: string): void;
  rollback(): void;
  clear(): void;
  history(): string;
  restore(history: string): void;
  free(): void;
}
interface WasmModule {
  default(options?: { module_or_path: BufferSource }): Promise<unknown>;
  BrowserRuntime: new (system: string, tools: string) => WasmRuntime;
}

export const DEFAULT_PROMPT = "You are Lethetic, a thoughtful coding and analysis assistant. " +
  "Work carefully, explain your results clearly, and use only the tools provided by the host. " +
  "You run in a browser and have no native filesystem or shell access.";

// wasm-bindgen's async initializer can instantiate twice when first callers overlap.
// Share the initialization promise so every runtime uses the same WASM memory.
let moduleReady: Promise<WasmModule> | null = null;
function loadRuntimeModule(wasm?: BufferSource): Promise<WasmModule> {
  if (!moduleReady) moduleReady = (async () => {
    const moduleUrl = new URL("../wasm/lethetic_wasm.js", import.meta.url);
    const module: WasmModule = await import(moduleUrl.href);
    await module.default(wasm ? { module_or_path: wasm } : undefined);
    return module;
  })().catch(error => { moduleReady = null; throw error; });
  return moduleReady;
}

/** Load the real Rust/WASM runtime. No native Lethetic server is required. */
export async function createRuntime(options: RuntimeOptions): Promise<LetheticRuntime> {
  const connection = validateConnection(options.connection);
  const module = await loadRuntimeModule(options.wasm);
  const tools = options.tools ?? [];
  const definitions = tools.map(({ name, description, parameters }) => ({ name, description, parameters }));
  return new LetheticRuntime(new module.BrowserRuntime(options.systemPrompt ?? DEFAULT_PROMPT,
    JSON.stringify(definitions)), { ...options, connection });
}

/** One conversation. Concurrent sends are rejected; separate instances are independent. */
export class LetheticRuntime {
  #wasm: WasmRuntime;
  #options: RuntimeOptions;
  #active: AbortController | null = null;
  #disposed = false;

  constructor(wasm: WasmRuntime, options: RuntimeOptions) {
    this.#wasm = wasm;
    this.#options = options;
  }

  get busy(): boolean { return this.#active !== null; }

  /** Send a complete turn, including host tool calls, and return the final answer. */
  async send(prompt: string, signal?: AbortSignal): Promise<string> {
    if (this.#disposed) throw new Error("This runtime was disposed.");
    if (this.busy) throw new Error("A turn is already running.");
    const controller = new AbortController();
    const cancel = (): void => controller.abort(signal?.reason);
    signal?.addEventListener("abort", cancel, { once: true });
    if (signal?.aborted) cancel();
    const timeoutMs = this.#options.timeoutMs ?? 180_000;
    const maxRounds = this.#options.maxRounds ?? 12;
    if (!Number.isInteger(maxRounds) || maxRounds < 1 || maxRounds > 100 ||
        !Number.isFinite(timeoutMs) || timeoutMs < 1 || timeoutMs > 3_600_000) {
      signal?.removeEventListener("abort", cancel);
      throw new Error("Use 1–100 rounds and a timeout between 1 ms and one hour.");
    }
    const timer = setTimeout(() => controller.abort(new Error("Turn timed out.")), timeoutMs);
    this.#active = controller;
    try {
      controller.signal.throwIfAborted();
      this.#wasm.begin(prompt);
      const connection = this.#options.connection;
      for (let round = 1; round <= maxRounds; round++) {
        controller.signal.throwIfAborted();
        const body = this.#wasm.request(connection.provider, connection.model);
        this.#emit({ type: "request", round });
        const started = performance.now();
        const response = await abortable((this.#options.fetch ?? globalThis.fetch)(connection.endpoint, {
          method: "POST", mode: "cors", credentials: "omit", redirect: "error",
          referrerPolicy: "no-referrer", cache: "no-store", signal: controller.signal,
          headers: requestHeaders(connection), body,
        }), controller.signal);
        if (!response.ok) {
          await response.body?.cancel();
          throw new Error(`Model request failed (HTTP ${response.status}). Check your endpoint, model, and key.`);
        }
        const responseText = await readResponse(response, controller.signal);
        controller.signal.throwIfAborted();
        this.#emit({ type: "response", usage: readUsage(connection.provider, JSON.parse(responseText)),
          durationMs: performance.now() - started });
        const turn = JSON.parse(this.#wasm.accept(connection.provider, responseText)) as {
          text: string; calls: ToolCall[];
        };
        if (turn.text) this.#emit({ type: "assistant", text: turn.text });
        if (turn.calls.length === 0) return turn.text;
        for (const call of turn.calls) {
          controller.signal.throwIfAborted();
          const tool = this.#options.tools?.find((candidate) => candidate.name === call.name);
          if (!tool) throw new Error("The model requested an unregistered tool.");
          this.#emit({ type: "tool_start", call });
          const output = await abortable(Promise.resolve().then(() => {
            controller.signal.throwIfAborted();
            return tool.execute(call.arguments, controller.signal);
          }), controller.signal);
          controller.signal.throwIfAborted();
          if (typeof output !== "string") throw new Error("Tool results must be strings.");
          this.#wasm.tool_result(call.id, output);
          this.#emit({ type: "tool_end", call, output });
        }
      }
      throw new Error("Tool round limit reached. Start a new turn with a smaller task.");
    } catch (error) {
      this.#wasm.rollback();
      if (controller.signal.aborted) throw controller.signal.reason;
      if (error instanceof TypeError) {
        throw new Error("Could not reach the model. Check CORS, TLS, and the endpoint address.");
      }
      throw error instanceof Error ? error : new Error(String(error));
    } finally {
      clearTimeout(timer);
      signal?.removeEventListener("abort", cancel);
      this.#active = null;
      if (this.#disposed) this.#wasm.free();
    }
  }

  cancel(): void { this.#active?.abort(new DOMException("Turn stopped.", "AbortError")); }

  clear(): void {
    if (this.busy || this.#disposed) throw new Error("Stop the turn before clearing the conversation.");
    this.#wasm.clear();
  }

  history(): unknown[] {
    if (this.#disposed) throw new Error("This runtime was disposed.");
    return JSON.parse(this.#wasm.history()) as unknown[];
  }

  /** Restore completed turns without replaying host tools. */
  restore(history: unknown[]): void {
    if (this.busy || this.#disposed) throw new Error("Stop the turn before restoring history.");
    this.#wasm.restore(JSON.stringify(history));
  }

  dispose(): void {
    if (this.#disposed) return;
    this.#disposed = true;
    if (this.busy) this.cancel();
    else this.#wasm.free();
  }

  #emit(event: RuntimeEvent): void {
    // Observer failures must not invalidate an already accepted model response.
    try { this.#options.onEvent?.(event); } catch { /* Host observers are isolated. */ }
  }
}

function requestHeaders(connection: Connection): Record<string, string> {
  const headers: Record<string, string> = { "Content-Type": "application/json" };
  if (connection.provider === "anthropic") {
    headers["anthropic-version"] = "2023-06-01";
    headers["anthropic-dangerous-direct-browser-access"] = "true";
    if (connection.apiKey) headers["x-api-key"] = connection.apiKey;
  } else if (connection.apiKey) headers["Authorization"] = `Bearer ${connection.apiKey}`;
  return headers;
}

async function readResponse(response: Response, signal: AbortSignal): Promise<string> {
  const reader = response.body?.getReader();
  if (!reader) throw new Error("The model returned an empty response.");
  const decoder = new TextDecoder();
  let size = 0;
  let text = "";
  try {
    while (true) {
      signal.throwIfAborted();
      const { done, value } = await abortable(reader.read(), signal);
      if (done) return text + decoder.decode();
      size += value.byteLength;
      if (size > 4 * 1024 * 1024) throw new Error("Model response exceeds 4 MiB.");
      text += decoder.decode(value, { stream: true });
    }
  } finally { await reader.cancel(); reader.releaseLock(); }
}

/** Race a host operation against cancellation even if the host ignores the signal. */
function abortable<T>(promise: Promise<T>, signal: AbortSignal): Promise<T> {
  return new Promise((resolve, reject) => {
    const abort = (): void => {
      signal.removeEventListener("abort", abort);
      reject(signal.reason);
    };
    signal.addEventListener("abort", abort, { once: true });
    if (signal.aborted) abort();
    void promise.then(resolve, reject).finally(() => signal.removeEventListener("abort", abort));
  });
}
