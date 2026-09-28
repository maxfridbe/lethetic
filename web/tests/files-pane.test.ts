import "./support/files-globals.js";
import assert from "node:assert/strict";
import test from "node:test";
import { renderChatView } from "../src/app/chat-view.js";
import { historyRecallMatches, type PendingHistoryRecall } from "../src/app/history-recall.js";
import { FileClient, boundedResponseBytes } from "../src/files/client.js";
import { fileLanguage } from "../src/files/editor.js";
import {
  FILE_VIEW_BYTES,
  parseFileError,
  parseFileList,
  parseFileRead,
  validFilePath,
} from "../src/files/protocol.js";
import { changeRows, parseGitDiff, parseGitStatus } from "../src/files/git.js";
import { FilesController } from "../src/files/state.js";
import { renderFilesPane, type FilePaneActions } from "../src/files/view.js";
import type {
  CommandOutcome,
  FileListEntry,
  FilesListResponse,
  FilesReadResponse,
  ICommandRequest,
} from "../src/generated/contracts.js";
import { noopChatViewActions } from "./support/app-fixtures.js";
import { deferred, nth, type Deferred } from "./support/collections.js";
import { all, attr, classAttr, find, isRecord } from "./support/vnode.js";

type FakeFileClient = Pick<FileClient, "list" | "read">;

/** The controller only uses list/read; the fake omits FileClient's private state. */
function fakeFileClient(client: FakeFileClient): FileClient {
  return client as FileClient;
}

const counts = { protected: 0, unsupported: 0, unreadable: 0 };
const list = (path = "", entries: FileListEntry[] = []): FilesListResponse =>
  ({ path, entries, truncated: false, exclusions: counts });
const file = (path: string, content = "text"): FilesReadResponse =>
  ({ path, content, size: String(new TextEncoder().encode(content).length) });
const entry = (path: string): FileListEntry =>
  ({ path, name: path.split("/").at(-1) ?? "", kind: "file", size: "4" });
const json = (value: unknown, options: { status?: number; headers?: Record<string, string> } = {}): Response =>
  new Response(JSON.stringify(value), { ...options, headers: { "Content-Type": "application/json", ...options.headers } });
const signal = (): AbortSignal => new AbortController().signal;
const flush = async (): Promise<void> => { for (let i = 0; i < 6; i += 1) await Promise.resolve(); };
const actions: FilePaneActions = { toggle() {}, navigate() {}, select() {}, refresh() {}, download() {}, copyPath() {},
  showTab() {}, refreshChanges() {}, selectChange() {}, toggleFolder() {}, setDiffLayout() {} };

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
  const stream = new ReadableStream<Uint8Array>({ pull(controller) {
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
  assert.equal(download.exclusions?.protected, 2);
  const unsafe = new FileClient(async () => new Response("oops", { headers: {
    "Content-Type": "text/html", "Content-Disposition": 'attachment; filename="../escape.html"',
  } }));
  await assert.rejects(unsafe.download("x", false, signal()), { code: "unavailable" });
});

test("obsolete reads and authentication reset cannot replace a new selection", async () => {
  const reads: Array<Deferred<FilesReadResponse> & { readonly path: string; readonly signal: AbortSignal }> = [];
  const controller = new FilesController(() => {}, () => {});
  controller.attach(fakeFileClient({ list: async () => list(), read(path, signal) {
    const pending = deferred<FilesReadResponse>(); reads.push({ ...pending, path, signal }); return pending.promise;
  } }));
  controller.toggle(); await flush();
  const first = controller.select("first");
  const second = controller.select("second");
  assert.equal(nth(reads, 0).signal.aborted, true);
  nth(reads, 1).resolve(file("second", "new")); await second;
  nth(reads, 0).resolve(file("first", "old")); await first;
  assert.equal(controller.state.preview?.content, "new");
  const oldEpoch = controller.select("third");
  controller.reset();
  nth(reads, 2).resolve(file("third", "private")); await oldEpoch;
  assert.equal(controller.state.preview, null);
  assert.equal(controller.state.open, false);
});

test("an obsolete refresh cannot override a newer directory or selected file", async () => {
  const pending = deferred<FilesListResponse>(); let lists = 0;
  const controller = new FilesController(() => {}, () => {});
  controller.attach(fakeFileClient({
    list: async (path) => ++lists === 2 ? pending.promise : list(path, [entry("first"), entry("second")]),
    read: async (path) => file(path),
  }));
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
  const copied: string[] = [];
  Object.defineProperty(globalThis, "navigator", { configurable: true, value: { clipboard: { writeText: async (path: string) => { copied.push(path); } } } });
  try {
    const controller = new FilesController(() => {}, () => {});
    controller.attach(fakeFileClient({ list: async () => list(), read: async (path) => file(path) }));
    controller.toggle(); await flush();
    await controller.select("src/名字.rs");
    assert.deepEqual(copied, []);
    await controller.copyPath();
    assert.deepEqual(copied, ["src/名字.rs"]);
  } finally {
    if (original) Object.defineProperty(globalThis, "navigator", original); else Reflect.deleteProperty(globalThis, "navigator");
  }
});

test("pane is above chat, collapses without an editor, and never renders file text as HTML", () => {
  const controller = new FilesController(() => {}, () => {});
  const closed = renderFilesPane(controller.state, true, actions);
  assert.equal(all(closed, () => true).some((node) => attr(node, "id") === "files-monaco"), false);
  const hostile = '<img src=x onerror="evil()"><script>evil()</script>';
  const pane = renderFilesPane({ ...controller.state, open: true, preview: file("x.html", hostile), selectedPath: "x.html" }, true, actions);
  assert.equal(JSON.stringify(pane).includes(hostile), false);
  assert.equal(all(pane, () => true).some((node) => {
    const props: unknown = node.data?.props;
    return isRecord(props) && Object.hasOwn(props, "innerHTML");
  }), false);
  assert.ok(all(pane, () => true).some((node) => attr(node, "id") === "files-monaco"));
  const tree = renderChatView({ state: { snapshot: null, live: false, activePanel: null,
    transportStatus: { phase: "idle", label: "Waiting", attempt: 0, retryInMilliseconds: null },
    remoteReason: null, pendingCount: 0, debuggerWide: true, debuggerDrawerDismissed: false,
    draft: "", chatStart: 0, toast: null, filesPane: pane, overlay: null }, actions: noopChatViewActions() });
  const column = find(tree, (node) => classAttr(node) === "conversation-column has-files");
  assert.ok(column);
  const children = column.children ?? [];
  assert.equal(nth(children, 0), pane);
  const chat = nth(children, 1);
  assert.ok(typeof chat === "object");
  assert.equal(attr(chat, "id"), "chat-scroll");
});

test("history recall needs exact request, entry, session and unchanged draft generation", () => {
  type SelectedOutcome = Extract<CommandOutcome, { type: "history_entry_selected" }>;
  const pending: PendingHistoryRecall = { requestId: "req", sessionId: "session", entryId: "entry", draftVersion: 4 };
  const request: ICommandRequest = { id: "req", expected_revision: 1, type: "select_history_entry", session_id: "session", entry_id: "entry" };
  const outcome: SelectedOutcome = { type: "history_entry_selected", session_id: "session", entry_id: "entry", editor_content: "complete\n".repeat(1000) };
  const matches = (p: PendingHistoryRecall | null = pending, o: SelectedOutcome = outcome, r: ICommandRequest = request,
    id = "req", session = "session", version = 4): boolean => historyRecallMatches(p, o, r, id, session, version);
  assert.equal(matches(), true);
  assert.equal(matches(null), false);
  assert.equal(matches({ ...pending, requestId: "newer" }), false);
  assert.equal(matches(pending, { ...outcome, entry_id: "other" }), false);
  assert.equal(matches(pending, outcome, request, "req", "new-session"), false);
  assert.equal(matches(pending, outcome, request, "req", "session", 5), false);
});

test("file language detection uses only registered local filename and extension mappings", () => {
  const languages: Parameters<typeof fileLanguage>[1] = [{ id: "rust", extensions: [".rs"] }, { id: "dockerfile", filenames: ["Dockerfile"] }];
  assert.equal(fileLanguage("src/main.rs", languages), "rust");
  assert.equal(fileLanguage("build/Dockerfile", languages), "dockerfile");
  assert.equal(fileLanguage("unknown.binary", languages), "plaintext");
});

const changed = (path: string, added: number | null, removed: number | null, kind: "added" | "modified" | "deleted" | "untracked" = "modified") =>
  ({ path, kind, added, removed });

test("git responses are exact and bounded before display", () => {
  const status = { repository: true, branch: "main", files: [changed("src/a.rs", 1, 2)], truncated: false, protected: 0 };
  assert.deepEqual(parseGitStatus(status), status);
  assert.equal(parseGitStatus({ ...status, extra: 1 }), null);
  assert.equal(parseGitStatus({ ...status, files: [changed("../x", 1, 1)] }), null);
  assert.equal(parseGitStatus({ ...status, files: [{ ...changed("a", 1, 1), kind: "renamed" }] }), null);
  assert.equal(parseGitStatus({ ...status, files: [changed("a", 1, 1), changed("a", 2, 2)] }), null);
  assert.ok(parseGitDiff({ path: "a", original: "x", modified: "y" }, "a"));
  assert.equal(parseGitDiff({ path: "b", original: "x", modified: "y" }, "a"), null);
});

test("change tree sums line counts into every folder and honours collapsed folders", () => {
  const files = [changed("src/deep/b.rs", 3, 1), changed("src/a.rs", 2, 0, "added"),
    changed("README.md", 1, 1), changed("src/deep/logo.png", null, null)];
  const rows = changeRows(files, new Set());
  assert.deepEqual(rows.map((row) => `${"  ".repeat(row.depth)}${row.name} +${row.added} -${row.removed}`), [
    "src +5 -1",
    "  deep +3 -1",
    "    b.rs +3 -1",
    "    logo.png +null -null",
    "  a.rs +2 -0",
    "README.md +1 -1",
  ]);
  assert.equal(rows[0]?.files, 3);
  const collapsed = changeRows(files, new Set(["src/deep"]));
  assert.deepEqual(collapsed.map((row) => row.path), ["src", "src/deep", "src/a.rs", "README.md"]);
  assert.equal(collapsed[1]?.added, 3);
});

test("changes tab renders counts beside every file and folder and a diff host", () => {
  const controller = new FilesController(() => {}, () => {});
  const state = { ...controller.state, open: true, tab: "changes" as const,
    changes: { repository: true, branch: "main", files: [changed("src/a.rs", 4, 2)], truncated: false, protected: 1 } };
  const pane = renderFilesPane(state, true, actions);
  const entries = all(pane, (node) => classAttr(node).includes("changes-entry"));
  assert.equal(entries.length, 2);
  for (const entry of entries) {
    assert.ok(find(entry, (node) => classAttr(node) === "changes-added"), "added count");
    assert.ok(find(entry, (node) => classAttr(node) === "changes-removed"), "removed count");
  }
  assert.ok(find(pane, (node) => attr(node, "id") === "files-diff-monaco"));
  assert.equal(find(pane, (node) => attr(node, "id") === "files-monaco"), null);
});
