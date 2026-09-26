export const CHAT_FOLLOW_HOLD_MILLISECONDS = 5_000;
export const CHAT_BOTTOM_TOLERANCE_PX = 56;
const POSITION_EPSILON_PX = 0.5;
function finiteOrZero(value) {
    return Number.isFinite(value) ? value : 0;
}
function normalizedScrollTop(metrics) {
    return Math.max(0, finiteOrZero(metrics.scrollTop));
}
export function isAtChatBottom(metrics) {
    const scrollHeight = Math.max(0, finiteOrZero(metrics.scrollHeight));
    const clientHeight = Math.max(0, finiteOrZero(metrics.clientHeight));
    const maximum = Math.max(0, scrollHeight - clientHeight);
    return maximum - normalizedScrollTop(metrics) <= CHAT_BOTTOM_TOLERANCE_PX;
}
export class ChatFollowController {
    #scheduler;
    #onResumeFollowing;
    #mode = "following";
    #manualScrollTop = null;
    #manualDeadline = null;
    #manualTimer = null;
    #frameRequest = null;
    #frameHandle = null;
    #ownedScrollTop = null;
    #intentStartTop = null;
    constructor(scheduler, onResumeFollowing) {
        this.#scheduler = scheduler;
        this.#onResumeFollowing = onResumeFollowing;
    }
    get mode() {
        return this.#mode;
    }
    get manualScrollTop() {
        return this.#manualScrollTop;
    }
    get remainingManualHoldMilliseconds() {
        if (this.#manualDeadline === null) {
            return 0;
        }
        return Math.max(0, this.#manualDeadline - this.#scheduler.now());
    }
    reset() {
        this.#clearManualTimer();
        this.#cancelFrame();
        this.#mode = "following";
        this.#manualScrollTop = null;
        this.#manualDeadline = null;
        this.#ownedScrollTop = null;
        this.#intentStartTop = null;
    }
    dispose() {
        this.reset();
    }
    noteUserIntent(metrics, readMetricsAfterInput) {
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
    observeScroll(metrics) {
        const scrollTop = normalizedScrollTop(metrics);
        if (this.#ownedScrollTop !== null &&
            Math.abs(scrollTop - this.#ownedScrollTop) <= POSITION_EPSILON_PX) {
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
            }
            else {
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
        }
        else {
            this.#enterManual(scrollTop);
        }
    }
    requestBottomPosition(apply) {
        if (this.#mode !== "following" || this.#intentStartTop !== null) {
            return;
        }
        this.#queueFrame({ type: "position", mode: "following", apply });
    }
    requestPreservedPosition(apply) {
        if (this.#mode !== "manual") {
            return;
        }
        this.#queueFrame({ type: "position", mode: "manual", apply });
    }
    #queueFrame(request) {
        if (this.#frameRequest?.type === "intent" &&
            request.type === "position") {
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
            if (current.mode !== this.#mode ||
                (current.mode === "following" && this.#intentStartTop !== null)) {
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
    #resolveIntent(metrics) {
        const start = this.#intentStartTop;
        this.#intentStartTop = null;
        if (start === null || metrics === null) {
            return;
        }
        const scrollTop = normalizedScrollTop(metrics);
        if (Math.abs(scrollTop - start) > POSITION_EPSILON_PX) {
            if (isAtChatBottom(metrics)) {
                this.#resumeFollowing(this.#mode === "manual");
            }
            else {
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
    #enterManual(scrollTop) {
        this.#cancelFrame();
        this.#ownedScrollTop = null;
        this.#intentStartTop = null;
        this.#mode = "manual";
        this.#manualScrollTop = Math.max(0, scrollTop);
        this.#manualDeadline =
            this.#scheduler.now() + CHAT_FOLLOW_HOLD_MILLISECONDS;
        this.#clearManualTimer();
        this.#manualTimer = this.#scheduler.setTimeout(() => this.#manualTimerExpired(), CHAT_FOLLOW_HOLD_MILLISECONDS);
    }
    #manualTimerExpired() {
        this.#manualTimer = null;
        if (this.#mode !== "manual" || this.#manualDeadline === null) {
            return;
        }
        const remaining = this.#manualDeadline - this.#scheduler.now();
        if (remaining > 0) {
            this.#manualTimer = this.#scheduler.setTimeout(() => this.#manualTimerExpired(), remaining);
            return;
        }
        this.#resumeFollowing(true);
    }
    #resumeFollowing(notify) {
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
    #clearManualTimer() {
        if (this.#manualTimer !== null) {
            this.#scheduler.clearTimeout(this.#manualTimer);
            this.#manualTimer = null;
        }
    }
    #cancelFrame() {
        if (this.#frameHandle !== null) {
            this.#scheduler.cancelAnimationFrame(this.#frameHandle);
            this.#frameHandle = null;
        }
        this.#frameRequest = null;
    }
}
