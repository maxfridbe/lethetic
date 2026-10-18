import type { RuntimeEvent } from "./sdk.js";
import type { ProviderUsage } from "./usage.js";

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
  #started: number | null = null;
  #toolStarted: number | null = null;
  constructor(readonly now: () => number = () => performance.now()) {}
  get elapsed(): number { return this.#started === null ? this.elapsedMs : this.now() - this.#started; }
  begin(): void {
    this.turn = new UsageTotals(); this.modelMs = 0; this.toolMs = 0; this.elapsedMs = 0;
    this.#toolStarted = null; this.#started = this.now();
  }
  event(event: RuntimeEvent): void {
    if (event.type === "request") { this.turn.requests++; this.session.requests++; }
    if (event.type === "response") {
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
  }
}
