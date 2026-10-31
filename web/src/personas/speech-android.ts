import { androidCommand } from "../native-credentials.js";
interface AudioState { id?: string; key?: string; active: boolean; ready?: boolean; error?: string }
export class AndroidSpeech {
  active: string | null = null;
  error = "";
  errorTarget: string | null = null;
  #keys = new Map<string, string>();
  #timer: ReturnType<typeof setTimeout> | undefined;
  #disposed = false;
  #generation = 0;
  constructor(readonly changed: () => void) { void this.#poll(); }
  async #command<T>(operation: string, payload: Record<string, unknown> = {}): Promise<T> {
    return androidCommand<T>("response_audio", {operation, payload});
  }
  async #poll(): Promise<void> {
    const generation = this.#generation;
    try {
      const status = await this.#command<AudioState>("status");
      if (this.#disposed || generation !== this.#generation) return;
      const old = JSON.stringify([this.active, this.error, this.#keys.size]);
      this.active = status.active ? status.id ?? null : null;
      if (status.id && status.key && status.ready) this.#keys.set(status.id, status.key);
      if (status.error) { this.error = status.error; this.errorTarget = status.id ?? null; }
      if (old !== JSON.stringify([this.active, this.error, this.#keys.size])) this.changed();
    } catch { /* A paused/recreating activity must not stop native playback. */ }
    finally { if (!this.#disposed) this.#timer = setTimeout(() => { void this.#poll(); }, 500); }
  }
  async play(id: string, format: string, requests: Record<string, unknown>[]): Promise<void> {
    this.#generation++; this.error = ""; this.errorTarget = null; this.active = id; this.changed();
    try { await this.#command("start", {id, format, requests}); }
    catch { this.active = null; this.error = "Android audio could not start. Check voice settings and retry with the app open."; this.errorTarget = id; }
    this.changed();
  }
  stop(): void {
    this.#generation++; this.active = null;
    void this.#command("stop").catch(() => { this.error = "Could not stop audio. Use the Android media notification."; this.changed(); });
  }
  available(id: string): boolean { return this.#keys.has(id); }
  async download(id: string): Promise<void> {
    const key = this.#keys.get(id);
    if (!key) throw Error("Play the complete response first to download its audio.");
    await this.#command("save", {key});
  }
  dispose(): void { this.#disposed = true; clearTimeout(this.#timer); }
}
