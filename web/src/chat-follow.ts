import type { BlockListView } from "./generated/contracts.js";

export const CHAT_FOLLOW_HOLD_MILLISECONDS = 10_000;

/** Ignore status-only snapshots; include streamed text and structured tool output. */
export function hasNewChatOutput(before: BlockListView, after: BlockListView): boolean {
  if (before === after) return false;
  const offset = after.omitted_before - before.omitted_before;
  return after.blocks.some((block, index) => {
    const old = before.blocks[index + offset];
    return old === undefined || block.kind !== old.kind || block.content !== old.content ||
      block.title !== old.title || block.success !== old.success ||
      (block.tool !== old.tool && JSON.stringify(block.tool) !== JSON.stringify(old.tool));
  });
}
export const CHAT_BOTTOM_TOLERANCE_PX = 56;

const POSITION_EPSILON_PX = 0.5;

export interface ChatScrollMetrics {
  readonly scrollTop: number;
  readonly scrollHeight: number;
  readonly clientHeight: number;
}

export interface ChatFollowScheduler {
  now(): number;
  setTimeout(callback: () => void, delayMilliseconds: number): number;
  clearTimeout(handle: number): void;
  requestAnimationFrame(callback: () => void): number;
  cancelAnimationFrame(handle: number): void;
}

export type ChatFollowMode = "following" | "manual";

type PositionCallback = (scrollTop: number | null) => number | null;
type MetricsCallback = () => ChatScrollMetrics | null;

type FrameRequest =
  | {
      readonly type: "intent";
      readonly readMetrics: MetricsCallback;
    }
  | {
      readonly type: "position";
      readonly mode: ChatFollowMode;
      readonly apply: PositionCallback;
    };

function finiteOrZero(value: number): number {
  return Number.isFinite(value) ? value : 0;
}

function normalizedScrollTop(metrics: ChatScrollMetrics): number {
  return Math.max(0, finiteOrZero(metrics.scrollTop));
}

export function isAtChatBottom(metrics: ChatScrollMetrics): boolean {
  const scrollHeight = Math.max(0, finiteOrZero(metrics.scrollHeight));
  const clientHeight = Math.max(0, finiteOrZero(metrics.clientHeight));
  const maximum = Math.max(0, scrollHeight - clientHeight);
  return maximum - normalizedScrollTop(metrics) <= CHAT_BOTTOM_TOLERANCE_PX;
}

export class ChatFollowController {
  readonly #scheduler: ChatFollowScheduler;
  readonly #onResumeFollowing: () => void;
  #mode: ChatFollowMode = "following";
  #turnActive = false;
  #newOutput = false;
  #pointerOver = false;
  #focusWithin = false;
  #automaticResume: { top: number; deadline: number } | null = null;
  #manualScrollTop: number | null = null;
  #manualDeadline: number | null = null;
  #manualTimer: number | null = null;
  #frameRequest: FrameRequest | null = null;
  #frameHandle: number | null = null;
  #ownedScrollTop: number | null = null;
  #intentStartTop: number | null = null;

  constructor(
    scheduler: ChatFollowScheduler,
    onResumeFollowing: () => void,
  ) {
    this.#scheduler = scheduler;
    this.#onResumeFollowing = onResumeFollowing;
  }

  get mode(): ChatFollowMode {
    return this.#mode;
  }

  get manualScrollTop(): number | null {
    return this.#manualScrollTop;
  }

  get remainingManualHoldMilliseconds(): number {
    if (this.#manualDeadline === null) {
      return 0;
    }
    return Math.max(0, this.#manualDeadline - this.#scheduler.now());
  }

  setTurnActive(active: boolean): void {
    if (active === this.#turnActive) {
      return;
    }
    this.#turnActive = active;
    if (!active) this.#newOutput = false;
    this.#updateAutoFollow();
  }

  noteNewOutput(): void {
    if (this.#mode !== "manual" || !this.#turnActive) return;
    this.#newOutput = true;
    this.#updateAutoFollow();
  }

  setPointerOver(over: boolean): void {
    this.#pointerOver = over;
    this.#updateAutoFollow();
  }

  setFocusWithin(within: boolean): void {
    this.#focusWithin = within;
    this.#updateAutoFollow();
  }

  #updateAutoFollow(): void {
    this.#clearManualTimer();
    const allowed = this.#turnActive && this.#newOutput && !this.#pointerOver && !this.#focusWithin;
    // An enter/focus/idle event can arrive between timer expiry and its scroll frame.
    if (!allowed && this.#automaticResume !== null) {
      this.#cancelFrame();
      this.#mode = "manual";
      this.#manualScrollTop = this.#automaticResume.top;
      this.#manualDeadline = this.#automaticResume.deadline;
      this.#automaticResume = null;
    }
    if (allowed && this.#mode === "manual" && this.#manualDeadline !== null) {
      this.#manualTimer = this.#scheduler.setTimeout(
        () => this.#manualTimerExpired(), this.remainingManualHoldMilliseconds,
      );
    }
  }

  reset(): void {
    this.#clearManualTimer();
    this.#cancelFrame();
    this.#mode = "following";
    this.#turnActive = false;
    this.#newOutput = false;
    this.#pointerOver = false;
    this.#focusWithin = false;
    this.#automaticResume = null;
    this.#manualScrollTop = null;
    this.#manualDeadline = null;
    this.#ownedScrollTop = null;
    this.#intentStartTop = null;
  }

  dispose(): void {
    this.reset();
  }

  noteUserIntent(
    metrics: ChatScrollMetrics,
    readMetricsAfterInput: MetricsCallback,
  ): void {
    this.#ownedScrollTop = null;
    this.#cancelFrame();

    if (this.#mode === "manual") {
      this.#enterManual(normalizedScrollTop(metrics));
    }

    this.#intentStartTop = normalizedScrollTop(metrics);
    this.#queueFrame({
      type: "intent",
      readMetrics: readMetricsAfterInput,
    });
  }

  observeScroll(metrics: ChatScrollMetrics): void {
    const scrollTop = normalizedScrollTop(metrics);
    if (
      this.#ownedScrollTop !== null &&
      Math.abs(scrollTop - this.#ownedScrollTop) <= POSITION_EPSILON_PX
    ) {
      return;
    }

    if (this.#intentStartTop !== null) {
      if (Math.abs(scrollTop - this.#intentStartTop) <= POSITION_EPSILON_PX) {
        return;
      }
      this.#intentStartTop = null;
      this.#cancelFrame();
      if (isAtChatBottom(metrics)) {
        this.#resumeFollowing(this.#mode === "manual");
      } else {
        this.#enterManual(scrollTop);
      }
      return;
    }

    if (this.#frameRequest?.type === "position") {
      return;
    }

    if (isAtChatBottom(metrics)) {
      if (this.#mode === "manual") {
        this.#resumeFollowing(true);
      }
    } else {
      this.#enterManual(scrollTop);
    }
  }

  requestBottomPosition(apply: PositionCallback): void {
    if (this.#mode !== "following" || this.#intentStartTop !== null) {
      return;
    }
    this.#queueFrame({ type: "position", mode: "following", apply });
  }

  requestPreservedPosition(apply: PositionCallback): void {
    if (this.#mode !== "manual") {
      return;
    }
    this.#queueFrame({ type: "position", mode: "manual", apply });
  }

  #queueFrame(request: FrameRequest): void {
    if (
      this.#frameRequest?.type === "intent" &&
      request.type === "position"
    ) {
      return;
    }
    this.#frameRequest = request;
    if (this.#frameHandle !== null) {
      return;
    }
    this.#frameHandle = this.#scheduler.requestAnimationFrame(() => {
      this.#frameHandle = null;
      const current = this.#frameRequest;
      this.#frameRequest = null;
      if (current === null) {
        return;
      }
      if (current.type === "intent") {
        this.#resolveIntent(current.readMetrics());
        return;
      }
      if (
        current.mode !== this.#mode ||
        (current.mode === "following" && this.#intentStartTop !== null)
      ) {
        return;
      }
      const target = current.mode === "manual" ? this.#manualScrollTop : null;
      if (current.mode === "manual" && target === null) {
        return;
      }
      const actual = current.apply(target);
      if (actual === null || !Number.isFinite(actual)) {
        return;
      }
      if (current.mode === "following") this.#automaticResume = null;
      this.#ownedScrollTop = Math.max(0, actual);
      if (current.mode === "manual") {
        this.#manualScrollTop = this.#ownedScrollTop;
      }
    });
  }

  #resolveIntent(metrics: ChatScrollMetrics | null): void {
    const start = this.#intentStartTop;
    this.#intentStartTop = null;
    if (start === null || metrics === null) {
      return;
    }
    const scrollTop = normalizedScrollTop(metrics);
    if (Math.abs(scrollTop - start) > POSITION_EPSILON_PX) {
      if (isAtChatBottom(metrics)) {
        this.#resumeFollowing(this.#mode === "manual");
      } else {
        this.#enterManual(scrollTop);
      }
      return;
    }
    if (this.#mode === "manual" && isAtChatBottom(metrics)) {
      this.#resumeFollowing(true);
      return;
    }
    if (this.#mode === "following" && !isAtChatBottom(metrics)) {
      this.#onResumeFollowing();
    }
  }

  #enterManual(scrollTop: number): void {
    this.#cancelFrame();
    this.#ownedScrollTop = null;
    this.#intentStartTop = null;
    this.#mode = "manual";
    this.#manualScrollTop = Math.max(0, scrollTop);
    this.#automaticResume = null;
    this.#newOutput = false;
    this.#manualDeadline = this.#scheduler.now() + CHAT_FOLLOW_HOLD_MILLISECONDS;
    this.#updateAutoFollow();
  }

  #manualTimerExpired(): void {
    this.#manualTimer = null;
    if (!this.#turnActive || !this.#newOutput || this.#pointerOver || this.#focusWithin ||
      this.#mode !== "manual" || this.#manualDeadline === null) {
      return;
    }
    const remaining = this.#manualDeadline - this.#scheduler.now();
    if (remaining > 0) {
      this.#manualTimer = this.#scheduler.setTimeout(
        () => this.#manualTimerExpired(),
        remaining,
      );
      return;
    }
    const pending = { top: this.#manualScrollTop ?? 0, deadline: this.#manualDeadline };
    this.#resumeFollowing(true, pending);
  }

  #resumeFollowing(notify: boolean, automatic: { top: number; deadline: number } | null = null): void {
    this.#automaticResume = automatic;
    const changed = this.#mode !== "following";
    this.#clearManualTimer();
    this.#cancelFrame();
    this.#mode = "following";
    this.#manualScrollTop = null;
    this.#manualDeadline = null;
    this.#ownedScrollTop = null;
    this.#intentStartTop = null;
    if (notify && changed) {
      this.#onResumeFollowing();
    }
  }

  #clearManualTimer(): void {
    if (this.#manualTimer !== null) {
      this.#scheduler.clearTimeout(this.#manualTimer);
      this.#manualTimer = null;
    }
  }

  #cancelFrame(): void {
    if (this.#frameHandle !== null) {
      this.#scheduler.cancelAnimationFrame(this.#frameHandle);
      this.#frameHandle = null;
    }
    this.#frameRequest = null;
  }
}
