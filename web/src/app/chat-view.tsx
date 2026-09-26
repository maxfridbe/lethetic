import { jsx as h } from "../../lib/snabbdom/build/index.js";
import type { VNode } from "../../lib/snabbdom/build/index.js";
import { ICON_GLYPHS } from "../generated/contracts.js";
import type {
  ProjectionLossView,
  RenderBlockView,
  WebAppSnapshot,
} from "../generated/contracts.js";
import { activitySpinner, uiIcon } from "../icons.js";
import { jsonPreformattedThunk } from "../json.js";
import {
  isMarkdownBlockKind,
  markdownContentThunk,
  toolResultContentThunk,
} from "../markdown.js";
import { boundedText } from "../safety.js";
import { renderDebugger } from "./debugger.js";
import {
  renderApplicationStatus,
  renderTransportStatus,
} from "./status.js";
import {
  CHAT_ROW_ESTIMATE,
  CHAT_WINDOW_SIZE,
  MAX_DRAFT_LENGTH,
  activityLabel,
  activitySpinnerKind,
  blockLabel,
  chatJsonSegmentAllocations,
  commandForId,
  inputValue,
  markdownBlockUsesJsonHighlighting,
  projectionLossMessages,
  sessionLabel,
  toolBlockUsesJsonHighlighting,
  toolResultUsesAutoRendering,
} from "./helpers.js";
import type { ChatViewContext } from "./state.js";

function renderHeader(context: ChatViewContext): VNode {
  const { snapshot, live, transportStatus } = context.state;
  return (
    <header attrs={{ class: "session-header" }}>
      <button
        attrs={{
          type: "button",
          class: "icon-button palette-button",
          "aria-label": "Open command palette",
          title: "Command palette (Ctrl+P)",
        }}
        props={{ disabled: snapshot === null }}
        on={{ click: context.actions.openPalette }}
      >
        <span attrs={{ "aria-hidden": "true" }}>{ICON_GLYPHS.command}</span>
      </button>
      <div attrs={{ class: "connection-summary" }}>
        <span
          attrs={{
            class: `connection-dot ${live ? "is-live" : "is-stale"}`,
            "aria-hidden": "true",
          }}
        ></span>
        <span>{transportStatus.label}</span>
      </div>
      <div attrs={{ class: "session-summary" }}>
        <span attrs={{ class: "eyebrow" }}>Session</span>
        <strong>
          {snapshot === null ? "Waiting for state" : sessionLabel(snapshot)}
        </strong>
      </div>
      <div attrs={{ class: "model-summary" }}>
        <span attrs={{ class: "eyebrow" }}>Model</span>
        <span>{snapshot?.status.model_label ?? "—"}</span>
      </div>
      <button
        attrs={{
          type: "button",
          class: "text-button",
          "aria-label": "Name or rename the current session",
        }}
        props={{ disabled: !live || snapshot === null }}
        on={{
          click: () => {
            if (snapshot === null) {
              return;
            }
            const command = commandForId(snapshot, "name-session");
            if (command !== null) {
              context.actions.invokeCommand(command);
            }
          },
        }}
      >
        {uiIcon("edit")}
        Name
      </button>
    </header>
  );
}

function projectionLosses(block: RenderBlockView): readonly ProjectionLossView[] {
  return block.tool === null
    ? [block.content_loss, block.title_loss]
    : [block.content_loss, block.title_loss, block.tool.payload_loss];
}

function renderProjectionLossNotices(block: RenderBlockView): VNode | null {
  const messages = projectionLossMessages(projectionLosses(block));
  if (messages.length === 0) {
    return null;
  }
  return (
    <div attrs={{ class: "projection-loss-notices", role: "note" }}>
      {messages.map((message) => (
        <p attrs={{ class: "truncation-note" }}>{message}</p>
      ))}
    </div>
  );
}

export function chatBlockAnchor(
  snapshot: WebAppSnapshot,
  index: number,
): string | null {
  const hasSyntheticNotice = snapshot.blocks.truncated;
  if (hasSyntheticNotice && index === 0) {
    return null;
  }
  const ordinal =
    snapshot.blocks.omitted_before + index - (hasSyntheticNotice ? 1 : 0);
  return `block-${ordinal}`;
}

function renderBlock(
  block: RenderBlockView,
  index: number,
  maxJsonSegments: number,
  anchor: string | null,
): VNode {
  if (block.kind === "divider") {
    return (
      <div
        key={anchor ?? `synthetic-block-${index}`}
        attrs={{
          class: "chat-divider",
          role: "separator",
          ...(anchor === null ? {} : { "data-chat-anchor": anchor }),
        }}
      >
        <span>{block.title ?? "Conversation boundary"}</span>
        {renderProjectionLossNotices(block)}
      </div>
    );
  }
  const content = block.tool?.payload ?? block.content;
  const displayContent = boundedText(content);
  const renderedContent = isMarkdownBlockKind(block.kind)
    ? markdownContentThunk(
        block.kind,
        displayContent,
        markdownBlockUsesJsonHighlighting(block),
        maxJsonSegments,
      )
    : toolResultUsesAutoRendering(block)
      ? toolResultContentThunk(
          displayContent,
          block.tool?.payload_loss.truncation !== null,
          maxJsonSegments,
        )
      : jsonPreformattedThunk(
          "block",
          displayContent,
          toolBlockUsesJsonHighlighting(block),
          maxJsonSegments,
        );
  return (
    <article
      key={anchor ?? `synthetic-block-${index}`}
      attrs={{
        class: `chat-block block-${block.kind}${
          block.success === false ? " is-failure" : ""
        }`,
        "aria-label": blockLabel(block),
        ...(anchor === null ? {} : { "data-chat-anchor": anchor }),
      }}
    >
      <header attrs={{ class: "block-header" }}>
        <strong>{block.title ?? blockLabel(block)}</strong>
        {block.tool === null ? null : (
          <span attrs={{ class: "block-kind" }}>{block.tool.kind}</span>
        )}
      </header>
      {content.length === 0 ? null : renderedContent}
      {renderProjectionLossNotices(block)}
      {block.usage === null ? null : (
        <details attrs={{ class: "block-usage" }}>
          <summary>Usage</summary>
          <span>{`${block.usage.total_tokens} total tokens`}</span>
          {block.estimated_cost === null ? null : (
            <span>{`${block.estimated_cost.display} estimated`}</span>
          )}
        </details>
      )}
    </article>
  );
}

function renderChat(context: ChatViewContext): VNode {
  const { snapshot, chatStart } = context.state;
  if (snapshot === null) {
    return (
      <main
        attrs={{
          id: "chat-scroll",
          class: "chat-scroll empty-chat",
          "aria-label": "Conversation",
        }}
      >
        <p>Authenticate to load the conversation.</p>
      </main>
    );
  }
  const blocks = snapshot.blocks.blocks;
  const end = Math.min(blocks.length, chatStart + CHAT_WINDOW_SIZE);
  const start = Math.max(
    0,
    Math.min(chatStart, Math.max(0, blocks.length - 1)),
  );
  const visible = blocks.slice(start, end);
  const jsonSegmentAllocations = chatJsonSegmentAllocations(visible);
  return (
    <main
      attrs={{
        id: "chat-scroll",
        class: "chat-scroll",
        "aria-label": "Conversation",
        tabindex: "0",
      }}
      on={{
        keydown: context.actions.onChatKeyDown,
        pointerdown: context.actions.onChatManualIntent,
        scroll: context.actions.onChatScroll,
        touchmove: context.actions.onChatManualIntent,
        touchstart: context.actions.onChatManualIntent,
        wheel: context.actions.onChatManualIntent,
      }}
    >
      {snapshot.blocks.omitted_before > 0 ? (
        <div attrs={{ class: "bounded-notice" }}>
          {`${snapshot.blocks.omitted_before} earlier blocks were omitted by the server.`}
        </div>
      ) : null}
      {start > 0 ? (
        <div
          attrs={{ class: "virtual-spacer", "aria-hidden": "true" }}
          style={{ height: `${start * CHAT_ROW_ESTIMATE}px` }}
        ></div>
      ) : null}
      <div attrs={{ class: "chat-window" }}>
        {visible.map((block, offset) =>
          renderBlock(
            block,
            start + offset,
            jsonSegmentAllocations[offset] ?? 0,
            chatBlockAnchor(snapshot, start + offset),
          ),
        )}
      </div>
      {end < blocks.length ? (
        <div
          attrs={{ class: "virtual-spacer", "aria-hidden": "true" }}
          style={{ height: `${(blocks.length - end) * CHAT_ROW_ESTIMATE}px` }}
        ></div>
      ) : null}
      {snapshot.blocks.truncated ? (
        <div attrs={{ class: "bounded-notice warning" }}>
          Conversation content was bounded by the server.
        </div>
      ) : null}
    </main>
  );
}

function renderActivity(context: ChatViewContext): VNode {
  const { snapshot, live } = context.state;
  if (snapshot === null) {
    return <section attrs={{ class: "activity-area is-empty" }}></section>;
  }
  const approval = snapshot.pending_approval;
  const question = snapshot.pending_question;
  const spinnerKind = activitySpinnerKind(snapshot.activity.kind, live);
  return (
    <section
      attrs={{
        class: `activity-area activity-${snapshot.activity.kind}`,
        "aria-label": "Current activity and requests",
      }}
    >
      <div attrs={{ class: "activity-summary" }}>
        <strong>
          {spinnerKind === null ? null : activitySpinner(spinnerKind)}
          <span>{activityLabel(snapshot.activity.kind)}</span>
        </strong>
        {snapshot.activity.progress_percent === null ? null : (
          <progress
            attrs={{ "aria-label": "Activity progress" }}
            props={{ value: snapshot.activity.progress_percent, max: 100 }}
          ></progress>
        )}
      </div>
      {approval === null ? null : (
        <button
          attrs={{ type: "button", class: "attention-card" }}
          props={{ disabled: !live }}
          on={{ click: () => context.actions.openPanel("tool_approval") }}
        >
          <strong>
            {uiIcon("warning")}
            {approval.tool_name}
          </strong>
          <span>{approval.description}</span>
          <span>Review approval</span>
        </button>
      )}
      {question === null ? null : (
        <button
          attrs={{ type: "button", class: "attention-card" }}
          props={{ disabled: !live }}
          on={{ click: () => context.actions.openPanel("ask_user") }}
        >
          <strong>{uiIcon("warning")}Answer requested</strong>
          <span>
            {question.questions[0]?.prompt ?? "The agent has a question."}
          </span>
          <span>
            {question.content_truncated
              ? "Question unavailable remotely — open to cancel"
              : "Open question"}
          </span>
        </button>
      )}
    </section>
  );
}

function renderComposer(context: ChatViewContext): VNode {
  const { snapshot, live, draft } = context.state;
  const canSubmit =
    live &&
    snapshot?.activity.fully_idle === true &&
    draft.trim().length > 0;
  const rows = Math.max(
    2,
    Math.min(10, draft.split("\n").length + Math.floor(draft.length / 96)),
  );
  return (
    <form
      attrs={{ class: "composer", "aria-label": "Prompt composer" }}
      on={{
        submit: (event: SubmitEvent) => {
          event.preventDefault();
          context.actions.submitPrompt();
        },
      }}
    >
      <label attrs={{ for: "composer-input", class: "sr-only" }}>Prompt</label>
      <textarea
        attrs={{
          id: "composer-input",
          maxlength: String(MAX_DRAFT_LENGTH),
          rows: String(rows),
          placeholder: live
            ? "Type a prompt. Enter submits; Shift+Enter adds a line."
            : "Waiting for a synchronized connection…",
          "aria-describedby": "composer-help",
        }}
        props={{ value: draft, disabled: !live || snapshot === null }}
        on={{
          input: (event: InputEvent) =>
            context.actions.updateDraft(inputValue(event)),
          keydown: (event: KeyboardEvent) => {
            if (
              event.key === "Enter" &&
              !event.shiftKey &&
              !event.isComposing
            ) {
              event.preventDefault();
              context.actions.submitPrompt();
            }
          },
        }}
      ></textarea>
      <div attrs={{ class: "composer-actions" }}>
        <span attrs={{ id: "composer-help", class: "composer-help" }}>
          Draft stays in this browser and is never persisted.
        </span>
        {snapshot?.activity.cancellable === true ? (
          <button
            attrs={{ type: "button", class: "danger-button" }}
            props={{ disabled: !live }}
            on={{ click: context.actions.stop }}
          >
            {uiIcon("stop")}
            Stop
          </button>
        ) : null}
        <button
          attrs={{ type: "submit", class: "primary-button" }}
          props={{ disabled: !canSubmit }}
        >
          {uiIcon("send")}
          Send
        </button>
      </div>
    </form>
  );
}

export function renderChatView(context: ChatViewContext): VNode {
  const { state } = context;
  const debuggerOpen = state.snapshot?.debugger.open === true;
  const debuggerVisible =
    debuggerOpen &&
    (state.debuggerWide || !state.debuggerDrawerDismissed);
  const debuggerMode = state.debuggerWide ? "debugger-wide" : "debugger-narrow";
  return (
    <div
      attrs={{
        id: "app",
        class: `app-shell ${debuggerMode}${debuggerVisible ? " debugger-open" : ""}`,
        "data-connection": state.transportStatus.phase,
        "data-debugger": debuggerVisible ? "open" : "closed",
        "aria-busy": state.live ? "false" : "true",
      }}
    >
      {renderHeader(context)}
      <div
        attrs={{
          class: `workspace ${debuggerMode}${debuggerVisible ? " has-debugger" : ""}`,
        }}
      >
        <div attrs={{ class: `conversation-column${state.filesPane == null ? "" : " has-files"}` }}>
          {state.filesPane}
          {renderChat(context)}
          {renderActivity(context)}
          {renderComposer(context)}
        </div>
        {renderDebugger(context)}
      </div>
      {renderApplicationStatus(state.snapshot)}
      {renderTransportStatus(state)}
      <div
        attrs={{
          class: "sr-only",
          role: "status",
          "aria-live": "polite",
          "aria-atomic": "true",
        }}
      >
        {state.toast ?? state.transportStatus.label}
      </div>
      {state.toast === null ? null : (
        <div attrs={{ class: "toast", role: "status" }}>{state.toast}</div>
      )}
      {state.overlay}
    </div>
  );
}
