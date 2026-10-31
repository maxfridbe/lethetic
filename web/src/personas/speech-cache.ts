/** Bounded, per-tab audio cache for browser/desktop playback; Android owns its disk cache. */
export class SpeechAudioCache {
  #files = new Map<string, Blob>();
  get(id: string): Blob | undefined { return this.#files.get(id); }
  put(id: string, chunks: Uint8Array<ArrayBuffer>[], rate: number): void {
    const size = chunks.reduce((n, chunk) => n + chunk.length, 0);
    if (size > 64 * 1024 * 1024) return;
    const header = new Uint8Array(44), view = new DataView(header.buffer);
    for (const [offset, text] of [[0, "RIFF"], [8, "WAVEfmt "], [36, "data"]] as const) header.set(new TextEncoder().encode(text), offset);
    view.setUint32(4, size + 36, true); view.setUint32(16, 16, true); view.setUint16(20, 1, true); view.setUint16(22, 1, true);
    view.setUint32(24, rate, true); view.setUint32(28, rate * 2, true); view.setUint16(32, 2, true); view.setUint16(34, 16, true); view.setUint32(40, size, true);
    this.#files.delete(id); this.#files.set(id, new Blob([header, ...chunks], {type: "audio/wav"}));
    let total = [...this.#files.values()].reduce((n, file) => n + file.size, 0);
    for (const [key, file] of this.#files) { if (total <= 128 * 1024 * 1024) break; this.#files.delete(key); total -= file.size; }
  }
}
export function pcm16(samples: Float32Array): Uint8Array<ArrayBuffer> {
  const bytes = new Uint8Array(samples.length * 2), view = new DataView(bytes.buffer);
  samples.forEach((sample, i) => view.setInt16(i * 2, Math.max(-32768, Math.min(32767, Math.round(sample * 32768))), true));
  return bytes;
}
