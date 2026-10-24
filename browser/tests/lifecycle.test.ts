import assert from "node:assert/strict";
import test from "node:test";
import { ExecutionBudget, executionTimeout } from "../src/lifecycle.js";
await test("timeout excludes background time and suspension gaps but counts responsive foreground work", () => {
  const budget = new ExecutionBudget(1000, 0);
  assert.equal(budget.tick(250, false), false);
  assert.equal(budget.tick(500, true), false);
  assert.equal(budget.tick(60_000, false), false);
  assert.equal(budget.remaining, 750);
  assert.equal(budget.tick(60_500, false), false);
  assert.equal(budget.tick(60_750, false), true);
});
await test("disabled and disposed deadlines do not fire; active deadline does", async () => {
  let fired = false;
  const cancel = executionTimeout(() => { fired = true; }, 10); cancel();
  const disabled = executionTimeout(() => { fired = true; }, 0);
  await new Promise(resolve => setTimeout(resolve, 25));
  assert.equal(fired, false); disabled();
  await new Promise<void>(resolve => { executionTimeout(resolve, 10); });
});
await test("wake lock is released exactly once and late acquisition cannot leak after cancellation", async () => {
  const previousNavigator = Object.getOwnPropertyDescriptor(globalThis, "navigator");
  const previousDocument = Object.getOwnPropertyDescriptor(globalThis, "document");
  const page = Object.assign(new EventTarget(), { hidden: false, visibilityState: "visible" });
  // Node has no window lifecycle target; this test only needs the document handlers.
  const previousAdd = Object.getOwnPropertyDescriptor(globalThis, "addEventListener");
  Object.defineProperty(globalThis, "addEventListener", { configurable: true, value: () => {} });
  Object.defineProperty(globalThis, "document", { configurable: true, value: page });
  let acquire: ((lock: WakeLockSentinel) => void) | undefined, releases = 0;
  Object.defineProperty(globalThis, "navigator", { configurable: true, value: { wakeLock: { request: () => new Promise<WakeLockSentinel>(resolve => { acquire = resolve; }) } } });
  try {
    const { keepAwakeDuringWork } = await import("../src/lifecycle.js");
    const release = keepAwakeDuringWork(); release(); release();
    acquire!({ release: async () => { releases++; } } as WakeLockSentinel);
    await Promise.resolve(); await Promise.resolve();
    assert.equal(releases, 1);
  } finally {
    for (const [key, descriptor] of [["navigator", previousNavigator], ["document", previousDocument], ["addEventListener", previousAdd]] as const) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor); else Reflect.deleteProperty(globalThis, key);
    }
  }
});
