import { androidHost } from "../native-credentials.js";
import { downloadBlob } from "../download.js";
import { AndroidSpeech } from "./speech-android.js";
import { SpeechAudioCache, pcm16 } from "./speech-cache.js";
import { fetchSpeech } from "./speech-transport.js";
import type { PersonaSpeech } from "./schema.js";

export const SPEECH_DEFAULTS: Record<PersonaSpeech["provider"], PersonaSpeech> = {
  openai: {enabled: false, provider: "openai", endpoint: "https://api.openai.com/v1/audio/speech", model: "gpt-4o-mini-tts", voice: "coral"},
  elevenlabs: {enabled: false, provider: "elevenlabs", endpoint: "https://api.elevenlabs.io/v1/text-to-speech/{voice_id}/stream", model: "eleven_multilingual_v2", voice: "JBFqnCBsd6RMkjVDRZzb"},
};
export function speechRequest(settings: PersonaSpeech, key: string, input: string): {url: string; init: RequestInit} {
  const headers: Record<string, string> = {"Content-Type": "application/json", "Accept": settings.format === "wav" ? "audio/wav, application/octet-stream" : "application/octet-stream"};
  if (key) headers[settings.provider === "openai" ? "Authorization" : "xi-api-key"] = settings.provider === "openai" ? `Bearer ${key}` : key;
  const url = settings.provider === "openai" ? settings.endpoint : `${settings.endpoint.replace("{voice_id}", encodeURIComponent(settings.voice))}?output_format=pcm_24000`;
  const body = settings.provider === "openai" ? {model: settings.model, voice: settings.voice, input, response_format: settings.format ?? "pcm"} : {model_id: settings.model, text: input};
  return {url, init: {method: "POST", headers, body: JSON.stringify(body), credentials: "omit", redirect: "error", cache: "no-store", referrerPolicy: "no-referrer"}};
}
/** Keep requests under provider text limits without dropping text or splitting surrogate pairs. */
export function splitSpeechText(text: string): string[] {
  const result: string[] = [];
  while (text.length > 4000) {
    let end = Math.max(text.lastIndexOf("\n", 4000), text.lastIndexOf(". ", 4000), text.lastIndexOf(" ", 4000));
    if (end < 2000) end = 4000;
    if (/[\uD800-\uDBFF]/u.test(text[end - 1]!)) end--;
    result.push(text.slice(0, end)); text = text.slice(end);
  }
  if (text) result.push(text);
  return result;
}
/** Raw 24 kHz signed little-endian PCM; HTTP chunks need not end on a sample boundary. */
export class PcmDecoder {
  #low: number | undefined;
  decode(bytes: Uint8Array): Float32Array<ArrayBuffer> {
    const samples = new Float32Array(Math.floor((bytes.length + (this.#low === undefined ? 0 : 1)) / 2));
    let offset = 0, sample = 0;
    const value = (low: number, high: number): number => ((low | high << 8) << 16 >> 16) / 32768;
    if (this.#low !== undefined && bytes.length) { samples[sample++] = value(this.#low, bytes[offset++]!); this.#low = undefined; }
    while (offset + 1 < bytes.length) { samples[sample++] = value(bytes[offset]!, bytes[offset + 1]!); offset += 2; }
    if (offset < bytes.length) this.#low = bytes[offset];
    return samples;
  }
  finish(): void { if (this.#low !== undefined) throw Error("Speech stream ended with an incomplete PCM sample."); }
}
function pause(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    const abort = (): void => { clearTimeout(timer); reject(signal.reason); };
    const timer = setTimeout(() => { signal.removeEventListener("abort", abort); resolve(); }, ms);
    signal.addEventListener("abort", abort, {once: true});
    if (signal.aborted) { signal.removeEventListener("abort", abort); abort(); }
  });
}
function abortable<T>(promise: Promise<T>, signal: AbortSignal): Promise<T> {
  return new Promise((resolve, reject) => {
    const abort = (): void => { reject(signal.reason); };
    signal.addEventListener("abort", abort, {once: true});
    promise.then(resolve, reject).finally(() => signal.removeEventListener("abort", abort));
    if (signal.aborted) { signal.removeEventListener("abort", abort); abort(); }
  });
}
/** WAV decoding needs a complete file. Bound encoded buffering per speech request. */
export const MAX_SPEECH_WAV_BYTES = 64 * 1024 * 1024;
export async function readSpeechWav(response: Response, signal: AbortSignal, progress: () => void): Promise<ArrayBuffer> {
  if (!response.body) throw Error("Speech endpoint returned no WAV file.");
  if (Number(response.headers.get("Content-Length")) > MAX_SPEECH_WAV_BYTES) {
    await response.body.cancel(); throw Error("Speech WAV file exceeds 64 MiB. Use shorter responses or PCM streaming.");
  }
  const reader = response.body.getReader(), chunks: Uint8Array[] = [];
  let size = 0;
  try {
    while (true) {
      signal.throwIfAborted(); progress();
      const {value, done} = await abortable(reader.read(), signal);
      if (done) break;
      size += value.length;
      if (size > MAX_SPEECH_WAV_BYTES) throw Error("Speech WAV file exceeds 64 MiB. Use shorter responses or PCM streaming.");
      if (value.length) chunks.push(value);
    }
    const bytes = new Uint8Array(size); let offset = 0;
    for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.length; }
    if (size < 12 || String.fromCharCode(...bytes.subarray(0, 4)) !== "RIFF" || String.fromCharCode(...bytes.subarray(8, 12)) !== "WAVE") throw Error("Speech endpoint did not return a RIFF/WAVE file. Select a binary WAV response, not JSON or a file URL.");
    return bytes.buffer;
  } finally { await reader.cancel().catch(() => {}); reader.releaseLock(); }
}
export class SpeechPlayer {
  active: string | null = null;
  error = "";
  errorTarget: string | null = null;
  #request: AbortController | null = null;
  #context: AudioContext | null = null;
  #cache = new SpeechAudioCache();
  #native: AndroidSpeech | undefined;
  constructor(readonly changed: () => void) {
    if (androidHost()) this.#native = new AndroidSpeech(() => {
      if (!this.#native) return;
      this.active = this.#native.active; this.error = this.#native.error; this.errorTarget = this.#native.errorTarget; this.changed();
    });
  }
  background(): void { if (!this.#native) this.stop(false); }
  dispose(): void { if (this.#native) this.#native.dispose(); else this.stop(false); }
  downloadable(id: string): boolean { return this.#native ? this.#native.available(id) : !!this.#cache.get(id); }
  async download(id: string): Promise<void> {
    try {
      if (this.#native) await this.#native.download(id);
      else { const file = this.#cache.get(id); if (!file) throw Error("Play the complete response first to download its audio."); await downloadBlob(file, "lethetic-response.wav"); }
    } catch (error) { this.error = error instanceof Error ? error.message : "Audio download failed."; this.errorTarget = id; this.changed(); }
  }
  stop(notify = true): void {
    this.#native?.stop();
    this.#request?.abort(); this.#request = null;
    if (this.#context) void this.#context.close().catch(() => {});
    this.#context = null; this.active = null;
    if (notify) this.changed();
  }
  async play(id: string, text: string, settings: PersonaSpeech, key: string): Promise<void> {
    if (this.#native) {
      const requests = splitSpeechText(text).map(input => { const {url, init} = speechRequest(settings, key, input); return {url, headers: init.headers, body: init.body}; });
      await this.#native.play(id, settings.format ?? "pcm", requests); return;
    }
    this.stop(false); this.error = ""; this.errorTarget = null;
    const request = new AbortController(); this.#request = request; this.active = id;
    const signal = request.signal;
    let idle: ReturnType<typeof setTimeout> | undefined, timedOut = false;
    const arm = (): void => { clearTimeout(idle); idle = setTimeout(() => { timedOut = true; request.abort(); }, 60000); };
    try {
      if (!text.trim()) throw Error("This response has no text to read.");
      const context = new AudioContext(); this.#context = context;
      // Resume synchronously from Play's user gesture, before any fetch.
      const resumed = context.resume(); this.changed(); arm(); await abortable(resumed, signal); clearTimeout(idle);
      signal.throwIfAborted();
      let next = context.currentTime + .08, totalSamples = 0;
      let captured: Uint8Array<ArrayBuffer>[] = [], capturedSize = 0, rate = 24000;
      const capture = (samples: Float32Array): void => {
        capturedSize += samples.length * 2;
        if (capturedSize <= 64 * 1024 * 1024) captured.push(pcm16(samples)); else captured = [];
      };
      for (const input of splitSpeechText(text)) {
        const {url, init} = speechRequest(settings, key, input);
        arm(); const response = await fetchSpeech(url, init, signal); clearTimeout(idle);
        if (!response.ok) {
          await response.body?.cancel();
          throw Error(`Speech endpoint returned HTTP ${response.status}. Check the API key, voice, model and account quota.`);
        }
        const contentType = response.headers.get("Content-Type")?.split(";", 1)[0]?.trim() ?? "";
        if (settings.format === "wav") {
          if (contentType && !/^(application\/octet-stream|audio\/(wav|wave|x-wav|vnd\.wave))$/iu.test(contentType)) {
            await response.body?.cancel(); throw Error("Speech endpoint must return a binary WAV file, not JSON or a file URL.");
          }
          arm();
          const file = await readSpeechWav(response, signal, arm); clearTimeout(idle);
          let buffer: AudioBuffer;
          try { buffer = await abortable(context.decodeAudioData(file), signal); }
          catch { signal.throwIfAborted(); throw Error("Speech WAV file is corrupt or uses a codec this browser cannot decode."); }
          signal.throwIfAborted();
          if (!buffer.length) throw Error("Speech endpoint returned an empty WAV recording.");
          rate = buffer.sampleRate;
          const mono = new Float32Array(buffer.length);
          for (let channel = 0; channel < buffer.numberOfChannels; channel++) { const values = buffer.getChannelData(channel); for (let i = 0; i < mono.length; i++) mono[i]! += values[i]! / buffer.numberOfChannels; }
          capture(mono);
          const source = context.createBufferSource(); source.buffer = buffer; source.connect(context.destination);
          source.onended = () => source.disconnect();
          next = Math.max(next, context.currentTime + .02); source.start(next); next += buffer.duration; totalSamples += buffer.length;
          // Finish this recording before downloading another, so long responses do not queue whole files.
          while (context.currentTime < next) {
            if (context.state !== "running") throw Error("Audio playback was interrupted. Press Play to restart.");
            await pause(50, signal);
          }
          continue;
        }
        if (!response.body || (contentType && !/^(application\/octet-stream|audio\/(pcm|x-pcm|raw))$/iu.test(contentType))) {
          await response.body?.cancel(); throw Error("Speech endpoint must return raw 24 kHz, 16-bit mono PCM audio.");
        }
        const reader = response.body.getReader(), decoder = new PcmDecoder();
        try {
          while (true) {
            arm(); const {value, done} = await reader.read(); clearTimeout(idle); signal.throwIfAborted();
            if (done) break;
            // Decode small slices and keep at most ~2 seconds scheduled, even for large HTTP chunks.
            for (let start = 0; start < value.length; start += 9600) {
              while (next - context.currentTime > 2) {
                if (context.state !== "running") throw Error("Audio playback was interrupted. Press Play to restart.");
                await pause(50, signal);
              }
              signal.throwIfAborted();
              const samples = decoder.decode(value.subarray(start, start + 9600));
              if (!samples.length) continue;
              capture(samples);
              const buffer = context.createBuffer(1, samples.length, 24000); buffer.copyToChannel(samples, 0);
              const source = context.createBufferSource(); source.buffer = buffer; source.connect(context.destination);
              source.onended = () => source.disconnect();
              next = Math.max(next, context.currentTime + .02); source.start(next); next += buffer.duration; totalSamples += samples.length;
            }
          }
          decoder.finish();
        } finally { await reader.cancel().catch(() => {}); reader.releaseLock(); }
      }
      if (!totalSamples) throw Error("Speech endpoint returned no audio.");
      if (captured.length) { this.#cache.put(id, captured, rate); this.changed(); }
      while (context.currentTime < next) {
        if (context.state !== "running") throw Error("Audio playback was interrupted. Press Play to restart.");
        await pause(50, signal);
      }
    } catch (error) {
      if (this.#request === request && (!signal.aborted || timedOut)) {
        // Never expose endpoint credentials or untrusted response bodies through the UI/report.
        this.error = timedOut ? "Speech endpoint did not respond for 60 seconds. Press Play to retry." : error instanceof TypeError ? "Speech request failed. Check endpoint connectivity and browser CORS access." : error instanceof Error ? error.message.replaceAll(key || "\0", "[redacted]") : "Speech playback failed.";
        this.errorTarget = id;
      }
    } finally {
      clearTimeout(idle);
      if (this.#request === request) this.stop();
    }
  }
}
