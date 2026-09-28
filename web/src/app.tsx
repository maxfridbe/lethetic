import {
  attributesModule,
  classModule,
  eventListenersModule,
  init,
  propsModule,
  styleModule,
} from "../lib/snabbdom/build/index.js";
import type { VNode } from "../lib/snabbdom/build/index.js";
import { ChatFollowController } from "./chat-follow.js";
import type { ChatScrollMetrics } from "./chat-follow.js";
import {
  captureChatAnchor,
  restoreChatAnchor,
} from "./app/chat-anchor.js";
import type { ChatAnchor } from "./app/chat-anchor.js";
import { THEME_CATALOG } from "./generated/contracts.js";
import type {
  ApprovalDecision,
  CommandOutcome,
  CommandView,
  ConfirmationView,
  ICommandRequest,
  ICommandResponse,
  IServerMessage,
  PanelId,
  PendingApprovalView,
  PendingQuestionView,
  QuestionPromptView,
  ThemeColorsView,
  WebAppSnapshot,
  WebCommand,
} from "./generated/contracts.js";
import {
  beginRemoteConnection,
  createRemoteState,
  disconnectRemote,
  reduceServerMessage,
} from "./protocol.js";
import type { RemoteState } from "./protocol.js";
import { assertNever, boundedText, isSafeCssColor } from "./safety.js";
import { BrowserTransport } from "./transport.js";
import type {
  BrowserTransportEvents,
  TransportStatus,
} from "./transport.js";
import { renderChatView } from "./app/chat-view.js";
import { historyRecallMatches } from "./app/history-recall.js";
import type { PendingHistoryRecall } from "./app/history-recall.js";
import { FileClient } from "./files/client.js";
import { FilesController } from "./files/state.js";
import { MonacoDiffViewer, MonacoFileEditor } from "./files/editor.js";
import { newlyFinished } from "./app/background-tasks.js";
import { renderFilesPane } from "./files/view.js";
import {
  CHAT_FOLLOW_SCHEDULER,
  CHAT_WINDOW_SIZE,
  MAX_DRAFT_LENGTH,
  MAX_PROMPT_BYTES,
  THEME_VARIABLES,
  approvalConfirmationKey,
  approvalHasHiddenContent,
  chatScrollMetrics,
  chatWindowStartForScroll,
  commandForId,
  confirmationMatches as retainedConfirmationMatches,
  confirmedCommand,
  isChatScrollKey,
  overlayFocusTransition,
  overlaySignature,
  panelData,
  panelInstanceKey,
  rebaseChatWindowStart,
  snapshotOverlaySignature,
  utf8Length,
} from "./app/helpers.js";
import type {
  AffirmativeApprovalDecision,
  ConfirmableCommand,
} from "./app/helpers.js";
import {
  handleGlobalKeyDown,
  handlePaletteKeyDown,
} from "./app/keyboard.js";
import { renderPanel } from "./app/panels.js";
import {
  getQuestionDraft,
  pruneQuestionDrafts,
  questionAnswers,
  questionAnswersComplete,
  questionAnswersFit,
  systemPromptSaveProblem,
} from "./app/state.js";
import type {
  ApprovalConfirmation,
  ChatViewContext,
  KeyboardContext,
  PaletteState,
  PanelViewContext,
  QuestionDraft,
} from "./app/state.js";

export {
  activitySpinnerKind,
  approvalConfirmationKey,
  approvalHasHiddenContent,
  approvalUsesJsonHighlighting,
  blockContentIsLossy,
  chatJsonSegmentAllocations,
  hiddenApprovalConfirmationMessage,
  markdownBlockUsesJsonHighlighting,
  panelInstanceKey,
  projectionLossMessages,
  toolBlockUsesJsonHighlighting,
  toolResultUsesAutoRendering,
} from "./app/helpers.js";

const PROTECTED_SERVER_PANELS = new Set<PanelId>([
  "tool_approval",
  "ask_user",
  "confirmation",
  "sessions",
  "name_session",
]);

function isProtectedServerPanel(panel: PanelId | null): boolean {
  return panel !== null && PROTECTED_SERVER_PANELS.has(panel);
}

export class SpaApplication implements BrowserTransportEvents {
  readonly #patch = init([
    attributesModule,
    classModule,
    eventListenersModule,
    propsModule,
    styleModule,
  ]);
  #tree: VNode | Element;
  #transport: BrowserTransport | null = null;
  #remote: RemoteState = createRemoteState();
  #transportStatus: TransportStatus = {
    phase: "idle",
    label: "Not connected",
    attempt: 0,
    retryInMilliseconds: null,
  };
  #draft = "";
  #draftVersion = 0;
  #historyRecall: PendingHistoryRecall | null = null;
  #filesEnabled = false;
  readonly #files = new FilesController(() => this.#render(), (message) => this.#showToast(message));
  readonly #fileEditor = new MonacoFileEditor(
    () => this.#showToast("Monaco could not load. Downloads and Copy Path remain available."),
    () => this.#synchronizeChatPosition(this.#remote.view, null),
  );
  readonly #diffViewer = new MonacoDiffViewer(
    () => this.#showToast("Monaco could not load the diff view."),
  );
  #promptRequestVersions = new Map<string, number>();
  #palette: PaletteState | null = null;
  #forcedPanel: PanelId | null = null;
  #dismissedOverlay: string | null = null;
  #toast: string | null = null;
  #toastTimer: number | null = null;
  #questionDrafts = new Map<string, QuestionDraft>();
  #editorName = "";
  #editorContent = "";
  #editorSource = "";
  #editorDirty = false;
  #editorVersion = 0;
  #editorRequestVersions = new Map<string, number>();
  #sessionName = "";
  #sessionNameDirty = false;
  #sessionNameSource: string | null = null;
  #sessionNameSessionId: string | null = null;
  #discardEditorConfirmation = false;
  #confirmable: ConfirmableCommand | null = null;
  #confirmableEditorVersion: number | null = null;
  #confirmationRequestId: string | null = null;
  #approvalConfirmation: ApprovalConfirmation | null = null;
  #chatStart = 0;
  readonly #chatFollow: ChatFollowController;
  #debuggerMedia: MediaQueryList | null = null;
  #debuggerWide = true;
  #narrowDebuggerDismissed = false;
  #debuggerDismissalNeedsReconcile = false;
  #debuggerDismissalRequestId: string | null = null;
  #focusBeforeOverlay: HTMLElement | null = null;
  #renderedPanelKey: string | null = null;
  #revisionLagTimer: number | null = null;
  #revisionLagTarget: number | null = null;
  #snapshotWaitTimer: number | null = null;
  readonly #onDebuggerMediaChange = (event: MediaQueryListEvent): void => {
    if (this.#debuggerWide === event.matches) {
      return;
    }
    this.#debuggerWide = event.matches;
    this.#render();
  };

  constructor(root: Element) {
    this.#tree = root;
    this.#chatFollow = new ChatFollowController(
      CHAT_FOLLOW_SCHEDULER,
      () => this.#resumeChatFollowing(),
    );
    if (typeof globalThis.matchMedia === "function") {
      this.#debuggerMedia = globalThis.matchMedia("(min-width: 1024px)");
      this.#debuggerWide = this.#debuggerMedia.matches;
      this.#debuggerMedia.addEventListener(
        "change",
        this.#onDebuggerMediaChange,
      );
    }
    globalThis.addEventListener("keydown", this.#onGlobalKeyDown);
    globalThis.addEventListener("pagehide", () => {
      this.#files.reset();
      this.#fileEditor.dispose();
    });
    this.#render();
  }

  attachTransport(transport: BrowserTransport): void {
    this.#transport = transport;
    this.#files.attach(new FileClient((endpoint, path, signal) => transport.requestFile(endpoint, path, signal)));
  }

  socketOpened(): void {
    this.#dismissedOverlay = null;
    this.#approvalConfirmation = null;
    this.#remote = beginRemoteConnection(this.#remote);
    this.#render();
  }

  authenticationEpochChanged(abandoned: readonly ICommandRequest[]): void {
    this.#historyRecall = null;
    this.#filesEnabled = false;
    this.#files.reset();
    this.#fileEditor.dispose();
    this.#approvalConfirmation = null;
    this.#narrowDebuggerDismissed = false;
    this.#debuggerDismissalNeedsReconcile = false;
    this.#debuggerDismissalRequestId = null;
    this.#chatFollow.reset();
    this.#chatStart = 0;
    const abandonedIds = new Set(abandoned.map((request) => request.id));
    for (const requestId of abandonedIds) {
      this.#editorRequestVersions.delete(requestId);
      this.#promptRequestVersions.delete(requestId);
    }
    if (
      this.#confirmationRequestId !== null &&
      abandonedIds.has(this.#confirmationRequestId)
    ) {
      this.#confirmationRequestId = null;
      this.#confirmable = null;
      this.#confirmableEditorVersion = null;
    }
    this.#clearSnapshotWait();
    if (this.#revisionLagTimer !== null) {
      globalThis.clearTimeout(this.#revisionLagTimer);
      this.#revisionLagTimer = null;
    }
    this.#revisionLagTarget = null;
    this.#remote = createRemoteState();
    if (abandoned.length > 0) {
      this.#showToast(
        `${String(abandoned.length)} pending command outcome(s) became unknown after the server session changed; they were not replayed.`,
      );
    } else {
      this.#render();
    }
  }

  serverMessage(
    message: Exclude<IServerMessage, { type: "command_response" }>,
  ): void {
    const previous = this.#remote;
    const reduction = reduceServerMessage(this.#remote, message);
    this.#remote = reduction.state;
    const nextView = this.#remote.view;
    if (nextView !== null && previous.view !== null) {
      const finished = newlyFinished(previous.view, nextView);
      if (finished.length > 0) {
        this.#showToast(finished.map((task) =>
          `${task.id} ${task.state_label}: ${task.description}`).join(" · "));
      }
    }
    if (message.type === "hello" && reduction.effect !== "fatal") {
      this.#filesEnabled = message.hello.capabilities.read_only_files;
      this.#transport?.noteHello(this.#filesEnabled);
      if (!this.#filesEnabled) this.#files.reset();
    }
    switch (reduction.effect) {
      case "fatal":
        this.#transport?.closePermanently(
          reduction.state.reason ?? "The server protocol is incompatible.",
        );
        break;
      case "request_snapshot":
        this.#requestSnapshot(reduction.state.reason ?? "Resynchronizing");
        break;
      case "none":
        if (reduction.state.phase === "live") {
          this.#transport?.markSynchronized();
          this.#clearSnapshotWait();
          this.#clearRevisionLagIfCurrent();
        }
        break;
      default:
        assertNever(reduction.effect, "remote effect");
    }
    if (previous.view !== this.#remote.view && this.#remote.view !== null) {
      this.#synchronizeLocalState(previous.view, this.#remote.view);
    }
    this.#reconcileDebuggerDismissal();
    this.#render();
  }

  commandResponse(response: ICommandResponse, request: ICommandRequest): void {
    if (this.#debuggerDismissalRequestId === response.id) {
      this.#debuggerDismissalRequestId = null;
      if (response.result.status === "error") {
        this.#narrowDebuggerDismissed = false;
        this.#debuggerDismissalNeedsReconcile = false;
      }
    }
    if (
      request.type === "approve_tool_once" ||
      request.type === "approve_tool_always" ||
      request.type === "deny_tool"
    ) {
      this.#approvalConfirmation = null;
    }
    if (this.#confirmationRequestId === response.id) {
      if (
        response.result.status !== "error" ||
        response.result.error.code !== "confirmation_required"
      ) {
        this.#confirmable = null;
        this.#confirmableEditorVersion = null;
      }
      this.#confirmationRequestId = null;
    }

    switch (response.result.status) {
      case "ok":
        this.#handleOutcome(response.result.outcome, request, response.id);
        if (response.result.revision > this.#remote.revision) {
          this.#scheduleRevisionLag(response.result.revision);
        }
        break;
      case "error": {
        const error = response.result.error;
        if (error.code === "confirmation_required") {
          this.#showToast("Review and confirm the destructive action.");
        } else {
          this.#showToast(error.message);
        }
        if (
          error.code === "stale_revision" ||
          (error.current_revision !== null &&
            error.current_revision > this.#remote.revision)
        ) {
          this.#remote = {
            ...this.#remote,
            phase: "stale",
            reason: "The command revision was stale.",
          };
          this.#requestSnapshot("Command state changed; resynchronizing");
        }
        break;
      }
      default:
        assertNever(response.result, "command result");
    }
    this.#editorRequestVersions.delete(response.id);
    this.#promptRequestVersions.delete(response.id);
    if (this.#historyRecall?.requestId === response.id) this.#historyRecall = null;
    this.#render();
  }

  transportStatus(status: TransportStatus): void {
    this.#transportStatus = status;
    if (status.phase !== "live") this.#files.suspend();
    if (status.phase === "incompatible" || status.phase === "closed") {
      this.#files.reset();
      this.#fileEditor.dispose();
      this.#filesEnabled = false;
    }
    if (
      status.phase === "reconnecting" ||
      status.phase === "offline" ||
      status.phase === "closed"
    ) {
      this.#approvalConfirmation = null;
      this.#remote = disconnectRemote(this.#remote, status.label);
    }
    this.#render();
  }

  protocolFault(reason: string): void {
    this.#approvalConfirmation = null;
    this.#showToast(reason);
    if (this.#remote.phase === "live") {
      this.#remote = { ...this.#remote, phase: "stale", reason };
      this.#requestSnapshot(reason);
    }
    this.#render();
  }

  #handleOutcome(
    outcome: CommandOutcome,
    request: ICommandRequest,
    responseId: string,
  ): void {
    switch (outcome.type) {
      case "applied":
        if (
          request.type === "save_system_prompt" &&
          this.#editorRequestVersions.get(responseId) === this.#editorVersion
        ) {
          this.#editorDirty = false;
          this.#editorName = request.name;
          this.#editorContent = request.content;
          this.#editorSource = `${request.name}:${request.content}`;
        }
        return;
      case "snapshot_queued":
        return;
      case "panel_opened":
        this.#forcedPanel = null;
        return;
      case "prompt_accepted":
        if (
          request.type === "send_prompt" &&
          this.#promptRequestVersions.get(responseId) === this.#draftVersion &&
          this.#draft.trim() === request.prompt
        ) {
          this.#draft = "";
          this.#draftVersion += 1;
        }
        return;
      case "history_entry_selected":
        if (historyRecallMatches(
          this.#historyRecall, outcome, request, responseId,
          this.#remote.view?.session.session_id ?? null, this.#draftVersion,
        )) {
          this.#draft = outcome.editor_content;
          this.#draftVersion += 1;
          this.#forcedPanel = null;
          this.#dismissedOverlay = overlaySignature(this.#remote);
        }
        return;
      case "tool_decision_recorded":
        this.#approvalConfirmation = null;
        this.#forcedPanel = null;
        return;
      case "session_created":
      case "session_loaded":
      case "session_deleted":
      case "sessions_wiped":
      case "session_renamed":
        this.#forcedPanel = null;
        return;
      case "shutting_down":
        this.#showToast("Lethetic is shutting down.");
        return;
      default:
        assertNever(outcome, "command outcome");
    }
  }

  #synchronizeLocalState(
    previous: WebAppSnapshot | null,
    current: WebAppSnapshot,
  ): void {
    this.#applyTheme(current);
    if (previous?.session.session_id !== current.session.session_id) this.#historyRecall = null;
    if (isProtectedServerPanel(current.overlay.active_panel)) {
      this.#palette = null;
      this.#discardEditorConfirmation = false;
    }
    this.#synchronizeDismissedOverlay(previous, current);
    this.#synchronizeDebuggerDismissal(previous, current);
    this.#synchronizeApprovalConfirmation(current);
    pruneQuestionDrafts(this.#questionDrafts, current.pending_question);
    this.#synchronizeEditor(current);
    this.#synchronizeSessionName(previous, current);
    this.#synchronizeChatWindow(previous, current);
  }

  #synchronizeDismissedOverlay(
    previous: WebAppSnapshot | null,
    current: WebAppSnapshot,
  ): void {
    const previousOverlay = snapshotOverlaySignature(previous);
    const currentOverlay = snapshotOverlaySignature(current);
    if (
      this.#dismissedOverlay !== null &&
      previousOverlay === this.#dismissedOverlay &&
      currentOverlay !== previousOverlay
    ) {
      this.#dismissedOverlay = null;
    }
  }

  #synchronizeDebuggerDismissal(
    previous: WebAppSnapshot | null,
    current: WebAppSnapshot,
  ): void {
    if (!current.debugger.open || previous?.debugger.open === false) {
      this.#narrowDebuggerDismissed = false;
      this.#debuggerDismissalNeedsReconcile = false;
      this.#debuggerDismissalRequestId = null;
    }
  }

  #synchronizeApprovalConfirmation(current: WebAppSnapshot): void {
    const approval = current.pending_approval;
    const confirmation = this.#approvalConfirmation;
    if (confirmation === null) {
      return;
    }
    if (
      approval === null ||
      confirmation.key !==
        approvalConfirmationKey(this.#remote.revision, approval) ||
      !approval.allowed_decisions.includes(confirmation.decision)
    ) {
      this.#approvalConfirmation = null;
    }
  }

  #synchronizeEditor(current: WebAppSnapshot): void {
    const data = panelData(current, "system_prompts");
    if (data === null || data.editor_content === null) {
      return;
    }
    const selected = data.prompts.find((prompt) => prompt.selected);
    const source = `${selected?.prompt_id ?? "new"}:${data.editor_content}`;
    if (this.#editorDirty || source === this.#editorSource) {
      return;
    }
    this.#editorContent = data.editor_content;
    this.#editorName = selected?.label ?? this.#editorName;
    this.#editorSource = source;
    this.#editorVersion += 1;
  }

  #synchronizeSessionName(
    previous: WebAppSnapshot | null,
    current: WebAppSnapshot,
  ): void {
    const data = panelData(current, "name_session");
    const previousData =
      previous === null ? null : panelData(previous, "name_session");
    if (data === null) {
      if (previousData !== null) {
        this.#sessionNameDirty = false;
        this.#sessionNameSource = null;
        this.#sessionNameSessionId = null;
      }
      return;
    }

    const source = data.current_name ?? "";
    const sessionChanged =
      this.#sessionNameSessionId !== current.session.session_id;
    if (
      sessionChanged ||
      (!this.#sessionNameDirty && source !== this.#sessionNameSource)
    ) {
      this.#sessionName = source;
      this.#sessionNameDirty = false;
    }
    this.#sessionNameSource = source;
    this.#sessionNameSessionId = current.session.session_id;
  }

  #synchronizeChatWindow(
    previous: WebAppSnapshot | null,
    current: WebAppSnapshot,
  ): void {
    const sessionChanged =
      previous === null ||
      previous.session.session_id !== current.session.session_id;
    if (sessionChanged) {
      this.#chatFollow.reset();
      this.#chatStart = Math.max(
        0,
        current.blocks.blocks.length - CHAT_WINDOW_SIZE,
      );
      return;
    }
    this.#chatStart = rebaseChatWindowStart(
      this.#chatStart,
      previous,
      current,
    );
  }

  #applyTheme(snapshot: WebAppSnapshot): void {
    const selected = snapshot.themes.find((theme) => theme.selected);
    const fallback = THEME_CATALOG[0];
    const colors = selected?.colors ?? fallback.colors;
    for (const key of Object.keys(THEME_VARIABLES) as Array<keyof ThemeColorsView>) {
      const candidate = colors[key];
      const safe = isSafeCssColor(candidate) ? candidate : fallback.colors[key];
      globalThis.document.documentElement.style.setProperty(
        THEME_VARIABLES[key],
        safe,
      );
    }
  }

  #requestSnapshot(reason: string): void {
    this.#approvalConfirmation = null;
    this.#transport?.markStale(reason);
    this.#transport?.requestSnapshot(this.#remote.revision);
    if (this.#snapshotWaitTimer !== null) {
      return;
    }
    this.#snapshotWaitTimer = globalThis.setTimeout(() => {
      this.#snapshotWaitTimer = null;
      if (this.#remote.phase === "stale") {
        this.#requestSnapshot("Still waiting for a current snapshot");
      }
    }, 2000);
  }

  #clearSnapshotWait(): void {
    if (this.#snapshotWaitTimer !== null) {
      globalThis.clearTimeout(this.#snapshotWaitTimer);
      this.#snapshotWaitTimer = null;
    }
  }

  #scheduleRevisionLag(targetRevision: number): void {
    this.#revisionLagTarget = Math.max(
      this.#revisionLagTarget ?? 0,
      targetRevision,
    );
    if (this.#revisionLagTimer !== null) {
      globalThis.clearTimeout(this.#revisionLagTimer);
    }
    this.#revisionLagTimer = globalThis.setTimeout(() => {
      this.#revisionLagTimer = null;
      const target = this.#revisionLagTarget;
      if (target !== null && this.#remote.revision < target) {
        this.#remote = {
          ...this.#remote,
          phase: "stale",
          reason: "The state stream lagged behind a command response.",
        };
        this.#requestSnapshot("State stream lag detected; resynchronizing");
        this.#render();
      } else {
        this.#revisionLagTarget = null;
      }
    }, 900);
  }

  #clearRevisionLagIfCurrent(): void {
    if (
      this.#revisionLagTarget === null ||
      this.#remote.revision < this.#revisionLagTarget
    ) {
      return;
    }
    if (this.#revisionLagTimer !== null) {
      globalThis.clearTimeout(this.#revisionLagTimer);
      this.#revisionLagTimer = null;
    }
    this.#revisionLagTarget = null;
  }

  #send(command: WebCommand): ICommandRequest | null {
    if (this.#remote.phase !== "live") {
      this.#showToast("Commands are disabled until state is synchronized.");
      return null;
    }
    const transport = this.#transport;
    const request = transport?.send(command, this.#remote.revision) ?? null;
    if (request === null) {
      this.#showToast(
        transport?.lastSendFailure ??
          "The command could not be queued while disconnected or busy.",
      );
    } else {
      this.#render();
    }
    return request;
  }

  #sendConfirmable(
    command: ConfirmableCommand,
    editorVersion: number | null = null,
  ): ICommandRequest | null {
    this.#confirmable = command;
    this.#confirmableEditorVersion = editorVersion;
    const request = this.#send(command);
    if (request === null) {
      this.#confirmable = null;
      this.#confirmableEditorVersion = null;
      this.#confirmationRequestId = null;
      return null;
    }
    if (editorVersion !== null) {
      this.#editorRequestVersions.set(request.id, editorVersion);
    }
    this.#confirmationRequestId = request.id;
    return request;
  }

  #invokeCommand(command: CommandView): void {
    if (!command.enabled) {
      this.#showToast(command.disabled_reason ?? "Command is unavailable.");
      return;
    }
    const snapshot = this.#remote.view;
    if (snapshot === null) {
      return;
    }
    this.#palette = null;
    this.#dismissedOverlay = null;
    switch (command.id) {
      case "clear-context":
        this.#sendConfirmable({
          type: "clear_context",
          session_id: snapshot.session.session_id,
          confirmed: false,
          confirmation_id: null,
        });
        return;
      case "delete-python-runtime":
        this.#sendConfirmable({
          type: "delete_python_runtime",
          session_id: snapshot.session.session_id,
          confirmed: false,
          confirmation_id: null,
        });
        return;
      case "quit":
        this.#sendConfirmable({
          type: "quit",
          confirmed: false,
          confirmation_id: null,
        });
        return;
      case "hotkeys":
      case "themes":
      case "input-history":
      case "loop-detection":
      case "system-prompt":
      case "clear-ui":
      case "toggle-debugger":
      case "sessions":
      case "name-session":
      case "latest-files":
      case "models":
      case "lsp-servers":
      case "agent-mode":
      case "agent-general":
      case "python-isolated":
      case "python-nonlocal":
      case "python-permissive":
      case "remote-control":
      case "toggle-todos":
      case "toggle-background-tasks":
      case "background-mode":
        this.#send({ type: "invoke_command", command_id: command.id });
        return;
      default:
        assertNever(command.id, "command ID");
    }
  }

  #reconcileDebuggerDismissal(): void {
    if (
      !this.#debuggerDismissalNeedsReconcile ||
      this.#remote.phase !== "live"
    ) {
      return;
    }
    const snapshot = this.#remote.view;
    const command =
      snapshot === null ? null : commandForId(snapshot, "toggle-debugger");
    if (
      snapshot === null ||
      !snapshot.debugger.open ||
      command === null ||
      !command.enabled
    ) {
      this.#narrowDebuggerDismissed = false;
      this.#debuggerDismissalNeedsReconcile = false;
      return;
    }
    this.#debuggerDismissalNeedsReconcile = false;
    const request = this.#send({
      type: "invoke_command",
      command_id: command.id,
    });
    if (request === null) {
      this.#narrowDebuggerDismissed = false;
      return;
    }
    this.#debuggerDismissalRequestId = request.id;
  }

  #toggleDebugger(): void {
    const snapshot = this.#remote.view;
    if (snapshot === null) {
      return;
    }
    if (
      snapshot.debugger.open &&
      !this.#debuggerWide &&
      this.#remote.phase !== "live"
    ) {
      this.#narrowDebuggerDismissed = true;
      this.#debuggerDismissalNeedsReconcile = true;
      this.#showToast(
        "Debugger hidden locally; shared state will reconcile after synchronization.",
      );
      return;
    }
    const command = commandForId(snapshot, "toggle-debugger");
    if (command === null) {
      return;
    }
    if (snapshot.debugger.open && !this.#debuggerWide) {
      if (!command.enabled) {
        this.#showToast(command.disabled_reason ?? "Command is unavailable.");
        return;
      }
      const request = this.#send({
        type: "invoke_command",
        command_id: command.id,
      });
      if (request !== null) {
        this.#narrowDebuggerDismissed = true;
        this.#debuggerDismissalRequestId = request.id;
        this.#render();
      }
      return;
    }
    this.#invokeCommand(command);
  }

  #sendStop(): void {
    const snapshot = this.#remote.view;
    if (snapshot === null || !snapshot.activity.cancellable || snapshot.activity.cancel_id === null) {
      return;
    }
    this.#send({
      type: "stop",
      session_id: snapshot.session.session_id,
      cancel_id: snapshot.activity.cancel_id,
    });
  }

  #submitPrompt(): void {
    const snapshot = this.#remote.view;
    const prompt = this.#draft.trim();
    if (
      snapshot === null ||
      !snapshot.activity.fully_idle ||
      prompt.length === 0
    ) {
      return;
    }
    if (utf8Length(prompt) > MAX_PROMPT_BYTES) {
      this.#showToast("Prompt exceeds the 128 KiB UTF-8 limit.");
      return;
    }
    const request = this.#send({
      type: "send_prompt",
      session_id: snapshot.session.session_id,
      prompt,
    });
    if (request !== null) {
      this.#promptRequestVersions.set(request.id, this.#draftVersion);
    }
  }

  #showToast(message: string): void {
    this.#toast = boundedText(message, 2048);
    if (this.#toastTimer !== null) {
      globalThis.clearTimeout(this.#toastTimer);
    }
    this.#toastTimer = globalThis.setTimeout(() => {
      this.#toast = null;
      this.#toastTimer = null;
      this.#render();
    }, 6000);
    this.#render();
  }

  #openPalette(): void {
    if (this.#palette !== null || this.#activePanel() !== null) {
      return;
    }
    this.#palette = { query: "", selected: 0 };
    this.#approvalConfirmation = null;
    this.#render();
  }

  #activePanel(): PanelId | null {
    const remotePanel = this.#remote.view?.overlay.active_panel ?? null;
    if (isProtectedServerPanel(remotePanel)) {
      return remotePanel;
    }
    if (this.#discardEditorConfirmation) {
      return "confirmation";
    }
    if (this.#palette !== null) {
      return "command_palette";
    }
    if (this.#forcedPanel !== null) {
      return this.#forcedPanel;
    }
    const signature = overlaySignature(this.#remote);
    if (signature === null || signature === this.#dismissedOverlay) {
      return null;
    }
    return remotePanel;
  }

  #closeOverlay(forceDiscard = false): void {
    if (
      this.#discardEditorConfirmation &&
      this.#activePanel() === "confirmation" &&
      !forceDiscard
    ) {
      this.#discardEditorConfirmation = false;
      this.#render();
      return;
    }
    const panel = this.#activePanel();
    const snapshot = this.#remote.view;
    const closingPalette = this.#palette !== null;
    const shouldDismissRemote =
      !closingPalette &&
      this.#remote.phase === "live" &&
      (snapshot?.overlay.active_panel ?? null) !== null;
    if (panel === "system_prompt" && this.#editorDirty && !forceDiscard) {
      this.#discardEditorConfirmation = true;
      this.#render();
      return;
    }
    if (forceDiscard) {
      this.#editorDirty = false;
      const data =
        snapshot === null ? null : panelData(snapshot, "system_prompts");
      this.#editorContent = data?.editor_content ?? "";
      this.#editorName =
        data?.prompts.find((prompt) => prompt.selected)?.label ?? "";
      this.#editorVersion += 1;
    }
    if (panel === "confirmation") {
      this.#confirmable = null;
      this.#confirmableEditorVersion = null;
      this.#confirmationRequestId = null;
    }
    if (panel === "tool_approval") {
      this.#approvalConfirmation = null;
    }
    if (shouldDismissRemote) {
      this.#send({ type: "dismiss_overlay" });
    }
    this.#discardEditorConfirmation = false;
    this.#palette = null;
    this.#forcedPanel = null;
    if (!closingPalette) {
      this.#dismissedOverlay = overlaySignature(this.#remote);
    }
    this.#render();
  }

  #rememberFocus(): void {
    const active = globalThis.document.activeElement;
    this.#focusBeforeOverlay = active instanceof HTMLElement ? active : null;
  }

  #render(): void {
    const snapshot = this.#remote.view;
    const chatBeforePatch = globalThis.document.getElementById("chat-scroll");
    const anchor =
      this.#chatFollow.mode === "manual" &&
      chatBeforePatch instanceof HTMLElement
        ? captureChatAnchor(chatBeforePatch)
        : null;
    this.#prepareChatWindow(snapshot);
    this.#tree = this.#patch(this.#tree, this.#view());
    const fileHost = this.#filesEnabled && this.#files.state.open
      ? globalThis.document.getElementById("files-monaco") : null;
    this.#fileEditor.sync(fileHost instanceof HTMLElement ? fileHost : null, this.#files.state.preview);
    const diffHost = this.#filesEnabled && this.#files.state.open
      ? globalThis.document.getElementById("files-diff-monaco") : null;
    this.#diffViewer.sync(diffHost instanceof HTMLElement ? diffHost : null,
      this.#files.state.diff, this.#files.state.diffLayout);

    const panel = this.#activePanel();
    const panelKey =
      panel === null ? null : panelInstanceKey(panel, this.#remote.view);
    const focusKey =
      panelKey ??
      (snapshot?.debugger.open === true &&
      !this.#debuggerWide &&
      !this.#narrowDebuggerDismissed
        ? "debugger-drawer"
        : null);
    this.#synchronizeOverlayFocus(focusKey);
    this.#renderedPanelKey = focusKey;
    this.#synchronizeChatPosition(snapshot, anchor);
  }

  #prepareChatWindow(snapshot: WebAppSnapshot | null): void {
    if (snapshot !== null && this.#chatFollow.mode === "following") {
      this.#chatStart = Math.max(
        0,
        snapshot.blocks.blocks.length - CHAT_WINDOW_SIZE,
      );
    }
  }

  #synchronizeOverlayFocus(panelKey: string | null): void {
    const transition = overlayFocusTransition(this.#renderedPanelKey, panelKey);
    if (transition === "none") {
      return;
    }
    if (transition === "opened") {
      if (this.#renderedPanelKey === null) {
        this.#rememberFocus();
      }
      globalThis.requestAnimationFrame(() => {
        const traps = Array.from(
          globalThis.document.querySelectorAll<HTMLElement>(
            "[data-focus-trap='true']",
          ),
        );
        const trap = traps.at(-1) ?? null;
        const target =
          trap?.querySelector<HTMLElement>("[data-autofocus='true']") ??
          trap?.querySelector<HTMLElement>(".overlay-header button") ??
          trap?.querySelector<HTMLElement>(
            "button:not([disabled]), input:not([disabled]), textarea:not([disabled]), [tabindex='0']",
          );
        target?.focus();
      });
      return;
    }
    const focus = this.#focusBeforeOverlay;
    globalThis.requestAnimationFrame(() => {
      if (focus?.isConnected === true) {
        focus.focus();
        if (globalThis.document.activeElement === focus) {
          return;
        }
      }
      globalThis.document
        .querySelector<HTMLElement>("#composer-input")
        ?.focus();
    });
  }

  #synchronizeChatPosition(
    snapshot: WebAppSnapshot | null,
    anchor: ChatAnchor | null,
  ): void {
    if (snapshot === null) {
      return;
    }
    const apply = (scrollTop: number | null): number | null =>
      this.#applyChatPosition(scrollTop, anchor);
    if (this.#chatFollow.mode === "following") {
      this.#chatFollow.requestBottomPosition(apply);
      return;
    }
    this.#chatFollow.requestPreservedPosition(apply);
  }

  #applyChatPosition(
    scrollTop: number | null,
    anchor: ChatAnchor | null,
  ): number | null {
    const chat = globalThis.document.getElementById("chat-scroll");
    if (!(chat instanceof HTMLElement)) {
      return null;
    }
    const maximum = Math.max(0, chat.scrollHeight - chat.clientHeight);
    if (scrollTop === null) {
      chat.scrollTop = maximum;
      return chat.scrollTop;
    }
    return restoreChatAnchor(chat, anchor, scrollTop);
  }

  #resumeChatFollowing(): void {
    const snapshot = this.#remote.view;
    if (snapshot === null) {
      return;
    }
    this.#chatStart = Math.max(
      0,
      snapshot.blocks.blocks.length - CHAT_WINDOW_SIZE,
    );
    this.#render();
  }

  #view(): VNode {
    const snapshot = this.#remote.view;
    const live = this.#remote.phase === "live";
    const panel = this.#activePanel();
    const overlay =
      panel === null
        ? null
        : renderPanel(this.#panelViewContext(panel, snapshot, live));
    return renderChatView(this.#chatViewContext(snapshot, live, panel, overlay));
  }

  #chatViewContext(
    snapshot: WebAppSnapshot | null,
    live: boolean,
    activePanel: PanelId | null,
    overlay: VNode | null,
  ): ChatViewContext {
    return {
      state: {
        snapshot,
        live,
        activePanel,
        transportStatus: this.#transportStatus,
        remoteReason: this.#remote.reason,
        pendingCount: this.#transport?.pendingCount ?? 0,
        debuggerWide: this.#debuggerWide,
        debuggerDrawerDismissed: this.#narrowDebuggerDismissed,
        draft: this.#draft,
        chatStart: this.#chatStart,
        toast: this.#toast,
        filesPane: this.#filesEnabled ? renderFilesPane(this.#files.state, live, {
          toggle: () => this.#files.toggle(),
          navigate: (path) => { void this.#files.navigate(path); },
          select: (path) => { void this.#files.select(path); },
          refresh: () => { void this.#files.refresh(); },
          download: (archive) => { void this.#files.download(archive); },
          copyPath: () => { void this.#files.copyPath(); },
          showTab: (tab) => this.#files.showTab(tab),
          refreshChanges: () => { void this.#files.refreshChanges(); },
          selectChange: (path) => { void this.#files.selectChange(path); },
          toggleFolder: (path) => this.#files.toggleFolder(path),
          setDiffLayout: (layout) => this.#files.setDiffLayout(layout),
        }) : null,
        overlay,
      },
      actions: {
        openPalette: () => this.#openPalette(),
        invokeCommand: (command) => this.#invokeCommand(command),
        openPanel: (panel) => {
          this.#forcedPanel = panel;
          this.#render();
        },
        updateDraft: (value) => {
          this.#draft = value.slice(0, MAX_DRAFT_LENGTH);
          this.#draftVersion += 1;
          this.#render();
        },
        submitPrompt: () => this.#submitPrompt(),
        stop: () => this.#sendStop(),
        toggleDebugger: () => this.#toggleDebugger(),
        onChatKeyDown: this.#onChatKeyDown,
        onChatManualIntent: this.#onChatManualIntent,
        onChatScroll: this.#onChatScroll,
      },
    };
  }

  #panelViewContext(
    panel: PanelId,
    snapshot: WebAppSnapshot | null,
    live: boolean,
  ): PanelViewContext {
    return {
      state: {
        panel,
        snapshot,
        live,
        palette: this.#palette,
        editorName: this.#editorName,
        editorContent: this.#editorContent,
        editorDirty: this.#editorDirty,
        sessionName: this.#sessionName,
        discardEditorConfirmation: this.#discardEditorConfirmation,
        approvalConfirmation: this.#approvalConfirmation,
        revision: this.#remote.revision,
      },
      actions: {
        close: (forceDiscard) => this.#closeOverlay(forceDiscard ?? false),
        invokeCommand: (command) => this.#invokeCommand(command),
        send: (command) => {
          this.#send(command);
        },
        sendConfirmable: (command) => {
          this.#sendConfirmable(command);
        },
        setPaletteQuery: (query) => {
          this.#palette = { query, selected: 0 };
          this.#render();
        },
        clampPaletteSelection: (selected) => {
          if (this.#palette !== null) {
            this.#palette.selected = selected;
          }
        },
        onPaletteKeyDown: this.#onPaletteKeyDown,
        selectHistoryEntry: (current, entry) => {
          const request = this.#send({
            type: "select_history_entry",
            session_id: current.session.session_id,
            entry_id: entry.entry_id,
          });
          if (request !== null) {
            this.#historyRecall = {
              requestId: request.id, sessionId: current.session.session_id,
              entryId: entry.entry_id, draftVersion: this.#draftVersion,
            };
          }
        },
        selectSystemPrompt: (current, promptId) => {
          if (this.#editorDirty) {
            this.#showToast("Save or discard the current editor draft first.");
            return;
          }
          this.#send({
            type: "select_system_prompt",
            session_id: current.session.session_id,
            prompt_id: promptId,
          });
        },
        submitSystemPrompt: (current, editorUnavailable) =>
          this.#submitSystemPrompt(current, editorUnavailable),
        updateEditorName: (name) => {
          this.#editorName = name.slice(0, 255);
          this.#editorDirty = true;
          this.#editorVersion += 1;
          this.#render();
        },
        updateEditorContent: (content) => {
          this.#editorContent = content.slice(0, MAX_DRAFT_LENGTH);
          this.#editorDirty = true;
          this.#editorVersion += 1;
          this.#render();
        },
        createSystemPrompt: () => {
          this.#editorName = "";
          this.#editorContent = "";
          this.#editorDirty = true;
          this.#editorVersion += 1;
          this.#render();
        },
        updateSessionName: (name) => {
          this.#sessionName = name.slice(0, 255);
          this.#sessionNameDirty = true;
          this.#render();
        },
        submitSessionName: (current) => {
          const value = this.#sessionName.trim();
          this.#send({
            type: "rename_session",
            session_id: current.session.session_id,
            name: value.length === 0 ? null : value,
          });
        },
        cancelApprovalConfirmation: () => {
          this.#approvalConfirmation = null;
          this.#render();
        },
        decideApproval: (approval, decision) =>
          this.#decideApproval(approval, decision),
        confirmHiddenApproval: (approval, confirmation) =>
          this.#confirmHiddenApproval(approval, confirmation),
        questionDraft: (questionId) => this.#questionDraft(questionId),
        toggleQuestionOption: (prompt, optionId, wasSelected) => {
          const draft = this.#questionDraft(prompt.question_id);
          if (!prompt.multiple) {
            draft.selected.clear();
          }
          if (wasSelected) {
            draft.selected.delete(optionId);
          } else {
            draft.selected.add(optionId);
          }
          this.#render();
        },
        updateQuestionOther: (questionId, value) => {
          this.#questionDraft(questionId).other = value.slice(
            0,
            MAX_DRAFT_LENGTH,
          );
          this.#render();
        },
        answersComplete: (question) => this.#answersComplete(question),
        submitAnswers: (question) => this.#submitAnswers(question),
        cancelQuestion: () => this.#sendStop(),
        cancelEditorDiscard: () => {
          this.#discardEditorConfirmation = false;
          this.#render();
        },
        confirmationMatches: (confirmation) =>
          this.#confirmationMatches(confirmation),
        confirm: (confirmation) => this.#confirm(confirmation),
      },
    };
  }

  #submitSystemPrompt(
    snapshot: WebAppSnapshot,
    editorUnavailable: boolean,
  ): void {
    const name = this.#editorName.trim();
    const problem = systemPromptSaveProblem(
      editorUnavailable,
      name,
      this.#editorContent,
    );
    if (problem !== null) {
      this.#showToast(problem);
      return;
    }
    this.#sendConfirmable(
      {
        type: "save_system_prompt",
        session_id: snapshot.session.session_id,
        name,
        content: this.#editorContent,
        confirmed_overwrite: false,
        confirmation_id: null,
      },
      this.#editorVersion,
    );
  }

  #decideApproval(
    approval: PendingApprovalView,
    decision: ApprovalDecision,
  ): void {
    if (decision === "deny") {
      this.#approvalConfirmation = null;
      this.#send({
        type: "deny_tool",
        session_id: approval.session_id,
        approval_id: approval.approval_id,
        tool_call_id: approval.tool_call_id,
      });
      return;
    }
    if (approvalHasHiddenContent(approval)) {
      this.#approvalConfirmation = {
        key: approvalConfirmationKey(this.#remote.revision, approval),
        decision,
      };
      this.#render();
      globalThis.requestAnimationFrame(() => {
        globalThis.document
          .querySelector<HTMLElement>(
            ".approval-confirmation [data-autofocus='true']",
          )
          ?.focus();
      });
      return;
    }
    this.#sendAffirmativeApproval(approval, decision, false);
  }

  #confirmHiddenApproval(
    approval: PendingApprovalView,
    confirmation: ApprovalConfirmation,
  ): void {
    if (
      confirmation.key !==
        approvalConfirmationKey(this.#remote.revision, approval) ||
      !approvalHasHiddenContent(approval) ||
      !approval.allowed_decisions.includes(confirmation.decision)
    ) {
      this.#approvalConfirmation = null;
      this.#showToast("The approval changed; review the current preview again.");
      return;
    }
    this.#sendAffirmativeApproval(approval, confirmation.decision, true);
  }

  #sendAffirmativeApproval(
    approval: PendingApprovalView,
    decision: AffirmativeApprovalDecision,
    acknowledgeHiddenContent: boolean,
  ): void {
    this.#approvalConfirmation = null;
    switch (decision) {
      case "approve_once":
        this.#send({
          type: "approve_tool_once",
          session_id: approval.session_id,
          approval_id: approval.approval_id,
          tool_call_id: approval.tool_call_id,
          acknowledge_hidden_content: acknowledgeHiddenContent,
        });
        return;
      case "approve_always":
        this.#send({
          type: "approve_tool_always",
          session_id: approval.session_id,
          approval_id: approval.approval_id,
          tool_call_id: approval.tool_call_id,
          acknowledge_hidden_content: acknowledgeHiddenContent,
        });
        return;
      default:
        assertNever(decision, "affirmative tool decision");
    }
  }

  #questionDraft(questionId: string): QuestionDraft {
    return getQuestionDraft(this.#questionDrafts, questionId);
  }

  #answersComplete(question: PendingQuestionView): boolean {
    return questionAnswersComplete(question, this.#questionDrafts);
  }

  #submitAnswers(question: PendingQuestionView): void {
    if (question.content_truncated) {
      this.#showToast(
        "Remote answering requires an exact, unredacted question projection.",
      );
      return;
    }
    if (!this.#answersComplete(question)) {
      return;
    }
    const answers = questionAnswers(question, this.#questionDrafts);
    if (!questionAnswersFit(answers)) {
      this.#showToast("Answers exceed the 64 KiB UTF-8 limit.");
      return;
    }
    this.#send({
      type: "answer_user",
      session_id: question.session_id,
      tool_call_id: question.tool_call_id,
      form_id: question.form_id,
      answers,
    });
  }

  #confirmationMatches(confirmation: ConfirmationView): boolean {
    return retainedConfirmationMatches(this.#confirmable, confirmation);
  }

  #confirm(confirmation: ConfirmationView): void {
    const command = this.#confirmable;
    if (command === null || !this.#confirmationMatches(confirmation)) {
      return;
    }
    const editorVersion = this.#confirmableEditorVersion;
    const request = this.#send(
      confirmedCommand(command, confirmation.confirmation_id),
    );
    if (
      request !== null &&
      command.type === "save_system_prompt" &&
      editorVersion !== null
    ) {
      this.#editorRequestVersions.set(request.id, editorVersion);
    }
    this.#confirmable = null;
    this.#confirmableEditorVersion = null;
    this.#confirmationRequestId = null;
  }

  #readChatMetrics(): ChatScrollMetrics | null {
    const chat = globalThis.document.getElementById("chat-scroll");
    return chat instanceof HTMLElement ? chatScrollMetrics(chat) : null;
  }

  readonly #onChatManualIntent = (event: Event): void => {
    const target = event.currentTarget;
    if (!(target instanceof HTMLElement)) {
      return;
    }
    this.#chatFollow.noteUserIntent(
      chatScrollMetrics(target),
      () => this.#readChatMetrics(),
    );
  };

  readonly #onChatKeyDown = (event: KeyboardEvent): void => {
    if (isChatScrollKey(event)) {
      this.#onChatManualIntent(event);
    }
  };

  readonly #onChatScroll = (event: Event): void => {
    const target = event.currentTarget;
    const snapshot = this.#remote.view;
    if (!(target instanceof HTMLElement) || snapshot === null) {
      return;
    }
    const metrics = chatScrollMetrics(target);
    this.#chatFollow.observeScroll(metrics);
    const next = chatWindowStartForScroll(
      snapshot.blocks.blocks.length,
      metrics.scrollTop,
      this.#chatFollow.mode === "following",
    );
    if (next !== this.#chatStart) {
      this.#chatStart = next;
      this.#render();
    }
  };

  #keyboardContext(): KeyboardContext {
    return {
      state: {
        snapshot: this.#remote.view,
        palette: this.#palette,
        activePanel: this.#activePanel(),
        debuggerDrawerOpen:
          this.#remote.view?.debugger.open === true &&
          !this.#debuggerWide &&
          !this.#narrowDebuggerDismissed,
      },
      actions: {
        openPalette: () => this.#openPalette(),
        closeOverlay: () => this.#closeOverlay(),
        stop: () => this.#sendStop(),
        armStop: () => this.#showToast("Press Esc again to stop."),
        toggleDebugger: () => this.#toggleDebugger(),
        requestQuit: () => {
          this.#sendConfirmable({
            type: "quit",
            confirmed: false,
            confirmation_id: null,
          });
        },
        setPaletteSelection: (selected) => {
          if (this.#palette === null) {
            return;
          }
          this.#palette.selected = selected;
          this.#render();
        },
        invokeCommand: (command) => this.#invokeCommand(command),
      },
    };
  }

  readonly #onPaletteKeyDown = (event: KeyboardEvent): void => {
    handlePaletteKeyDown(event, this.#keyboardContext());
  };

  readonly #onGlobalKeyDown = (event: KeyboardEvent): void => {
    handleGlobalKeyDown(event, this.#keyboardContext());
  };
}
