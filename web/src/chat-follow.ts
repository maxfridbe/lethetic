export const CHAT_FOLLOW_HOLD_MILLISECONDS = 5_000;
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

  reset(): void {
    this.#clearManualTimer();
    this.#cancelFrame();
    this.#mode = "following";
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
    this.#manualDeadline =
      this.#scheduler.now() + CHAT_FOLLOW_HOLD_MILLISECONDS;
    this.#clearManualTimer();
    this.#manualTimer = this.#scheduler.setTimeout(
      () => this.#manualTimerExpired(),
      CHAT_FOLLOW_HOLD_MILLISECONDS,
    );
  }

  #manualTimerExpired(): void {
    this.#manualTimer = null;
    if (this.#mode !== "manual" || this.#manualDeadline === null) {
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
    this.#resumeFollowing(true);
  }

  #resumeFollowing(notify: boolean): void {
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
