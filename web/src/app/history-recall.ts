import type { CommandOutcome, ICommandRequest } from "../generated/contracts.js";

export interface PendingHistoryRecall {
  readonly requestId: string;
  readonly sessionId: string;
  readonly entryId: string;
  readonly draftVersion: number;
}

export function historyRecallMatches(
  pending: PendingHistoryRecall | null,
  outcome: Extract<CommandOutcome, { type: "history_entry_selected" }>,
  request: ICommandRequest,
  responseId: string,
  sessionId: string | null,
  draftVersion: number,
): boolean {
  return pending !== null && request.type === "select_history_entry" &&
    pending.requestId === responseId && request.id === responseId &&
    pending.sessionId === sessionId && outcome.session_id === sessionId &&
    request.session_id === sessionId && outcome.entry_id === pending.entryId &&
    request.entry_id === pending.entryId && pending.draftVersion === draftVersion;
}
