import { modelAuthHeaders, validateConnection } from "./connection.js";
import type { Connection } from "./connection.js";
import type { TurnLimits } from "./turn-limits.js";

export interface EndpointLimits { name: string; context: number | null; input: number | null; output: number | null }
export interface ModelLimits { context: number | null; output: number | null; endpoints: EndpointLimits[]; fetchedAt: number }
const object = (value: unknown): Record<string, unknown> => value !== null && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : {};
const positive = (value: unknown): number | null => typeof value === "number" && Number.isSafeInteger(value) && value > 0 ? value : null;
export function isOpenRouter(connection?: Connection): boolean {
  if (!connection || connection.provider !== "openai") return false;
  try { const url = new URL(connection.endpoint); return url.origin === "https://openrouter.ai" && url.pathname.replace(/\/$/, "") === "/api/v1/chat/completions"; } catch { return false; }
}

/** Advertised model limits are distinct from per-provider limits and user budgets. */
export async function fetchModelLimits(connection: Connection, signal: AbortSignal, fetcher: typeof fetch = fetch): Promise<ModelLimits> {
  const validated = validateConnection(connection);
  if (!isOpenRouter(validated)) throw new Error("Automatic limits are available for OpenRouter connections.");
  const init: RequestInit = { signal, headers: { Accept: "application/json", ...modelAuthHeaders(validated) }, credentials: "omit", redirect: "error", cache: "no-store" };
  const response = await fetcher("https://openrouter.ai/api/v1/models", init);
  signal.throwIfAborted();
  if (!response.ok) throw new Error(`Model limits unavailable (HTTP ${response.status}).`);
  const data = object(await response.json())["data"];
  if (!Array.isArray(data)) throw new Error("Invalid model catalog.");
  // Routing variants share the base model's metadata; :free stays an exact model.
  const base = validated.model.replace(/:(?:nitro|floor|online)$/, "");
  const model = object(data.find(item => object(item)["id"] === validated.model) ?? data.find(item => object(item)["id"] === base));
  if (!model["id"]) throw new Error("Selected model is not in the OpenRouter catalog. Session budgets remain available.");
  const top = object(model["top_provider"]);
  const result: ModelLimits = { context: positive(model["context_length"]) ?? positive(top["context_length"]), output: positive(top["max_completion_tokens"]), endpoints: [], fetchedAt: Date.now() };
  const parts = String(model["id"]).split("/");
  if (parts.length === 2 && parts.every(Boolean)) {
    try {
      const providers = await fetcher(`https://openrouter.ai/api/v1/models/${parts.map(encodeURIComponent).join("/")}/endpoints`, init);
      if (providers.ok) {
        const endpoints = object(object(await providers.json())["data"])["endpoints"];
        if (Array.isArray(endpoints)) result.endpoints = endpoints.map(item => {
          const endpoint = object(item);
          return { name: String(endpoint["name"] ?? endpoint["provider_name"] ?? "Provider"), context: positive(endpoint["context_length"]), input: positive(endpoint["max_prompt_tokens"]), output: positive(endpoint["max_completion_tokens"]) };
        });
      }
    } catch { signal.throwIfAborted(); /* Catalog limits remain useful if provider metadata is unavailable. */ }
  }
  signal.throwIfAborted();
  return result;
}

/** Reserve output within the advertised total context; never enlarge saved user budgets. */
export function effectiveLimits(configured: TurnLimits, model: ModelLimits | null): TurnLimits {
  // With one provider there is no alternate route to satisfy a larger request.
  const provider = model?.endpoints.length === 1 ? model.endpoints[0] : undefined;
  const context = Math.min(model?.context ?? Infinity, provider?.context ?? Infinity);
  const output = Math.min(configured.maxOutputTokens, model?.output ?? Infinity, provider?.output ?? Infinity, Math.max(1, context - 256));
  return { ...configured, maxOutputTokens: output, contextTokens: Math.min(configured.contextTokens, Math.max(256, context - output), provider?.input ?? Infinity) };
}

/** Memory-only cache. Connection changes cancel old fetches and cannot overwrite new metadata. */
export class ModelLimitState {
  value: ModelLimits | null = null;
  status = "Model limits not supplied by this endpoint; using session budgets.";
  #identity = "";
  #controller: AbortController | null = null;
  #pending: Promise<void> = Promise.resolve();
  #checked = 0;
  constructor(readonly changed: () => void, readonly fetcher?: typeof fetch) {}
  ensure(connection?: Connection, force = false): Promise<void> {
    const identity = JSON.stringify(connection ? [connection.provider, connection.endpoint, connection.model, connection.apiKey] : []);
    if (!force && identity === this.#identity && (this.#controller || Date.now() - this.#checked < 600000)) return this.#pending;
    this.#identity = identity; this.#checked = Date.now(); this.#controller?.abort(); this.#controller = null;
    this.value = null;
    if (!isOpenRouter(connection)) { this.status = "Model limits not supplied by this endpoint; using session budgets."; this.#pending = Promise.resolve(); return this.#pending; }
    const controller = new AbortController(); this.#controller = controller;
    this.status = "Loading OpenRouter model limits…";
    const timer = setTimeout(() => controller.abort(), 10000);
    this.#pending = fetchModelLimits(connection!, controller.signal, this.fetcher).then(value => {
      if (this.#controller !== controller) return;
      this.value = value; this.status = "OpenRouter model limits · provider limits may differ";
    }).catch(() => {
      if (this.#controller === controller) this.status = "OpenRouter limits unavailable; using session budgets. Refresh to retry.";
    }).finally(() => {
      clearTimeout(timer);
      if (this.#controller === controller) { this.#controller = null; this.changed(); }
    });
    return this.#pending;
  }
  async ready(connection: Connection | undefined, signal: AbortSignal): Promise<void> {
    signal.throwIfAborted();
    let abort!: () => void;
    const stopped = new Promise<never>((_resolve, reject) => { abort = () => reject(signal.reason); });
    signal.addEventListener("abort", abort, {once:true});
    try { await Promise.race([this.ensure(connection), stopped]); }
    finally { signal.removeEventListener("abort", abort); }
  }
  dispose(): void { this.#controller?.abort(); this.#controller = null; }
}
