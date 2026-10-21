import { highlightCode } from "../highlight.js";
import { pythonToolCode } from "./helpers.js";
import { jsx as h } from "../../lib/snabbdom/build/index.js";
import type { VNode } from "../../lib/snabbdom/build/index.js";
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
import { renderBackgroundTasks } from "./background-tasks.js";
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
        {context.state.paletteIcon ?? uiIcon("command")}
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
        <div attrs={{ class: "session-name-row" }}>
          <strong>
            {snapshot === null ? "Waiting for state" : sessionLabel(snapshot)}
          </strong>
          <button
            attrs={{
              type: "button",
              class: "icon-button session-rename",
              "aria-label": "Name or rename the current session",
              title: "Rename session",
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
          </button>
        </div>
      </div>
      <div attrs={{ class: "model-summary" }}>
        <span attrs={{ class: "eyebrow" }}>Model</span>
        <span>{snapshot?.status.model_label ?? "—"}</span>
      </div>
      {context.state.headerActions ?? null}
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
  actions: VNode | null,
  codeActions?: (code: string, language: string | null) => VNode | null,
  copyContents?: (content: string) => void,
  originalPython = false,
  setPythonOriginal?: (original: boolean) => void,
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
  const python = pythonToolCode(block);
  const parsed = python !== null && !originalPython;
  const pythonDisplay = python === null ? "" : boundedText(python);
  const renderedContent = parsed ? <pre attrs={{ class: "python-tool-code", "data-python-view": "parsed" }}><code attrs={{ class: "code-highlight" }}>{highlightCode(pythonDisplay, block.tool?.tool_name === "dotnet" ? "csharp" : "python") ?? [pythonDisplay]}</code></pre> : isMarkdownBlockKind(block.kind)
    ? markdownContentThunk(
        block.kind,
        displayContent,
        markdownBlockUsesJsonHighlighting(block),
        maxJsonSegments,
        codeActions,
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
      {actions}
      {block.duration_label === null ? null : (
        <p attrs={{ class: "block-duration" }}>⏱ {block.duration_label}</p>
      )}
      {block.usage === null ? null : (
        <details attrs={{ class: "block-usage" }}>
          <summary>Usage</summary>
          <span>{`${block.usage.total_tokens} total tokens`}</span>
          {block.estimated_cost === null ? null : (
            <span>{`${block.estimated_cost.display} estimated`}</span>
          )}
        </details>
      )}
      <footer attrs={{ class: "chat-block-actions" }}>
        {python === null ? null : <span attrs={{ class: "python-view-controls", role: "group", "aria-label": "Python code display" }}>
          <button attrs={{ type: "button", "data-python-view": "parsed", "aria-pressed": String(parsed), title: "Show code with indentation and syntax highlighting" }}
            props={{ disabled: !setPythonOriginal }} on={{ click: () => setPythonOriginal?.(false) }}>{uiIcon("code")}Parsed</button>
          <button attrs={{ type: "button", "data-python-view": "original", "aria-pressed": String(!parsed), title: "Show the original tool arguments" }}
            props={{ disabled: !setPythonOriginal }} on={{ click: () => setPythonOriginal?.(true) }}>{uiIcon("file")}Original</button>
        </span>}

        <button attrs={{ type: "button", "data-copy-turn": "", title: "Copy this turn’s contents" }}
          props={{ disabled: !copyContents || !content }} on={{ click: () => copyContents?.(parsed ? python : content) }}>
          {uiIcon("copy")}Copy
        </button>
      </footer>
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
            context.state.blockActions?.(block) ?? null,
            context.state.codeActions,
            context.actions.copyContents,
            context.state.originalPythonBlocks?.has(`${snapshot.session.session_id}:${chatBlockAnchor(snapshot, start + offset) ?? start + offset}`) ?? false,
            context.actions.setPythonOriginal ? original => context.actions.setPythonOriginal?.(`${snapshot.session.session_id}:${chatBlockAnchor(snapshot, start + offset) ?? start + offset}`, original) : undefined,
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
          <span attrs={{ class: `activity-label${spinnerKind === "regular" ? " is-processing" : ""}` }}>{activityLabel(snapshot.activity.kind)}</span>
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
            ? context.state.promptPlaceholder ?? "Type a prompt. Enter submits; Shift+Enter adds a line. Ctrl+P opens commands."
            : "Waiting for a synchronized connection…",
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
      {state.customStatus ?? <div attrs={{ class: "status-stack" }}>
        {renderBackgroundTasks(state.snapshot)}
        {renderApplicationStatus(state.snapshot)}
      </div>}
      {state.customStatus ? null : renderTransportStatus(state)}
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
