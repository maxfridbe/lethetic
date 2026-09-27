import { jsx as h } from "../../lib/snabbdom/build/index.js";
import { ICON_GLYPHS } from "../generated/contracts.js";
import { activitySpinner, uiIcon } from "../icons.js";
import { jsonPreformattedThunk } from "../json.js";
import { isMarkdownBlockKind, markdownContentThunk, toolResultContentThunk, } from "../markdown.js";
import { boundedText } from "../safety.js";
import { renderDebugger } from "./debugger.js";
import { renderApplicationStatus, renderTransportStatus, } from "./status.js";
import { CHAT_ROW_ESTIMATE, CHAT_WINDOW_SIZE, MAX_DRAFT_LENGTH, activityLabel, activitySpinnerKind, blockLabel, chatJsonSegmentAllocations, commandForId, inputValue, markdownBlockUsesJsonHighlighting, projectionLossMessages, sessionLabel, toolBlockUsesJsonHighlighting, toolResultUsesAutoRendering, } from "./helpers.js";
function renderHeader(context) {
    const { snapshot, live, transportStatus } = context.state;
    return (h("header", { attrs: { class: "session-header" } },
        h("button", { attrs: {
                type: "button",
                class: "icon-button palette-button",
                "aria-label": "Open command palette",
                title: "Command palette (Ctrl+P)",
            }, props: { disabled: snapshot === null }, on: { click: context.actions.openPalette } },
            h("span", { attrs: { "aria-hidden": "true" } }, ICON_GLYPHS.command)),
        h("div", { attrs: { class: "connection-summary" } },
            h("span", { attrs: {
                    class: `connection-dot ${live ? "is-live" : "is-stale"}`,
                    "aria-hidden": "true",
                } }),
            h("span", null, transportStatus.label)),
        h("div", { attrs: { class: "session-summary" } },
            h("span", { attrs: { class: "eyebrow" } }, "Session"),
            h("div", { attrs: { class: "session-name-row" } },
                h("strong", null, snapshot === null ? "Waiting for state" : sessionLabel(snapshot)),
                h("button", { attrs: {
                        type: "button",
                        class: "icon-button session-rename",
                        "aria-label": "Name or rename the current session",
                        title: "Rename session",
                    }, props: { disabled: !live || snapshot === null }, on: {
                        click: () => {
                            if (snapshot === null) {
                                return;
                            }
                            const command = commandForId(snapshot, "name-session");
                            if (command !== null) {
                                context.actions.invokeCommand(command);
                            }
                        },
                    } }, uiIcon("edit")))),
        h("div", { attrs: { class: "model-summary" } },
            h("span", { attrs: { class: "eyebrow" } }, "Model"),
            h("span", null, snapshot?.status.model_label ?? "—"))));
}
function projectionLosses(block) {
    return block.tool === null
        ? [block.content_loss, block.title_loss]
        : [block.content_loss, block.title_loss, block.tool.payload_loss];
}
function renderProjectionLossNotices(block) {
    const messages = projectionLossMessages(projectionLosses(block));
    if (messages.length === 0) {
        return null;
    }
    return (h("div", { attrs: { class: "projection-loss-notices", role: "note" } }, messages.map((message) => (h("p", { attrs: { class: "truncation-note" } }, message)))));
}
export function chatBlockAnchor(snapshot, index) {
    const hasSyntheticNotice = snapshot.blocks.truncated;
    if (hasSyntheticNotice && index === 0) {
        return null;
    }
    const ordinal = snapshot.blocks.omitted_before + index - (hasSyntheticNotice ? 1 : 0);
    return `block-${ordinal}`;
}
function renderBlock(block, index, maxJsonSegments, anchor) {
    if (block.kind === "divider") {
        return (h("div", { key: anchor ?? `synthetic-block-${index}`, attrs: {
                class: "chat-divider",
                role: "separator",
                ...(anchor === null ? {} : { "data-chat-anchor": anchor }),
            } },
            h("span", null, block.title ?? "Conversation boundary"),
            renderProjectionLossNotices(block)));
    }
    const content = block.tool?.payload ?? block.content;
    const displayContent = boundedText(content);
    const renderedContent = isMarkdownBlockKind(block.kind)
        ? markdownContentThunk(block.kind, displayContent, markdownBlockUsesJsonHighlighting(block), maxJsonSegments)
        : toolResultUsesAutoRendering(block)
            ? toolResultContentThunk(displayContent, block.tool?.payload_loss.truncation !== null, maxJsonSegments)
            : jsonPreformattedThunk("block", displayContent, toolBlockUsesJsonHighlighting(block), maxJsonSegments);
    return (h("article", { key: anchor ?? `synthetic-block-${index}`, attrs: {
            class: `chat-block block-${block.kind}${block.success === false ? " is-failure" : ""}`,
            "aria-label": blockLabel(block),
            ...(anchor === null ? {} : { "data-chat-anchor": anchor }),
        } },
        h("header", { attrs: { class: "block-header" } },
            h("strong", null, block.title ?? blockLabel(block)),
            block.tool === null ? null : (h("span", { attrs: { class: "block-kind" } }, block.tool.kind))),
        content.length === 0 ? null : renderedContent,
        renderProjectionLossNotices(block),
        block.usage === null ? null : (h("details", { attrs: { class: "block-usage" } },
            h("summary", null, "Usage"),
            h("span", null, `${block.usage.total_tokens} total tokens`),
            block.estimated_cost === null ? null : (h("span", null, `${block.estimated_cost.display} estimated`))))));
}
function renderChat(context) {
    const { snapshot, chatStart } = context.state;
    if (snapshot === null) {
        return (h("main", { attrs: {
                id: "chat-scroll",
                class: "chat-scroll empty-chat",
                "aria-label": "Conversation",
            } },
            h("p", null, "Authenticate to load the conversation.")));
    }
    const blocks = snapshot.blocks.blocks;
    const end = Math.min(blocks.length, chatStart + CHAT_WINDOW_SIZE);
    const start = Math.max(0, Math.min(chatStart, Math.max(0, blocks.length - 1)));
    const visible = blocks.slice(start, end);
    const jsonSegmentAllocations = chatJsonSegmentAllocations(visible);
    return (h("main", { attrs: {
            id: "chat-scroll",
            class: "chat-scroll",
            "aria-label": "Conversation",
            tabindex: "0",
        }, on: {
            keydown: context.actions.onChatKeyDown,
            pointerdown: context.actions.onChatManualIntent,
            scroll: context.actions.onChatScroll,
            touchmove: context.actions.onChatManualIntent,
            touchstart: context.actions.onChatManualIntent,
            wheel: context.actions.onChatManualIntent,
        } },
        snapshot.blocks.omitted_before > 0 ? (h("div", { attrs: { class: "bounded-notice" } }, `${snapshot.blocks.omitted_before} earlier blocks were omitted by the server.`)) : null,
        start > 0 ? (h("div", { attrs: { class: "virtual-spacer", "aria-hidden": "true" }, style: { height: `${start * CHAT_ROW_ESTIMATE}px` } })) : null,
        h("div", { attrs: { class: "chat-window" } }, visible.map((block, offset) => renderBlock(block, start + offset, jsonSegmentAllocations[offset] ?? 0, chatBlockAnchor(snapshot, start + offset)))),
        end < blocks.length ? (h("div", { attrs: { class: "virtual-spacer", "aria-hidden": "true" }, style: { height: `${(blocks.length - end) * CHAT_ROW_ESTIMATE}px` } })) : null,
        snapshot.blocks.truncated ? (h("div", { attrs: { class: "bounded-notice warning" } }, "Conversation content was bounded by the server.")) : null));
}
function renderActivity(context) {
    const { snapshot, live } = context.state;
    if (snapshot === null) {
        return h("section", { attrs: { class: "activity-area is-empty" } });
    }
    const approval = snapshot.pending_approval;
    const question = snapshot.pending_question;
    const spinnerKind = activitySpinnerKind(snapshot.activity.kind, live);
    return (h("section", { attrs: {
            class: `activity-area activity-${snapshot.activity.kind}`,
            "aria-label": "Current activity and requests",
        } },
        h("div", { attrs: { class: "activity-summary" } },
            h("strong", null,
                spinnerKind === null ? null : activitySpinner(spinnerKind),
                h("span", null, activityLabel(snapshot.activity.kind))),
            snapshot.activity.progress_percent === null ? null : (h("progress", { attrs: { "aria-label": "Activity progress" }, props: { value: snapshot.activity.progress_percent, max: 100 } }))),
        approval === null ? null : (h("button", { attrs: { type: "button", class: "attention-card" }, props: { disabled: !live }, on: { click: () => context.actions.openPanel("tool_approval") } },
            h("strong", null,
                uiIcon("warning"),
                approval.tool_name),
            h("span", null, approval.description),
            h("span", null, "Review approval"))),
        question === null ? null : (h("button", { attrs: { type: "button", class: "attention-card" }, props: { disabled: !live }, on: { click: () => context.actions.openPanel("ask_user") } },
            h("strong", null,
                uiIcon("warning"),
                "Answer requested"),
            h("span", null, question.questions[0]?.prompt ?? "The agent has a question."),
            h("span", null, question.content_truncated
                ? "Question unavailable remotely — open to cancel"
                : "Open question")))));
}
function renderComposer(context) {
    const { snapshot, live, draft } = context.state;
    const canSubmit = live &&
        snapshot?.activity.fully_idle === true &&
        draft.trim().length > 0;
    const rows = Math.max(2, Math.min(10, draft.split("\n").length + Math.floor(draft.length / 96)));
    return (h("form", { attrs: { class: "composer", "aria-label": "Prompt composer" }, on: {
            submit: (event) => {
                event.preventDefault();
                context.actions.submitPrompt();
            },
        } },
        h("label", { attrs: { for: "composer-input", class: "sr-only" } }, "Prompt"),
        h("textarea", { attrs: {
                id: "composer-input",
                maxlength: String(MAX_DRAFT_LENGTH),
                rows: String(rows),
                placeholder: live
                    ? "Type a prompt. Enter submits; Shift+Enter adds a line. Ctrl+P opens commands."
                    : "Waiting for a synchronized connection…",
                "aria-describedby": "composer-help",
            }, props: { value: draft, disabled: !live || snapshot === null }, on: {
                input: (event) => context.actions.updateDraft(inputValue(event)),
                keydown: (event) => {
                    if (event.key === "Enter" &&
                        !event.shiftKey &&
                        !event.isComposing) {
                        event.preventDefault();
                        context.actions.submitPrompt();
                    }
                },
            } }),
        h("div", { attrs: { class: "composer-actions" } },
            h("span", { attrs: { id: "composer-help", class: "composer-help" } }, "Draft stays in this browser and is never persisted."),
            snapshot?.activity.cancellable === true ? (h("button", { attrs: { type: "button", class: "danger-button" }, props: { disabled: !live }, on: { click: context.actions.stop } },
                uiIcon("stop"),
                "Stop")) : null,
            h("button", { attrs: { type: "submit", class: "primary-button" }, props: { disabled: !canSubmit } },
                uiIcon("send"),
                "Send"))));
}
export function renderChatView(context) {
    const { state } = context;
    const debuggerOpen = state.snapshot?.debugger.open === true;
    const debuggerVisible = debuggerOpen &&
        (state.debuggerWide || !state.debuggerDrawerDismissed);
    const debuggerMode = state.debuggerWide ? "debugger-wide" : "debugger-narrow";
    return (h("div", { attrs: {
            id: "app",
            class: `app-shell ${debuggerMode}${debuggerVisible ? " debugger-open" : ""}`,
            "data-connection": state.transportStatus.phase,
            "data-debugger": debuggerVisible ? "open" : "closed",
            "aria-busy": state.live ? "false" : "true",
        } },
        renderHeader(context),
        h("div", { attrs: {
                class: `workspace ${debuggerMode}${debuggerVisible ? " has-debugger" : ""}`,
            } },
            h("div", { attrs: { class: `conversation-column${state.filesPane == null ? "" : " has-files"}` } },
                state.filesPane,
                renderChat(context),
                renderActivity(context),
                renderComposer(context)),
            renderDebugger(context)),
        renderApplicationStatus(state.snapshot),
        renderTransportStatus(state),
        h("div", { attrs: {
                class: "sr-only",
                role: "status",
                "aria-live": "polite",
                "aria-atomic": "true",
            } }, state.toast ?? state.transportStatus.label),
        state.toast === null ? null : (h("div", { attrs: { class: "toast", role: "status" } }, state.toast)),
        state.overlay));
}
