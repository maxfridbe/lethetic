import { downloadBlob } from "../../web/src/download.js";
import { TRACE_LIMITS } from "./provider-trace.js";
import type { SavedSession } from "./sessions.js";
import { decodeBytes, emptyWorkspace, parseWorkspace } from "./workspace.js";

const encoder = new TextEncoder();
function redact(text: string, keys: readonly string[]): string {
  for (const key of keys) if (key) text = text.replaceAll(key, "[REDACTED]");
  return text;
}
function clean(value: unknown, keys: readonly string[]): unknown {
  if (typeof value === "string") return redact(value, keys);
  if (Array.isArray(value)) return value.map(item => clean(item, keys));
  if (value && typeof value === "object") return Object.fromEntries(Object.entries(value)
    .filter(([key]) => !["apiKey", "authorization", "password", "token"].includes(key))
    .map(([key, item]) => [key, clean(item, keys)]));
  return value;
}
function header(name: string, size: number, type: string, time: number): Uint8Array<ArrayBuffer> {
  const bytes = new Uint8Array(512);
  const write = (offset: number, text: string): void => { bytes.set(encoder.encode(text), offset); };
  const octal = (offset: number, length: number, value: number): void => write(offset, value.toString(8).padStart(length - 1, "0") + "\0");
  write(0, name); octal(100, 8, type === "5" ? 0o755 : 0o644); octal(108, 8, 0); octal(116, 8, 0);
  octal(124, 12, size); octal(136, 12, time); write(148, "        "); write(156, type); write(257, "ustar\0"); write(263, "00");
  write(148, bytes.reduce((sum, byte) => sum + byte, 0).toString(8).padStart(6, "0") + "\0 ");
  return bytes;
}
function pathRecord(path: string): Uint8Array<ArrayBuffer> {
  const payload = ` path=${path}\n`; let length = encoder.encode(payload).length + 1;
  while (encoder.encode(String(length) + payload).length !== length) length = encoder.encode(String(length) + payload).length;
  return encoder.encode(String(length) + payload);
}

/** Portable POSIX pax tar, compressed by the browser; no server upload. */
export async function sessionDebugBundle(session: SavedSession, diagnostics: unknown, keys: readonly string[] = []): Promise<Blob> {
  const workspace = parseWorkspace(session.workspace ?? emptyWorkspace());
  const parts: BlobPart[] = []; let count = 0; const time = Math.floor(Date.now() / 1000);
  const member = (path: string, data: Uint8Array<ArrayBuffer>, directory = false): void => {
    const pax = pathRecord(path);
    parts.push(header(`PaxHeaders/${count}`, pax.length, "x", time), pax, new Uint8Array((512 - pax.length % 512) % 512));
    parts.push(header(`entry-${count++}`, data.length, directory ? "5" : "0", time), data, new Uint8Array((512 - data.length % 512) % 512));
  };
  const json = (value: unknown): Uint8Array<ArrayBuffer> => encoder.encode(JSON.stringify(clean(value, keys), null, 2) + "\n");
  const { workspace: _workspace, providerTrace, ...metadata } = session;
  member("provider-traces.jsonl", encoder.encode((providerTrace?.requests ?? []).map(trace => JSON.stringify(clean(trace, keys))).join("\n") + "\n"));
  member("session.json", json(metadata));
  member("diagnostics.json", json({ formatVersion: 2, providerTrace: { limits: TRACE_LIMITS, capturedRequests: providerTrace?.requests.length ?? 0, omittedRequests: providerTrace?.omittedRequests ?? 0 }, exportedAt: new Date().toISOString(), sessionId: session.id, diagnostics,
    workspace: { root: session.mode === "dotnet-wasm" ? "/home/dotnetdide" : "/home/pyodide", directories: workspace.directories, files: workspace.files.map(file => ({ path: file.path, bytes: decodeBytes(file.data).length })) } }));
  member("README.txt", encoder.encode("Lethetic session debug bundle\n\nsession.json: transcript, model history and session settings.\ndiagnostics.json: browser/runtime state and workspace inventory.\nworkspace/: files from the last completed code checkpoint, including browser edits.\nprovider-traces.jsonl: redacted request bodies and raw response text captured before parsing, including failed attempts.\nResponse head and tail are contiguous only when responseOmittedBytes is zero. Omission counts measure UTF-8 bytes after redaction; receivedBytes is before redaction. responseComplete means EOF or protocol end was observed; false means capture ended early. Pending attempts may still be running or were interrupted by closing the page.\nCapture retains at most 16 attempts / 8 MiB per session, 128 KiB request and 1 MiB response per attempt. Older requests may be evicted. Older sessions have no retroactive stream capture. HTTP error bodies retain at most 64 KiB (head/tail of received text); responseComplete is false when reading stops at this limit.\nConnection keys are excluded; known keys are redacted from text.\n"));
  for (const directory of workspace.directories) member(`workspace/${directory}/`, new Uint8Array(), true);
  for (const file of workspace.files) {
    let bytes = decodeBytes(file.data);
    try {
      const text = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes);
      bytes = encoder.encode(redact(text, keys));
    } catch { /* Binary files are preserved byte for byte. */ }
    member(`workspace/${file.path}`, bytes);
  }
  parts.push(new Uint8Array(1024));
  const compressed = new Blob(parts).stream().pipeThrough(new CompressionStream("gzip"));
  return new Blob([await new Response(compressed).arrayBuffer()], { type: "application/gzip" });
}
export function downloadSessionBundle(bundle: Blob, id: string): Promise<boolean> {
  return downloadBlob(bundle, `lethetic-session-${id}.tar.gz`);
}
