export function historyRecallMatches(pending, outcome, request, responseId, sessionId, draftVersion) {
    return pending !== null && request.type === "select_history_entry" &&
        pending.requestId === responseId && request.id === responseId &&
        pending.sessionId === sessionId && outcome.session_id === sessionId &&
        request.session_id === sessionId && outcome.entry_id === pending.entryId &&
        request.entry_id === pending.entryId && pending.draftVersion === draftVersion;
}
