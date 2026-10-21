import { DEFAULT_TURN_LIMITS, parseTurnLimits, LOOP_MODES, parseLoopMode } from "./turn-limits.js";
import { abortable, delay } from "./async.js";
import { readModelResponse } from "./stream.js";
import type { WasmStream, StreamDelta } from "./stream.js";
import { validateConnection, modelAuthHeaders } from "./connection.js";
import { readUsage } from "./usage.js";
import type { ProviderUsage } from "./usage.js";
export type { ProviderUsage } from "./usage.js";
import type { Connection } from "./connection.js";
export type { Connection } from "./connection.js";
export { createDotnetTool } from "./dotnet.js";
export type { DotnetConsoleMessage, DotnetTool, DotnetRuntime, DotnetLoader, WireSite, WireRequest, WireResponse } from "./dotnet.js";
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
  | StreamDelta
  | { type: "response_reset" }
  | { type: "retry"; attempt: number; delayMs: number; error: string }
  | { type: "request"; round: number; contextTokens?: number }
  | { type: "response"; usage: ProviderUsage; durationMs: number }
  | { type: "assistant"; text: string }
  | { type: "thinking"; text: string }
  | { type: "tool_start"; call: ToolCall }
  | { type: "tool_end"; call: ToolCall; output: string };

export interface RuntimeOptions {
  connection: Connection;
  systemPrompt?: string;
  tools?: readonly Tool[];
  onEvent?: (event: RuntimeEvent) => void;
  /** Defaults to 32 model requests per user turn. */
  maxRounds?: number;
  /** Disable only for endpoints without streaming; defaults to true. */
  stream?: boolean;
  /** Timeout for the entire turn including tools; defaults to 10 minutes. Zero disables. */
  timeoutMs?: number;
  /** Transient retries per model request, default 2; no completed tool is replayed. */
  providerRetries?: number;
  /** CLI context input budget, estimated at four bytes per token; default 32768. */
  contextTokens?: number;
  /** Model output budget, including reasoning; default 4096. */
  maxOutputTokens?: number;
  /** CLI loop detection mode; default combined. */
  loopDetection?: string;
  /** Optional injected fetch for an embedding host or tests. */
  fetch?: typeof globalThis.fetch;
  /** Optional bytes for hosts that load WASM themselves (including Node). */
  wasm?: BufferSource;
}

interface WasmRuntime {
  set_limits(context: number, output: number): void;
  begin(prompt: string): void;
  request(provider: string, model: string): string;
  last_request_tokens(): number;
  accept(provider: string, response: string): string;
  tool_result(id: string, output: string): void;
  rollback(): void;
  clear(): void;
  history(): string;
  restore(history: string): void;
  free(): void;
}
export interface WasmCompaction {
  requests(): string; accept(id: number, text: string): void; progress(): string; result(): string | undefined; free(): void;
}
interface WasmModule {
  BrowserCompaction: new (history: string, context: number, output: number) => WasmCompaction;
  check_loop(text: string, mode: string): string | undefined;
  retryable_error(error: string): boolean;
  retry_delay_ms(attempt: number): number;
  BrowserStream: new (provider: string) => WasmStream;
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

/** Host binding to the same compaction coordinator used by the native CLI. */
export async function createCompactionPlan(messages: readonly unknown[], options: Pick<RuntimeOptions, "wasm" | "contextTokens" | "maxOutputTokens">): Promise<WasmCompaction> {
  const module = await loadRuntimeModule(options.wasm);
  return new module.BrowserCompaction(JSON.stringify(messages), options.contextTokens ?? DEFAULT_TURN_LIMITS.contextTokens,
    options.maxOutputTokens ?? DEFAULT_TURN_LIMITS.maxOutputTokens);
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
    this.#options = {...options, connection: validateConnection(options.connection)};
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
    let limits, loopMode: string;
    try { limits = parseTurnLimits({ timeoutMs: this.#options.timeoutMs ?? DEFAULT_TURN_LIMITS.timeoutMs,
      maxRounds: this.#options.maxRounds ?? DEFAULT_TURN_LIMITS.maxRounds,
      providerRetries: this.#options.providerRetries ?? DEFAULT_TURN_LIMITS.providerRetries,
      contextTokens: this.#options.contextTokens ?? DEFAULT_TURN_LIMITS.contextTokens,
      maxOutputTokens: this.#options.maxOutputTokens ?? DEFAULT_TURN_LIMITS.maxOutputTokens });
      loopMode = LOOP_MODES.find(mode => mode[0] === parseLoopMode(this.#options.loopDetection ?? "combined"))![2]; }
    catch (error) { signal?.removeEventListener("abort", cancel); throw error; }
    const { timeoutMs, maxRounds, providerRetries } = limits;
    const timer = timeoutMs === 0 ? null : setTimeout(() => controller.abort(new Error(
      `Turn timed out after ${Math.round(timeoutMs / 1000)} seconds. Change Ctrl+P → Turn limits for longer work.`)), timeoutMs);
    let responsePending = false;
    this.#active = controller;
    try {
      controller.signal.throwIfAborted();
      this.#wasm.set_limits(limits.contextTokens, limits.maxOutputTokens);
      this.#wasm.begin(prompt);
      const connection = this.#options.connection;
      for (let round = 1; round <= maxRounds; round++) {
        controller.signal.throwIfAborted();
        const request = JSON.parse(this.#wasm.request(connection.provider, connection.model)) as Record<string, unknown>;
        request["stream"] = this.#options.stream !== false;
        if (!request["stream"]) delete request["stream_options"];
        if (request["stream"] && connection.provider === "openai") request["stream_options"] = { include_usage: true };
        if (connection.thinking !== undefined) {
          request["reasoning"] = connection.thinking === "enabled" ? { enabled: true, exclude: false } : { enabled: false };
        }
        const body = JSON.stringify(request);
        const module = await loadRuntimeModule(this.#options.wasm);
        const checkLoop = (text: string): void => {
          const reason = module.check_loop(text, loopMode);
          if (reason) throw new Error(`Loop detected: ${reason}. Change Ctrl+P → Loop detection if needed.`);
        };
        let responseText = "";
        const started = performance.now();
        responsePending = true;
        for (let attempt = 0; ; attempt++) {
          controller.signal.throwIfAborted();
          this.#emit({ type: "request", round, contextTokens: this.#wasm.last_request_tokens() });
          let retryAfter: number | null = null;
          try {
            let thinking = "", checked = 0;
            const response = await abortable((this.#options.fetch ?? globalThis.fetch)(connection.endpoint, {
              method: "POST", mode: "cors", credentials: "omit", redirect: "error",
              referrerPolicy: "no-referrer", cache: "no-store", signal: controller.signal,
              headers: requestHeaders(connection), body,
            }), controller.signal);
            if (!response.ok) {
              const header = response.headers.get("Retry-After");
              if (header !== null) {
                const seconds = Number(header);
                const duration = Number.isFinite(seconds) ? seconds * 1000 : Date.parse(header) - Date.now();
                if (Number.isFinite(duration)) retryAfter = Math.max(0, Math.min(60_000, duration));
              }
              await response.body?.cancel();
              throw new Error(`Model request failed (HTTP ${response.status}). Check your endpoint, model, and key.`);
            }
            responseText = await readModelResponse(response, controller.signal,
              () => new module.BrowserStream(connection.provider), event => {
                if (event.type === "thinking_delta") {
                  thinking += event.text;
                  if (thinking.length - checked >= 512) { checked = thinking.length; checkLoop(thinking); }
                }
                this.#emit(event);
              });
            if (thinking) checkLoop(thinking);
            break;
          } catch (error) {
            controller.signal.throwIfAborted();
            const message = error instanceof Error ? error.message : String(error);
            if (attempt >= providerRetries || !(error instanceof TypeError || module.retryable_error(message))) throw error;
            this.#emit({ type: "response_reset" });
            const delayMs = retryAfter ?? module.retry_delay_ms(attempt + 1);
            this.#emit({ type: "retry", attempt: attempt + 1, delayMs, error: message });
            await delay(delayMs, controller.signal);
          }
        }
        controller.signal.throwIfAborted();
        this.#emit({ type: "response", usage: readUsage(connection.provider, JSON.parse(responseText)),
          durationMs: performance.now() - started });
        const turn = JSON.parse(this.#wasm.accept(connection.provider, responseText)) as {
          text: string; thinking: string; calls: ToolCall[];
        };
        responsePending = false;
        if (turn.thinking) this.#emit({ type: "thinking", text: turn.thinking });
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
      if (responsePending) this.#emit({ type: "response_reset" });
      this.#wasm.rollback();
      if (controller.signal.aborted) throw controller.signal.reason;
      if (error instanceof TypeError) {
        throw new Error("Could not reach the model. Check CORS, TLS, and the endpoint address.");
      }
      throw error instanceof Error ? error : new Error(String(error));
    } finally {
      if (timer !== null) clearTimeout(timer);
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
  return { "Content-Type": "application/json", ...modelAuthHeaders(connection) };
}

export type { DotnetRelay } from "./network-route.js";
