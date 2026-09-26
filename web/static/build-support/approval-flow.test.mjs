import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

const stage = process.argv[2];
if (stage === undefined) {
  throw new Error("usage: approval-flow.test.mjs <compiled-web-root>");
}

globalThis.addEventListener = () => {};
globalThis.window = globalThis;

const appUrl = pathToFileURL(resolve(stage, "src/app.js"));
const contractsUrl = pathToFileURL(resolve(stage, "src/generated/contracts.js"));
const iconsUrl = pathToFileURL(resolve(stage, "src/icons.js"));
const jsonUrl = pathToFileURL(resolve(stage, "src/json.js"));
const keyboardUrl = pathToFileURL(resolve(stage, "src/app/keyboard.js"));
const protocolUrl = pathToFileURL(resolve(stage, "src/protocol.js"));
const {
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
} = await import(appUrl.href);
const {
  COMMAND_ORDER,
  SPINNER_FRAMES,
  TOOL_SPINNER_FRAMES,
  WFE_PROTOCOL_VERSION,
  WFE_PROTOCOL_SCHEMA_SHA256,
} = await import(contractsUrl.href);
const { activitySpinner } = await import(iconsUrl.href);
const { inspectJson } = await import(jsonUrl.href);
const { handlePaletteKeyDown } = await import(keyboardUrl.href);
const {
  applyStateChange,
  beginRemoteConnection,
  clientContractGate,
  commandResponseMatchesRequest,
  createRemoteState,
  parseServerMessage,
  reduceServerMessage,
} = await import(protocolUrl.href);

const SESSION_ID = "11111111-2222-4333-8444-555555555555";

function projectionLoss(overrides = {}) {
  return {
    filtered: false,
    redacted: false,
    truncation: null,
    ...overrides,
  };
}

function approval(overrides = {}) {
  return {
    approval_id: "approval-1",
    session_id: SESSION_ID,
    tool_call_id: "tool-call-1",
    tool_name: "write_file",
    description: "Review write",
    preview: '{\n  "path": "[REDACTED-PATH]"\n}',
    preview_redacted: true,
    preview_truncated: false,
    can_view_original: false,
    allowed_decisions: ["approve_once", "approve_always", "deny"],
    ...overrides,
  };
}

function snapshotForApproval(current) {
  return {
    pending_approval: current,
    pending_question: null,
    overlay: { data: null },
  };
}

function patchMessage(value) {
  return JSON.stringify({
    type: "state_patch",
    patch: {
      sequence: 2,
      base_revision: 0,
      revision: 1,
      changes: [{ type: "pending_approval", value }],
    },
  });
}

function toolBlockPatch(tool) {
  return JSON.stringify({
    type: "state_patch",
    patch: {
      sequence: 2,
      base_revision: 0,
      revision: 1,
      changes: [
        {
          type: "blocks",
          value: {
            blocks: [
              {
                kind: tool.kind === "call" ? "tool_call" : "tool_result",
                content: "",
                content_loss: projectionLoss(),
                tool,
                title: null,
                title_loss: projectionLoss(),
                success: null,
                usage: null,
                estimated_cost: null,
              },
            ],
            omitted_before: 0,
            truncated: false,
          },
        },
      ],
    },
  });
}

function emptyUsage() {
  return {
    uncached_input_tokens: "0",
    cache_read_input_tokens: "0",
    cache_creation_input_tokens: "0",
    output_tokens: "0",
    total_input_tokens: "0",
    total_tokens: "0",
    breakdown_complete: true,
  };
}

function accountingTotals() {
  return {
    usage: emptyUsage(),
    estimated_cost: null,
    request_count: "0",
    long_context_request_count: "0",
    unpriced_request_count: "0",
    incomplete_usage_request_count: "0",
  };
}

function completeSnapshot() {
  return {
    session: {
      session_id: SESSION_ID,
      display_name: null,
      fallback_label: "Session",
    },
    blocks: { blocks: [], omitted_before: 0, truncated: false },
    activity: {
      kind: "idle",
      fully_idle: true,
      cancellable: false,
      cancel_id: null,
      progress_percent: null,
    },
    pending_approval: null,
    pending_question: null,
    commands: COMMAND_ORDER.map((id) => ({
      id,
      label: id,
      icon: "command",
      enabled: true,
      disabled_reason: null,
      behavior: "execute",
      accelerator: null,
      description: id,
    })),
    sessions: {
      sessions: [
        {
          session_id: SESSION_ID,
          display_name: null,
          fallback_label: "Session",
          selected: true,
        },
      ],
      has_more: false,
    },
    models: [],
    themes: [],
    usage: {
      latest_turn: accountingTotals(),
      session: accountingTotals(),
    },
    status: {
      stop_reason: "Ready",
      stop_reason_loss: projectionLoss(),
      model_label: "model",
      provider_label: "provider",
      provider_transport: "open_ai_chat_completions",
      python: {
        profile: "general",
        target: null,
        backend: null,
        network: null,
        workspace_access: null,
        grant_count: 0,
        policy_source: "config",
        container: null,
      },
      tokens_per_second: null,
      prompt_tokens_per_second: null,
      context_tokens: "0",
      context_limit_tokens: "262144",
      context_source: "local_estimate",
      request_usage: null,
      memory_mebibytes: "32",
      file_count: 0,
      visible_block_count: 0,
      git_state: "clean",
    },
    debugger: {
      open: false,
      summary: "Actor idle · browser mirror ready",
      entries: [],
      omitted_before: 0,
    },
    overlay: { active_panel: null, data: null },
  };
}

function snapshotMessage(state) {
  return JSON.stringify({
    type: "state_snapshot",
    snapshot: {
      protocol_version: WFE_PROTOCOL_VERSION,
      sequence: 0,
      revision: 0,
      state,
    },
  });
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

function keyboardEvent(key, overrides = {}) {
  const commandSurface = {};
  return {
    key,
    ctrlKey: false,
    altKey: false,
    shiftKey: false,
    metaKey: false,
    isComposing: false,
    repeat: false,
    defaultPrevented: false,
    target: commandSurface,
    currentTarget: commandSurface,
    preventDefault() {
      this.defaultPrevented = true;
    },
    ...overrides,
  };
}

function paletteKeyboardContext(snapshot, invoked, overrides = {}) {
  return {
    state: {
      snapshot,
      palette: { query: "", selected: 0 },
      activePanel: "command_palette",
      ...overrides,
    },
    actions: {
      setPaletteSelection: (selected) => {
        overrides.selection?.(selected);
      },
      invokeCommand: (command) => invoked.push(command.id),
    },
  };
}

function snapshotWithCanonicalAccelerators() {
  const snapshot = completeSnapshot();
  const accelerators = new Map([
    ["hotkeys", "h"],
    ["themes", "t"],
    ["clear-ui", "c"],
    ["toggle-debugger", "d"],
  ]);
  for (const command of snapshot.commands) {
    command.accelerator = accelerators.get(command.id) ?? null;
  }
  return snapshot;
}

test("palette command surface dispatches canonical accelerators only in context", () => {
  const snapshot = snapshotWithCanonicalAccelerators();
  for (const [key, expected] of [
    ["h", "hotkeys"],
    ["H", "hotkeys"],
    ["t", "themes"],
    ["T", "themes"],
    ["c", "clear-ui"],
    ["C", "clear-ui"],
    ["d", "toggle-debugger"],
    ["D", "toggle-debugger"],
  ]) {
    const invoked = [];
    const event = keyboardEvent(key, { shiftKey: key === key.toUpperCase() });
    handlePaletteKeyDown(event, paletteKeyboardContext(snapshot, invoked));
    assert.deepEqual(invoked, [expected], key);
    assert.equal(event.defaultPrevented, true, key);
  }

  for (const overrides of [
    { ctrlKey: true },
    { altKey: true },
    { metaKey: true },
    { repeat: true },
    { isComposing: true },
    { defaultPrevented: true },
    { target: {}, currentTarget: {} },
  ]) {
    const invoked = [];
    const event = keyboardEvent("d", overrides);
    handlePaletteKeyDown(event, paletteKeyboardContext(snapshot, invoked));
    assert.deepEqual(invoked, []);
  }

  const protectedSnapshot = snapshotWithCanonicalAccelerators();
  protectedSnapshot.overlay.active_panel = "tool_approval";
  const protectedInvocations = [];
  handlePaletteKeyDown(
    keyboardEvent("d"),
    paletteKeyboardContext(protectedSnapshot, protectedInvocations, {
      activePanel: "tool_approval",
    }),
  );
  assert.deepEqual(protectedInvocations, []);

  const staleOverlaySnapshot = snapshotWithCanonicalAccelerators();
  staleOverlaySnapshot.overlay.active_panel = "themes";
  const staleOverlayInvocations = [];
  handlePaletteKeyDown(
    keyboardEvent("d"),
    paletteKeyboardContext(staleOverlaySnapshot, staleOverlayInvocations),
  );
  assert.deepEqual(staleOverlayInvocations, ["toggle-debugger"]);

  const wrongPanelInvocations = [];
  handlePaletteKeyDown(
    keyboardEvent("d"),
    paletteKeyboardContext(snapshot, wrongPanelInvocations, {
      activePanel: "ask_user",
    }),
  );
  assert.deepEqual(wrongPanelInvocations, []);

  snapshot.commands.find((command) => command.id === "toggle-debugger").enabled = false;
  const disabledInvocations = [];
  handlePaletteKeyDown(
    keyboardEvent("d"),
    paletteKeyboardContext(snapshot, disabledInvocations),
  );
  assert.deepEqual(disabledInvocations, []);
});

test("palette command surface owns navigation and Enter", () => {
  const snapshot = snapshotWithCanonicalAccelerators();
  let selection = null;
  const invoked = [];
  const context = paletteKeyboardContext(snapshot, invoked, {
    selection: (value) => {
      selection = value;
    },
  });
  const down = keyboardEvent("ArrowDown");
  handlePaletteKeyDown(down, context);
  assert.equal(selection, 1);
  assert.equal(down.defaultPrevented, true);

  const enter = keyboardEvent("Enter");
  handlePaletteKeyDown(enter, context);
  assert.deepEqual(invoked, [COMMAND_ORDER[0]]);
  assert.equal(enter.defaultPrevented, true);
});

test("protocol v6 admits bound decisions for hidden approval previews", () => {
  assert.equal(clientContractGate(), true);
  assert.equal(parseServerMessage(patchMessage(approval())).ok, true);
  assert.equal(
    parseServerMessage(
      patchMessage(approval({ preview_redacted: false, preview_truncated: true })),
    ).ok,
    true,
  );

  assert.equal(
    parseServerMessage(patchMessage(approval({ can_view_original: true }))).ok,
    false,
  );
  assert.equal(
    parseServerMessage(
      patchMessage(
        approval({ allowed_decisions: ["approve_once", "approve_once", "deny"] }),
      ),
    ).ok,
    false,
  );
  assert.equal(
    parseServerMessage(
      patchMessage(approval({ allowed_decisions: ["approve_once"] })),
    ).ok,
    false,
  );

  const missingFlag = approval();
  delete missingFlag.preview_redacted;
  assert.equal(parseServerMessage(patchMessage(missingFlag)).ok, false);
});

test("protocol v6 requires exact tool projection loss records", () => {
  const complete = {
    kind: "call",
    tool_name: "write_file",
    payload: '{"path":"notes.txt"}',
    payload_loss: projectionLoss(),
  };
  assert.equal(parseServerMessage(toolBlockPatch(complete)).ok, true);
  for (const payload_loss of [
    projectionLoss({ filtered: true }),
    projectionLoss({ redacted: true }),
    projectionLoss({ truncation: "size_limit" }),
    projectionLoss({ truncation: "invalid_source" }),
  ]) {
    assert.equal(
      parseServerMessage(toolBlockPatch({ ...complete, payload_loss })).ok,
      true,
    );
  }

  const mutateBlock = (tool, mutate) => {
    const message = JSON.parse(toolBlockPatch(tool));
    mutate(message.patch.changes[0].value.blocks[0]);
    return JSON.stringify(message);
  };
  assert.equal(
    parseServerMessage(
      mutateBlock(complete, (block) => {
        block.content_loss.redacted = true;
      }),
    ).ok,
    false,
  );
  assert.equal(
    parseServerMessage(
      mutateBlock(complete, (block) => {
        block.kind = "tool_result";
      }),
    ).ok,
    false,
  );
  assert.equal(
    parseServerMessage(
      mutateBlock(complete, (block) => {
        block.content = "duplicate payload";
      }),
    ).ok,
    false,
  );
  assert.equal(
    parseServerMessage(
      mutateBlock(complete, (block) => {
        block.kind = "text";
      }),
    ).ok,
    false,
  );

  const missing = structuredClone(complete);
  delete missing.payload_loss.redacted;
  assert.equal(parseServerMessage(toolBlockPatch(missing)).ok, false);
  assert.equal(
    parseServerMessage(
      toolBlockPatch({ ...complete, payload_redacted: false }),
    ).ok,
    false,
  );
  assert.equal(
    parseServerMessage(
      mutateBlock(complete, (block) => {
        block.content_truncated = false;
      }),
    ).ok,
    false,
  );
});

test("protocol v6 validates expanded status and independent debugger state", () => {
  const snapshot = completeSnapshot();
  assert.equal(parseServerMessage(snapshotMessage(snapshot)).ok, true);

  const withContainerAndCost = structuredClone(snapshot);
  withContainerAndCost.status.python = {
    profile: "python_only",
    target: "sandbox",
    backend: "podman",
    network: "full",
    workspace_access: "read_write",
    grant_count: 0,
    policy_source: "one_time",
    container: {
      kind: "transient",
      name: "lethetic-python-transient-123-0",
      active: true,
    },
  };
  withContainerAndCost.usage.latest_turn.estimated_cost = {
    display: "$0.000001",
    currency: "USD",
    nanos: "1000",
    incomplete: false,
    mixed_pricing: false,
    long_context_applied: false,
    pricing_effective_as_of: "2026-01-01",
    pricing_valid_through: null,
    provenance_kind: "api_equivalent_estimate",
  };
  assert.equal(
    parseServerMessage(snapshotMessage(withContainerAndCost)).ok,
    true,
  );

  const cliLocked = structuredClone(withContainerAndCost);
  cliLocked.status.python.policy_source = "cli_locked";
  assert.equal(parseServerMessage(snapshotMessage(cliLocked)).ok, true);
  const changed = applyStateChange(snapshot, {
    type: "status",
    value: cliLocked.status,
  });
  assert.equal(changed.status.python.policy_source, "cli_locked");

  const displaySpellingOnWire = structuredClone(cliLocked);
  displaySpellingOnWire.status.python.policy_source = "cli-locked";
  assert.equal(
    parseServerMessage(snapshotMessage(displaySpellingOnWire)).ok,
    false,
  );

  const debuggerValue = {
    open: true,
    summary: "Actor active · browser mirror ready",
    entries: [
      {
        code: "remote_control_degraded",
        severity: "warning",
        message: "Mirror recovered",
      },
    ],
    omitted_before: 2,
  };
  const debuggerPatch = JSON.stringify({
    type: "state_patch",
    patch: {
      sequence: 1,
      base_revision: 0,
      revision: 1,
      changes: [{ type: "debugger", value: debuggerValue }],
    },
  });
  assert.equal(parseServerMessage(debuggerPatch).ok, true);
  assert.deepEqual(
    applyStateChange(snapshot, { type: "debugger", value: debuggerValue }).debugger,
    debuggerValue,
  );

  const oldDiagnostics = structuredClone(snapshot);
  oldDiagnostics.diagnostics = oldDiagnostics.debugger.entries;
  delete oldDiagnostics.debugger;
  assert.equal(parseServerMessage(snapshotMessage(oldDiagnostics)).ok, false);

  const oldDebuggerPanel = structuredClone(snapshot);
  oldDebuggerPanel.overlay.active_panel = "debugger";
  assert.equal(parseServerMessage(snapshotMessage(oldDebuggerPanel)).ok, false);

  const malformedLoss = structuredClone(snapshot);
  malformedLoss.status.stop_reason_loss.truncation = "unknown";
  assert.equal(parseServerMessage(snapshotMessage(malformedLoss)).ok, false);

  const missingDisplay = structuredClone(withContainerAndCost);
  delete missingDisplay.usage.latest_turn.estimated_cost.display;
  assert.equal(parseServerMessage(snapshotMessage(missingDisplay)).ok, false);
});

test("protocol v6 binds cancellation targets and optional file capability exactly", () => {
  const idle = completeSnapshot();
  assert.equal(parseServerMessage(snapshotMessage(idle)).ok, true);

  const active = structuredClone(idle);
  active.activity = {
    kind: "processing",
    fully_idle: false,
    cancellable: true,
    cancel_id: "cancel-current-turn",
    progress_percent: null,
  };
  assert.equal(parseServerMessage(snapshotMessage(active)).ok, true);

  const missingCancelId = structuredClone(active);
  delete missingCancelId.activity.cancel_id;
  assert.equal(parseServerMessage(snapshotMessage(missingCancelId)).ok, false);
  const inconsistentCancelId = structuredClone(active);
  inconsistentCancelId.activity.cancellable = false;
  assert.equal(parseServerMessage(snapshotMessage(inconsistentCancelId)).ok, false);

  const capabilities = {
    state_patches: true,
    request_replay: true,
    session_names: true,
    exact_tool_approval: true,
    read_only_files: false,
  };
  const hello = {
    type: "hello",
    hello: {
      protocol_version: WFE_PROTOCOL_VERSION,
      minimum_protocol_version: WFE_PROTOCOL_VERSION,
      server_name: "lethetic",
      sequence: 0,
      revision: 0,
      capabilities,
      schema_sha256: WFE_PROTOCOL_SCHEMA_SHA256,
    },
  };
  assert.equal(parseServerMessage(JSON.stringify(hello)).ok, true);
  hello.hello.capabilities.read_only_files = true;
  assert.equal(parseServerMessage(JSON.stringify(hello)).ok, true);
  delete hello.hello.capabilities.read_only_files;
  assert.equal(parseServerMessage(JSON.stringify(hello)).ok, false);
});

test("protocol v6 accepts only authoritative history recall outcomes", () => {
  const request = {
    id: "history-request",
    expected_revision: 7,
    type: "select_history_entry",
    session_id: SESSION_ID,
    entry_id: "history-42",
  };
  const response = {
    id: request.id,
    result: {
      status: "ok",
      revision: 7,
      outcome: {
        type: "history_entry_selected",
        session_id: SESSION_ID,
        entry_id: "history-42",
        editor_content: "complete original prompt",
      },
    },
  };
  const parsed = parseServerMessage(
    JSON.stringify({ type: "command_response", response }),
  );
  assert.equal(parsed.ok, true);
  assert.equal(commandResponseMatchesRequest(response, request), true);
  assert.equal(
    commandResponseMatchesRequest(response, { ...request, entry_id: "other" }),
    false,
  );
  assert.equal(
    commandResponseMatchesRequest(response, {
      ...request,
      session_id: "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
    }),
    false,
  );
  assert.equal(
    commandResponseMatchesRequest(
      {
        ...response,
        result: { status: "ok", revision: 7, outcome: { type: "applied" } },
      },
      request,
    ),
    false,
  );

  const malformed = structuredClone(response);
  malformed.result.outcome.editor_content = null;
  assert.equal(
    parseServerMessage(
      JSON.stringify({ type: "command_response", response: malformed }),
    ).ok,
    false,
  );
});

test("protocol v6 reducer fails closed on v5 hello and snapshot", () => {
  const capabilities = {
    state_patches: true,
    request_replay: true,
    session_names: true,
    exact_tool_approval: true,
    read_only_files: false,
  };
  const helloMessage = (protocolVersion, minimumProtocolVersion) =>
    JSON.stringify({
      type: "hello",
      hello: {
        protocol_version: protocolVersion,
        minimum_protocol_version: minimumProtocolVersion,
        server_name: "lethetic",
        sequence: 0,
        revision: 0,
        capabilities,
        schema_sha256: WFE_PROTOCOL_SCHEMA_SHA256,
      },
    });

  const legacyHello = parseServerMessage(helloMessage(5, 5));
  assert.equal(legacyHello.ok, true);
  let remote = beginRemoteConnection(createRemoteState());
  let reduced = reduceServerMessage(remote, legacyHello.message);
  assert.equal(reduced.state.phase, "incompatible");
  assert.equal(reduced.effect, "fatal");

  const currentHello = parseServerMessage(
    helloMessage(WFE_PROTOCOL_VERSION, WFE_PROTOCOL_VERSION),
  );
  assert.equal(currentHello.ok, true);
  remote = beginRemoteConnection(createRemoteState());
  reduced = reduceServerMessage(remote, currentHello.message);
  assert.equal(reduced.state.phase, "awaiting_snapshot");
  assert.equal(reduced.effect, "none");

  const legacySnapshotValue = JSON.parse(snapshotMessage(completeSnapshot()));
  legacySnapshotValue.snapshot.protocol_version = 5;
  const legacySnapshot = parseServerMessage(
    JSON.stringify(legacySnapshotValue),
  );
  assert.equal(legacySnapshot.ok, true);
  reduced = reduceServerMessage(reduced.state, legacySnapshot.message);
  assert.equal(reduced.state.phase, "incompatible");
  assert.equal(reduced.effect, "fatal");
});

test("hidden approval confirmation is bound to every displayed identity", () => {
  const current = approval();
  assert.equal(approvalHasHiddenContent(current), true);
  assert.equal(
    approvalHasHiddenContent(
      approval({ preview_redacted: false, preview_truncated: false }),
    ),
    false,
  );

  const key = approvalConfirmationKey(7, current);
  assert.equal(key, approvalConfirmationKey(7, approval()));
  assert.notEqual(key, approvalConfirmationKey(8, approval()));
  assert.notEqual(
    key,
    approvalConfirmationKey(7, approval({ preview: "different" })),
  );
  assert.notEqual(
    key,
    approvalConfirmationKey(
      7,
      approval({ allowed_decisions: ["approve_once", "deny"] }),
    ),
  );
});

test("approval panel identity changes with the pending call", () => {
  const first = approval();
  const second = approval({
    approval_id: "approval-2",
    tool_call_id: "tool-call-2",
  });
  assert.notEqual(
    panelInstanceKey("tool_approval", snapshotForApproval(first)),
    panelInstanceKey("tool_approval", snapshotForApproval(second)),
  );
  assert.equal(
    panelInstanceKey("tool_approval", snapshotForApproval(first)),
    panelInstanceKey("tool_approval", snapshotForApproval(approval())),
  );
});

test("live activity maps exhaustively to the authored TUI spinners", async () => {
  const expected = {
    idle: null,
    loading_session: null,
    awaiting_approval: null,
    executing_tool: "tool",
    awaiting_answer: null,
    processing: "regular",
    managing_lsp: "tool",
  };
  for (const [kind, spinner] of Object.entries(expected)) {
    assert.equal(activitySpinnerKind(kind, true), spinner, kind);
    assert.equal(activitySpinnerKind(kind, false), null, `offline ${kind}`);
  }

  assert.deepEqual(Array.from(SPINNER_FRAMES), ["◰", "◳", "◲", "◱"]);
  assert.deepEqual(Array.from(TOOL_SPINNER_FRAMES), [
    "⢎ ",
    "⠎⠁",
    "⠊⠑",
    "⠈⠱",
    " ⠱",
    "⠠⠰",
    "⠄⠄",
    "⠆⠄",
  ]);

  for (const [kind, frames] of [
    ["regular", SPINNER_FRAMES],
    ["tool", TOOL_SPINNER_FRAMES],
  ]) {
    const vnode = activitySpinner(kind);
    assert.equal(vnode.sel, "span");
    assert.equal(vnode.data.attrs.class, `activity-spinner activity-spinner-${kind}`);
    assert.equal(vnode.data.attrs["aria-hidden"], "true");
    assert.equal(vnode.data.on, undefined);
    assert.equal(vnode.children.length, frames.length);
    assert.deepEqual(vnode.children.map(vnodeText), Array.from(frames));
    assert.ok(
      vnode.children.every(
        (frame) => frame.data.attrs.class === "activity-spinner-frame",
      ),
    );
  }

  const styles = await readFile(resolve(stage, "styles.css"), "utf8");
  assert.match(
    styles,
    /\.activity-spinner-regular \.activity-spinner-frame\s*\{[^}]*400ms[^}]*\}/su,
  );
  assert.match(
    styles,
    /\.activity-spinner-tool \.activity-spinner-frame\s*\{[^}]*800ms[^}]*\}/su,
  );
  for (let frame = 2; frame <= 8; frame += 1) {
    assert.match(
      styles,
      new RegExp(
        `\\.activity-spinner-frame:nth-child\\(${String(frame)}\\)\\s*\\{[^}]*animation-delay: ${String((frame - 1) * 100)}ms`,
        "su",
      ),
    );
  }
  assert.match(
    styles,
    /@media \(prefers-reduced-motion: reduce\)[\s\S]*?\.activity-spinner-frame\s*\{[^}]*animation: none !important;[^}]*opacity: 0;[^}]*\}[\s\S]*?\.activity-spinner-frame:first-child\s*\{[^}]*opacity: 1;/u,
  );
});

test("projection loss notices are exact and never inferred from payload text", () => {
  assert.deepEqual(
    projectionLossMessages([
      projectionLoss({ filtered: true }),
      projectionLoss({ redacted: true }),
      projectionLoss({ truncation: "size_limit" }),
      projectionLoss({ truncation: "invalid_source" }),
    ]),
    [
      "WFE intentionally omitted fields outside this browser-safe projection.",
      "WFE replaced sensitive values before browser delivery.",
      "WFE shortened this value to a fixed browser limit; the omitted tail is not shown.",
      "WFE replaced an invalid or incomplete value with a safe projection.",
    ],
  );
  assert.deepEqual(projectionLossMessages([projectionLoss()]), []);
  assert.deepEqual(
    projectionLossMessages([
      projectionLoss(),
      projectionLoss(),
    ]),
    [],
  );
});

test("JSON highlighting is attempted only for complete structured payloads", () => {
  assert.equal(
    approvalUsesJsonHighlighting(
      approval({ preview_redacted: false, preview_truncated: false }),
    ),
    true,
  );
  assert.equal(
    approvalUsesJsonHighlighting(
      approval({
        tool_name: "python",
        preview_redacted: false,
        preview_truncated: false,
      }),
    ),
    false,
  );
  assert.equal(approvalUsesJsonHighlighting(approval()), true);
  assert.equal(
    approvalUsesJsonHighlighting(
      approval({ preview_redacted: false, preview_truncated: true }),
    ),
    false,
  );

  assert.equal(toolBlockUsesJsonHighlighting({ tool: null }), false);
  for (const payload_loss of [
    projectionLoss({ filtered: true }),
    projectionLoss({ redacted: true }),
  ]) {
    assert.equal(
      toolBlockUsesJsonHighlighting({ tool: { payload_loss } }),
      true,
    );
  }
  for (const truncation of ["size_limit", "invalid_source"]) {
    assert.equal(
      toolBlockUsesJsonHighlighting({
        tool: { payload_loss: projectionLoss({ truncation }) },
      }),
      false,
    );
  }
  assert.equal(
    blockContentIsLossy({
      content_loss: projectionLoss(),
      tool: { payload_loss: projectionLoss({ redacted: true }) },
    }),
    true,
  );
  assert.equal(
    blockContentIsLossy({
      content_loss: projectionLoss({ filtered: true }),
      tool: null,
    }),
    true,
  );
  assert.equal(
    blockContentIsLossy({
      content_loss: projectionLoss(),
      tool: { payload_loss: projectionLoss() },
    }),
    false,
  );

  assert.equal(
    markdownBlockUsesJsonHighlighting({
      content_loss: projectionLoss({ redacted: true }),
      tool: null,
    }),
    true,
  );
  assert.equal(
    markdownBlockUsesJsonHighlighting({
      content_loss: projectionLoss({ truncation: "size_limit" }),
      tool: null,
    }),
    false,
  );
  assert.equal(
    markdownBlockUsesJsonHighlighting({
      content_loss: projectionLoss(),
      tool: {
        payload_loss: projectionLoss({ truncation: "invalid_source" }),
      },
    }),
    false,
  );
  assert.equal(
    toolResultUsesAutoRendering({
      kind: "tool_result",
      tool: { kind: "result" },
    }),
    true,
  );
  assert.equal(
    toolResultUsesAutoRendering({
      kind: "tool_call",
      tool: { kind: "call" },
    }),
    false,
  );
  assert.equal(
    toolResultUsesAutoRendering({ kind: "tool_result", tool: null }),
    false,
  );
});

test("visible chat allocates exact JSON demand before large prose", () => {
  const toolBlock = (kind, payload, overrides = {}) => ({
    kind: kind === "call" ? "tool_call" : "tool_result",
    content: "",
    content_loss: projectionLoss(),
    tool: {
      kind,
      tool_name: kind === "call" ? "write_file" : "repo_overview",
      payload,
      payload_loss: projectionLoss(),
      ...overrides,
    },
  });
  const tinyPayload = '{"path":"[REDACTED-PATH]"}';
  const tinyDemand = inspectJson(tinyPayload);
  assert.equal(tinyDemand.status, "valid");
  const redactedCall = toolBlock("call", tinyPayload, {
    payload_loss: projectionLoss({ redacted: true }),
  });
  const largeProse = toolBlock(
    "result",
    `# Overview\n\n${"plain prose ".repeat(5_000)}`,
  );
  const allocations = chatJsonSegmentAllocations([
    redactedCall,
    largeProse,
  ]);
  assert.equal(allocations[0], tinyDemand.segments);
  assert.equal(allocations[1], 4_096 - tinyDemand.segments);
  assert.ok(allocations.reduce((sum, value) => sum + value, 0) <= 4_096);

  const older = toolBlock(
    "call",
    `[${Array.from({ length: 1_000 }, () => "0").join(",")}]`,
  );
  const newest = toolBlock(
    "call",
    `[${Array.from({ length: 1_500 }, () => "0").join(",")}]`,
  );
  const exhausted = chatJsonSegmentAllocations([older, newest]);
  assert.equal(exhausted[0], 0);
  assert.equal(exhausted[1], 3_001);

  const truncated = chatJsonSegmentAllocations([
    toolBlock("call", tinyPayload, {
      payload_loss: projectionLoss({ truncation: "size_limit" }),
    }),
  ]);
  assert.equal(truncated[0], 0);
});

test("confirmation copy distinguishes one-call and policy-wide hidden authority", () => {
  const redacted = approval();
  const truncated = approval({ preview_redacted: false, preview_truncated: true });
  const once = hiddenApprovalConfirmationMessage("approve_once", redacted);
  const always = hiddenApprovalConfirmationMessage("approve_always", truncated);

  assert.match(once, /exact current call once/u);
  assert.match(once, /sensitive values were replaced/u);
  assert.match(always, /all later tool calls/u);
  assert.match(always, /current execution policy/u);
  assert.match(always, /unseen tail/u);
});
