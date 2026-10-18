export const STAT_IDS = ["connection", "model", "input", "output", "cached", "cacheWrite", "reasoning", "total", "session", "requests", "elapsed", "modelTime", "toolTime", "python"] as const;
export type StatId = typeof STAT_IDS[number];
const STORAGE_KEY = "lethetic.pinned-stats.v1";

export class PinnedStats {
  ids: StatId[] = ["input", "output", "elapsed"];
  error = "";
  constructor(readonly storage?: Pick<Storage, "getItem" | "setItem">) {
    try {
      const saved: unknown = JSON.parse(storage?.getItem(STORAGE_KEY) ?? "null");
      if (Array.isArray(saved)) this.ids = [...new Set(saved.filter((id): id is StatId => STAT_IDS.includes(id as StatId)))];
    } catch { /* Default pins when browser storage is unavailable or damaged. */ }
  }
  toggle(id: StatId): void {
    this.ids = this.ids.includes(id) ? this.ids.filter(item => item !== id) : [...this.ids, id];
    try { this.storage?.setItem(STORAGE_KEY, JSON.stringify(this.ids)); this.error = ""; }
    catch { this.error = "Pins changed for this tab; browser storage is unavailable."; }
  }
}
