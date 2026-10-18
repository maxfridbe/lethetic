import assert from "node:assert/strict";
import test from "node:test";
import { BrowserFiles } from "../src/files.js";
import { SessionStore, sessionRoute } from "../src/sessions.js";
import type { SavedSession } from "../src/sessions.js";
import { BrowserMetrics } from "../src/metrics.js";
import { parseFileList, parseFileRead } from "../../web/src/files/protocol.js";

function storage() {
  const values = new Map<string, string>();
  return { values, fail: false, getItem(key: string): string | null { return values.get(key) ?? null; },
    setItem(key: string, value: string): void { if (this.fail) throw new Error("Quota exceeded"); values.set(key, value); } };
}
const saved = (id = crypto.randomUUID()): SavedSession => ({ id, name: null, label: "Hello", updatedAt: new Date().toISOString(),
  blocks: [], messages: [{ role: "user", content: "hello" }, { role: "assistant", content: "hi" }],
  inputs: ["hello"], system: "Be helpful", connection: null, python: false, theme: "dracula" });

await test("local files use the existing directory/read/download protocol and survive reopening", async () => {
  const disk = storage(); const files = new BrowserFiles(disk);
  files.save("reports/example.py", "print('héllo')"); files.save("readme.md", "# Hello");
  const reopened = new BrowserFiles(disk); const signal = new AbortController().signal;
  const root = parseFileList(await (await reopened.request("/api/files/list", "", signal)).json(), "");
  assert.ok(root); assert.deepEqual(root.entries.map((entry) => entry.name), ["readme.md", "reports"]);
  const nested = parseFileList(await (await reopened.request("/api/files/list", "reports", signal)).json(), "reports");
  assert.equal(nested?.entries[0]?.path, "reports/example.py");
  const read = parseFileRead(await (await reopened.request("/api/files/read", "reports/example.py", signal)).json(), "reports/example.py");
  assert.equal(read?.content, "print('héllo')");
  const download = await reopened.request("/api/files/download", "reports/example.py", signal);
  assert.equal(await download.text(), read?.content);
  assert.match(download.headers.get("Content-Disposition") ?? "", /example.py/);
  assert.equal((await reopened.request("/api/files/read", "../secrets", signal)).status, 400);
  reopened.remove("reports/example.py"); assert.equal(files.list().length, 1);
  disk.values.clear(); assert.deepEqual(files.list(), []);
});

await test("invalid file paths, size, collisions and quota failures preserve saved files", () => {
  const disk = storage(); const files = new BrowserFiles(disk); files.save("folder/one.txt", "keep");
  for (const path of ["../outside", "/absolute", "folder//two", "folder"]) assert.throws(() => files.save(path, "bad"));
  assert.throws(() => files.save("big", "é".repeat(600_000)), /1 MiB/);
  disk.fail = true; assert.throws(() => files.save("folder/one.txt", "overwrite"), /Quota/);
  assert.equal(files.list()[0]?.content, "keep");
  disk.fail = false; disk.values.set("lethetic.files.v1", "broken");
  assert.throws(() => files.save("file", "new"));
  assert.equal(disk.values.get("lethetic.files.v1"), "broken");
});

await test("sessions preserve context and metrics, strip credentials and merge other sessions", () => {
  const disk = storage(); const first = new SessionStore(disk); const other = new SessionStore(disk);
  const session = saved();
  session.connection = { name: "Test", provider: "openai", endpoint: "https://example.com/v1/chat/completions", model: "test", apiKey: "secret" } as SavedSession["connection"];
  const metrics = new BrowserMetrics(() => 10); metrics.begin();
  metrics.event({ type: "request", round: 1 });
  metrics.event({ type: "response", durationMs: 5, usage: { input: 1200, output: 64, total: 1264, cached: 1024, cacheWrite: null, reasoning: null } });
  metrics.finish(); session.metrics = metrics.snapshot();
  first.save(session); const second = saved(); other.save(second); first.save(session);
  const rows = other.list(); assert.equal(rows.length, 2);
  assert.ok(!JSON.stringify(rows).includes("secret"));
  const restored = rows.find((row) => row.id === session.id); assert.ok(restored?.metrics);
  assert.deepEqual(restored.messages, session.messages);
  const resumed = new BrowserMetrics(); resumed.restore(restored.metrics);
  assert.equal(resumed.session.label("cached"), "1,024");
  resumed.begin(); assert.equal(resumed.session.label("total"), "1,264");
  other.remove(second.id); assert.equal(first.list().length, 1); first.clear(); assert.deepEqual(other.list(), []);
});

await test("damaged sessions and failed saves never replace existing storage", () => {
  const disk = storage(); const sessions = new SessionStore(disk); sessions.save(saved());
  const before = disk.values.get("lethetic.sessions.v1"); disk.fail = true;
  assert.throws(() => sessions.save(saved()), /Quota/); assert.equal(disk.values.get("lethetic.sessions.v1"), before);
  disk.fail = false; disk.values.set("lethetic.sessions.v1", "broken");
  assert.throws(() => sessions.save(saved())); assert.equal(disk.values.get("lethetic.sessions.v1"), "broken");
});

await test("Pages routes support a specific session, latest and new without server rewrites", () => {
  const id = crypto.randomUUID();
  assert.equal(sessionRoute(""), "latest"); assert.equal(sessionRoute("#/latest"), "latest");
  assert.equal(sessionRoute("#/new"), "new"); assert.equal(sessionRoute(`#/sessions/${id}`), id);
  for (const route of ["#/sessions/nope", "#/sessions/../secret", "#https://example.com"]) assert.throws(() => sessionRoute(route));
});

await test("tab credentials survive reload without entering saved metadata and are removed with their connection", async () => {
  const {saveConnections, loadConnections, saveConnectionKeys, restoreConnectionKeys} = await import("../src/connection.js");
  const device = storage(); const tab = storage();
  const connection = {name:"Test",provider:"openai" as const,endpoint:"https://example.com/v1/chat/completions",model:"test",apiKey:"tab-secret"};
  saveConnections(device,[connection]); saveConnectionKeys(tab,[connection]);
  assert.ok(!JSON.stringify([...device.values]).includes("tab-secret"));
  assert.equal(restoreConnectionKeys(tab,loadConnections(device))[0]?.apiKey,"tab-secret");
  assert.equal(restoreConnectionKeys(tab,[{...connection,endpoint:"https://elsewhere.com/v1/chat/completions",apiKey:undefined}])[0]?.apiKey,undefined);
  assert.equal(restoreConnectionKeys(tab,[{...connection,apiKey:"explicit"}])[0]?.apiKey,"explicit");
  saveConnectionKeys(tab,[]); assert.equal(restoreConnectionKeys(tab,loadConnections(device))[0]?.apiKey,undefined);
});
