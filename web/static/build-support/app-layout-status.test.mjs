import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

const stage = process.argv[2];
if (stage === undefined) {
  throw new Error("usage: app-layout-status.test.mjs <compiled-web-root>");
}

globalThis.addEventListener = () => {};
globalThis.window = globalThis;

const moduleUrl = (path) => pathToFileURL(resolve(stage, path)).href;
const { adjustedAnchorScrollTop } = await import(
  moduleUrl("src/app/chat-anchor.js")
);
const { chatBlockAnchor, renderChatView } = await import(
  moduleUrl("src/app/chat-view.js")
);
const { renderDebugger } = await import(moduleUrl("src/app/debugger.js"));
const { rebaseChatWindowStart } = await import(
  moduleUrl("src/app/helpers.js")
);
const { handleGlobalKeyDown } = await import(
  moduleUrl("src/app/keyboard.js")
);
const { renderPanel } = await import(moduleUrl("src/app/panels.js"));
const {
  applicationStatusFields,
  pythonStatusLabel,
  transportStatusFields,
  usageBreakdown,
} = await import(moduleUrl("src/app/status.js"));

const SESSION_ID = "11111111-2222-4333-8444-555555555555";

function loss(overrides = {}) {
  return {
    filtered: false,
    redacted: false,
    truncation: null,
    ...overrides,
  };
}

function usage(overrides = {}) {
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

function accounting(display, nanos) {
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

function snapshotFixture() {
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

function transportState(snapshot = snapshotFixture()) {
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
    overlay: null,
  };
}

function vnodeText(vnode) {
  if (typeof vnode === "string") {
    return vnode;
  }
  if (typeof vnode?.text === "string") {
    return vnode.text;
  }
  return Array.isArray(vnode?.children)
    ? vnode.children.map(vnodeText).join("")
    : "";
}

function walk(vnode, visit) {
  if (vnode === null || typeof vnode !== "object") {
    return;
  }
  visit(vnode);
  for (const child of vnode.children ?? []) {
    walk(child, visit);
  }
}

function findVNode(vnode, predicate) {
  let found = null;
  walk(vnode, (candidate) => {
    if (found === null && predicate(candidate)) {
      found = candidate;
    }
  });
  return found;
}

function chatContext(debuggerWide) {
  const toggles = [];
  return {
    state: { ...transportState(), debuggerWide },
    actions: {
      openPalette: () => {},
      invokeCommand: () => {},
      openPanel: () => {},
      updateDraft: () => {},
      submitPrompt: () => {},
      stop: () => {},
      toggleDebugger: () => toggles.push("toggle"),
      onChatKeyDown: () => {},
      onChatManualIntent: () => {},
      onChatScroll: () => {},
    },
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
  assert.equal(
    usageBreakdown(snapshot.status.request_usage).endsWith("*"),
    true,
  );
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
    phase: "retrying",
    label: "Connection interrupted",
    attempt: 4,
    retryInMilliseconds: 2_400,
  };
  const rendered = JSON.stringify(transportStatusFields(state));
  assert.match(rendered, /retrying: Connection interrupted/u);
  assert.match(rendered, /not synchronized/u);
  assert.match(rendered, /3s \(attempt 4\)/u);
  assert.match(rendered, /"Pending","value":"3"/u);
  assert.match(rendered, /Waiting for exact snapshot/u);
  assert.match(rendered, /server-bounded/u);
});

test("debugger renders as a wide aside or narrow focus-trapped drawer", () => {
  const wideContext = chatContext(true);
  const wide = renderDebugger(wideContext);
  assert.equal(wide.sel, "aside");
  assert.equal(wide.data.attrs.class, "debugger-pane");
  assert.equal(wide.data.attrs.role, undefined);
  assert.match(vnodeText(wide), /3 older diagnostic event\(s\)/u);
  assert.match(vnodeText(wide), /Hostile <img src=x onerror=alert\(1\)> remains text/u);
  walk(wide, (vnode) => assert.equal(vnode.data?.props?.innerHTML, undefined));

  const narrowContext = chatContext(false);
  const narrow = renderDebugger(narrowContext);
  assert.equal(narrow.data.attrs.class, "debugger-drawer-layer");
  const drawer = findVNode(
    narrow,
    (vnode) => vnode.data?.attrs?.class === "debugger-drawer",
  );
  assert.equal(drawer.data.attrs.role, "dialog");
  assert.equal(drawer.data.attrs["aria-modal"], "true");
  assert.equal(drawer.data.attrs["data-focus-trap"], "true");
  const close = findVNode(
    narrow,
    (vnode) => vnode.data?.attrs?.class?.includes("debugger-close") === true,
  );
  assert.equal(close.data.attrs["data-autofocus"], "true");
  close.data.on.click();
  assert.deepEqual(narrowContext.toggles, ["toggle"]);

  const coveredContext = chatContext(false);
  coveredContext.state.activePanel = "tool_approval";
  const covered = renderDebugger(coveredContext);
  const coveredDrawer = findVNode(
    covered,
    (vnode) => vnode.data?.attrs?.class === "debugger-drawer",
  );
  assert.match(covered.data.attrs.class, /is-covered/u);
  assert.equal(covered.data.attrs["aria-hidden"], "true");
  assert.equal(covered.data.attrs.inert, "");
  assert.equal(coveredDrawer.data.attrs["aria-modal"], undefined);
  assert.equal(coveredDrawer.data.attrs["data-focus-trap"], undefined);

  const dismissedContext = chatContext(false);
  dismissedContext.state.debuggerDrawerDismissed = true;
  assert.equal(renderDebugger(dismissedContext), null);
});

test("root layout spans status rows and keeps the debugger outside overlays", () => {
  const context = chatContext(true);
  const view = renderChatView(context);
  assert.match(view.data.attrs.class, /debugger-wide/u);
  assert.match(view.data.attrs.class, /debugger-open/u);
  const workspace = findVNode(
    view,
    (vnode) => vnode.data?.attrs?.class?.startsWith("workspace ") === true,
  );
  assert.ok(workspace);
  assert.ok(
    findVNode(
      workspace,
      (vnode) => vnode.data?.attrs?.class === "conversation-column",
    ),
  );
  assert.ok(
    findVNode(workspace, (vnode) => vnode.data?.attrs?.class === "debugger-pane"),
  );
  assert.ok(
    findVNode(view, (vnode) => vnode.data?.attrs?.class === "status-level"),
  );
  assert.ok(
    findVNode(
      view,
      (vnode) => vnode.data?.attrs?.class === "footer-level transport-level",
    ),
  );

  const dismissed = chatContext(false);
  dismissed.state.debuggerDrawerDismissed = true;
  const dismissedView = renderChatView(dismissed);
  assert.doesNotMatch(dismissedView.data.attrs.class, /debugger-open/u);
  assert.equal(dismissedView.data.attrs["data-debugger"], "closed");
  assert.equal(
    findVNode(
      dismissedView,
      (vnode) => vnode.data?.attrs?.class === "debugger-drawer",
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
    blocks: Array.from({ length: 240 }, () => ({})),
    omitted_before: 20,
    truncated: true,
  };
  const advanced = snapshotFixture();
  advanced.blocks = {
    blocks: Array.from({ length: 210 }, () => ({})),
    omitted_before: 50,
    truncated: true,
  };
  assert.equal(rebaseChatWindowStart(80, previous, advanced), 50);

  const recovered = snapshotFixture();
  recovered.blocks = {
    blocks: Array.from({ length: 300 }, () => ({})),
    omitted_before: 10,
    truncated: false,
  };
  assert.equal(rebaseChatWindowStart(80, previous, recovered), 89);
  assert.equal(rebaseChatWindowStart(239, previous, advanced), 209);
});

function keyboardEvent(key) {
  return {
    key,
    ctrlKey: false,
    altKey: false,
    shiftKey: false,
    metaKey: false,
    target: {},
    preventDefault() {
      this.defaultPrevented = true;
    },
    defaultPrevented: false,
  };
}

function keyboardContext(overrides = {}) {
  const calls = [];
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
  handleGlobalKeyDown(keyboardEvent("Escape"), fixture.context);
  assert.deepEqual(fixture.calls, ["overlay"]);

  fixture = keyboardContext({ debuggerDrawerOpen: true });
  handleGlobalKeyDown(keyboardEvent("Escape"), fixture.context);
  assert.deepEqual(fixture.calls, ["debugger"]);

  const snapshot = snapshotFixture();
  snapshot.activity.cancellable = true;
  fixture = keyboardContext({ snapshot });
  handleGlobalKeyDown(keyboardEvent("Escape"), fixture.context);
  assert.deepEqual(fixture.calls, ["stop"]);

  fixture = keyboardContext();
  handleGlobalKeyDown(keyboardEvent("d"), fixture.context);
  assert.deepEqual(fixture.calls, []);
});

test("Tab remains inside the topmost focus trap", () => {
  const originalDocument = globalThis.document;
  const focused = [];
  const makeFocusable = (name) => ({
    name,
    offsetParent: {},
    focus() {
      focused.push(name);
      globalThis.document.activeElement = this;
    },
  });
  const underlyingFirst = makeFocusable("underlying-first");
  const underlyingLast = makeFocusable("underlying-last");
  const topFirst = makeFocusable("top-first");
  const topLast = makeFocusable("top-last");
  const underlyingElements = new Set([underlyingFirst, underlyingLast]);
  const topElements = new Set([topFirst, topLast]);
  const underlying = {
    querySelectorAll: () => [underlyingFirst, underlyingLast],
    contains: (element) => underlyingElements.has(element),
  };
  const top = {
    querySelectorAll: () => [topFirst, topLast],
    contains: (element) => topElements.has(element),
  };
  globalThis.document = {
    activeElement: underlyingFirst,
    querySelectorAll: () => [underlying, top],
  };
  try {
    const fixture = keyboardContext({
      activePanel: "tool_approval",
      debuggerDrawerOpen: true,
    });
    const enterTop = keyboardEvent("Tab");
    handleGlobalKeyDown(enterTop, fixture.context);
    assert.equal(enterTop.defaultPrevented, true);
    assert.deepEqual(focused, ["top-first"]);

    globalThis.document.activeElement = topLast;
    const wrapForward = keyboardEvent("Tab");
    handleGlobalKeyDown(wrapForward, fixture.context);
    assert.equal(wrapForward.defaultPrevented, true);
    assert.deepEqual(focused, ["top-first", "top-first"]);

    globalThis.document.activeElement = topFirst;
    const wrapBackward = keyboardEvent("Tab");
    wrapBackward.shiftKey = true;
    handleGlobalKeyDown(wrapBackward, fixture.context);
    assert.equal(wrapBackward.defaultPrevented, true);
    assert.deepEqual(focused, ["top-first", "top-first", "top-last"]);
  } finally {
    if (originalDocument === undefined) {
      delete globalThis.document;
    } else {
      globalThis.document = originalDocument;
    }
  }
});

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
    actions: {
      close: () => {},
      invokeCommand: () => {},
      setPaletteQuery: () => {},
      clampPaletteSelection: () => {},
      onPaletteKeyDown: () => {},
    },
  });
  const search = findVNode(panel, (vnode) => vnode.data?.attrs?.id === "palette-search");
  const surface = findVNode(panel, (vnode) => vnode.data?.attrs?.id === "palette-list");
  assert.equal(search.data.attrs["data-autofocus"], undefined);
  assert.equal(search.data.on.keydown, undefined);
  assert.equal(surface.data.attrs["data-autofocus"], "true");
  assert.equal(surface.data.attrs.tabindex, "0");
  assert.equal(typeof surface.data.on.keydown, "function");
  assert.ok(surface.children.every((option) => option.sel === "li"));
});

test("CSS keeps four viewport rows, complete docks, and independent debugger scroll", async () => {
  const styles = await readFile(resolve(stage, "styles.css"), "utf8");
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
