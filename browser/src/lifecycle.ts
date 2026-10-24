import { nativeWorking } from "./native-host.js";
/** Time budgets count visible, responsive execution, not time spent suspended by the OS.
 * A long scheduler gap is observable, but its cause (freeze, sleep, overload) is not.
 */
export class ExecutionBudget {
  #last: number;
  constructor(public remaining: number, now: number) { this.#last = now; }
  tick(now: number, paused: boolean): boolean {
    const elapsed = Math.max(0, now - this.#last); this.#last = now;
    if (!paused && elapsed <= 5000) this.remaining -= elapsed;
    return this.remaining <= 0;
  }
}
export interface LifecycleEvent { at: string; event: string; visibility: string }
const events: LifecycleEvent[] = [];
let installed = false;
function record(event: string): void {
  events.push({ at: new Date().toISOString(), event, visibility: typeof document === "undefined" ? "worker" : document.visibilityState });
  if (events.length > 100) events.shift();
}
export function lifecycleDiagnostics(): readonly LifecycleEvent[] { return events.map(event => ({ ...event })); }
function install(): void {
  if (installed || typeof document === "undefined") return;
  installed = true;
  record((document as Document & { wasDiscarded?: boolean }).wasDiscarded ? "loaded-after-discard" : "loaded");
  for (const event of ["visibilitychange", "freeze", "resume"]) document.addEventListener(event, () => record(event));
  for (const event of ["pagehide", "pageshow", "online", "offline"]) globalThis.addEventListener(event, () => record(event));
}
/** Returns a disposer. A zero budget disables the deadline. */
export function executionTimeout(callback: () => void, milliseconds: number): () => void {
  install();
  if (milliseconds === 0) return () => {};
  const budget = new ExecutionBudget(milliseconds, performance.now());
  const timer = setInterval(() => {
    if (budget.tick(performance.now(), typeof document !== "undefined" && document.hidden)) { clearInterval(timer); callback(); }
  }, Math.min(250, milliseconds));
  return () => clearInterval(timer);
}

/** Best effort visible-screen lock. Native hosts independently prevent idle system sleep. */
export function keepAwakeDuringWork(): () => void {
  install();
  void nativeWorking(true).catch(() => record("native-lock-unavailable"));
  let stopped = false, pending = false;
  let lock: WakeLockSentinel | null = null;
  const acquire = async (): Promise<void> => {
    if (stopped || pending || lock || typeof navigator === "undefined" || !navigator.wakeLock || document.hidden) return;
    pending = true;
    try {
      const acquired = await navigator.wakeLock.request("screen");
      if (stopped) { await acquired.release(); return; }
      lock = acquired;
      acquired.addEventListener("release", () => { if (lock === acquired) lock = null; });
      record("screen-lock-acquired");
    } catch { record("screen-lock-unavailable"); }
    finally { pending = false; }
  };
  const change = (): void => { void acquire(); };
  if (typeof document !== "undefined") document.addEventListener("visibilitychange", change);
  void acquire();
  return () => {
    if (stopped) return;
    stopped = true;
    void nativeWorking(false).catch(() => record("native-lock-release-failed"));
    if (typeof document !== "undefined") document.removeEventListener("visibilitychange", change);
    void lock?.release().catch(() => {}); lock = null;
  };
}
