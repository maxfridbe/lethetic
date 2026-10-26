import assert from "node:assert/strict";
import test from "node:test";
import {
  CHAT_BOTTOM_TOLERANCE_PX,
  CHAT_FOLLOW_HOLD_MILLISECONDS,
  ChatFollowController,
  isAtChatBottom,
  hasNewChatOutput,
  type ChatFollowScheduler,
  type ChatScrollMetrics,
} from "../src/chat-follow.js";

interface FakeTimer {
  readonly at: number;
  readonly callback: () => void;
}

class FakeScheduler implements ChatFollowScheduler {
  nowMilliseconds = 0;
  nextHandle = 1;
  readonly timers = new Map<number, FakeTimer>();
  readonly frames = new Map<number, () => void>();

  now = (): number => this.nowMilliseconds;

  setTimeout = (callback: () => void, delayMilliseconds: number): number => {
    const handle = this.nextHandle++;
    this.timers.set(handle, {
      at: this.nowMilliseconds + Math.max(0, delayMilliseconds),
      callback,
    });
    return handle;
  };

  clearTimeout = (handle: number): void => {
    this.timers.delete(handle);
  };

  requestAnimationFrame = (callback: () => void): number => {
    const handle = this.nextHandle++;
    this.frames.set(handle, callback);
    return handle;
  };

  cancelAnimationFrame = (handle: number): void => {
    this.frames.delete(handle);
  };

  advance(milliseconds: number): void {
    const target = this.nowMilliseconds + milliseconds;
    while (true) {
      const next = Array.from(this.timers.entries())
        .filter(([, timer]) => timer.at <= target)
        .sort((left, right) => left[1].at - right[1].at || left[0] - right[0])[0];
      if (next === undefined) {
        break;
      }
      const [handle, timer] = next;
      this.timers.delete(handle);
      this.nowMilliseconds = timer.at;
      timer.callback();
    }
    this.nowMilliseconds = target;
  }

  flushAnimationFrame(): void {
    const frames = Array.from(this.frames.entries()).sort(
      (left, right) => left[0] - right[0],
    );
    this.frames.clear();
    for (const [, callback] of frames) {
      callback();
    }
  }
}

function metrics(
  scrollTop: number,
  scrollHeight = 1_000,
  clientHeight = 200,
): ChatScrollMetrics {
  return { scrollTop, scrollHeight, clientHeight };
}

function fixture(active = true) {
  const scheduler = new FakeScheduler();
  let resumed = 0;
  const controller = new ChatFollowController(scheduler, () => { resumed += 1; });
  controller.setTurnActive(active);
  return { scheduler, controller, resumes: () => resumed };
}

test("bottom detection uses the shared tolerance and the hold is ten seconds", () => {
  assert.equal(CHAT_FOLLOW_HOLD_MILLISECONDS, 10_000);
  assert.equal(CHAT_BOTTOM_TOLERANCE_PX, 56);
  assert.equal(isAtChatBottom(metrics(744)), true);
  assert.equal(isAtChatBottom(metrics(743.99)), false);
  assert.equal(isAtChatBottom(metrics(0, 100, 200)), true);
  assert.equal(isAtChatBottom(metrics(Number.NaN)), false);
});

test("active status alone never resumes; new output after ten seconds does", () => {
  const {scheduler, controller, resumes} = fixture();
  controller.observeScroll(metrics(300));
  scheduler.advance(20_000);
  assert.equal(controller.mode, "manual");
  assert.equal(scheduler.timers.size, 0);
  controller.noteNewOutput();
  scheduler.advance(0);
  assert.equal(controller.mode, "following");
  assert.equal(resumes(), 1);
});

test("new output waits ten seconds since the last scroll, not since its arrival", () => {
  const {scheduler, controller, resumes} = fixture();
  controller.observeScroll(metrics(500));
  scheduler.advance(6_000);
  controller.noteNewOutput();
  scheduler.advance(3_999);
  assert.equal(controller.mode, "manual");
  assert.equal(controller.remainingManualHoldMilliseconds, 1);
  controller.setTurnActive(true);
  controller.noteNewOutput();
  scheduler.advance(1);
  assert.equal(resumes(), 1);
});

test("a new manual scroll restarts the deadline and requires fresh output", () => {
  const {scheduler, controller, resumes} = fixture();
  controller.observeScroll(metrics(500));
  controller.noteNewOutput();
  scheduler.advance(9_000);
  controller.noteUserIntent(metrics(500), () => metrics(400));
  scheduler.flushAnimationFrame();
  assert.equal(controller.manualScrollTop, 400);
  scheduler.advance(10_000);
  assert.equal(resumes(), 0);
  controller.noteNewOutput();
  scheduler.advance(0);
  assert.equal(resumes(), 1);
});

for (const blocker of ["pointer", "focus"] as const) {
  test(`${blocker} blocks auto-follow beyond the deadline without resetting scroll time`, () => {
    const {scheduler, controller, resumes} = fixture();
    controller.observeScroll(metrics(300));
    if (blocker === "pointer") controller.setPointerOver(true);
    else controller.setFocusWithin(true);
    controller.noteNewOutput();
    scheduler.advance(30_000);
    assert.equal(controller.mode, "manual");
    assert.equal(resumes(), 0);
    if (blocker === "pointer") controller.setPointerOver(false);
    else controller.setFocusWithin(false);
    scheduler.advance(0);
    assert.equal(resumes(), 1);
  });
}

test("both hover and focus must clear; leaving early still waits for ten seconds", () => {
  const {scheduler, controller, resumes} = fixture();
  controller.observeScroll(metrics(300));
  controller.setPointerOver(true);
  controller.setFocusWithin(true);
  controller.noteNewOutput();
  scheduler.advance(4_000);
  controller.setPointerOver(false);
  assert.equal(scheduler.timers.size, 0);
  controller.setFocusWithin(false);
  scheduler.advance(5_999);
  assert.equal(resumes(), 0);
  scheduler.advance(1);
  assert.equal(resumes(), 1);
});

test("manually reaching bottom overrides hover, focus, missing output and the timer", () => {
  const {scheduler, controller, resumes} = fixture(false);
  controller.observeScroll(metrics(300));
  controller.setPointerOver(true);
  controller.setFocusWithin(true);
  controller.observeScroll(metrics(800));
  assert.equal(controller.mode, "following");
  assert.equal(resumes(), 1);
  assert.equal(scheduler.timers.size, 0);
});

test("idle, completed and disconnected turns do not jump on old output", () => {
  const {scheduler, controller, resumes} = fixture();
  controller.observeScroll(metrics(300));
  controller.noteNewOutput();
  scheduler.advance(9_999);
  controller.setTurnActive(false);
  controller.noteNewOutput();
  scheduler.advance(20_000);
  controller.setTurnActive(true);
  scheduler.advance(0);
  assert.equal(controller.mode, "manual");
  assert.equal(resumes(), 0);
  controller.noteNewOutput();
  scheduler.advance(0);
  assert.equal(resumes(), 1);
});

for (const blocker of ["pointer", "focus", "idle"] as const) {
  test(`${blocker} arriving before the queued bottom write cancels the jump`, () => {
    const scheduler = new FakeScheduler();
    let writes = 0;
    const controller = new ChatFollowController(scheduler, () => {
      controller.requestBottomPosition(() => { writes += 1; return 800; });
    });
    controller.setTurnActive(true);
    controller.observeScroll(metrics(300));
    controller.noteNewOutput();
    scheduler.advance(10_000);
    if (blocker === "pointer") controller.setPointerOver(true);
    else if (blocker === "focus") controller.setFocusWithin(true);
    else controller.setTurnActive(false);
    scheduler.flushAnimationFrame();
    assert.equal(writes, 0);
    assert.equal(controller.mode, "manual");
    assert.equal(controller.manualScrollTop, 300);
  });
}

test("position restoration and owned scroll events do not extend the hold", () => {
  const {scheduler, controller} = fixture();
  controller.observeScroll(metrics(500));
  controller.noteNewOutput();
  scheduler.advance(6_000);
  controller.preservePosition(target => { assert.equal(target, 500); return 480; });
  scheduler.flushAnimationFrame();
  controller.observeScroll(metrics(480));
  assert.equal(controller.remainingManualHoldMilliseconds, 4_000);
  assert.equal(controller.manualScrollTop, 480);
  scheduler.advance(4_000);
  assert.equal(controller.mode, "following");
});

test("new user input invalidates queued following and upward intent wins over tolerance", () => {
  const scheduler = new FakeScheduler();
  let writes = 0;
  const controller = new ChatFollowController(scheduler, () => {
    controller.requestBottomPosition(() => { writes += 1; return 800; });
  });
  controller.setTurnActive(true);
  controller.observeScroll(metrics(300));
  controller.noteNewOutput();
  scheduler.advance(10_000);
  controller.noteUserIntent(metrics(800), () => metrics(650));
  scheduler.flushAnimationFrame();
  assert.equal(writes, 0);
  assert.equal(controller.manualScrollTop, 650);
  controller.noteUserIntent(metrics(650, 900), () => metrics(100, 900));
  scheduler.flushAnimationFrame();
  assert.equal(controller.mode, "manual");
  assert.equal(controller.manualScrollTop, 100);
});

test("session reset cancels stale timers, frames, output and reading state", () => {
  const {scheduler, controller, resumes} = fixture();
  controller.observeScroll(metrics(300));
  controller.noteNewOutput();
  controller.preservePosition(() => 300);
  controller.reset();
  scheduler.advance(20_000);
  scheduler.flushAnimationFrame();
  assert.equal(resumes(), 0);
  assert.equal(controller.mode, "following");
  assert.equal(scheduler.timers.size, 0);
});

test("output detection ignores identical snapshots and metadata but sees new blocks and streamed text", () => {
  const item = { kind: "text" as const, content: "hello", title: null, tool: null,
    content_loss: {filtered:false, redacted:false, truncation:null},
    title_loss: {filtered:false, redacted:false, truncation:null}, success:null,
    usage:null, estimated_cost:null, duration_label:null };
  const before = {blocks: [item], omitted_before:0, truncated:false};
  assert.equal(hasNewChatOutput(before, structuredClone(before)), false);
  assert.equal(hasNewChatOutput(before, {...before, blocks:[{...item, duration_label:"1s"}]}), false);
  assert.equal(hasNewChatOutput(before, {...before, blocks:[{...item, content:"hello world"}]}), true);
  assert.equal(hasNewChatOutput(before, {...before, blocks:[item,item]}), true);
  assert.equal(hasNewChatOutput(before, {...before, blocks:[]}), false);
});

test("following coalesces writes and ignores its own scroll events", () => {
  const {scheduler, controller} = fixture();
  const writes: string[] = [];
  controller.requestBottomPosition(() => { writes.push("old"); return 800; });
  controller.requestBottomPosition(() => { writes.push("latest"); return 800; });
  assert.equal(scheduler.frames.size, 1);
  scheduler.flushAnimationFrame();
  controller.observeScroll(metrics(800));
  assert.deepEqual(writes, ["latest"]);
  assert.equal(controller.mode, "following");
});

test("input probes distinguish user movement from content growth", () => {
  const {scheduler, controller, resumes} = fixture();
  controller.noteUserIntent(metrics(800), () => metrics(800, 1_200));
  scheduler.flushAnimationFrame();
  assert.equal(controller.mode, "following");
  assert.equal(resumes(), 1);
  controller.noteUserIntent(metrics(800), () => metrics(600));
  scheduler.flushAnimationFrame();
  assert.equal(controller.mode, "manual");
  assert.equal(controller.manualScrollTop, 600);
});

test("patch preservation is synchronous and cannot rewind later scroll input", () => {
  const {scheduler, controller} = fixture();
  controller.observeScroll(metrics(500));
  let position = 500, writes = 0;
  controller.preservePosition(() => { writes++; return position; });
  assert.equal(writes, 1);
  position = 488;
  controller.observeScroll(metrics(position));
  scheduler.flushAnimationFrame();
  assert.equal(writes, 1);
  assert.equal(controller.manualScrollTop, 488);
});
