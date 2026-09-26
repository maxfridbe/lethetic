import assert from "node:assert/strict";
import test from "node:test";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

const stage = process.argv[2];
if (stage === undefined) {
  throw new Error("usage: chat-follow.test.mjs <compiled-web-root>");
}

const followUrl = pathToFileURL(resolve(stage, "src/chat-follow.js"));
const {
  CHAT_BOTTOM_TOLERANCE_PX,
  CHAT_FOLLOW_HOLD_MILLISECONDS,
  ChatFollowController,
  isAtChatBottom,
} = await import(followUrl.href);

class FakeScheduler {
  nowMilliseconds = 0;
  nextHandle = 1;
  timers = new Map();
  frames = new Map();

  now = () => this.nowMilliseconds;

  setTimeout = (callback, delayMilliseconds) => {
    const handle = this.nextHandle++;
    this.timers.set(handle, {
      at: this.nowMilliseconds + Math.max(0, delayMilliseconds),
      callback,
    });
    return handle;
  };

  clearTimeout = (handle) => {
    this.timers.delete(handle);
  };

  requestAnimationFrame = (callback) => {
    const handle = this.nextHandle++;
    this.frames.set(handle, callback);
    return handle;
  };

  cancelAnimationFrame = (handle) => {
    this.frames.delete(handle);
  };

  advance(milliseconds) {
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

  flushAnimationFrame() {
    const frames = Array.from(this.frames.entries()).sort(
      (left, right) => left[0] - right[0],
    );
    this.frames.clear();
    for (const [, callback] of frames) {
      callback();
    }
  }
}

function metrics(scrollTop, scrollHeight = 1_000, clientHeight = 200) {
  return { scrollTop, scrollHeight, clientHeight };
}

test("bottom detection uses one exported tolerance", () => {
  assert.equal(CHAT_BOTTOM_TOLERANCE_PX, 56);
  assert.equal(isAtChatBottom(metrics(744)), true);
  assert.equal(isAtChatBottom(metrics(743.99)), false);
  assert.equal(isAtChatBottom(metrics(0, 100, 200)), true);
  assert.equal(isAtChatBottom(metrics(Number.NaN, 1_000, 200)), false);
});

test("manual activity replaces one five-second inactivity deadline", () => {
  const scheduler = new FakeScheduler();
  let resumed = 0;
  const controller = new ChatFollowController(scheduler, () => {
    resumed += 1;
  });

  controller.observeScroll(metrics(500));
  assert.equal(controller.mode, "manual");
  assert.equal(controller.manualScrollTop, 500);
  assert.equal(
    controller.remainingManualHoldMilliseconds,
    CHAT_FOLLOW_HOLD_MILLISECONDS,
  );
  assert.equal(scheduler.timers.size, 1);

  scheduler.advance(3_000);
  controller.noteUserIntent(metrics(480), () => metrics(480));
  scheduler.flushAnimationFrame();
  assert.equal(controller.manualScrollTop, 480);
  assert.equal(controller.remainingManualHoldMilliseconds, 5_000);
  assert.equal(scheduler.timers.size, 1);

  let preservedTarget = null;
  controller.requestPreservedPosition((target) => {
    preservedTarget = target;
    return 470;
  });
  scheduler.flushAnimationFrame();
  assert.equal(preservedTarget, 480);
  assert.equal(controller.manualScrollTop, 470);

  scheduler.advance(4_999);
  controller.observeScroll(metrics(470));
  assert.equal(controller.remainingManualHoldMilliseconds, 1);
  assert.equal(resumed, 0);
  scheduler.advance(1);
  assert.equal(controller.mode, "following");
  assert.equal(controller.manualScrollTop, null);
  assert.equal(resumed, 1);
});

test("manually reaching the bottom resumes immediately", () => {
  const scheduler = new FakeScheduler();
  let resumed = 0;
  const controller = new ChatFollowController(scheduler, () => {
    resumed += 1;
  });

  controller.observeScroll(metrics(400));
  scheduler.advance(750);
  controller.observeScroll(metrics(800));
  assert.equal(controller.mode, "following");
  assert.equal(controller.remainingManualHoldMilliseconds, 0);
  assert.equal(scheduler.timers.size, 0);
  assert.equal(resumed, 1);
});

test("pre-input bottom tolerance cannot override an upward manual scroll", () => {
  const scheduler = new FakeScheduler();
  let resumed = 0;
  const controller = new ChatFollowController(scheduler, () => {
    resumed += 1;
  });

  controller.observeScroll(metrics(700));
  assert.equal(controller.mode, "manual");

  const nearBottomAfterResize = metrics(700, 950, 200);
  assert.equal(isAtChatBottom(nearBottomAfterResize), true);
  controller.noteUserIntent(
    nearBottomAfterResize,
    () => metrics(100, 950, 200),
  );
  assert.equal(controller.mode, "manual");
  assert.equal(resumed, 0);
  scheduler.flushAnimationFrame();
  assert.equal(controller.mode, "manual");
  assert.equal(controller.manualScrollTop, 100);
  assert.equal(resumed, 0);
});

test("expiry resumes without a state patch and ordinary reconnect work preserves it", () => {
  const scheduler = new FakeScheduler();
  let resumed = 0;
  const controller = new ChatFollowController(scheduler, () => {
    resumed += 1;
  });

  controller.observeScroll(metrics(300));
  scheduler.advance(2_000);
  controller.requestPreservedPosition(() => 300);
  scheduler.flushAnimationFrame();
  assert.equal(controller.mode, "manual");
  assert.equal(controller.remainingManualHoldMilliseconds, 3_000);

  scheduler.advance(3_000);
  assert.equal(controller.mode, "following");
  assert.equal(resumed, 1);
});

test("owned scroll events are suppressed and positioning is coalesced", () => {
  const scheduler = new FakeScheduler();
  const controller = new ChatFollowController(scheduler, () => {});
  const applied = [];

  controller.requestBottomPosition(() => {
    applied.push("stale");
    return 800;
  });
  controller.requestBottomPosition((target) => {
    assert.equal(target, null);
    applied.push("latest");
    return 800;
  });
  assert.equal(scheduler.frames.size, 1);
  scheduler.flushAnimationFrame();
  assert.deepEqual(applied, ["latest"]);

  controller.observeScroll(metrics(800));
  assert.equal(controller.mode, "following");
  controller.observeScroll(metrics(800));
  assert.equal(controller.mode, "following");
});

test("new input invalidates a queued bottom write", () => {
  const scheduler = new FakeScheduler();
  let bottomWrites = 0;
  let controller;
  controller = new ChatFollowController(scheduler, () => {
    controller.requestBottomPosition(() => {
      bottomWrites += 1;
      return 800;
    });
  });

  controller.observeScroll(metrics(250));
  scheduler.advance(5_000);
  assert.equal(controller.mode, "following");
  assert.equal(scheduler.frames.size, 1);

  controller.noteUserIntent(metrics(800), () => metrics(650));
  assert.equal(scheduler.frames.size, 1);
  scheduler.flushAnimationFrame();
  assert.equal(bottomWrites, 0);
  assert.equal(controller.mode, "manual");
  assert.equal(controller.manualScrollTop, 650);
});

test("input probes distinguish user movement from content growth", () => {
  const scheduler = new FakeScheduler();
  let resumeRequests = 0;
  const controller = new ChatFollowController(scheduler, () => {
    resumeRequests += 1;
  });

  controller.noteUserIntent(metrics(800), () => metrics(800, 1_200, 200));
  scheduler.flushAnimationFrame();
  assert.equal(controller.mode, "following");
  assert.equal(resumeRequests, 1);

  controller.noteUserIntent(metrics(800), () => metrics(700));
  scheduler.flushAnimationFrame();
  assert.equal(controller.mode, "manual");
  assert.equal(controller.manualScrollTop, 700);
});

test("session or authentication reset cancels stale timers and frames", () => {
  const scheduler = new FakeScheduler();
  let resumed = 0;
  const controller = new ChatFollowController(scheduler, () => {
    resumed += 1;
  });

  controller.observeScroll(metrics(200));
  controller.requestPreservedPosition(() => 200);
  assert.equal(scheduler.timers.size, 1);
  assert.equal(scheduler.frames.size, 1);
  scheduler.advance(1_000);

  controller.reset();
  assert.equal(controller.mode, "following");
  assert.equal(controller.remainingManualHoldMilliseconds, 0);
  assert.equal(scheduler.timers.size, 0);
  assert.equal(scheduler.frames.size, 0);
  scheduler.advance(10_000);
  scheduler.flushAnimationFrame();
  assert.equal(resumed, 0);
});
