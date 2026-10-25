import assert from "node:assert/strict";
import test from "node:test";
import { downloadBlob } from "../../web/src/download.js";

async function withHost(invoke: (command: string, args: Record<string, unknown>) => Promise<unknown>, run: () => Promise<void>) {
  const old = Object.getOwnPropertyDescriptor(globalThis, "window");
  const host = { top: null as unknown, __LETHETIC_HOST_TOKEN__: "main-window-only", __TAURI__: { core: { invoke } } }; host.top = host;
  Object.defineProperty(globalThis, "window", { configurable: true, value: host });
  try { await run(); } finally { if (old) Object.defineProperty(globalThis, "window", old); else Reflect.deleteProperty(globalThis, "window"); }
}
await test("native downloads send bounded binary chunks and only finish after all bytes", async () => {
  const bytes = Uint8Array.from({length: 700_000}, (_, i) => i % 256), received: number[] = [], calls: string[] = [];
  await withHost(async (command, args) => {
    assert.equal(args.token, "main-window-only"); calls.push(command.split("|")[1]!);
    switch (calls.at(-1)) {
      case "begin_download": assert.equal(args.name, "binary.zip"); assert.equal(args.size, bytes.length); return "opaque-transfer";
      case "write_download": {
        assert.equal(args.id, "opaque-transfer"); assert.equal(args.offset, received.length);
        const decoded = atob(String(args.data)); assert.ok(decoded.length <= 256 * 1024);
        for (const byte of decoded) received.push(byte.charCodeAt(0)); return null;
      }
      case "finish_download": assert.deepEqual(new Uint8Array(received), bytes); return true;
      default: return null;
    }
  }, async () => { assert.equal(await downloadBlob(new Blob([bytes]), "binary.zip"), true); });
  assert.deepEqual(calls, ["begin_download", "write_download", "write_download", "write_download", "finish_download", "cancel_download"]);
});
await test("native cancellation returns false and write failures clean staging without browser fallback", async () => {
  let cancelled = false;
  await withHost(async (command) => {
    if (command.endsWith("begin_download")) return "cancel";
    if (command.endsWith("finish_download")) return false;
    if (command.endsWith("cancel_download")) cancelled = true;
    return null;
  }, async () => { assert.equal(await downloadBlob(new Blob([]), "empty.txt"), false); });
  assert.equal(cancelled, true); cancelled = false;
  await withHost(async (command) => {
    if (command.endsWith("begin_download")) return "fail";
    if (command.endsWith("write_download")) throw new Error("Disk full");
    if (command.endsWith("finish_download")) throw new Error("Must not finish");
    if (command.endsWith("cancel_download")) cancelled = true;
    return null;
  }, async () => { await assert.rejects(downloadBlob(new Blob(["data"]), "file.txt"), /Disk full/); });
  assert.equal(cancelled, true);
});
await test("aborted native saves do not open a picker", async () => {
  const controller = new AbortController(); let finished = false, cancelled = false;
  await withHost(async (command) => {
    if (command.endsWith("begin_download")) { controller.abort(); return "aborted"; }
    if (command.endsWith("finish_download")) finished = true;
    if (command.endsWith("cancel_download")) cancelled = true;
    return null;
  }, async () => { await assert.rejects(downloadBlob(new Blob(["data"]), "file.txt", controller.signal), /abort/i); });
  assert.equal(finished, false); assert.equal(cancelled, true);
});

await test("a missing native bridge never silently falls back to a WebView anchor", async () => {
  await withHost(async () => null, async () => {
    Reflect.deleteProperty(window, "__TAURI__");
    await assert.rejects(downloadBlob(new Blob(["data"]), "file.txt"), /Native save dialog is unavailable/);
  });
});
