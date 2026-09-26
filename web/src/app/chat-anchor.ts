export interface ChatAnchor {
  readonly key: string;
  readonly viewportOffset: number;
}

export function adjustedAnchorScrollTop(
  currentScrollTop: number,
  maximumScrollTop: number,
  previousOffset: number,
  currentOffset: number,
): number {
  return Math.max(
    0,
    Math.min(
      currentScrollTop + currentOffset - previousOffset,
      maximumScrollTop,
    ),
  );
}

export function captureChatAnchor(chat: HTMLElement | null): ChatAnchor | null {
  if (chat === null) {
    return null;
  }
  const viewport = chat.getBoundingClientRect();
  for (const candidate of chat.querySelectorAll<HTMLElement>("[data-chat-anchor]")) {
    const rectangle = candidate.getBoundingClientRect();
    if (rectangle.bottom <= viewport.top || rectangle.top >= viewport.bottom) {
      continue;
    }
    const key = candidate.dataset["chatAnchor"];
    if (key !== undefined && key.length > 0) {
      return {
        key,
        viewportOffset: rectangle.top - viewport.top,
      };
    }
  }
  return null;
}

function findAnchorElement(
  chat: HTMLElement,
  key: string,
): HTMLElement | null {
  for (const candidate of chat.querySelectorAll<HTMLElement>("[data-chat-anchor]")) {
    if (candidate.dataset["chatAnchor"] === key) {
      return candidate;
    }
  }
  return null;
}

export function restoreChatAnchor(
  chat: HTMLElement,
  anchor: ChatAnchor | null,
  fallbackScrollTop: number,
): number {
  const maximum = Math.max(0, chat.scrollHeight - chat.clientHeight);
  const candidate = anchor === null ? null : findAnchorElement(chat, anchor.key);
  const next =
    candidate === null || anchor === null
      ? Math.max(0, Math.min(fallbackScrollTop, maximum))
      : adjustedAnchorScrollTop(
          chat.scrollTop,
          maximum,
          anchor.viewportOffset,
          candidate.getBoundingClientRect().top -
            chat.getBoundingClientRect().top,
        );
  chat.scrollTop = next;
  return chat.scrollTop;
}
