import "./support/app-globals.js";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { adjustedAnchorScrollTop } from "../src/app/chat-anchor.js";
import { chatBlockAnchor, renderChatView } from "../src/app/chat-view.js";
import { renderDebugger } from "../src/app/debugger.js";
import { rebaseChatWindowStart } from "../src/app/helpers.js";
import { handleGlobalKeyDown } from "../src/app/keyboard.js";
import { renderPanel } from "../src/app/panels.js";
import type {
  ChatViewActions,
  ChatViewState,
  KeyboardContext,
  KeyboardState,
  PanelActions,
} from "../src/app/state.js";
import {
  applicationStatusFields,
  pythonStatusLabel,
  transportStatusFields,
  usageBreakdown,
} from "../src/app/status.js";
import type {
  AccountingTotalsView,
  ProjectionLossView,
  RenderBlockView,
  UsageView,
  WebAppSnapshot,
} from "../src/generated/contracts.js";
import { noopChatViewActions } from "./support/app-fixtures.js";
import {
  asKeyboardEvent,
  fakeKeyboardEvent,
  type Mutable,
} from "./support/dom-fakes.js";
import {
  attr,
  classAttr,
  find,
  fire,
  listener,
  propValue,
  vnodeText,
  visit,
} from "./support/vnode.js";

const SESSION_ID = "11111111-2222-4333-8444-555555555555";

/** A snapshot carrying an extra unsafe field that must never be rendered. */
type SnapshotFixture = WebAppSnapshot & { current_dir: string };

function loss(overrides: Partial<ProjectionLossView> = {}): ProjectionLossView {
  return {
    filtered: false,
    redacted: false,
    truncation: null,
    ...overrides,
  };
}

function usage(overrides: Partial<UsageView> = {}): UsageView {
  return {
    uncached_input_tokens: "11",
    cache_read_input_tokens: "22",
    cache_creation_input_tokens: "33",
    output_tokens: "44",
    total_input_tokens: "66",
    total_tokens: "110",
    breakdown_complete: true,
    ...overrides,
  };
}

function accounting(display: string, nanos: string): AccountingTotalsView {
  return {
    usage: usage(),
    estimated_cost: {
      display,
      currency: "USD",
      nanos,
      incomplete: false,
      mixed_pricing: false,
      long_context_applied: false,
      pricing_effective_as_of: "2026-01-01",
      pricing_valid_through: null,
      provenance_kind: "fixture",
    },
    request_count: "1",
    long_context_request_count: "0",
    unpriced_request_count: "0",
    incomplete_usage_request_count: "0",
  };
}

function textBlock(): RenderBlockView {
  return {
    kind: "text",
    content: "",
    content_loss: loss(),
    tool: null,
    title: null,
    title_loss: loss(),
    success: null,
    usage: null,
    estimated_cost: null,
  };
}

function snapshotFixture(): SnapshotFixture {
  return {
    session: {
      session_id: SESSION_ID,
      display_name: "Status fixture",
      fallback_label: "Session",
    },
    blocks: { blocks: [], omitted_before: 7, truncated: true },
    activity: {
      kind: "idle",
      fully_idle: true,
      cancellable: false,
      cancel_id: null,
      progress_percent: null,
    },
    pending_approval: null,
    pending_question: null,
    commands: [
      {
        id: "hotkeys",
        label: "Hotkeys",
        icon: "command",
        enabled: true,
        disabled_reason: null,
        behavior: "open_panel",
        accelerator: "h",
        description: "test command",
      },
      {
        id: "themes",
        label: "Themes",
        icon: "command",
        enabled: true,
        disabled_reason: null,
        behavior: "open_panel",
        accelerator: "t",
        description: "test command",
      },
      {
        id: "clear-ui",
        label: "Clear UI",
        icon: "command",
        enabled: true,
        disabled_reason: null,
        behavior: "execute",
        accelerator: "c",
        description: "test command",
      },
      {
        id: "toggle-debugger",
        label: "Debugger",
        icon: "debug",
        enabled: true,
        disabled_reason: null,
        behavior: "execute",
        accelerator: "d",
        description: "test command",
      },
    ],
    sessions: { sessions: [], has_more: false },
    models: [],
    themes: [],
    usage: {
      latest_turn: accounting("$0.000012*", "RAW_TURN_NANOS"),
      session: accounting("$0.000034†", "RAW_SESSION_NANOS"),
    },
    status: {
      stop_reason: "Ready after safe projection",
      stop_reason_loss: loss({ redacted: true }),
      model_label: "safe-model",
      provider_label: "safe-provider",
      provider_transport: "claude_code_proxy",
      python: {
        profile: "python_only",
        target: "sandbox",
        backend: "podman",
        network: "public_only",
        workspace_access: "read_write",
        grant_count: 0,
        policy_source: "one_time",
        container: {
          kind: "retained",
          name: "lethetic-python-550e8400-e29b-41d4-a716-446655440000",
          active: false,
        },
      },
      tokens_per_second: "12.34",
      prompt_tokens_per_second: "5.67",
      context_tokens: "123456",
      context_limit_tokens: "1000000",
      context_source: "server_usage",
      request_usage: usage({ breakdown_complete: false }),
      memory_mebibytes: "321",
      file_count: 9,
      visible_block_count: 17,
      git_state: "dirty",
    },
    debugger: {
      open: true,
      summary: "Actor ready · idle · browser mirror ready",
      entries: [
        {
          code: "connection_interrupted",
          severity: "warning",
          message: "Hostile <img src=x onerror=alert(1)> remains text.",
        },
        {
          code: "unknown",
          severity: "info",
          message: "Application actor is ready.",
        },
      ],
      omitted_before: 3,
    },
    overlay: { active_panel: null, data: null },
    current_dir: "UNSAFE_PATH_SENTINEL",
  };
}

function transportState(snapshot: WebAppSnapshot = snapshotFixture()): Mutable<ChatViewState> {
  return {
    snapshot,
    live: true,
    activePanel: null,
    transportStatus: {
      phase: "live",
      label: "Connected",
      attempt: 2,
      retryInMilliseconds: null,
    },
    remoteReason: null,
    pendingCount: 3,
    debuggerWide: true,
    debuggerDrawerDismissed: false,
    draft: "",
    chatStart: 0,
    toast: null,
    filesPane: null,
    overlay: null,
  };
}

interface ChatFixture {
  readonly state: Mutable<ChatViewState>;
  readonly actions: ChatViewActions;
  readonly toggles: string[];
}

function chatContext(debuggerWide: boolean): ChatFixture {
  const toggles: string[] = [];
  return {
    state: { ...transportState(), debuggerWide },
    actions: noopChatViewActions({
      toggleDebugger: () => toggles.push("toggle"),
    }),
    toggles,
  };
}

test("application status exposes every safe field and only formatted costs", () => {
  const snapshot = snapshotFixture();
  const fields = applicationStatusFields(snapshot);
  const rendered = JSON.stringify(fields);
  for (const expected of [
    "Ready after safe projection",
    "safe-model",
    "safe-provider",
    "Claude Code proxy",
    "Python Podman",
    "network public-only",
    "lethetic-python-550e8400-e29b-41d4-a716-446655440000",
    "inactive",
    "123456/1000000",
    "server usage",
    "uncached 11",
    "cache-read 22",
    "cache-write 33",
    "output 44",
    "$0.000012*",
    "$0.000034†",
    "12.34 tok/s",
    "321 MiB",
    "Files",
    "Blocks",
    "dirty",
    "WFE replaced sensitive values before browser delivery.",
  ]) {
    assert.match(rendered, new RegExp(expected.replaceAll("$", "\\$"), "u"));
  }
  for (const forbidden of [
    "RAW_TURN_NANOS",
    "RAW_SESSION_NANOS",
    "UNSAFE_PATH_SENTINEL",
  ]) {
    assert.doesNotMatch(rendered, new RegExp(forbidden, "u"));
  }
  const requestUsage = snapshot.status.request_usage;
  assert.ok(requestUsage !== null);
  assert.equal(usageBreakdown(requestUsage).endsWith("*"), true);
  assert.match(pythonStatusLabel(snapshot.status.python), /0 grant\(s\)/u);
  const cliLocked = structuredClone(snapshot.status.python);
  cliLocked.policy_source = "cli_locked";
  assert.match(pythonStatusLabel(cliLocked), /cli-locked/u);
});

test("transport row reports retry pending and mirror state separately", () => {
  const state = transportState();
  state.live = false;
  state.remoteReason = "Waiting for exact snapshot";
  state.transportStatus = {
    phase: "reconnecting",
    label: "Connection interrupted",
    attempt: 4,
    retryInMilliseconds: 2_400,
  };
  const rendered = JSON.stringify(transportStatusFields(state));
  assert.match(rendered, /reconnecting: Connection interrupted/u);
  assert.match(rendered, /not synchronized/u);
  assert.match(rendered, /3s \(attempt 4\)/u);
  assert.match(rendered, /"Pending","value":"3"/u);
  assert.match(rendered, /Waiting for exact snapshot/u);
  assert.match(rendered, /server-bounded/u);
});

test("debugger renders as a wide aside or narrow focus-trapped drawer", () => {
  const wideContext = chatContext(true);
  const wide = renderDebugger(wideContext);
  assert.ok(wide);
  assert.equal(wide.sel, "aside");
  assert.equal(attr(wide, "class"), "debugger-pane");
  assert.equal(attr(wide, "role"), undefined);
  assert.match(vnodeText(wide), /3 older diagnostic event\(s\)/u);
  assert.match(vnodeText(wide), /Hostile <img src=x onerror=alert\(1\)> remains text/u);
  visit(wide, (vnode) => assert.equal(propValue(vnode, "innerHTML"), undefined));

  const narrowContext = chatContext(false);
  const narrow = renderDebugger(narrowContext);
  assert.ok(narrow);
  assert.equal(attr(narrow, "class"), "debugger-drawer-layer");
  const drawer = find(
    narrow,
    (vnode) => attr(vnode, "class") === "debugger-drawer",
  );
  assert.ok(drawer);
  assert.equal(attr(drawer, "role"), "dialog");
  assert.equal(attr(drawer, "aria-modal"), "true");
  assert.equal(attr(drawer, "data-focus-trap"), "true");
  const close = find(
    narrow,
    (vnode) => classAttr(vnode).includes("debugger-close"),
  );
  assert.ok(close);
  assert.equal(attr(close, "data-autofocus"), "true");
  fire(close, "click");
  assert.deepEqual(narrowContext.toggles, ["toggle"]);

  const coveredContext = chatContext(false);
  coveredContext.state.activePanel = "tool_approval";
  const covered = renderDebugger(coveredContext);
  assert.ok(covered);
  const coveredDrawer = find(
    covered,
    (vnode) => attr(vnode, "class") === "debugger-drawer",
  );
  assert.ok(coveredDrawer);
  assert.match(classAttr(covered), /is-covered/u);
  assert.equal(attr(covered, "aria-hidden"), "true");
  assert.equal(attr(covered, "inert"), "");
  assert.equal(attr(coveredDrawer, "aria-modal"), undefined);
  assert.equal(attr(coveredDrawer, "data-focus-trap"), undefined);

  const dismissedContext = chatContext(false);
  dismissedContext.state.debuggerDrawerDismissed = true;
  assert.equal(renderDebugger(dismissedContext), null);
});

test("root layout spans status rows and keeps the debugger outside overlays", () => {
  const context = chatContext(true);
  const view = renderChatView(context);
  assert.match(classAttr(view), /debugger-wide/u);
  assert.match(classAttr(view), /debugger-open/u);
  const workspace = find(
    view,
    (vnode) => classAttr(vnode).startsWith("workspace "),
  );
  assert.ok(workspace);
  assert.ok(
    find(
      workspace,
      (vnode) => attr(vnode, "class") === "conversation-column",
    ),
  );
  assert.ok(
    find(workspace, (vnode) => attr(vnode, "class") === "debugger-pane"),
  );
  assert.ok(
    find(view, (vnode) => attr(vnode, "class") === "status-level"),
  );
  assert.ok(
    find(
      view,
      (vnode) => attr(vnode, "class") === "footer-level transport-level",
    ),
  );

  const dismissed = chatContext(false);
  dismissed.state.debuggerDrawerDismissed = true;
  const dismissedView = renderChatView(dismissed);
  assert.doesNotMatch(classAttr(dismissedView), /debugger-open/u);
  assert.equal(attr(dismissedView, "data-debugger"), "closed");
  assert.equal(
    find(
      dismissedView,
      (vnode) => attr(vnode, "class") === "debugger-drawer",
    ),
    null,
  );
});

test("chat anchors remain stable across server truncation and reflow", () => {
  const snapshot = snapshotFixture();
  assert.equal(chatBlockAnchor(snapshot, 0), null);
  assert.equal(chatBlockAnchor(snapshot, 1), "block-7");
  assert.equal(chatBlockAnchor(snapshot, 4), "block-10");
  assert.equal(adjustedAnchorScrollTop(400, 1_000, 80, 130), 450);
  assert.equal(adjustedAnchorScrollTop(20, 1_000, 80, 10), 0);
  assert.equal(adjustedAnchorScrollTop(980, 1_000, 20, 90), 1_000);

  const previous = snapshotFixture();
  previous.blocks = {
    blocks: Array.from({ length: 240 }, textBlock),
    omitted_before: 20,
    truncated: true,
  };
  const advanced = snapshotFixture();
  advanced.blocks = {
    blocks: Array.from({ length: 210 }, textBlock),
    omitted_before: 50,
    truncated: true,
  };
  assert.equal(rebaseChatWindowStart(80, previous, advanced), 50);

  const recovered = snapshotFixture();
  recovered.blocks = {
    blocks: Array.from({ length: 300 }, textBlock),
    omitted_before: 10,
    truncated: false,
  };
  assert.equal(rebaseChatWindowStart(80, previous, recovered), 89);
  assert.equal(rebaseChatWindowStart(239, previous, advanced), 209);
});

interface KeyboardFixture {
  readonly context: KeyboardContext;
  readonly calls: string[];
}

function keyboardContext(overrides: Partial<KeyboardState> = {}): KeyboardFixture {
  const calls: string[] = [];
  return {
    context: {
      state: {
        snapshot: snapshotFixture(),
        palette: null,
        activePanel: null,
        debuggerDrawerOpen: false,
        ...overrides,
      },
      actions: {
        openPalette: () => calls.push("palette"),
        closeOverlay: () => calls.push("overlay"),
        stop: () => calls.push("stop"),
        armStop: () => calls.push("arm-stop"),
        toggleDebugger: () => calls.push("debugger"),
        requestQuit: () => calls.push("quit"),
        setPaletteSelection: () => {},
        invokeCommand: () => calls.push("command"),
      },
    },
    calls,
  };
}

test("Escape prioritizes protected overlays then narrow debugger then Stop", () => {
  let fixture = keyboardContext({
    activePanel: "tool_approval",
    debuggerDrawerOpen: true,
  });
  handleGlobalKeyDown(asKeyboardEvent(fakeKeyboardEvent("Escape")), fixture.context);
  assert.deepEqual(fixture.calls, ["overlay"]);

  fixture = keyboardContext({ debuggerDrawerOpen: true });
  handleGlobalKeyDown(asKeyboardEvent(fakeKeyboardEvent("Escape")), fixture.context);
  assert.deepEqual(fixture.calls, ["debugger"]);

  const snapshot = snapshotFixture();
  snapshot.activity.cancellable = true;
  fixture = keyboardContext({ snapshot });
  handleGlobalKeyDown(asKeyboardEvent(fakeKeyboardEvent("Escape")), fixture.context);
  assert.deepEqual(fixture.calls, ["arm-stop"]);
  handleGlobalKeyDown(asKeyboardEvent(fakeKeyboardEvent("Escape")), fixture.context);
  assert.deepEqual(fixture.calls, ["arm-stop", "stop"]);

  fixture = keyboardContext();
  handleGlobalKeyDown(asKeyboardEvent(fakeKeyboardEvent("d")), fixture.context);
  assert.deepEqual(fixture.calls, []);
});

interface FakeFocusable {
  readonly name: string;
  readonly offsetParent: object;
  focus(): void;
}

interface FakeFocusTrap {
  querySelectorAll(): FakeFocusable[];
  contains(element: unknown): boolean;
}

interface FakeDocument {
  activeElement: FakeFocusable;
  querySelectorAll(): FakeFocusTrap[];
}

test("Tab remains inside the topmost focus trap", () => {
  const originalDocument = Object.getOwnPropertyDescriptor(globalThis, "document");
  const focused: string[] = [];
  const makeFocusable = (name: string): FakeFocusable => ({
    name,
    offsetParent: {},
    focus() {
      focused.push(name);
      fakeDocument.activeElement = this;
    },
  });
  const underlyingFirst = makeFocusable("underlying-first");
  const underlyingLast = makeFocusable("underlying-last");
  const topFirst = makeFocusable("top-first");
  const topLast = makeFocusable("top-last");
  const underlyingElements = new Set<unknown>([underlyingFirst, underlyingLast]);
  const topElements = new Set<unknown>([topFirst, topLast]);
  const underlying: FakeFocusTrap = {
    querySelectorAll: () => [underlyingFirst, underlyingLast],
    contains: (element) => underlyingElements.has(element),
  };
  const top: FakeFocusTrap = {
    querySelectorAll: () => [topFirst, topLast],
    contains: (element) => topElements.has(element),
  };
  const fakeDocument: FakeDocument = {
    activeElement: underlyingFirst,
    querySelectorAll: () => [underlying, top],
  };
  // Only the members used by the focus trap exist; Node has no Document.
  Reflect.set(globalThis, "document", fakeDocument);
  try {
    const fixture = keyboardContext({
      activePanel: "tool_approval",
      debuggerDrawerOpen: true,
    });
    const enterTop = fakeKeyboardEvent("Tab");
    handleGlobalKeyDown(asKeyboardEvent(enterTop), fixture.context);
    assert.equal(enterTop.defaultPrevented, true);
    assert.deepEqual(focused, ["top-first"]);

    fakeDocument.activeElement = topLast;
    const wrapForward = fakeKeyboardEvent("Tab");
    handleGlobalKeyDown(asKeyboardEvent(wrapForward), fixture.context);
    assert.equal(wrapForward.defaultPrevented, true);
    assert.deepEqual(focused, ["top-first", "top-first"]);

    fakeDocument.activeElement = topFirst;
    const wrapBackward = fakeKeyboardEvent("Tab");
    wrapBackward.shiftKey = true;
    handleGlobalKeyDown(asKeyboardEvent(wrapBackward), fixture.context);
    assert.equal(wrapBackward.defaultPrevented, true);
    assert.deepEqual(focused, ["top-first", "top-first", "top-last"]);
  } finally {
    if (originalDocument === undefined) {
      Reflect.deleteProperty(globalThis, "document");
    } else {
      Object.defineProperty(globalThis, "document", originalDocument);
    }
  }
});

function noopPanelActions(overrides: Partial<PanelActions> = {}): PanelActions {
  return {
    close: () => {},
    invokeCommand: () => {},
    send: () => {},
    sendConfirmable: () => {},
    setPaletteQuery: () => {},
    clampPaletteSelection: () => {},
    onPaletteKeyDown: () => {},
    selectHistoryEntry: () => {},
    selectSystemPrompt: () => {},
    submitSystemPrompt: () => {},
    updateEditorName: () => {},
    updateEditorContent: () => {},
    createSystemPrompt: () => {},
    updateSessionName: () => {},
    submitSessionName: () => {},
    cancelApprovalConfirmation: () => {},
    decideApproval: () => {},
    confirmHiddenApproval: () => {},
    questionDraft: () => ({ selected: new Set<string>(), other: "" }),
    toggleQuestionOption: () => {},
    updateQuestionOther: () => {},
    answersComplete: () => false,
    submitAnswers: () => {},
    cancelQuestion: () => {},
    cancelEditorDiscard: () => {},
    confirmationMatches: () => false,
    confirm: () => {},
    ...overrides,
  };
}

test("palette autofocus belongs to the noneditable command surface", () => {
  const snapshot = snapshotFixture();
  const panel = renderPanel({
    state: {
      panel: "command_palette",
      snapshot,
      live: true,
      palette: { query: "", selected: 0 },
      editorName: "",
      editorContent: "",
      editorDirty: false,
      sessionName: "",
      discardEditorConfirmation: false,
      approvalConfirmation: null,
      revision: 0,
    },
    actions: noopPanelActions(),
  });
  const search = find(panel, (vnode) => attr(vnode, "id") === "palette-search");
  const surface = find(panel, (vnode) => attr(vnode, "id") === "palette-list");
  assert.ok(search);
  assert.ok(surface);
  assert.equal(attr(search, "data-autofocus"), undefined);
  assert.equal(listener(search, "keydown"), undefined);
  assert.equal(attr(surface, "data-autofocus"), "true");
  assert.equal(attr(surface, "tabindex"), "0");
  assert.equal(typeof listener(surface, "keydown"), "function");
  assert.ok(
    (surface.children ?? []).every(
      (option) => typeof option === "object" && option.sel === "li",
    ),
  );
});

test("CSS keeps four viewport rows, complete docks, and independent debugger scroll", async () => {
  // Tests run from <compiled root>/tests/, beside the copied distribution assets.
  const styles = await readFile(new URL("../styles.css", import.meta.url), "utf8");
  assert.match(
    styles,
    /\.app-shell\s*\{[^}]*grid-template-rows: auto minmax\(0, 1fr\) auto auto;/su,
  );
  assert.match(
    styles,
    /\.workspace\.debugger-wide\.has-debugger\s*\{[^}]*grid-template-columns:/su,
  );
  assert.match(styles, /\.conversation-column\s*\{[^}]*min-height: 0;/su);
  assert.match(styles, /\.debugger-entries\s*\{[^}]*overflow: auto;/su);
  assert.match(styles, /\.chat-scroll\s*\{[^}]*overflow-anchor: none;/su);
  assert.doesNotMatch(styles, /\.status-level span:nth-child/u);
  assert.doesNotMatch(styles, /\.footer-level span:nth-child/u);
});
