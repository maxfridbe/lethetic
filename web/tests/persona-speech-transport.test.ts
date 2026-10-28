import assert from "node:assert/strict";
import test from "node:test";
import { fetchSpeech } from "../src/personas/speech-transport.js";

test("native HTTP speech streams through authenticated IPC without browser fetch", async () => {
  const previous = Object.getOwnPropertyDescriptor(globalThis, "window"), originalFetch = globalThis.fetch;
  const calls: {command: string; args: Record<string, unknown>}[] = [];
  const events = [{kind: "head", status: 200, headers: {"content-type": "audio/pcm"}}, {kind: "chunk", data: "AAD/fw=="}, {kind: "done"}];
  const host: Record<string, unknown> = {__LETHETIC_HOST_TOKEN__: "host-secret", __TAURI__: {core: {invoke: async (command: string, args: Record<string, unknown>) => {
    calls.push({command, args}); if (command.endsWith("next_speech")) return events.shift(); return undefined;
  }}}};
  host["top"] = host; Object.defineProperty(globalThis, "window", {configurable: true, value: host});
  globalThis.fetch = async () => {throw Error("Native speech must not use browser fetch");};
  try {
    const response = await fetchSpeech("http://brainiac-nvidia:8000/v1/audio/speech", {method: "POST", headers: {Authorization: "Bearer voice-key"}, body: '{"input":"hello"}'}, new AbortController().signal);
    assert.deepEqual([...new Uint8Array(await response.arrayBuffer())], [0, 0, 255, 127]);
    const start = calls[0]!; assert.equal(start.command, "plugin:lethetic-host|begin_speech");
    assert.equal(start.args["token"], "host-secret"); assert.equal(typeof start.args["id"], "string");
    assert.deepEqual(start.args["request"], {url: "http://brainiac-nvidia:8000/v1/audio/speech", headers: {authorization: "Bearer voice-key"}, body: '{"input":"hello"}'});
    assert.ok(calls.some(c => c.command.endsWith("cancel_speech")));
    assert.ok(calls.every(c => c.args["id"] === start.args["id"]));
  } finally {globalThis.fetch = originalFetch; if (previous) Object.defineProperty(globalThis, "window", previous); else Reflect.deleteProperty(globalThis, "window");}
});
test("abort racing native registration cancels again after start and never falls back", async () => {
  const previous = Object.getOwnPropertyDescriptor(globalThis, "window");
  let registered: (() => void) | undefined, cancels = 0;
  const host: Record<string, unknown> = {__LETHETIC_HOST_TOKEN__: "host-secret", __TAURI__: {core: {invoke: async (command: string) => {
    if (command.endsWith("begin_speech")) return new Promise<void>(resolve => {registered = resolve;});
    if (command.endsWith("cancel_speech")) {cancels++; return;}
    throw Error("Cancelled request must not read");
  }}}};
  host["top"] = host; Object.defineProperty(globalThis, "window", {configurable: true, value: host});
  try {
    const abort = new AbortController(), result = fetchSpeech("http://lan/speech", {body: "{}"}, abort.signal);
    abort.abort(); assert.ok(registered); registered(); await assert.rejects(result, {name: "AbortError"}); assert.equal(cancels, 2);
  } finally {if (previous) Object.defineProperty(globalThis, "window", previous); else Reflect.deleteProperty(globalThis, "window");}
});
test("ordinary browsers and iframes use fetch; a missing native bridge fails closed", async () => {
  const previous = Object.getOwnPropertyDescriptor(globalThis, "window"), originalFetch = globalThis.fetch;
  let fetched = 0;
  globalThis.fetch = async () => {fetched++; return new Response("audio");};
  const host: Record<string, unknown> = {__LETHETIC_HOST_TOKEN__: "host-secret", top: {}};
  Object.defineProperty(globalThis, "window", {configurable: true, value: host});
  try {
    const abort = new AbortController();
    assert.equal(await (await fetchSpeech("http://lan/speech", {}, abort.signal)).text(), "audio"); assert.equal(fetched, 1);
    host["top"] = host;
    await assert.rejects(fetchSpeech("http://lan/speech", {}, abort.signal), /Update the executable/); assert.equal(fetched, 1);
    delete host["__LETHETIC_HOST_TOKEN__"];
    await fetchSpeech("http://lan/speech", {}, abort.signal); assert.equal(fetched, 2);
  } finally {globalThis.fetch = originalFetch; if (previous) Object.defineProperty(globalThis, "window", previous); else Reflect.deleteProperty(globalThis, "window");}
});
