import assert from "node:assert/strict";
import test from "node:test";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

const stage = process.argv[2];
assert.ok(stage, "pass the compiled web distribution");
globalThis.window = { requestAnimationFrame: (callback) => setTimeout(callback, 0) };
const moduleUrl = (path) => pathToFileURL(resolve(stage, path)).href;
const { validFilePath, parseFileList, parseFileRead, parseFileError, FILE_VIEW_BYTES } = await import(moduleUrl("src/files/protocol.js"));
const { FileClient, boundedResponseBytes } = await import(moduleUrl("src/files/client.js"));
const { FilesController } = await import(moduleUrl("src/files/state.js"));
const { renderFilesPane } = await import(moduleUrl("src/files/view.js"));
const { renderChatView } = await import(moduleUrl("src/app/chat-view.js"));
const { historyRecallMatches } = await import(moduleUrl("src/app/history-recall.js"));
const { fileLanguage } = await import(moduleUrl("src/files/editor.js"));
const counts = { protected: 0, unsupported: 0, unreadable: 0 };
const list = (path = "", entries = []) => ({ path, entries, truncated: false, exclusions: counts });
const file = (path, content = "text") => ({ path, content, size: String(new TextEncoder().encode(content).length) });
const entry = (path) => ({ path, name: path.split("/").at(-1), kind: "file", size: "4" });
const json = (value, options = {}) => new Response(JSON.stringify(value), { ...options, headers: { "Content-Type": "application/json", ...options.headers } });
const signal = () => new AbortController().signal;
const deferred = () => {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
};
const flush = async () => { for (let i = 0; i < 6; i += 1) await Promise.resolve(); };
function* nodes(node) {
  if (node === null || typeof node !== "object") return;
  yield node;
  for (const child of node.children ?? []) yield* nodes(child);
}
const actions = { toggle() {}, navigate() {}, select() {}, refresh() {}, download() {}, copyPath() {} };

test("file paths and exact response shapes are confined before display", () => {
  for (const path of ["/abs", "..", "a/../b", "a/./b", "a//b", "a/", "C:/x", "a/C:/x", "a\\b", "a\0b", "a\u0085b", "a/".repeat(65) + "x", "x".repeat(4097)]) {
    assert.equal(validFilePath(path), false, path);
  }
  assert.equal(validFilePath(""), true);
  assert.equal(validFilePath("", false), false);
  assert.equal(validFilePath("src/名字.txt", false), true);
  assert.ok(parseFileList(list("src", [entry("src/main.rs")]), "src"));
  assert.equal(parseFileList(list("src", [entry("other/main.rs")]), "src"), null);
  assert.equal(parseFileList(list("", [entry("x"), entry("x")]), ""), null);
  assert.equal(parseFileList({ ...list(), token: "unexpected" }, ""), null);
  assert.equal(parseFileList({ ...list(), exclusions: { ...counts, protected: -1 } }, ""), null);
  assert.equal(parseFileList(list("", Array.from({ length: 1001 }, (_, i) => entry(String(i)))), ""), null);
});

test("UTF-8 previews are exact, bounded, and never silently truncated", () => {
  const value = file("x.rs", "名字🙂\n");
  assert.deepEqual(parseFileRead(value, "x.rs"), value);
  assert.equal(parseFileRead({ ...value, size: String(value.content.length) }, "x.rs"), null);
  assert.equal(parseFileRead(value, "y.rs"), null);
  assert.equal(parseFileRead(file("x", "a".repeat(FILE_VIEW_BYTES + 1)), "x"), null);
});

test("HTTP reader enforces announced and streamed bounds including fragmented input", async () => {
  await assert.rejects(boundedResponseBytes(new Response("x", { headers: { "Content-Length": "99" } }), 8, signal()), { code: "too_large" });
  await assert.rejects(boundedResponseBytes(new Response("xxx"), 2, signal()), { code: "too_large" });
  await assert.rejects(boundedResponseBytes(new Response("x", { headers: { "Content-Length": "2" } }), 8, signal()), { code: "unavailable" });
  let sent = 0;
  const stream = new ReadableStream({ pull(controller) {
    if (sent++ === 1000) controller.close(); else controller.enqueue(Uint8Array.of(65));
  } });
  const result = await boundedResponseBytes(new Response(stream), 1000, signal());
  assert.equal(result.byteLength, 1000);
  assert.equal(result.buffer.byteLength, 1000);
  assert.equal(result.every((byte) => byte === 65), true);
});

test("file client validates requests, errors, and binary attachment metadata", async () => {
  let calls = 0;
  const client = new FileClient(async (_endpoint, path) => { calls += 1; return json(file(path)); });
  await assert.rejects(client.read("../secret", signal()), { code: "bad_request" });
  assert.equal(calls, 0);
  assert.equal((await client.read("x", signal())).content, "text");
  assert.equal(parseFileError({ error: { code: "protected", message: "LEAK /private/path", retryable: false } }).message.includes("LEAK"), false);
  const zip = new FileClient(async () => new Response(Uint8Array.of(80, 75, 3, 4), { headers: {
    "Content-Type": "application/zip", "Content-Disposition": 'attachment; filename="folder.zip"',
    "x-lethetic-files-excluded-protected": "2", "x-lethetic-files-excluded-unsupported": "1", "x-lethetic-files-excluded-unreadable": "0",
  } }));
  const download = await zip.download("", true, signal());
  assert.equal(download.filename, "folder.zip");
  assert.equal(download.blob.size, 4);
  assert.equal(download.exclusions.protected, 2);
  const unsafe = new FileClient(async () => new Response("oops", { headers: {
    "Content-Type": "text/html", "Content-Disposition": 'attachment; filename="../escape.html"',
  } }));
  await assert.rejects(unsafe.download("x", false, signal()), { code: "unavailable" });
});

test("obsolete reads and authentication reset cannot replace a new selection", async () => {
  const reads = [];
  const controller = new FilesController(() => {}, () => {});
  controller.attach({ list: async () => list(), read(path, signal) {
    const pending = deferred(); reads.push({ ...pending, path, signal }); return pending.promise;
  } });
  controller.toggle(); await flush();
  const first = controller.select("first");
  const second = controller.select("second");
  assert.equal(reads[0].signal.aborted, true);
  reads[1].resolve(file("second", "new")); await second;
  reads[0].resolve(file("first", "old")); await first;
  assert.equal(controller.state.preview.content, "new");
  const oldEpoch = controller.select("third");
  controller.reset();
  reads[2].resolve(file("third", "private")); await oldEpoch;
  assert.equal(controller.state.preview, null);
  assert.equal(controller.state.open, false);
});

test("an obsolete refresh cannot override a newer directory or selected file", async () => {
  const pending = deferred(); let lists = 0;
  const controller = new FilesController(() => {}, () => {});
  controller.attach({ list: async (path) => ++lists === 2 ? pending.promise : list(path, [entry("first"), entry("second")]), read: async (path) => file(path) });
  controller.toggle(); await flush();
  await controller.select("first");
  const refresh = controller.refresh();
  await controller.navigate("");
  await controller.select("second");
  pending.resolve(list("", [entry("first"), entry("second")])); await refresh;
  assert.equal(controller.state.selectedPath, "second");
});

test("copy path only writes the literal root-relative path on explicit action", async () => {
  const original = Object.getOwnPropertyDescriptor(globalThis, "navigator");
  const copied = [];
  Object.defineProperty(globalThis, "navigator", { configurable: true, value: { clipboard: { writeText: async (path) => { copied.push(path); } } } });
  try {
    const controller = new FilesController(() => {}, () => {});
    controller.attach({ list: async () => list(), read: async (path) => file(path) });
    controller.toggle(); await flush();
    await controller.select("src/名字.rs");
    assert.deepEqual(copied, []);
    await controller.copyPath();
    assert.deepEqual(copied, ["src/名字.rs"]);
  } finally {
    if (original) Object.defineProperty(globalThis, "navigator", original); else delete globalThis.navigator;
  }
});

test("pane is above chat, collapses without an editor, and never renders file text as HTML", () => {
  const controller = new FilesController(() => {}, () => {});
  const closed = renderFilesPane(controller.state, true, actions);
  assert.equal([...nodes(closed)].some((node) => node.data?.attrs?.id === "files-monaco"), false);
  const hostile = '<img src=x onerror="evil()"><script>evil()</script>';
  const pane = renderFilesPane({ ...controller.state, open: true, preview: file("x.html", hostile), selectedPath: "x.html" }, true, actions);
  assert.equal(JSON.stringify(pane).includes(hostile), false);
  assert.equal([...nodes(pane)].some((node) => Object.hasOwn(node.data?.props ?? {}, "innerHTML")), false);
  assert.ok([...nodes(pane)].some((node) => node.data?.attrs?.id === "files-monaco"));
  const tree = renderChatView({ state: { snapshot: null, live: false, activePanel: null,
    transportStatus: { phase: "idle", label: "Waiting", attempt: 0, retryInMilliseconds: null },
    remoteReason: null, pendingCount: 0, debuggerWide: true, debuggerDrawerDismissed: false,
    draft: "", chatStart: 0, toast: null, filesPane: pane, overlay: null }, actions: {} });
  const column = [...nodes(tree)].find((node) => node.data?.attrs?.class === "conversation-column has-files");
  assert.equal(column.children[0], pane);
  assert.equal(column.children[1].data.attrs.id, "chat-scroll");
});

test("history recall needs exact request, entry, session and unchanged draft generation", () => {
  const pending = { requestId: "req", sessionId: "session", entryId: "entry", draftVersion: 4 };
  const request = { id: "req", expected_revision: 1, type: "select_history_entry", session_id: "session", entry_id: "entry" };
  const outcome = { type: "history_entry_selected", session_id: "session", entry_id: "entry", editor_content: "complete\n".repeat(1000) };
  const matches = (p = pending, o = outcome, r = request, id = "req", session = "session", version = 4) => historyRecallMatches(p, o, r, id, session, version);
  assert.equal(matches(), true);
  assert.equal(matches(null), false);
  assert.equal(matches({ ...pending, requestId: "newer" }), false);
  assert.equal(matches(pending, { ...outcome, entry_id: "other" }), false);
  assert.equal(matches(pending, outcome, request, "req", "new-session"), false);
  assert.equal(matches(pending, outcome, request, "req", "session", 5), false);
});

test("file language detection uses only registered local filename and extension mappings", () => {
  const languages = [{ id: "rust", extensions: [".rs"] }, { id: "dockerfile", filenames: ["Dockerfile"] }];
  assert.equal(fileLanguage("src/main.rs", languages), "rust");
  assert.equal(fileLanguage("build/Dockerfile", languages), "dockerfile");
  assert.equal(fileLanguage("unknown.binary", languages), "plaintext");
});
