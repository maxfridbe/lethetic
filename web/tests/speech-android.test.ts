import test from "node:test";
import assert from "node:assert/strict";
import { SpeechPlayer, SPEECH_DEFAULTS } from "../src/personas/speech.js";
test("Android owns playback across page suspension and UI recreation; download uses native cached file", async () => {
  const previous = Object.getOwnPropertyDescriptor(globalThis, "window");
  let state: Record<string, unknown> = {active: false}, stops = 0, saved = false, starts = 0;
  const host: Record<string, unknown> = {__LETHETIC_ANDROID__: true, __LETHETIC_HOST_TOKEN__: "main-window"};
  host["top"] = host;
  host["__TAURI__"] = {core: {invoke: async (command: string, args: Record<string, unknown>) => {
    assert.equal(command, "plugin:lethetic-host|response_audio"); assert.equal(args["token"], "main-window");
    const payload = args["payload"] as Record<string, unknown>;
    switch (args["operation"]) {
      case "status": return state;
      case "start": starts++; state = {id: payload["id"], key: "cached-key", active: true, ready: true}; return {};
      case "stop": stops++; state = {...state, active: false}; return {};
      case "save": assert.equal(payload["key"], "cached-key"); saved = true; return {};
      default: throw Error("Unexpected operation");
    }
  }}};
  Object.defineProperty(globalThis, "window", {configurable: true, value: host});
  const first = new SpeechPlayer(() => {});
  let reopened: SpeechPlayer | undefined;
  try {
    await first.play("session:turn", "Read me", SPEECH_DEFAULTS.openai, "voice-key");
    first.background(); first.dispose(); assert.equal(stops, 0); assert.equal(starts, 1);
    reopened = new SpeechPlayer(() => {});
    for (let i = 0; i < 20 && !reopened.downloadable("session:turn"); i++) await new Promise(r => setTimeout(r, 1));
    assert.equal(reopened.active, "session:turn"); assert.equal(reopened.downloadable("session:turn"), true);
    await reopened.download("session:turn"); assert.equal(saved, true); assert.equal(starts, 1);
    reopened.stop(); assert.equal(stops, 1);
  } finally {
    first.dispose(); reopened?.dispose();
    if (previous) Object.defineProperty(globalThis, "window", previous); else Reflect.deleteProperty(globalThis, "window");
  }
});
