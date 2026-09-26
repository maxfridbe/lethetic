import type { ChatViewActions } from "../../src/app/state.js";

export function noopChatViewActions(
  overrides: Partial<ChatViewActions> = {},
): ChatViewActions {
  return {
    openPalette: () => {},
    invokeCommand: () => {},
    openPanel: () => {},
    updateDraft: () => {},
    submitPrompt: () => {},
    stop: () => {},
    toggleDebugger: () => {},
    onChatKeyDown: () => {},
    onChatManualIntent: () => {},
    onChatScroll: () => {},
    ...overrides,
  };
}
