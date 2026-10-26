/** Decorative status copy; never represents real tool operations or model reasoning. */
export const THINKING_LINES = [
  "Reticulating splines… oh… I mean, doing whatever you just told me…",
  "Turning caffeine into plausible next tokens…",
  "Consulting the committee of approximately a trillion parameters…",
  "Checking whether the obvious solution is secretly the complicated one…",
  "Putting the ducks in a row. One of them is a compiler…",
  "Untangling the headphones of logic…",
  "Asking the rubber duck a follow-up question…",
  "Moving this from ‘probably’ toward ‘let’s check’…",
  "Finding the semicolon that holds the universe together…",
  "Giving the edge cases their own little helmets…",
  "Teaching the solution to survive contact with reality…",
  "Looking busy, but with significantly more matrix multiplication…",
] as const;
export const THINKING_LINE_INTERVAL_MS = 12_000;

interface ThinkingScheduler { setTimeout(callback: () => void, delay: number): number; clearTimeout(timer: number): void }

export class ThinkingLines {
  #timer: ReturnType<typeof setTimeout> | null = null;
  #key: string | null = null;
  #index = 0;
  constructor(readonly changed: () => void, readonly scheduler: ThinkingScheduler = globalThis) {}
  current(key: string | null, reducedMotion: boolean): string | null {
    if (key !== this.#key) { this.dispose(); this.#key = key; }
    if (key === null) return null;
    if (reducedMotion && this.#timer !== null) { this.scheduler.clearTimeout(this.#timer); this.#timer = null; }
    if (!reducedMotion && this.#timer === null) {
      this.#timer = this.scheduler.setTimeout(() => {
        this.#timer = null;
        if (this.#key !== key) return;
        this.#index = (this.#index + 1) % THINKING_LINES.length;
        this.changed();
      }, THINKING_LINE_INTERVAL_MS);
    }
    return THINKING_LINES[this.#index]!;
  }
  dispose(): void { if (this.#timer !== null) this.scheduler.clearTimeout(this.#timer); this.#timer = null; this.#key = null; this.#index = 0; }
}
