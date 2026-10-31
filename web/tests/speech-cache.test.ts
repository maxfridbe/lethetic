import test from "node:test";
import assert from "node:assert/strict";
import { SpeechAudioCache, pcm16 } from "../src/personas/speech-cache.js";
test("downloaded speech is a playable PCM WAV with signed samples and correct size", async () => {
  const cache = new SpeechAudioCache();
  cache.put("response", [pcm16(new Float32Array([-1, 0, 1, .5]))], 24000);
  const blob = cache.get("response")!; assert.equal(blob.type, "audio/wav");
  const bytes = await blob.arrayBuffer(), view = new DataView(bytes);
  assert.equal(bytes.byteLength, 52); assert.equal(view.getUint32(24, true), 24000); assert.equal(view.getUint32(40, true), 8);
  assert.equal(view.getInt16(44, true), -32768); assert.equal(view.getInt16(48, true), 32767);
  assert.equal(new TextDecoder().decode(bytes.slice(0, 4)), "RIFF");
});
