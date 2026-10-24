import { abortable } from "./async.js";

export type StreamDelta = { type: "assistant_delta" | "thinking_delta"; text: string };
export interface WasmStream {
  push(data: string): string;
  finish(): string;
  free(): void;
}

export interface ResponseObserver { chunk(text: string, bytes: number): void; received(): void }

/** Decode UTF-8 and SSE framing here; provider parsing/validation uses Rust/WASM. */
export async function readModelResponse(response: Response, signal: AbortSignal,
  createStream: () => WasmStream, onDelta: (event: StreamDelta) => void, observer?: ResponseObserver): Promise<string> {
  const reader = response.body?.getReader();
  if (!reader) throw new Error("The model returned an empty response.");
  const stream = response.headers.get("Content-Type")?.split(";")[0]?.trim().toLowerCase() === "text/event-stream" ? createStream() : null;
  const decoder = new TextDecoder();
  let bytes = 0, buffer = "", data: string[] = [], completed = false;
  const dispatch = (): void => {
    if (!data.length) return;
    const event = JSON.parse(stream!.push(data.join("\n"))) as { updates: StreamDelta[]; done: boolean };
    data = [];
    for (const delta of event.updates) onDelta(delta);
    completed = event.done;
  };
  const lines = (eof: boolean): void => {
    while (!completed) {
      const end = buffer.search(/[\r\n]/);
      if (end < 0 || (!eof && end === buffer.length - 1 && buffer[end] === "\r")) break;
      const line = buffer.slice(0, end);
      buffer = buffer.slice(end + (buffer.slice(end, end + 2) === "\r\n" ? 2 : 1));
      if (!line) dispatch();
      else if (line.startsWith("data:")) data.push(line.slice(5).replace(/^ /, ""));
    }
    if (eof && !completed) {
      if (buffer.startsWith("data:")) data.push(buffer.slice(5).replace(/^ /, ""));
      buffer = ""; dispatch();
    }
  };
  try {
    while (true) {
      signal.throwIfAborted();
      const chunk = await abortable(reader.read(), signal);
      if (chunk.done) {
        const final = decoder.decode();
        observer?.chunk(final, 0); observer?.received();
        buffer += final;
        if (!stream) return buffer;
        lines(true); return stream.finish();
      }
      const text = decoder.decode(chunk.value, { stream: true });
      observer?.chunk(text, chunk.value.byteLength);
      bytes += chunk.value.byteLength;
      if (bytes > 4 * 1024 * 1024) throw new Error("Model response exceeds 4 MiB.");
      buffer += text;
      if (stream) {
        lines(false);
        if (completed) { observer?.received(); return stream.finish(); }
      }
    }
  } finally {
    stream?.free();
    await reader.cancel().catch(() => {}); reader.releaseLock();
  }
}

/** Error bodies are useful diagnostics, but never need the full model-response budget. */
export async function captureErrorResponse(response: Response, signal: AbortSignal, observer: ResponseObserver): Promise<void> {
  const reader = response.body?.getReader();
  if (!reader) { observer.received(); return; }
  const decoder = new TextDecoder(); let received = 0;
  // An error response must not hold up retry just because its body never closes.
  const deadline = new AbortController();
  const abort = (): void => deadline.abort(signal.reason);
  signal.addEventListener("abort", abort, {once:true});
  if (signal.aborted) abort();
  const timer = setTimeout(() => deadline.abort(new Error("Diagnostic body read timed out.")), 2000);
  try {
    while (received < 64 * 1024) {
      const chunk = await abortable(reader.read(), deadline.signal);
      if (chunk.done) { observer.chunk(decoder.decode(), 0); observer.received(); return; }
      // The trace window bounds storage even if the final network chunk exceeds this read budget.
      observer.chunk(decoder.decode(chunk.value, {stream:true}), chunk.value.byteLength);
      received += chunk.value.byteLength;
    }
    observer.chunk(decoder.decode(), 0);
  } finally {
    clearTimeout(timer); signal.removeEventListener("abort", abort);
    await reader.cancel().catch(() => {}); reader.releaseLock();
  }
}
