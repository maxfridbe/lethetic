export function adjustedAnchorScrollTop(currentScrollTop, maximumScrollTop, previousOffset, currentOffset) {
    return Math.max(0, Math.min(currentScrollTop + currentOffset - previousOffset, maximumScrollTop));
}
export function captureChatAnchor(chat) {
    if (chat === null) {
        return null;
    }
    const viewport = chat.getBoundingClientRect();
    for (const candidate of chat.querySelectorAll("[data-chat-anchor]")) {
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
function findAnchorElement(chat, key) {
    for (const candidate of chat.querySelectorAll("[data-chat-anchor]")) {
        if (candidate.dataset["chatAnchor"] === key) {
            return candidate;
        }
    }
    return null;
}
export function restoreChatAnchor(chat, anchor, fallbackScrollTop) {
    const maximum = Math.max(0, chat.scrollHeight - chat.clientHeight);
    const candidate = anchor === null ? null : findAnchorElement(chat, anchor.key);
    const next = candidate === null || anchor === null
        ? Math.max(0, Math.min(fallbackScrollTop, maximum))
        : adjustedAnchorScrollTop(chat.scrollTop, maximum, anchor.viewportOffset, candidate.getBoundingClientRect().top -
            chat.getBoundingClientRect().top);
    chat.scrollTop = next;
    return chat.scrollTop;
}
