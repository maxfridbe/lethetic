import type { Motion, PersonaSound } from "./schema.js";
const frames: Record<Motion, Keyframe[]> = {
  bounce: [{transform: "translateY(0)"}, {transform: "translateY(-12px)"}, {transform: "translateY(0)"}],
  nod: [{transform: "rotate(0)"}, {transform: "rotate(12deg)"}, {transform: "rotate(0)"}],
  shake: [{transform: "translateX(0)"}, {transform: "translateX(-6px)"}, {transform: "translateX(6px)"}, {transform: "translateX(0)"}],
  spin: [{transform: "rotate(0)"}, {transform: "rotate(360deg)"}],
  glow: [{filter: "drop-shadow(0 0 0px #38bdf8)"}, {filter: "drop-shadow(0 0 9px #38bdf8)"}, {filter: "drop-shadow(0 0 0px #38bdf8)"}],
};
export class PersonaEffects {
  #context: AudioContext | null = null;
  #audio: HTMLAudioElement | null = null;
  #oscillators: OscillatorNode[] = [];
  #timer: number | null = null;
  #animation: Animation | null = null;
  #generation = 0;
  error = "";
  constructor(readonly failed: (message: string) => void = () => {}) {}
  #fail(message: string): void { this.error = message; this.failed(message); }
  /** Only called from an explicit gesture, never from a model event. */
  async unlock(): Promise<void> {
    try { this.#context ??= new AudioContext(); await this.#context.resume().catch(() => { this.#fail("Audio is blocked by this browser. Use Test sound after enabling it."); }); }
    catch { this.#fail("Audio is unavailable in this browser."); }
  }
  animate(element: HTMLElement | null, motion: Motion, reduced: boolean): void {
    this.#animation?.cancel(); this.#animation = null;
    if (element && !reduced && typeof element.animate === "function") this.#animation = element.animate(frames[motion], {duration: 650, easing: "ease-in-out"});
  }
  play(sound: PersonaSound): void {
    this.stopSound(); this.error = "";
    const generation = this.#generation;
    if (sound.src) {
      const audio = new Audio(sound.src); this.#audio = audio; audio.volume = sound.volume;
      audio.onended = () => { if (this.#audio === audio) this.stopSound(); };
      void audio.play().catch(() => { if (generation === this.#generation) this.#fail("Sound could not play. Check its format or use Test sound to allow playback."); });
      this.#timer = window.setTimeout(() => this.stopSound(), 4000);
      return;
    }
    const context = this.#context;
    if (!context || context.state !== "running") { this.#fail("Sound needs a user gesture. Use Test sound or submit a message."); return; }
    let start = context.currentTime;
    for (const note of sound.notes ?? []) {
      const oscillator = context.createOscillator(), gain = context.createGain();
      oscillator.type = "sine"; oscillator.frequency.value = note.frequency;
      const end = start + note.durationMs / 1000;
      gain.gain.setValueAtTime(0, start); gain.gain.linearRampToValueAtTime(sound.volume * .3, start + .015); gain.gain.linearRampToValueAtTime(0, end);
      oscillator.connect(gain); gain.connect(context.destination); oscillator.start(start); oscillator.stop(end);
      oscillator.onended = () => { oscillator.disconnect(); gain.disconnect(); this.#oscillators = this.#oscillators.filter(o => o !== oscillator); };
      this.#oscillators.push(oscillator); start = end;
    }
  }
  stopSound(): void {
    this.#generation++;
    if (this.#timer !== null) window.clearTimeout(this.#timer); this.#timer = null;
    this.#audio?.pause(); if (this.#audio) { this.#audio.removeAttribute("src"); this.#audio.load(); } this.#audio = null;
    for (const oscillator of this.#oscillators) { try { oscillator.stop(); } catch { /* Already ended. */ } } this.#oscillators = [];
  }
  stop(): void { this.stopSound(); this.#animation?.cancel(); this.#animation = null; }
  dispose(): void { this.stop(); if (this.#context) void this.#context.close().catch(() => {}); this.#context = null; }
}
