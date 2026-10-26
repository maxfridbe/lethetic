import { CHAT_OVERSCAN, CHAT_ROW_ESTIMATE, CHAT_WINDOW_SIZE } from "./helpers.js";

export interface ChatWindow {
  start: number;
  end: number;
  top: number;
  bottom: number;
}

/** Row heights are keyed by transcript ordinal, not by a shifting array index. */
export class MeasuredChatWindow {
  #heights = new Map<number, number>();
  #offsets: number[] = [0];
  #prefix = 0;
  #count = 0;
  #dirty = true;

  reset(): void { this.#heights.clear(); this.#dirty = true; }
  sync(count: number, prefix: number): void {
    if (count === this.#count && prefix === this.#prefix) return;
    this.#count = count; this.#prefix = prefix; this.#dirty = true;
    for (const key of this.#heights.keys()) {
      if (key < prefix || key >= prefix + count) this.#heights.delete(key);
    }
  }
  measure(index: number, height: number): boolean {
    if (index < 0 || index >= this.#count || !Number.isFinite(height) || height <= 0) return false;
    const key = this.#prefix + index;
    if (Math.abs((this.#heights.get(key) ?? CHAT_ROW_ESTIMATE) - height) < 0.5) return false;
    this.#heights.set(key, height); this.#dirty = true; return true;
  }
  measureOrdinal(ordinal: number, height: number): boolean { return this.measure(ordinal - this.#prefix, height); }
  #offsetsForRows(): number[] {
    if (this.#dirty) {
      this.#offsets = [0];
      for (let index = 0; index < this.#count; index++) {
        this.#offsets.push(this.#offsets[index]! + (this.#heights.get(this.#prefix + index) ?? CHAT_ROW_ESTIMATE));
      }
      this.#dirty = false;
    }
    return this.#offsets;
  }
  range(top: number, height: number, following: boolean, anchorIndex?: number): ChatWindow {
    const offsets = this.#offsetsForRows(), total = offsets[this.#count]!;
    const rowAt = (offset: number): number => {
      let low = 0, high = this.#count;
      while (low < high) {
        const middle = Math.floor((low + high) / 2);
        if (offsets[middle + 1]! <= offset) low = middle + 1; else high = middle;
      }
      return Math.min(low, Math.max(0, this.#count - 1));
    };
    const viewport = Math.max(1, height);
    const first = following ? rowAt(Math.max(0, total - viewport))
      : anchorIndex === undefined ? rowAt(Math.max(0, top)) : Math.max(0, Math.min(anchorIndex, this.#count - 1));
    const start = Math.max(0, first - CHAT_OVERSCAN, following ? this.#count - CHAT_WINDOW_SIZE : 0);
    const last = following ? this.#count : rowAt(Math.max(top, offsets[first]!) + viewport) + 1 + CHAT_OVERSCAN;
    const end = Math.min(this.#count, Math.max(first + 1, last), start + CHAT_WINDOW_SIZE);
    return {start, end, top: offsets[start]!, bottom: total - offsets[end]!};
  }
}

/** Measure only mounted rows; resize/streaming/image changes coalesce into a frame. */
export class ChatVirtualizer {
  readonly rows = new MeasuredChatWindow();
  #width = 0;
  #frame: number | null = null;
  #observer: ResizeObserver | null = null;
  #observed = new Set<Element>();
  #sizes = new WeakMap<Element, string>();
  constructor(readonly render: () => void) {}
  requestRender(): void {
    if (this.#frame !== null) return;
    this.#frame = requestAnimationFrame(() => { this.#frame = null; this.render(); });
  }
  measure(chat: HTMLElement): boolean {
    if (this.#width !== chat.clientWidth) {
      this.#width = chat.clientWidth; this.rows.reset();
    }
    const window = chat.querySelector<HTMLElement>(".chat-window");
    const gap = window ? Number.parseFloat(getComputedStyle(window).rowGap) || 0 : 0;
    let changed = false;
    for (const row of chat.querySelectorAll<HTMLElement>("[data-chat-index]")) {
      const anchor = row.dataset["chatAnchor"];
      const height = row.getBoundingClientRect().height + gap;
      changed = (anchor?.startsWith("block-")
        ? this.rows.measureOrdinal(Number(anchor.slice(6)), height)
        : this.rows.measure(Number(row.dataset["chatIndex"]), height)) || changed;
    }
    return changed;
  }
  afterRender(chat: HTMLElement | null): void {
    if (!chat) { this.dispose(); return; }
    if (this.measure(chat)) this.requestRender();
    this.#observer ??= new ResizeObserver(entries => {
      if (entries.some(({target}) => this.#sizes.get(target) !== this.#size(target))) this.requestRender();
    });
    const current = new Set<Element>([chat, ...chat.querySelectorAll("[data-chat-index]")]);
    for (const previous of this.#observed) if (!current.has(previous)) this.#observer.unobserve(previous);
    for (const element of current) {
      this.#sizes.set(element, this.#size(element));
      if (!this.#observed.has(element)) this.#observer.observe(element);
    }
    this.#observed = current;
  }
  #size(element: Element): string {
    const rectangle = element.getBoundingClientRect(); return `${rectangle.width}:${rectangle.height}`;
  }
  reset(): void { this.dispose(); this.rows.reset(); this.#width = 0; }
  dispose(): void {
    if (this.#frame !== null) cancelAnimationFrame(this.#frame);
    this.#frame = null; this.#observer?.disconnect(); this.#observer = null; this.#observed.clear();
  }
}
