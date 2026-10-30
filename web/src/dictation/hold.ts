/** A short click still sends. A hold records, and release only transcribes. */
export interface HoldBackend {
  start(id: string): Promise<void>;
  finish(id: string): Promise<string>;
  cancel(id: string): Promise<void>;
}
export type HoldPhase = "idle" | "armed" | "starting" | "recording" | "transcribing";
export interface HoldClock { set(callback: () => void, ms: number): number; clear(id: number): void }
const clock: HoldClock = {set: (cb, ms) => globalThis.setTimeout(cb, ms), clear: id => globalThis.clearTimeout(id)};
export class HoldToTalk {
  phase: HoldPhase = "idle";
  #timer: number | null = null;
  #deadline: number | null = null;
  #id: string | null = null;
  #released = false;
  #suppressClick = false;
  constructor(private readonly backend: HoldBackend, private readonly changed: () => void,
    private readonly text: (value: string) => void, private readonly error: (value: unknown) => void,
    private readonly scheduler: HoldClock = clock) {}
  get active(): boolean { return this.phase !== "idle" && this.phase !== "armed"; }
  press(): void {
    if (this.phase !== "idle") return;
    this.#suppressClick = false; this.#released = false; this.phase = "armed";
    this.#timer = this.scheduler.set(() => { this.#timer = null; void this.#start(); }, 300);
  }
  release(): void {
    if (this.phase === "armed") { this.#clearTimers(); this.phase = "idle"; return; }
    this.#released = true;
    if (this.phase === "recording") void this.#finish();
  }
  consumeClick(): boolean { const blocked = this.#suppressClick || this.active; this.#suppressClick = false; return blocked; }
  clearClick(): void { if (!this.active) this.#suppressClick = false; }
  cancel(): void {
    this.#clearTimers(); const id = this.#id; this.#id = null;
    this.#suppressClick ||= this.active; this.phase = "idle";
    if (id) void this.backend.cancel(id).catch(() => {});
    this.changed();
  }
  #clearTimers(): void {
    if (this.#timer !== null) this.scheduler.clear(this.#timer);
    if (this.#deadline !== null) this.scheduler.clear(this.#deadline);
    this.#timer = this.#deadline = null;
  }
  async #start(): Promise<void> {
    const id = crypto.randomUUID(); this.#id = id; this.#suppressClick = true;
    this.phase = "starting"; this.changed();
    try {
      await this.backend.start(id);
      if (this.#id !== id) { await this.backend.cancel(id); return; }
      this.phase = "recording"; this.changed();
      this.#deadline = this.scheduler.set(() => this.release(), 120_000);
      if (this.#released) await this.#finish();
    } catch (error) {
      if (this.#id === id) { this.cancel(); this.error(error); }
    }
  }
  async #finish(): Promise<void> {
    const id = this.#id; if (!id || this.phase !== "recording") return;
    this.#clearTimers(); this.phase = "transcribing"; this.changed();
    try {
      const text = await this.backend.finish(id);
      if (this.#id !== id) return;
      this.#id = null; this.phase = "idle"; this.text(text); this.changed();
    } catch (error) {
      if (this.#id === id) { this.cancel(); this.error(error); }
    }
  }
}
