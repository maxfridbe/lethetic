import assert from "node:assert/strict";
import test from "node:test";
import { navigateDocument } from "./navigation.js";
import type { CdpListener } from "./navigation.js";

const settle = (): Promise<void> => new Promise(resolve => setTimeout(resolve, 0));

function events() {
  const listeners = new Set<CdpListener>();
  return {
    listeners,
    subscribe(listener: CdpListener): () => void { listeners.add(listener); return () => { listeners.delete(listener); }; },
    commit(loaderId: string, frameId = "main", sessionId = "page", parentId?: string): void {
      for (const listener of listeners) listener({method: "Page.frameNavigated", sessionId, params: {frame: {id: frameId, loaderId, parentId}}});
    },
    loaded(loaderId: string, frameId = "main", sessionId = "page"): void {
      for (const listener of listeners) listener({method: "Page.lifecycleEvent", sessionId, params: {name: "DOMContentLoaded", frameId, loaderId}});
    },
  };
}

await test("reload waits for the new main loader, ignoring stale and iframe events", async () => {
  const stream = events(); let completed = false;
  const pending = navigateDocument(async () => ({}), stream.subscribe, "page", "Page.reload").then(() => { completed = true; });
  await settle();
  assert.equal(completed, false, "command acknowledgement is not document readiness");
  stream.loaded("old");
  stream.commit("iframe", "child", "page", "main"); stream.loaded("iframe", "child");
  stream.commit("other-page", "main", "another-session"); stream.loaded("other-page", "main", "another-session");
  await settle(); assert.equal(completed, false);
  stream.commit("new");
  stream.loaded("old"); stream.loaded("new", "child");
  await settle(); assert.equal(completed, false);
  stream.loaded("new"); await pending;
  assert.equal(completed, true); assert.equal(stream.listeners.size, 0);
});

await test("navigation events can arrive before the command response", async () => {
  const stream = events();
  await navigateDocument(async (method, params) => {
    assert.equal(method, "Page.navigate"); assert.equal(params?.["url"], "https://example.test/");
    stream.commit("fast"); stream.loaded("fast"); return {};
  }, stream.subscribe, "page", "Page.navigate", {url: "https://example.test/"});
  assert.equal(stream.listeners.size, 0);
});

await test("navigation errors fail without retrying and remove listeners", async () => {
  for (const failure of ["protocol", "network"]) {
    const stream = events(); let calls = 0;
    await assert.rejects(navigateDocument(async () => {
      calls++; if (failure === "protocol") throw new Error("Target closed");
      return {errorText: "net::ERR_CONNECTION_REFUSED"};
    }, stream.subscribe, "page", "Page.navigate"), failure === "protocol" ? /Target closed/ : /ERR_CONNECTION_REFUSED/);
    assert.equal(calls, 1); assert.equal(stream.listeners.size, 0);
  }
});

await test("missing document readiness times out and removes listeners", async () => {
  const stream = events();
  await assert.rejects(navigateDocument(async () => ({}), stream.subscribe, "page", "Page.reload", {}, 5), /DOMContentLoaded/);
  assert.equal(stream.listeners.size, 0);
});
