import assert from "node:assert/strict";
import test from "node:test";
import { parsePersona, parseSpeech } from "../src/personas/schema.js";
import { readSpeechWav, MAX_SPEECH_WAV_BYTES, PcmDecoder, SPEECH_DEFAULTS, speechRequest, splitSpeechText, SpeechPlayer } from "../src/personas/speech.js";

const persona = {version: 1, id: "voice", name: "Voice", description: "", narration: "", quips: ["Working"]};
test("speech settings roundtrip without secrets and reject unsafe endpoints/unknown fields", () => {
  for (const config of Object.values(SPEECH_DEFAULTS)) assert.deepEqual(parsePersona(JSON.stringify({...persona, speech: config})).speech, config);
  for (const endpoint of ["javascript:alert(1)", "https://user:secret@example.com/speech", "https://example.com/speech?key=secret", "https://example.com/speech#secret"]) {
    assert.throws(() => parseSpeech({...SPEECH_DEFAULTS.openai, endpoint}));
  }
  assert.throws(() => parseSpeech({...SPEECH_DEFAULTS.openai, apiKey: "secret"}), /Unknown/);
  assert.throws(() => parseSpeech({...SPEECH_DEFAULTS.elevenlabs, endpoint: "https://api.example/speech"}), /voice_id/);
  assert.throws(() => parseSpeech({...SPEECH_DEFAULTS.elevenlabs, endpoint: "https://{voice_id}.example/stream"}), /voice_id/);
  assert.equal(parseSpeech({...SPEECH_DEFAULTS.openai, endpoint: "http://brainiac-nvidia:8000/v1/audio/speech"}).endpoint, "http://brainiac-nvidia:8000/v1/audio/speech");
  assert.equal(parseSpeech({...SPEECH_DEFAULTS.openai, endpoint: "http://localhost:8080/v1/audio/speech"}).provider, "openai");
});
test("provider requests use distinct authentication and binary streaming formats", () => {
  const openai = speechRequest(SPEECH_DEFAULTS.openai, "openai-secret", "hello");
  assert.equal(new Headers(openai.init.headers).get("Authorization"), "Bearer openai-secret");
  assert.equal(new Headers(openai.init.headers).get("xi-api-key"), null);
  assert.deepEqual(JSON.parse(String(openai.init.body)), {model: "gpt-4o-mini-tts", voice: "coral", input: "hello", response_format: "pcm"});
  const eleven = speechRequest({...SPEECH_DEFAULTS.elevenlabs, voice: "voice/id"}, "eleven-secret", "world");
  assert.match(eleven.url, /voice%2Fid\/stream\?output_format=pcm_24000$/);
  assert.equal(new Headers(eleven.init.headers).get("xi-api-key"), "eleven-secret");
  assert.equal(new Headers(eleven.init.headers).get("Authorization"), null);
  assert.deepEqual(JSON.parse(String(eleven.init.body)), {model_id: "eleven_multilingual_v2", text: "world"});
  assert.equal(eleven.init.credentials, "omit"); assert.equal(eleven.init.redirect, "error");
  assert.equal(new Headers(speechRequest(SPEECH_DEFAULTS.openai, "", "local").init.headers).get("Authorization"), null);
});
test("PCM conversion survives byte boundaries, handles signed extremes, and rejects truncated samples", () => {
  const bytes = new Uint8Array([0, 128, 255, 127, 0, 0, 0, 64, 0, 192]);
  const expected = [-1, 32767 / 32768, 0, .5, -.5];
  for (let size = 1; size <= bytes.length; size++) {
    const decoder = new PcmDecoder(), result: number[] = [];
    for (let offset = 0; offset < bytes.length; offset += size) result.push(...decoder.decode(bytes.subarray(offset, offset + size)));
    decoder.finish(); assert.deepEqual(result, expected);
  }
  const decoder = new PcmDecoder(); decoder.decode(new Uint8Array([1])); decoder.decode(new Uint8Array());
  assert.throws(() => decoder.finish(), /incomplete/);
});
test("long response splitting preserves all text including emoji at boundaries", () => {
  for (const value of ["a".repeat(3999) + "🎉" + "b".repeat(8000), "Sentence. ".repeat(2000), "word ".repeat(4000), "hello"]) {
    const parts = splitSpeechText(value); assert.equal(parts.join(""), value);
    assert.ok(parts.every(part => part.length <= 4000 && !/[\uD800-\uDBFF]$/u.test(part)));
  }
});
test("playback starts before EOF; Stop aborts and closes audio; stale playback cannot stop its replacement", async () => {
  const originalContext = Object.getOwnPropertyDescriptor(globalThis, "AudioContext"), originalFetch = globalThis.fetch;
  let started = 0, closed = 0;
  class Context {
    currentTime = 0; state = "running"; destination = {};
    async resume(): Promise<void> {} async close(): Promise<void> {closed++;}
    createBuffer(_channels: number, size: number, rate: number) {return {duration: size / rate, copyToChannel() {}};}
    createBufferSource() {return {buffer: null, connect() {}, disconnect() {}, onended: null, start() {started++;}};}
  }
  Object.defineProperty(globalThis, "AudioContext", {configurable: true, value: Context});
  const signals: AbortSignal[] = [];
  globalThis.fetch = async (_url, init) => {
    const signal = init!.signal!; signals.push(signal);
    return new Response(new ReadableStream<Uint8Array>({start(c) {
      c.enqueue(new Uint8Array(4800)); signal.addEventListener("abort", () => c.error(signal.reason), {once: true});
    }}), {headers: {"Content-Type": "application/octet-stream"}});
  };
  const player = new SpeechPlayer(() => {});
  const wait = async (predicate: () => boolean) => { for (let i = 0; i < 100; i++) {if (predicate()) return; await new Promise(r => setTimeout(r, 1));} throw Error("Player condition timed out"); };
  try {
    const first = player.play("first", "hello", SPEECH_DEFAULTS.openai, "secret");
    await wait(() => started === 1); assert.equal(player.active, "first");
    const second = player.play("second", "world", SPEECH_DEFAULTS.elevenlabs, "secret");
    await wait(() => started === 2); await first;
    assert.equal(signals[0]!.aborted, true); assert.equal(player.active, "second");
    player.stop(); await second;
    assert.equal(signals[1]!.aborted, true); assert.equal(player.active, null); assert.equal(closed, 2); assert.equal(player.error, "");
    globalThis.fetch = async () => new Response('{"error":"secret echoed by provider"}', {status: 401});
    await player.play("failed", "hello", SPEECH_DEFAULTS.openai, "secret");
    assert.match(player.error, /HTTP 401/); assert.ok(!player.error.includes("secret")); assert.equal(player.errorTarget, "failed");
    globalThis.fetch = async () => new Response("error", {headers: {"Content-Type": "application/json"}});
    await player.play("bad-format", "hello", SPEECH_DEFAULTS.openai, "secret"); assert.match(player.error, /raw 24 kHz/);
  } finally { player.stop(); globalThis.fetch = originalFetch; if (originalContext) Object.defineProperty(globalThis, "AudioContext", originalContext); else Reflect.deleteProperty(globalThis, "AudioContext"); }
});

test("WAV settings persist, old personas retain PCM, and requests ask for the file", () => {
  const wav = {...SPEECH_DEFAULTS.openai, format: "wav" as const};
  assert.deepEqual(parsePersona(JSON.stringify({...persona, speech: wav})).speech, wav);
  assert.equal(parseSpeech(SPEECH_DEFAULTS.openai).format, undefined);
  const request = speechRequest(wav, "key", "read me");
  assert.equal(JSON.parse(String(request.init.body)).response_format, "wav");
  assert.match(new Headers(request.init.headers).get("Accept")!, /audio\/wav/);
  assert.throws(() => parseSpeech({...wav, format: "mp3"}), /Choose PCM/);
  assert.throws(() => parseSpeech({...SPEECH_DEFAULTS.elevenlabs, format: "wav"}), /OpenAI/);
});
test("WAV download handles chunked headers, rejects wrong files/oversize bodies, and cancels pending reads", async () => {
  const controller = new AbortController(), header = new TextEncoder().encode("RIFF0000WAVE");
  const stream = new ReadableStream<Uint8Array>({start(c) {for (const byte of header) c.enqueue(new Uint8Array([byte])); c.close();}});
  assert.deepEqual(new Uint8Array(await readSpeechWav(new Response(stream), controller.signal, () => {})), header);
  await assert.rejects(readSpeechWav(new Response('{"url":"https://example.com/file.wav"}'), controller.signal, () => {}), /RIFF\/WAVE/);
  await assert.rejects(readSpeechWav(new Response(header, {headers: {"Content-Length": String(MAX_SPEECH_WAV_BYTES + 1)}}), controller.signal, () => {}), /64 MiB/);
  const chunk = new Uint8Array(1024 * 1024); let pulls = 0, cancelled = false;
  const oversized = new ReadableStream<Uint8Array>({pull(c) {pulls++; c.enqueue(chunk);}, cancel() {cancelled = true;}});
  await assert.rejects(readSpeechWav(new Response(oversized), controller.signal, () => {}), /64 MiB/);
  assert.ok(cancelled); assert.ok(pulls <= 66);
  let stopped = false;
  const pending = new ReadableStream<Uint8Array>({cancel() {stopped = true;}});
  const reading = readSpeechWav(new Response(pending), controller.signal, () => {});
  controller.abort(); await assert.rejects(reading, {name: "AbortError"}); assert.ok(stopped);
});
test("Stop during WAV decoding prevents late playback and releases the audio context", async () => {
  const previous = Object.getOwnPropertyDescriptor(globalThis, "AudioContext"), originalFetch = globalThis.fetch;
  let finish: ((buffer: AudioBuffer) => void) | undefined, played = false, closed = false;
  class Context {
    currentTime = 0; state = "running"; destination = {};
    async resume(): Promise<void> {} async close(): Promise<void> {closed = true;}
    decodeAudioData() { return new Promise<AudioBuffer>(resolve => { finish = resolve; }); }
    createBufferSource() {played = true; throw Error("Cancelled WAV must never create a source");}
  }
  Object.defineProperty(globalThis, "AudioContext", {configurable: true, value: Context});
  globalThis.fetch = async () => new Response("RIFF0000WAVE", {headers: {"Content-Type": "audio/wav"}});
  const player = new SpeechPlayer(() => {});
  try {
    const playing = player.play("wav", "hello", {...SPEECH_DEFAULTS.openai, format: "wav"}, "");
    for (let i = 0; i < 100 && !finish; i++) await new Promise(r => setTimeout(r, 1));
    assert.ok(finish); player.stop(); await playing;
    finish({length: 24000, duration: 1} as AudioBuffer); await Promise.resolve();
    assert.ok(closed); assert.equal(played, false); assert.equal(player.active, null); assert.equal(player.error, "");
  } finally { player.stop(); globalThis.fetch = originalFetch; if (previous) Object.defineProperty(globalThis, "AudioContext", previous); else Reflect.deleteProperty(globalThis, "AudioContext"); }
});
