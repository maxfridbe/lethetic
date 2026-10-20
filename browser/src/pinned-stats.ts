export const STAT_IDS = ["context", "requestUsage", "contextMax", "inputBudget", "outputBudget", "stop", "activity", "thinking", "files", "fileBytes", "blocks", "rate", "timeout", "rounds", "retries", "loop", "connection", "model", "input", "output", "cached", "cacheWrite", "reasoning", "total", "session", "requests", "elapsed", "modelTime", "toolTime", "python"] as const;
export type StatId = typeof STAT_IDS[number];
const STORAGE_KEY = "lethetic.pinned-stats.v2";
const LEGACY_KEY = "lethetic.pinned-stats.v1";

export class PinnedStats {
  ids: StatId[] = ["context", "input", "output", "elapsed"];
  error = "";
  constructor(readonly storage?: Pick<Storage, "getItem" | "setItem">) {
    try {
      const current = storage?.getItem(STORAGE_KEY) ?? null;
      const saved: unknown = JSON.parse(current ?? storage?.getItem(LEGACY_KEY) ?? "null");
      if (Array.isArray(saved)) {
        this.ids = [...new Set(saved.filter((id): id is StatId => STAT_IDS.includes(id as StatId)))];
        if (current === null && JSON.stringify(this.ids) === '["input","output","elapsed"]') this.ids.unshift("context");
      }
    } catch { /* Default pins when browser storage is unavailable or damaged. */ }
  }
  toggle(id: StatId): void {
    this.ids = this.ids.includes(id) ? this.ids.filter(item => item !== id) : [...this.ids, id];
    try { this.storage?.setItem(STORAGE_KEY, JSON.stringify(this.ids)); this.error = ""; }
    catch { this.error = "Pins changed for this tab; browser storage is unavailable."; }
  }
}
