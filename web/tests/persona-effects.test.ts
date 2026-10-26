import assert from "node:assert/strict";
import test from "node:test";
import { PersonaEffects } from "../src/personas/effects.js";

test("persona sound respects gestures, plays in background, and handles clipping, cancellation and teardown", async () => {
  const previous = new Map(["document", "window", "AudioContext", "Audio"].map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  const document = {hidden: false}, timers = new Map<number, () => void>(); let timer = 0, tones = 0, stopped = 0, paused = 0, closed = 0;
  const frequencies: number[] = [], failures: string[] = [];
  class Context {
    state = "suspended"; currentTime = 1; destination = {};
    async resume(): Promise<void> { this.state = "running"; }
    async close(): Promise<void> { closed++; }
    createGain() { return {gain: {setValueAtTime() {}, linearRampToValueAtTime() {}}, connect() {}, disconnect() {}}; }
    createOscillator() { const frequency = {value: 0}; return {frequency, type: "", onended: null, connect() {}, disconnect() {}, start() {tones++; frequencies.push(frequency.value);}, stop() {stopped++;}}; }
  }
  class Clip {
    volume = 1; onended: (() => void) | null = null;
    constructor(readonly src: string) {}
    async play(): Promise<void> { if (this.src.endsWith("AAAA")) throw Error("Unsupported codec"); }
    pause(): void {paused++;} removeAttribute(): void {} load(): void {}
  }
  Object.defineProperty(globalThis, "document", {configurable: true, value: document});
  Object.defineProperty(globalThis, "window", {configurable: true, value: {setTimeout: (callback: () => void, ms: number) => {assert.equal(ms, 4000); timers.set(++timer, callback); return timer;}, clearTimeout: (id: number) => timers.delete(id)}});
  Object.defineProperty(globalThis, "AudioContext", {configurable: true, value: Context});
  Object.defineProperty(globalThis, "Audio", {configurable: true, value: Clip});
  const effects = new PersonaEffects(message => failures.push(message));
  try {
    const sound = {volume: .2, notes: [{frequency: 440, durationMs: 100}]};
    effects.play(sound); assert.equal(tones, 0); assert.match(failures.at(-1)!, /gesture/);
    await effects.unlock(); effects.play(sound); assert.equal(tones, 1); assert.deepEqual(frequencies, [440]);
    document.hidden = true; effects.play(sound); assert.equal(tones, 2); document.hidden = false;
    effects.play({volume: .1, src: "data:audio/wav;base64,AQID"}); assert.equal(timers.size, 1);
    timers.get(timer)!(); assert.equal(timers.size, 0); assert.ok(paused > 0);
    effects.play({volume: .1, src: "data:audio/wav;base64,AAAA"}); await Promise.resolve(); await Promise.resolve();
    assert.match(failures.at(-1)!, /could not play/);
    let animations = 0; const target = {animate: () => {animations++; return {cancel() {}};}} as unknown as HTMLElement;
    effects.animate(target, "bounce", true); assert.equal(animations, 0);
    effects.animate(target, "bounce", false); assert.equal(animations, 1);
    effects.dispose(); assert.equal(timers.size, 0); assert.equal(closed, 1); assert.ok(stopped >= 1);
  } finally { for (const [key, descriptor] of previous) { if (descriptor) Object.defineProperty(globalThis, key, descriptor); else Reflect.deleteProperty(globalThis, key); } }
});

test("pacing travels both ways, loops only when requested, and respects reduced motion", () => {
  let cancelled = 0;
  const calls: {frames: Keyframe[]; timing: KeyframeAnimationOptions}[] = [];
  const target = {animate: (frames: Keyframe[], timing: KeyframeAnimationOptions) => {
    calls.push({frames, timing}); return {cancel() { cancelled++; }};
  }} as unknown as HTMLElement;
  const effects = new PersonaEffects();
  effects.animate(target, "pace", false, true);
  assert.equal(calls[0]!.timing.iterations, Infinity);
  assert.ok(calls[0]!.frames.some(frame => String(frame["transform"]).includes("-48px")));
  assert.equal(calls[0]!.frames[0]!["transform"], calls[0]!.frames.at(-1)!["transform"]);
  effects.animate(target, "pace", false);
  assert.equal(cancelled, 1);
  assert.equal(calls[1]!.timing.iterations, 1);
  effects.animate(target, "pace", true, true);
  assert.equal(cancelled, 2);
  assert.equal(calls.length, 2);
});
