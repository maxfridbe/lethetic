import type { RuntimeEvent } from "./sdk.js";
import type { ProviderUsage } from "./usage.js";

export interface MetricsSnapshot {
  turn: { requests: number; responses: number; sums: ProviderUsage; reports: Record<keyof ProviderUsage, number> };
  session: MetricsSnapshot["turn"];
  modelMs: number; toolMs: number; elapsedMs: number;
  context?: { tokens: number; source: "server_usage" | "local_estimate" };
  latestUsage?: ProviderUsage;
}

export function parseMetrics(value: unknown): MetricsSnapshot {
  if (!value || typeof value !== "object") throw new Error("Invalid saved usage.");
  const item = value as MetricsSnapshot;
  for (const group of [item.turn, item.session]) {
    if (!group || !Number.isSafeInteger(group.requests) || group.requests < 0 ||
        !Number.isSafeInteger(group.responses) || group.responses < 0 || group.responses > group.requests) throw new Error("Invalid saved usage.");
    for (const key of ["input", "output", "total", "cached", "cacheWrite", "reasoning"] as const) {
      const sum = group.sums?.[key]; const reports = group.reports?.[key];
      if ((sum !== null && (!Number.isSafeInteger(sum) || sum < 0)) || !Number.isSafeInteger(reports) || reports < 0 || reports > group.responses) throw new Error("Invalid saved usage.");
    }
  }
  for (const time of [item.modelMs, item.toolMs, item.elapsedMs]) if (!Number.isFinite(time) || time < 0) throw new Error("Invalid saved timing.");
  if (item.context !== undefined && (!item.context || !Number.isSafeInteger(item.context.tokens) || item.context.tokens < 0 ||
      !["server_usage", "local_estimate"].includes(item.context.source))) throw new Error("Invalid saved context usage.");
  if (item.latestUsage !== undefined && !item.latestUsage) throw new Error("Invalid latest usage.");
  if (item.latestUsage) for (const key of ["input", "output", "total", "cached", "cacheWrite", "reasoning"] as const) {
    const value = item.latestUsage[key];
    if (value !== null && (!Number.isSafeInteger(value) || value < 0)) throw new Error("Invalid latest usage.");
  }
  return structuredClone(item);
}

export class UsageTotals {
  requests = 0;
  responses = 0;
  readonly sums: ProviderUsage = { input: null, output: null, total: null, cached: null, cacheWrite: null, reasoning: null };
  readonly reports: Record<keyof ProviderUsage, number> = { input: 0, output: 0, total: 0, cached: 0, cacheWrite: 0, reasoning: 0 };
  add(usage: ProviderUsage): void {
    this.responses++;
    for (const key of Object.keys(this.sums) as (keyof ProviderUsage)[]) {
      const value = usage[key];
      if (value === null) continue;
      const total = (this.sums[key] ?? 0) + value;
      if (!Number.isSafeInteger(total)) continue;
      this.sums[key] = total; this.reports[key]++;
    }
  }
  label(key: keyof ProviderUsage): string {
    const value = this.sums[key];
    if (value === null) return "Not reported";
    return value.toLocaleString() + (this.reports[key] < this.requests ? " (partial)" : "");
  }
}

export class BrowserMetrics {
  turn = new UsageTotals();
  session = new UsageTotals();
  modelMs = 0;
  toolMs = 0;
  elapsedMs = 0;
  context: NonNullable<MetricsSnapshot["context"]> = { tokens: 0, source: "local_estimate" };
  latestUsage: ProviderUsage | undefined;
  #started: number | null = null;
  #toolStarted: number | null = null;
  constructor(readonly now: () => number = () => performance.now()) {}
  get elapsed(): number { return this.#started === null ? this.elapsedMs : this.now() - this.#started; }
  begin(): void {
    this.turn = new UsageTotals(); this.modelMs = 0; this.toolMs = 0; this.elapsedMs = 0;
    this.#toolStarted = null; this.#started = this.now();
  }
  event(event: RuntimeEvent): void {
    if (event.type === "request") { this.turn.requests++; this.session.requests++;
      if (event.contextTokens !== undefined) this.context = { tokens: event.contextTokens, source: "local_estimate" };
    }
    if (event.type === "response") {
      this.latestUsage = event.usage;
      if (event.usage.input !== null) this.context = { tokens: event.usage.input, source: "server_usage" };
      this.turn.add(event.usage); this.session.add(event.usage); this.modelMs += event.durationMs;
    }
    if (event.type === "tool_start") this.#toolStarted = this.now();
    if (event.type === "tool_end") this.#endTool();
  }
  #endTool(): void {
    if (this.#toolStarted !== null) this.toolMs += this.now() - this.#toolStarted;
    this.#toolStarted = null;
  }
  finish(): void { this.elapsedMs = this.elapsed; this.#started = null; this.#endTool(); }
  reset(): void {
    this.finish(); this.turn = new UsageTotals(); this.session = new UsageTotals();
    this.elapsedMs = 0; this.modelMs = 0; this.toolMs = 0;
    this.context = { tokens: 0, source: "local_estimate" }; this.latestUsage = undefined;
  }
  snapshot(): MetricsSnapshot {
    return { turn: structuredClone(this.turn), session: structuredClone(this.session), modelMs: this.modelMs,
      toolMs: this.toolMs, elapsedMs: this.elapsed, context: this.context, ...(this.latestUsage ? {latestUsage: this.latestUsage} : {}) };
  }
  restore(value: MetricsSnapshot): void {
    const saved = parseMetrics(value); this.reset();
    for (const key of ["turn", "session"] as const) {
      this[key].requests = saved[key].requests; this[key].responses = saved[key].responses;
      Object.assign(this[key].sums, saved[key].sums); Object.assign(this[key].reports, saved[key].reports);
    }
    this.context = saved.context ?? { tokens: 0, source: "local_estimate" }; this.latestUsage = saved.latestUsage;
    this.modelMs = saved.modelMs; this.toolMs = saved.toolMs; this.elapsedMs = saved.elapsedMs;
  }
}
