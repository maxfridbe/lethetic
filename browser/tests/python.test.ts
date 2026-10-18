import assert from "node:assert/strict";
import test from "node:test";
import { createPythonTool } from "../src/python.js";

class FakeWorker extends EventTarget {
  requests: { id: number; code: string; indexURL: string }[] = [];
  terminated = false;
  postMessage(data: { id: number; code: string; indexURL: string }): void { this.requests.push(data); }
  terminate(): void { this.terminated = true; }
  answer(output: string): void {
    this.dispatchEvent(new MessageEvent("message", { data: { id: this.requests.at(-1)?.id, output } }));
  }
}

await test("Python worker is lazy, persistent, and receives no model credentials", async () => {
  const workers: FakeWorker[] = [];
  const python = createPythonTool({ workerFactory: () => {
    const worker = new FakeWorker(); workers.push(worker); return worker as unknown as Worker;
  } });
  assert.equal(workers.length, 0);
  const running = python.execute({ code: "x = 40" }, new AbortController().signal);
  const worker = workers[0]; assert.ok(worker);
  assert.deepEqual(Object.keys(worker.requests[0] ?? {}).sort(), ["code", "id", "indexURL"]);
  worker.answer("ok");
  assert.equal(await running, "ok");
  const again = python.execute({ code: "x + 2" }, new AbortController().signal);
  worker.answer("42");
  assert.equal(await again, "42");
  assert.equal(workers.length, 1);
  python.reset();
  assert.equal(worker.terminated, true);
});

await test("cancelling Python terminates execution and a fresh worker is created", async () => {
  const workers: FakeWorker[] = [];
  const python = createPythonTool({ workerFactory: () => {
    const worker = new FakeWorker(); workers.push(worker); return worker as unknown as Worker;
  } });
  const controller = new AbortController();
  const running = python.execute({ code: "while True: pass" }, controller.signal);
  controller.abort();
  await assert.rejects(Promise.resolve(running), /stopped/);
  assert.equal(workers[0]?.terminated, true);
  const next = python.execute({ code: "1+1" }, new AbortController().signal);
  assert.equal(workers.length, 2);
  workers[1]?.answer("2");
  assert.equal(await next, "2");
  python.reset();
});

await test("timeouts, worker failures, and concurrent execution are handled", async () => {
  const worker = new FakeWorker();
  const python = createPythonTool({ workerFactory: () => worker as unknown as Worker, timeoutMs: 10 });
  const running = python.execute({ code: "slow" }, new AbortController().signal);
  await assert.rejects(Promise.resolve(python.execute({ code: "again" }, new AbortController().signal)), /already running/);
  await assert.rejects(Promise.resolve(running), /timed out/);
  assert.equal(worker.terminated, true);
  const another = python.execute({ code: "go" }, new AbortController().signal);
  worker.dispatchEvent(new Event("error"));
  await assert.rejects(Promise.resolve(another), /could not load/);
  python.reset();
});
