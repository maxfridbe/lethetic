import {
  COMMAND_ORDER,
  ICON_IDS,
  WFE_MAX_SERVER_MESSAGE_BYTES,
  WFE_PROTOCOL_SCHEMA_SHA256,
  WFE_PROTOCOL_VERSION,
} from "./generated/contracts.js";
import { isBackgroundTaskList } from "./background-protocol.js";
import { isSkillCatalogView, isSkillChoiceView } from "./skills-protocol.js";
import type * as Wire from "./generated/contracts.js";
import {
  assertNever,
  isBoolean,
  isFiniteInteger,
  isRecord,
  isString,
} from "./safety.js";

const MAX_WIRE_MESSAGE_BYTES = WFE_MAX_SERVER_MESSAGE_BYTES;
const MAX_WIRE_ARRAY_LENGTH = 20_000;
const CANONICAL_SESSION_ID =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/u;
// The schema fingerprint is generated from the Rust contracts at build time
// (web/src/generated/contracts.ts) and the server computes the same digest at
// runtime. The live handshake compares the two, so a stale bundle is rejected
// without any hand-maintained copy of the hash.
export function clientContractGate(): boolean {
  return (
    WFE_PROTOCOL_VERSION === 6 &&
    /^[0-9a-f]{64}$/u.test(WFE_PROTOCOL_SCHEMA_SHA256)
  );
}

function isExactRecord(
  value: unknown,
  keys: readonly string[],
): value is Record<string, unknown> {
  if (!isRecord(value)) {
    return false;
  }
  const actual = Object.keys(value);
  return (
    actual.length === keys.length &&
    actual.every((key) => keys.includes(key))
  );
}

function isArrayOf<T>(
  value: unknown,
  predicate: (item: unknown) => item is T,
): value is Array<T> {
  return (
    Array.isArray(value) &&
    value.length <= MAX_WIRE_ARRAY_LENGTH &&
    value.every((item) => predicate(item))
  );
}

function isNullable<T>(
  value: unknown,
  predicate: (item: unknown) => item is T,
): value is T | null {
  return value === null || predicate(value);
}

function isCanonicalSessionId(value: unknown): value is string {
  return isString(value) && CANONICAL_SESSION_ID.test(value);
}

function isCatalogKey<T extends string>(
  value: unknown,
  catalog: Readonly<Record<T, true>>,
): value is T {
  return isString(value) && Object.prototype.hasOwnProperty.call(catalog, value);
}

const COMMAND_IDS = Object.fromEntries(
  COMMAND_ORDER.map((id) => [id, true] as const),
) as Readonly<Record<Wire.CommandId, true>>;
const GENERATED_ICON_IDS = Object.fromEntries(
  ICON_IDS.map((id) => [id, true] as const),
) as Readonly<Record<Wire.IconId, true>>;

const COMMAND_BEHAVIORS = {
  execute: true,
  open_panel: true,
  confirm: true,
} as const satisfies Readonly<Record<Wire.CommandBehavior, true>>;
const LSP_ACTIONS = {
  install: true,
  enable: true,
  disable: true,
  cancel_install: true,
} as const satisfies Readonly<Record<Wire.LspAction, true>>;
const ERROR_CODES = {
  bad_request: true,
  protocol_mismatch: true,
  stale_revision: true,
  session_mismatch: true,
  tool_call_mismatch: true,
  request_id_conflict: true,
  busy: true,
  confirmation_required: true,
  not_found: true,
  save_failed: true,
  backend_unavailable: true,
  internal: true,
} as const satisfies Readonly<Record<Wire.CommandErrorCode, true>>;
const PANEL_IDS = {
  command_palette: true,
  hotkeys: true,
  themes: true,
  input_history: true,
  loop_detection: true,
  system_prompt: true,
  sessions: true,
  name_session: true,
  latest_files: true,
  models: true,
  lsp_servers: true,
  agent_mode: true,
  tool_approval: true,
  ask_user: true,
  confirmation: true,
  skills: true,
} as const satisfies Readonly<Record<Wire.PanelId, true>>;
const BLOCK_KINDS = {
  text: true,
  user: true,
  thought: true,
  markdown: true,
  tool_call: true,
  tool_result: true,
  divider: true,
  formulating: true,
  truncation: true,
} as const satisfies Readonly<Record<Wire.BlockKind, true>>;
const TOOL_BLOCK_KINDS = {
  call: true,
  result: true,
} as const satisfies Readonly<Record<Wire.ToolBlockKind, true>>;
const PROJECTION_TRUNCATION_KINDS = {
  size_limit: true,
  invalid_source: true,
} as const satisfies Readonly<Record<Wire.ProjectionTruncationKind, true>>;
const ACTIVITY_KINDS = {
  idle: true,
  loading_session: true,
  awaiting_approval: true,
  executing_tool: true,
  awaiting_answer: true,
  processing: true,
  managing_lsp: true,
} as const satisfies Readonly<Record<Wire.ActivityKind, true>>;
const APPROVAL_DECISIONS = {
  approve_once: true,
  approve_always: true,
  deny: true,
} as const satisfies Readonly<Record<Wire.ApprovalDecision, true>>;
const MODEL_TRANSPORTS = {
  open_ai_chat_completions: true,
  claude_code_proxy: true,
} as const satisfies Readonly<Record<Wire.ModelTransportView, true>>;
const CONTEXT_USAGE_SOURCES = {
  server_usage: true,
  server_prompt: true,
  local_estimate: true,
} as const satisfies Readonly<Record<Wire.ContextUsageSourceView, true>>;
const PYTHON_PROFILES = {
  general: true,
  python_only: true,
} as const satisfies Readonly<Record<Wire.PythonProfileView, true>>;
const PYTHON_TARGETS = {
  host: true,
  sandbox: true,
} as const satisfies Readonly<Record<Wire.PythonTargetView, true>>;
const PYTHON_BACKENDS = {
  bubblewrap: true,
  podman: true,
} as const satisfies Readonly<Record<Wire.PythonBackendView, true>>;
const NETWORK_ACCESS = {
  none: true,
  public_only: true,
  full: true,
} as const satisfies Readonly<Record<Wire.NetworkAccessView, true>>;
const WORKSPACE_ACCESS = {
  read_only: true,
  read_write: true,
} as const satisfies Readonly<Record<Wire.WorkspaceAccessView, true>>;
const PYTHON_POLICY_SOURCES = {
  config: true,
  global: true,
  project: true,
  one_time: true,
  cli_locked: true,
} as const satisfies Readonly<Record<Wire.PythonPolicySourceView, true>>;
const PYTHON_CONTAINER_KINDS = {
  retained: true,
  transient: true,
} as const satisfies Readonly<Record<Wire.PythonContainerKindView, true>>;
const GIT_STATES = {
  clean: true,
  dirty: true,
  unknown: true,
} as const satisfies Readonly<Record<Wire.GitStateView, true>>;
const DIAGNOSTIC_SEVERITIES = {
  info: true,
  warning: true,
  error: true,
} as const satisfies Readonly<Record<Wire.DiagnosticSeverity, true>>;
const DIAGNOSTIC_CODES = {
  connection_interrupted: true,
  save_failed: true,
  session_load_failed: true,
  tool_failed: true,
  remote_control_degraded: true,
  content_truncated: true,
  unknown: true,
} as const satisfies Readonly<Record<Wire.DiagnosticCode, true>>;
const LSP_STATES = {
  available: true,
  installed: true,
  installing: true,
  enabled: true,
  disabled: true,
  failed: true,
} as const satisfies Readonly<Record<Wire.LspServerStateView, true>>;
const DESTRUCTIVE_ACTIONS = {
  clear_context: true,
  delete_python_runtime: true,
  delete_session: true,
  wipe_sessions: true,
  quit: true,
  overwrite_system_prompt: true,
} as const satisfies Readonly<Record<Wire.DestructiveActionView, true>>;
const OUTCOME_TYPES = {
  applied: true,
  panel_opened: true,
  snapshot_queued: true,
  prompt_accepted: true,
  history_entry_selected: true,
  tool_decision_recorded: true,
  session_created: true,
  session_loaded: true,
  session_renamed: true,
  session_deleted: true,
  sessions_wiped: true,
  shutting_down: true,
} as const satisfies Readonly<Record<Wire.CommandOutcome["type"], true>>;
const PANEL_DATA_TYPES = {
  hotkeys: true,
  input_history: true,
  latest_files: true,
  system_prompts: true,
  lsp_servers: true,
  loop_modes: true,
  agent_modes: true,
  name_session: true,
  confirmation: true,
  skills: true,
} as const satisfies Readonly<Record<Wire.PanelDataView["type"], true>>;
const STATE_CHANGE_TYPES = {
  session: true,
  blocks: true,
  activity: true,
  pending_approval: true,
  pending_question: true,
  commands: true,
  sessions: true,
  models: true,
  themes: true,
  usage: true,
  status: true,
  debugger: true,
  overlay: true,
} as const satisfies Readonly<Record<Wire.StateChange["type"], true>>;
const SERVER_MESSAGE_TYPES = {
  hello: true,
  command_response: true,
  state_snapshot: true,
  state_patch: true,
} as const satisfies Readonly<Record<Wire.IServerMessage["type"], true>>;

function isCommandId(value: unknown): value is Wire.CommandId {
  return isCatalogKey(value, COMMAND_IDS);
}

function isIconId(value: unknown): value is Wire.IconId {
  return isCatalogKey(value, GENERATED_ICON_IDS);
}

function isCommandView(value: unknown): value is Wire.CommandView {
  return (
    isExactRecord(value, [
      "id",
      "label",
      "icon",
      "enabled",
      "disabled_reason",
      "behavior",
      "accelerator",
      "description",
    ]) &&
    isCommandId(value["id"]) &&
    isString(value["label"]) &&
    isIconId(value["icon"]) &&
    isBoolean(value["enabled"]) &&
    isNullable(value["disabled_reason"], isString) &&
    isCatalogKey(value["behavior"], COMMAND_BEHAVIORS) &&
    isNullable(value["accelerator"], isString) &&
    isString(value["description"])
  );
}

function isProjectionLossView(value: unknown): value is Wire.ProjectionLossView {
  return (
    isExactRecord(value, ["filtered", "redacted", "truncation"]) &&
    isBoolean(value["filtered"]) &&
    isBoolean(value["redacted"]) &&
    isNullable(
      value["truncation"],
      (item): item is Wire.ProjectionTruncationKind =>
        isCatalogKey(item, PROJECTION_TRUNCATION_KINDS),
    )
  );
}

function isCompleteProjectionLoss(value: unknown): boolean {
  return (
    isProjectionLossView(value) &&
    !value.filtered &&
    !value.redacted &&
    value.truncation === null
  );
}

function isToolBlockView(value: unknown): value is Wire.ToolBlockView {
  return (
    isExactRecord(value, ["kind", "tool_name", "payload", "payload_loss"]) &&
    isCatalogKey(value["kind"], TOOL_BLOCK_KINDS) &&
    isString(value["tool_name"]) &&
    isString(value["payload"]) &&
    isProjectionLossView(value["payload_loss"])
  );
}

function renderBlockToolSemantics(value: Record<string, unknown>): boolean {
  const kind = value["kind"];
  const tool = value["tool"];
  if (kind === "tool_call" || kind === "tool_result") {
    if (!isToolBlockView(tool)) {
      return false;
    }
    const expectedToolKind = kind === "tool_call" ? "call" : "result";
    return (
      tool.kind === expectedToolKind &&
      value["content"] === "" &&
      isCompleteProjectionLoss(value["content_loss"])
    );
  }
  return tool === null;
}

function isUsageView(value: unknown): value is Wire.UsageView {
  return (
    isExactRecord(value, [
      "uncached_input_tokens",
      "cache_read_input_tokens",
      "cache_creation_input_tokens",
      "output_tokens",
      "total_input_tokens",
      "total_tokens",
      "breakdown_complete",
    ]) &&
    isString(value["uncached_input_tokens"]) &&
    isString(value["cache_read_input_tokens"]) &&
    isString(value["cache_creation_input_tokens"]) &&
    isString(value["output_tokens"]) &&
    isString(value["total_input_tokens"]) &&
    isString(value["total_tokens"]) &&
    isBoolean(value["breakdown_complete"])
  );
}

function isCostView(value: unknown): value is Wire.CostView {
  return (
    isExactRecord(value, [
      "display",
      "currency",
      "nanos",
      "incomplete",
      "mixed_pricing",
      "long_context_applied",
      "pricing_effective_as_of",
      "pricing_valid_through",
      "provenance_kind",
    ]) &&
    isString(value["display"]) &&
    isString(value["currency"]) &&
    isString(value["nanos"]) &&
    isBoolean(value["incomplete"]) &&
    isBoolean(value["mixed_pricing"]) &&
    isBoolean(value["long_context_applied"]) &&
    isString(value["pricing_effective_as_of"]) &&
    isNullable(value["pricing_valid_through"], isString) &&
    isString(value["provenance_kind"])
  );
}

function isRenderBlockView(value: unknown): value is Wire.RenderBlockView {
  return (
    isExactRecord(value, [
      "kind",
      "content",
      "content_loss",
      "tool",
      "title",
      "title_loss",
      "success",
      "usage",
      "estimated_cost",
      "duration_label",
    ]) &&
    isCatalogKey(value["kind"], BLOCK_KINDS) &&
    isString(value["content"]) &&
    isProjectionLossView(value["content_loss"]) &&
    isNullable(value["tool"], isToolBlockView) &&
    isNullable(value["title"], isString) &&
    isProjectionLossView(value["title_loss"]) &&
    (value["title"] !== null || isCompleteProjectionLoss(value["title_loss"])) &&
    isNullable(value["success"], isBoolean) &&
    isNullable(value["usage"], isUsageView) &&
    isNullable(value["estimated_cost"], isCostView) &&
    isNullable(value["duration_label"], isString) &&
    renderBlockToolSemantics(value)
  );
}

function isBlockListView(value: unknown): value is Wire.BlockListView {
  return (
    isExactRecord(value, ["blocks", "omitted_before", "truncated"]) &&
    isArrayOf(value["blocks"], isRenderBlockView) &&
    isFiniteInteger(value["omitted_before"]) &&
    isBoolean(value["truncated"])
  );
}

function isAccountingTotalsView(
  value: unknown,
): value is Wire.AccountingTotalsView {
  return (
    isExactRecord(value, [
      "usage",
      "estimated_cost",
      "request_count",
      "long_context_request_count",
      "unpriced_request_count",
      "incomplete_usage_request_count",
    ]) &&
    isUsageView(value["usage"]) &&
    isNullable(value["estimated_cost"], isCostView) &&
    isString(value["request_count"]) &&
    isString(value["long_context_request_count"]) &&
    isString(value["unpriced_request_count"]) &&
    isString(value["incomplete_usage_request_count"])
  );
}

function isUsageSummaryView(value: unknown): value is Wire.UsageSummaryView {
  return (
    isExactRecord(value, ["latest_turn", "session"]) &&
    isAccountingTotalsView(value["latest_turn"]) &&
    isAccountingTotalsView(value["session"])
  );
}

function isActivityView(value: unknown): value is Wire.ActivityView {
  return (
    isExactRecord(value, [
      "kind",
      "fully_idle",
      "cancellable",
      "cancel_id",
      "progress_percent",
    ]) &&
    isCatalogKey(value["kind"], ACTIVITY_KINDS) &&
    isBoolean(value["fully_idle"]) &&
    isBoolean(value["cancellable"]) &&
    isNullable(value["cancel_id"], isString) &&
    value["cancellable"] === (value["cancel_id"] !== null) &&
    isNullable(
      value["progress_percent"],
      (item): item is number =>
        typeof item === "number" &&
        Number.isFinite(item) &&
        item >= 0 &&
        item <= 100,
    )
  );
}

function isApprovalDecisionSet(
  value: unknown,
  previewRedacted: unknown,
  previewTruncated: unknown,
  canViewOriginal: unknown,
): value is Array<Wire.ApprovalDecision> {
  if (
    !isArrayOf(
      value,
      (item): item is Wire.ApprovalDecision =>
        isCatalogKey(item, APPROVAL_DECISIONS),
    ) ||
    value.length === 0 ||
    new Set(value).size !== value.length ||
    !value.includes("deny")
  ) {
    return false;
  }
  return (
    (previewRedacted !== true && previewTruncated !== true) ||
    canViewOriginal === false
  );
}

function isPendingApprovalView(
  value: unknown,
): value is Wire.PendingApprovalView {
  return (
    isExactRecord(value, [
      "approval_id",
      "session_id",
      "tool_call_id",
      "tool_name",
      "description",
      "preview",
      "preview_redacted",
      "preview_truncated",
      "can_view_original",
      "allowed_decisions",
    ]) &&
    isString(value["approval_id"]) &&
    isCanonicalSessionId(value["session_id"]) &&
    isString(value["tool_call_id"]) &&
    isString(value["tool_name"]) &&
    isString(value["description"]) &&
    isString(value["preview"]) &&
    isBoolean(value["preview_redacted"]) &&
    isBoolean(value["preview_truncated"]) &&
    isBoolean(value["can_view_original"]) &&
    isApprovalDecisionSet(
      value["allowed_decisions"],
      value["preview_redacted"],
      value["preview_truncated"],
      value["can_view_original"],
    )
  );
}

function isQuestionOptionView(
  value: unknown,
): value is Wire.QuestionOptionView {
  return (
    isExactRecord(value, ["option_id", "label", "description"]) &&
    isString(value["option_id"]) &&
    isString(value["label"]) &&
    isNullable(value["description"], isString)
  );
}

function isQuestionPromptView(
  value: unknown,
): value is Wire.QuestionPromptView {
  return (
    isExactRecord(value, [
      "question_id",
      "prompt",
      "options",
      "multiple",
      "allows_other",
    ]) &&
    isString(value["question_id"]) &&
    isString(value["prompt"]) &&
    isArrayOf(value["options"], isQuestionOptionView) &&
    isBoolean(value["multiple"]) &&
    isBoolean(value["allows_other"])
  );
}

function isPendingQuestionView(
  value: unknown,
): value is Wire.PendingQuestionView {
  return (
    isExactRecord(value, [
      "form_id",
      "session_id",
      "tool_call_id",
      "questions",
      "content_truncated",
    ]) &&
    isString(value["form_id"]) &&
    isCanonicalSessionId(value["session_id"]) &&
    isString(value["tool_call_id"]) &&
    isArrayOf(value["questions"], isQuestionPromptView) &&
    isBoolean(value["content_truncated"])
  );
}

function isSessionHeaderView(value: unknown): value is Wire.SessionHeaderView {
  return (
    isExactRecord(value, ["session_id", "display_name", "fallback_label"]) &&
    isCanonicalSessionId(value["session_id"]) &&
    isNullable(value["display_name"], isString) &&
    isString(value["fallback_label"])
  );
}

function isSessionChoiceView(value: unknown): value is Wire.SessionChoiceView {
  return (
    isExactRecord(value, [
      "session_id",
      "display_name",
      "fallback_label",
      "selected",
    ]) &&
    isCanonicalSessionId(value["session_id"]) &&
    isNullable(value["display_name"], isString) &&
    isString(value["fallback_label"]) &&
    isBoolean(value["selected"])
  );
}

function isSessionListView(value: unknown): value is Wire.SessionListView {
  return (
    isExactRecord(value, ["sessions", "has_more"]) &&
    isArrayOf(value["sessions"], isSessionChoiceView) &&
    isBoolean(value["has_more"])
  );
}

function isModelChoiceView(value: unknown): value is Wire.ModelChoiceView {
  return (
    isExactRecord(value, [
      "model_id",
      "label",
      "model_name",
      "transport",
      "available",
      "selected",
    ]) &&
    isString(value["model_id"]) &&
    isString(value["label"]) &&
    isString(value["model_name"]) &&
    isCatalogKey(value["transport"], MODEL_TRANSPORTS) &&
    isBoolean(value["available"]) &&
    isBoolean(value["selected"])
  );
}

function isThemeColorsView(value: unknown): value is Wire.ThemeColorsView {
  return (
    isExactRecord(value, [
      "output_fg",
      "input_fg",
      "highlight_fg",
      "system_fg",
      "thought_fg",
      "tool_fg",
      "success_fg",
      "error_fg",
      "warning_fg",
      "json_key_fg",
      "json_val_fg",
      "input_bg",
      "thought_bg",
      "tool_bg",
      "terminal_bg",
    ]) &&
    Object.values(value).every(isString)
  );
}

function isThemeView(value: unknown): value is Wire.ThemeView {
  return (
    isExactRecord(value, ["theme_id", "name", "colors", "selected"]) &&
    isString(value["theme_id"]) &&
    isString(value["name"]) &&
    isThemeColorsView(value["colors"]) &&
    isBoolean(value["selected"])
  );
}

function isPythonContainerView(value: unknown): value is Wire.PythonContainerView {
  if (
    !(
      isExactRecord(value, ["kind", "name", "active"]) &&
      isCatalogKey(value["kind"], PYTHON_CONTAINER_KINDS) &&
      isString(value["name"]) &&
      isBoolean(value["active"])
    )
  ) {
    return false;
  }
  if (value["kind"] === "retained") {
    return /^lethetic-python-[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/u.test(
      value["name"],
    );
  }
  return (
    value["active"] &&
    /^lethetic-python-transient-[1-9][0-9]*-[0-9]+$/u.test(value["name"])
  );
}

function isPythonIsolationView(value: unknown): value is Wire.PythonIsolationView {
  if (
    !(
      isExactRecord(value, [
        "profile",
        "target",
        "backend",
        "network",
        "workspace_access",
        "grant_count",
        "policy_source",
        "container",
      ]) &&
      isCatalogKey(value["profile"], PYTHON_PROFILES) &&
      isNullable(
        value["target"],
        (item): item is Wire.PythonTargetView => isCatalogKey(item, PYTHON_TARGETS),
      ) &&
      isNullable(
        value["backend"],
        (item): item is Wire.PythonBackendView =>
          isCatalogKey(item, PYTHON_BACKENDS),
      ) &&
      isNullable(
        value["network"],
        (item): item is Wire.NetworkAccessView =>
          isCatalogKey(item, NETWORK_ACCESS),
      ) &&
      isNullable(
        value["workspace_access"],
        (item): item is Wire.WorkspaceAccessView =>
          isCatalogKey(item, WORKSPACE_ACCESS),
      ) &&
      isFiniteInteger(value["grant_count"]) &&
      value["grant_count"] >= 0 &&
      isCatalogKey(value["policy_source"], PYTHON_POLICY_SOURCES) &&
      isNullable(value["container"], isPythonContainerView)
    )
  ) {
    return false;
  }

  if (value["profile"] === "general") {
    return (
      value["target"] === null &&
      value["backend"] === null &&
      value["network"] === null &&
      value["workspace_access"] === null &&
      value["grant_count"] === 0 &&
      value["container"] === null
    );
  }
  if (value["target"] === "host") {
    return (
      value["backend"] === null &&
      value["network"] === null &&
      value["workspace_access"] === null &&
      value["grant_count"] === 0 &&
      value["container"] === null
    );
  }
  if (value["target"] !== "sandbox") {
    return (
      value["backend"] === null &&
      value["network"] === null &&
      value["workspace_access"] === null &&
      value["grant_count"] === 0 &&
      value["container"] === null
    );
  }
  return value["container"] === null || value["backend"] === "podman";
}

function isStatusView(value: unknown): value is Wire.StatusView {
  return (
    isExactRecord(value, [
      "stop_reason",
      "stop_reason_loss",
      "model_label",
      "provider_label",
      "provider_transport",
      "python",
      "tokens_per_second",
      "prompt_tokens_per_second",
      "context_tokens",
      "context_limit_tokens",
      "context_source",
      "request_usage",
      "memory_mebibytes",
      "file_count",
      "visible_block_count",
      "git_state",
      "tool_use",
      "background_tasks",
    ]) &&
    isString(value["stop_reason"]) &&
    isProjectionLossView(value["stop_reason_loss"]) &&
    isString(value["model_label"]) &&
    isString(value["provider_label"]) &&
    isCatalogKey(value["provider_transport"], MODEL_TRANSPORTS) &&
    isPythonIsolationView(value["python"]) &&
    isNullable(value["tokens_per_second"], isString) &&
    isNullable(value["prompt_tokens_per_second"], isString) &&
    isString(value["context_tokens"]) &&
    isString(value["context_limit_tokens"]) &&
    isCatalogKey(value["context_source"], CONTEXT_USAGE_SOURCES) &&
    isNullable(value["request_usage"], isUsageView) &&
    isString(value["memory_mebibytes"]) &&
    isFiniteInteger(value["file_count"]) &&
    value["file_count"] >= 0 &&
    isFiniteInteger(value["visible_block_count"]) &&
    value["visible_block_count"] >= 0 &&
    isCatalogKey(value["git_state"], GIT_STATES) &&
    isString(value["tool_use"]) &&
    isBackgroundTaskList(value["background_tasks"])
  );
}

function isDiagnosticView(value: unknown): value is Wire.DiagnosticView {
  return (
    isExactRecord(value, ["code", "severity", "message"]) &&
    isCatalogKey(value["code"], DIAGNOSTIC_CODES) &&
    isCatalogKey(value["severity"], DIAGNOSTIC_SEVERITIES) &&
    isString(value["message"])
  );
}

function isDebuggerView(value: unknown): value is Wire.DebuggerView {
  return (
    isExactRecord(value, ["open", "summary", "entries", "omitted_before"]) &&
    isBoolean(value["open"]) &&
    isString(value["summary"]) &&
    isArrayOf(value["entries"], isDiagnosticView) &&
    isFiniteInteger(value["omitted_before"]) &&
    value["omitted_before"] >= 0
  );
}

function isHistoryEntryView(value: unknown): value is Wire.HistoryEntryView {
  return (
    isExactRecord(value, ["entry_id", "label"]) &&
    isString(value["entry_id"]) &&
    isString(value["label"])
  );
}

function isFileChoiceView(value: unknown): value is Wire.FileChoiceView {
  return (
    isExactRecord(value, ["file_id", "label"]) &&
    isString(value["file_id"]) &&
    isString(value["label"])
  );
}

function isSystemPromptChoiceView(
  value: unknown,
): value is Wire.SystemPromptChoiceView {
  return (
    isExactRecord(value, ["prompt_id", "label", "selected"]) &&
    isString(value["prompt_id"]) &&
    isString(value["label"]) &&
    isBoolean(value["selected"])
  );
}

function isModeChoiceView(value: unknown): value is Wire.ModeChoiceView {
  return (
    isExactRecord(value, [
      "mode_id",
      "label",
      "selected",
      "enabled",
      "disabled_reason",
    ]) &&
    isString(value["mode_id"]) &&
    isString(value["label"]) &&
    isBoolean(value["selected"]) &&
    isBoolean(value["enabled"]) &&
    isNullable(value["disabled_reason"], isString)
  );
}

function isLspServerChoiceView(
  value: unknown,
): value is Wire.LspServerChoiceView {
  return (
    isExactRecord(value, ["server_id", "label", "state", "allowed_actions"]) &&
    isString(value["server_id"]) &&
    isString(value["label"]) &&
    isCatalogKey(value["state"], LSP_STATES) &&
    isArrayOf(
      value["allowed_actions"],
      (item): item is Wire.LspAction => isCatalogKey(item, LSP_ACTIONS),
    )
  );
}

function isShortcutView(value: unknown): value is Wire.ShortcutView {
  return (
    isExactRecord(value, ["keys", "label"]) &&
    isString(value["keys"]) &&
    isString(value["label"])
  );
}

function isConfirmationView(value: unknown): value is Wire.ConfirmationView {
  return (
    isExactRecord(value, [
      "confirmation_id",
      "action",
      "session_id",
      "title",
      "message",
    ]) &&
    isString(value["confirmation_id"]) &&
    isCatalogKey(value["action"], DESTRUCTIVE_ACTIONS) &&
    isNullable(value["session_id"], isCanonicalSessionId) &&
    isString(value["title"]) &&
    isString(value["message"])
  );
}

function isPanelDataView(value: unknown): value is Wire.PanelDataView {
  if (
    !isRecord(value) ||
    !isCatalogKey(value["type"], PANEL_DATA_TYPES)
  ) {
    return false;
  }
  switch (value["type"]) {
    case "hotkeys":
      return (
        isExactRecord(value, ["type", "shortcuts"]) &&
        isArrayOf(value["shortcuts"], isShortcutView)
      );
    case "input_history":
      return (
        isExactRecord(value, ["type", "entries", "has_more"]) &&
        isArrayOf(value["entries"], isHistoryEntryView) &&
        isBoolean(value["has_more"])
      );
    case "latest_files":
      return (
        isExactRecord(value, ["type", "files", "has_more"]) &&
        isArrayOf(value["files"], isFileChoiceView) &&
        isBoolean(value["has_more"])
      );
    case "system_prompts":
      return (
        isExactRecord(value, [
          "type",
          "prompts",
          "editor_content",
          "content_truncated",
        ]) &&
        isArrayOf(value["prompts"], isSystemPromptChoiceView) &&
        isNullable(value["editor_content"], isString) &&
        isBoolean(value["content_truncated"])
      );
    case "lsp_servers":
      return (
        isExactRecord(value, ["type", "servers"]) &&
        isArrayOf(value["servers"], isLspServerChoiceView)
      );
    case "loop_modes":
    case "agent_modes":
      return (
        isExactRecord(value, ["type", "modes"]) &&
        isArrayOf(value["modes"], isModeChoiceView)
      );
    case "name_session":
      return (
        isExactRecord(value, ["type", "current_name"]) &&
        isNullable(value["current_name"], isString)
      );
    case "confirmation":
      return (
        isExactRecord(value, ["type", "confirmation"]) &&
        isConfirmationView(value["confirmation"])
      );
    case "skills":
      return (
        isExactRecord(value, ["type", "skills", "catalog", "message"]) &&
        isArrayOf(value["skills"], isSkillChoiceView) &&
        isArrayOf(value["catalog"], isSkillCatalogView) &&
        isNullable(value["message"], isString)
      );
  }
}

function isOverlayView(value: unknown): value is Wire.OverlayView {
  return (
    isExactRecord(value, ["active_panel", "data"]) &&
    isNullable(
      value["active_panel"],
      (item): item is Wire.PanelId => isCatalogKey(item, PANEL_IDS),
    ) &&
    isNullable(value["data"], isPanelDataView)
  );
}

function expectedPanelForData(data: Wire.PanelDataView): Wire.PanelId {
  switch (data.type) {
    case "hotkeys":
      return "hotkeys";
    case "input_history":
      return "input_history";
    case "latest_files":
      return "latest_files";
    case "system_prompts":
      return "system_prompt";
    case "lsp_servers":
      return "lsp_servers";
    case "loop_modes":
      return "loop_detection";
    case "agent_modes":
      return "agent_mode";
    case "name_session":
      return "name_session";
    case "confirmation":
      return "confirmation";
    case "skills":
      return "skills";
    default:
      return assertNever(data, "panel data");
  }
}

function confirmationIdentityIsValid(
  confirmation: Wire.ConfirmationView,
  activeSessionId: string,
): boolean {
  switch (confirmation.action) {
    case "clear_context":
    case "delete_python_runtime":
    case "overwrite_system_prompt":
      return confirmation.session_id === activeSessionId;
    case "delete_session":
      return confirmation.session_id !== null;
    case "wipe_sessions":
    case "quit":
      return confirmation.session_id === null;
    default:
      return assertNever(confirmation.action, "confirmation action");
  }
}

function snapshotIdentityIsValid(snapshot: Wire.WebAppSnapshot): boolean {
  const activeSessionId = snapshot.session.session_id;
  if (
    snapshot.pending_approval?.session_id !== undefined &&
    snapshot.pending_approval.session_id !== activeSessionId
  ) {
    return false;
  }
  if (
    snapshot.pending_question?.session_id !== undefined &&
    snapshot.pending_question.session_id !== activeSessionId
  ) {
    return false;
  }
  if (snapshot.pending_approval !== null && snapshot.pending_question !== null) {
    return false;
  }
  if (
    (snapshot.pending_approval !== null &&
      snapshot.overlay.active_panel !== "tool_approval") ||
    (snapshot.pending_question !== null &&
      snapshot.overlay.active_panel !== "ask_user")
  ) {
    return false;
  }

  const sessionIds = snapshot.sessions.sessions.map((session) => session.session_id);
  if (new Set(sessionIds).size !== sessionIds.length) {
    return false;
  }
  const selectedSessions = snapshot.sessions.sessions.filter(
    (session) => session.selected,
  );
  if (
    selectedSessions.length > 1 ||
    selectedSessions.some((session) => session.session_id !== activeSessionId)
  ) {
    return false;
  }

  if (
    snapshot.commands.length !== COMMAND_ORDER.length ||
    snapshot.commands.some(
      (command, index) => command.id !== COMMAND_ORDER[index],
    )
  ) {
    return false;
  }

  const question = snapshot.pending_question;
  if (question !== null) {
    if (question.questions.length === 0) {
      return false;
    }
    const questionIds = question.questions.map((prompt) => prompt.question_id);
    if (new Set(questionIds).size !== questionIds.length) {
      return false;
    }
    for (const prompt of question.questions) {
      const optionIds = prompt.options.map((option) => option.option_id);
      if (
        new Set(optionIds).size !== optionIds.length ||
        (!prompt.allows_other && optionIds.length === 0)
      ) {
        return false;
      }
    }
  }

  const data = snapshot.overlay.data;
  if (
    data !== null &&
    snapshot.overlay.active_panel !== expectedPanelForData(data)
  ) {
    return false;
  }
  return (
    data?.type !== "confirmation" ||
    confirmationIdentityIsValid(data.confirmation, activeSessionId)
  );
}

function isWebAppSnapshot(value: unknown): value is Wire.WebAppSnapshot {
  if (
    !(
      isExactRecord(value, [
        "session",
        "blocks",
        "activity",
        "pending_approval",
        "pending_question",
        "commands",
        "sessions",
        "models",
        "themes",
        "usage",
        "status",
        "debugger",
        "overlay",
      ]) &&
      isSessionHeaderView(value["session"]) &&
      isBlockListView(value["blocks"]) &&
      isActivityView(value["activity"]) &&
      isNullable(value["pending_approval"], isPendingApprovalView) &&
      isNullable(value["pending_question"], isPendingQuestionView) &&
      isArrayOf(value["commands"], isCommandView) &&
      isSessionListView(value["sessions"]) &&
      isArrayOf(value["models"], isModelChoiceView) &&
      isArrayOf(value["themes"], isThemeView) &&
      isUsageSummaryView(value["usage"]) &&
      isStatusView(value["status"]) &&
      isDebuggerView(value["debugger"]) &&
      isOverlayView(value["overlay"])
    )
  ) {
    return false;
  }
  return snapshotIdentityIsValid(value as unknown as Wire.WebAppSnapshot);
}

function isStateChange(value: unknown): value is Wire.StateChange {
  if (
    !isExactRecord(value, ["type", "value"]) ||
    !isCatalogKey(value["type"], STATE_CHANGE_TYPES)
  ) {
    return false;
  }
  switch (value["type"]) {
    case "session":
      return isSessionHeaderView(value["value"]);
    case "blocks":
      return isBlockListView(value["value"]);
    case "activity":
      return isActivityView(value["value"]);
    case "pending_approval":
      return isNullable(value["value"], isPendingApprovalView);
    case "pending_question":
      return isNullable(value["value"], isPendingQuestionView);
    case "commands":
      return isArrayOf(value["value"], isCommandView);
    case "sessions":
      return isSessionListView(value["value"]);
    case "models":
      return isArrayOf(value["value"], isModelChoiceView);
    case "themes":
      return isArrayOf(value["value"], isThemeView);
    case "usage":
      return isUsageSummaryView(value["value"]);
    case "status":
      return isStatusView(value["value"]);
    case "debugger":
      return isDebuggerView(value["value"]);
    case "overlay":
      return isOverlayView(value["value"]);
  }
}

function isCapabilities(value: unknown): value is Wire.ProtocolCapabilities {
  return (
    isExactRecord(value, [
      "state_patches",
      "request_replay",
      "session_names",
      "exact_tool_approval",
      "read_only_files",
    ]) &&
    isBoolean(value["state_patches"]) &&
    isBoolean(value["request_replay"]) &&
    isBoolean(value["session_names"]) &&
    isBoolean(value["exact_tool_approval"]) &&
    isBoolean(value["read_only_files"])
  );
}

function isProtocolHello(value: unknown): value is Wire.IProtocolHello {
  return (
    isExactRecord(value, [
      "protocol_version",
      "minimum_protocol_version",
      "server_name",
      "sequence",
      "revision",
      "capabilities",
      "schema_sha256",
    ]) &&
    isString(value["schema_sha256"]) &&
    isFiniteInteger(value["protocol_version"]) &&
    isFiniteInteger(value["minimum_protocol_version"]) &&
    isString(value["server_name"]) &&
    isFiniteInteger(value["sequence"]) &&
    isFiniteInteger(value["revision"]) &&
    isCapabilities(value["capabilities"])
  );
}

function isCommandOutcome(value: unknown): value is Wire.CommandOutcome {
  if (!isRecord(value) || !isCatalogKey(value["type"], OUTCOME_TYPES)) {
    return false;
  }
  switch (value["type"]) {
    case "applied":
    case "snapshot_queued":
    case "sessions_wiped":
    case "shutting_down":
      return isExactRecord(value, ["type"]);
    case "panel_opened":
      return (
        isExactRecord(value, ["type", "panel"]) &&
        isCatalogKey(value["panel"], PANEL_IDS)
      );
    case "prompt_accepted":
    case "session_created":
    case "session_loaded":
    case "session_deleted":
      return (
        isExactRecord(value, ["type", "session_id"]) &&
        isCanonicalSessionId(value["session_id"])
      );
    case "history_entry_selected":
      return (
        isExactRecord(value, [
          "type",
          "session_id",
          "entry_id",
          "editor_content",
        ]) &&
        isCanonicalSessionId(value["session_id"]) &&
        isString(value["entry_id"]) &&
        isString(value["editor_content"])
      );
    case "tool_decision_recorded":
      return (
        isExactRecord(value, ["type", "tool_call_id"]) &&
        isString(value["tool_call_id"])
      );
    case "session_renamed":
      return (
        isExactRecord(value, ["type", "session_id", "display_name"]) &&
        isCanonicalSessionId(value["session_id"]) &&
        isNullable(value["display_name"], isString)
      );
  }
}

function isCommandError(value: unknown): value is Wire.CommandError {
  return (
    isExactRecord(value, ["code", "message", "current_revision", "retryable"]) &&
    isCatalogKey(value["code"], ERROR_CODES) &&
    isString(value["message"]) &&
    isNullable(value["current_revision"], isFiniteInteger) &&
    isBoolean(value["retryable"])
  );
}

function isCommandResponse(value: unknown): value is Wire.ICommandResponse {
  if (
    !isExactRecord(value, ["id", "result"]) ||
    !isString(value["id"]) ||
    !isRecord(value["result"])
  ) {
    return false;
  }
  const result = value["result"];
  if (result["status"] === "ok") {
    return (
      isExactRecord(result, ["status", "revision", "outcome"]) &&
      isFiniteInteger(result["revision"]) &&
      isCommandOutcome(result["outcome"])
    );
  }
  if (result["status"] === "error") {
    return (
      isExactRecord(result, ["status", "error"]) &&
      isCommandError(result["error"])
    );
  }
  return false;
}

function isStateSnapshot(value: unknown): value is Wire.IStateSnapshot {
  return (
    isExactRecord(value, ["protocol_version", "sequence", "revision", "state"]) &&
    isFiniteInteger(value["protocol_version"]) &&
    isFiniteInteger(value["sequence"]) &&
    isFiniteInteger(value["revision"]) &&
    isWebAppSnapshot(value["state"])
  );
}

function isStatePatch(value: unknown): value is Wire.IStatePatch {
  if (
    !isExactRecord(value, ["sequence", "base_revision", "revision", "changes"]) ||
    !isFiniteInteger(value["sequence"]) ||
    value["sequence"] === 0 ||
    !isFiniteInteger(value["base_revision"]) ||
    !isFiniteInteger(value["revision"]) ||
    value["revision"] <= value["base_revision"] ||
    !isArrayOf(value["changes"], isStateChange) ||
    value["changes"].length === 0
  ) {
    return false;
  }
  const sections = new Set(value["changes"].map((change) => change.type));
  return sections.size === value["changes"].length;
}

function isServerMessage(value: unknown): value is Wire.IServerMessage {
  if (!isRecord(value) || !isCatalogKey(value["type"], SERVER_MESSAGE_TYPES)) {
    return false;
  }
  switch (value["type"]) {
    case "hello":
      return (
        isExactRecord(value, ["type", "hello"]) &&
        isProtocolHello(value["hello"])
      );
    case "command_response":
      return (
        isExactRecord(value, ["type", "response"]) &&
        isCommandResponse(value["response"])
      );
    case "state_snapshot":
      return (
        isExactRecord(value, ["type", "snapshot"]) &&
        isStateSnapshot(value["snapshot"])
      );
    case "state_patch":
      return (
        isExactRecord(value, ["type", "patch"]) &&
        isStatePatch(value["patch"])
      );
  }
}

export type ParseServerMessageResult =
  | { readonly ok: true; readonly message: Wire.IServerMessage }
  | { readonly ok: false; readonly reason: string };

export function parseServerMessage(payload: unknown): ParseServerMessageResult {
  if (typeof payload !== "string") {
    return { ok: false, reason: "The server sent a non-text WebSocket frame." };
  }
  if (new TextEncoder().encode(payload).byteLength > MAX_WIRE_MESSAGE_BYTES) {
    return { ok: false, reason: "The server message exceeded the client limit." };
  }
  let decoded: unknown;
  try {
    decoded = JSON.parse(payload) as unknown;
  } catch {
    return { ok: false, reason: "The server sent malformed JSON." };
  }
  if (!isServerMessage(decoded)) {
    return {
      ok: false,
      reason: `The server message did not match protocol v${String(WFE_PROTOCOL_VERSION)}.`,
    };
  }
  return { ok: true, message: decoded };
}

export function commandResponseMatchesRequest(
  response: Wire.ICommandResponse,
  request: Wire.ICommandRequest,
): boolean {
  if (response.id !== request.id) {
    return false;
  }
  if (response.result.status === "error") {
    return true;
  }
  if (response.result.revision < request.expected_revision) {
    return false;
  }

  const outcome = response.result.outcome;
  switch (request.type) {
    case "invoke_command": {
      const expectedPanel: Wire.PanelId | null = (() => {
        switch (request.command_id) {
          case "hotkeys":
            return "hotkeys";
          case "themes":
            return "themes";
          case "input-history":
            return "input_history";
          case "loop-detection":
            return "loop_detection";
          case "system-prompt":
            return "system_prompt";
          case "sessions":
            return "sessions";
          case "name-session":
            return "name_session";
          case "latest-files":
            return "latest_files";
          case "models":
            return "models";
          case "lsp-servers":
            return "lsp_servers";
          case "agent-mode":
          case "agent-general":
          case "python-isolated":
          case "python-nonlocal":
          case "python-permissive":
            return "agent_mode";
          case "clear-ui":
          case "toggle-debugger":
          case "remote-control":
          case "toggle-todos":
          case "toggle-background-tasks":
          case "background-mode":
          case "tool-call-mode":
            return null;
          case "skills":
            return "skills";
          case "clear-context":
          case "delete-python-runtime":
          case "quit":
            return "confirmation";
          default:
            return assertNever(request.command_id, "invoked command");
        }
      })();
      if (
        request.command_id === "clear-context" ||
        request.command_id === "delete-python-runtime" ||
        request.command_id === "quit"
      ) {
        return false;
      }
      return expectedPanel === null
        ? outcome.type === "applied"
        : outcome.type === "panel_opened" && outcome.panel === expectedPanel;
    }
    case "send_prompt":
      return (
        outcome.type === "prompt_accepted" &&
        outcome.session_id === request.session_id
      );
    case "approve_tool_once":
    case "approve_tool_always":
    case "deny_tool":
    case "answer_user":
      return (
        outcome.type === "tool_decision_recorded" &&
        outcome.tool_call_id === request.tool_call_id
      );
    case "select_history_entry":
      return (
        outcome.type === "history_entry_selected" &&
        outcome.session_id === request.session_id &&
        outcome.entry_id === request.entry_id
      );
    case "rename_session":
      return (
        outcome.type === "session_renamed" &&
        outcome.session_id === request.session_id
      );
    case "new_session":
      return outcome.type === "session_created";
    case "resume_session":
      return (
        outcome.type === "session_loaded" &&
        outcome.session_id === request.session_id
      );
    case "delete_session":
      return (
        request.confirmed &&
        outcome.type === "session_deleted" &&
        outcome.session_id === request.session_id
      );
    case "wipe_sessions":
      return request.confirmed && outcome.type === "sessions_wiped";
    case "clear_context":
    case "delete_python_runtime":
      return request.confirmed && outcome.type === "applied";
    case "quit":
      return request.confirmed && outcome.type === "shutting_down";
    case "request_snapshot":
      return outcome.type === "snapshot_queued";
    case "stop":
    case "select_theme":
    case "select_model":
    case "select_latest_file":
    case "set_skill_enabled":
    case "install_skill":
    case "select_system_prompt":
    case "save_system_prompt":
    case "set_loop_detection":
    case "set_agent_mode":
    case "run_lsp_action":
    case "dismiss_overlay":
      return outcome.type === "applied";
    default:
      return assertNever(request, "command response request");
  }
}

export type RemotePhase =
  | "disconnected"
  | "awaiting_hello"
  | "awaiting_snapshot"
  | "live"
  | "stale"
  | "incompatible";

interface PatchHeader {
  readonly sequence: number;
  readonly baseRevision: number;
  readonly revision: number;
  readonly fingerprint: string;
}

export interface RemoteState {
  readonly phase: RemotePhase;
  readonly hello: Wire.IProtocolHello | null;
  readonly view: Wire.WebAppSnapshot | null;
  readonly sequence: number;
  readonly revision: number;
  readonly lastPatch: PatchHeader | null;
  readonly reason: string | null;
}

export type RemoteEffect = "none" | "request_snapshot" | "fatal";

export interface RemoteReduction {
  readonly state: RemoteState;
  readonly effect: RemoteEffect;
}

export function createRemoteState(): RemoteState {
  return {
    phase: "disconnected",
    hello: null,
    view: null,
    sequence: 0,
    revision: 0,
    lastPatch: null,
    reason: "Disconnected",
  };
}

export function beginRemoteConnection(previous: RemoteState): RemoteState {
  return {
    ...previous,
    phase: "awaiting_hello",
    hello: null,
    lastPatch: null,
    reason: "Waiting for the server protocol hello",
  };
}

export function disconnectRemote(
  previous: RemoteState,
  reason: string,
): RemoteState {
  if (previous.phase === "incompatible") {
    return previous;
  }
  return {
    ...previous,
    phase: "disconnected",
    hello: null,
    lastPatch: null,
    reason,
  };
}

function stale(previous: RemoteState, reason: string): RemoteReduction {
  return {
    state: { ...previous, phase: "stale", reason },
    effect: "request_snapshot",
  };
}

function incompatible(previous: RemoteState, reason: string): RemoteReduction {
  return {
    state: { ...previous, phase: "incompatible", reason },
    effect: "fatal",
  };
}

function helloProblem(hello: Wire.IProtocolHello): string | null {
  if (!clientContractGate()) {
    return "This client was built against an unsupported schema.";
  }
  if (
    hello.protocol_version !== WFE_PROTOCOL_VERSION ||
    hello.minimum_protocol_version !== WFE_PROTOCOL_VERSION ||
    hello.server_name !== "lethetic"
  ) {
    return "The server and browser protocol versions are incompatible.";
  }
  if (hello.schema_sha256 !== WFE_PROTOCOL_SCHEMA_SHA256) {
    return "This browser bundle was built for a different server build; reload the page.";
  }
  const capabilities = hello.capabilities;
  if (
    !capabilities.state_patches ||
    !capabilities.request_replay ||
    !capabilities.session_names ||
    !capabilities.exact_tool_approval
  ) {
    return "The server does not provide the required protocol capabilities.";
  }
  return null;
}

function sameHello(
  left: Wire.IProtocolHello,
  right: Wire.IProtocolHello,
): boolean {
  return (
    left.protocol_version === right.protocol_version &&
    left.minimum_protocol_version === right.minimum_protocol_version &&
    left.server_name === right.server_name &&
    left.schema_sha256 === right.schema_sha256 &&
    left.sequence === right.sequence &&
    left.revision === right.revision &&
    left.capabilities.state_patches === right.capabilities.state_patches &&
    left.capabilities.request_replay === right.capabilities.request_replay &&
    left.capabilities.session_names === right.capabilities.session_names &&
    left.capabilities.exact_tool_approval ===
      right.capabilities.exact_tool_approval &&
    left.capabilities.read_only_files === right.capabilities.read_only_files
  );
}

function canonicalJson(value: unknown): string {
  if (value === null || typeof value === "boolean" || typeof value === "number") {
    return JSON.stringify(value);
  }
  if (typeof value === "string") {
    return JSON.stringify(value);
  }
  if (Array.isArray(value)) {
    return `[${value.map((item) => canonicalJson(item)).join(",")}]`;
  }
  if (isRecord(value)) {
    return `{${Object.keys(value)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`)
      .join(",")}}`;
  }
  return "unsupported";
}

export function applyStateChange(
  current: Wire.WebAppSnapshot,
  change: Wire.StateChange,
): Wire.WebAppSnapshot {
  switch (change.type) {
    case "session":
      return { ...current, session: change.value };
    case "blocks":
      return { ...current, blocks: change.value };
    case "activity":
      return { ...current, activity: change.value };
    case "pending_approval":
      return { ...current, pending_approval: change.value };
    case "pending_question":
      return { ...current, pending_question: change.value };
    case "commands":
      return { ...current, commands: change.value };
    case "sessions":
      return { ...current, sessions: change.value };
    case "models":
      return { ...current, models: change.value };
    case "themes":
      return { ...current, themes: change.value };
    case "usage":
      return { ...current, usage: change.value };
    case "status":
      return { ...current, status: change.value };
    case "debugger":
      return { ...current, debugger: change.value };
    case "overlay":
      return { ...current, overlay: change.value };
    default:
      return assertNever(change, "state change");
  }
}

export function reduceServerMessage(
  previous: RemoteState,
  message: Wire.IServerMessage,
): RemoteReduction {
  switch (message.type) {
    case "hello": {
      const problem = helloProblem(message.hello);
      if (problem !== null) {
        return incompatible(previous, problem);
      }
      if (previous.phase !== "awaiting_hello") {
        if (previous.hello !== null && sameHello(previous.hello, message.hello)) {
          return { state: previous, effect: "none" };
        }
        return incompatible(previous, "The server sent an unexpected protocol hello.");
      }
      return {
        state: {
          ...previous,
          phase: "awaiting_snapshot",
          hello: message.hello,
          reason: "Waiting for the initial state snapshot",
        },
        effect: "none",
      };
    }
    case "command_response":
      return { state: previous, effect: "none" };
    case "state_snapshot": {
      const snapshot = message.snapshot;
      if (snapshot.protocol_version !== WFE_PROTOCOL_VERSION) {
        return incompatible(previous, "The snapshot protocol version is incompatible.");
      }
      if (previous.hello === null) {
        return incompatible(previous, "The server sent state before its protocol hello.");
      }
      if (previous.phase === "awaiting_snapshot") {
        if (
          snapshot.sequence < previous.hello.sequence ||
          snapshot.revision < previous.hello.revision
        ) {
          return stale(previous, "The initial snapshot lagged behind the server hello.");
        }
      } else if (previous.phase === "live" || previous.phase === "stale") {
        if (
          snapshot.sequence < previous.sequence ||
          snapshot.revision < previous.revision
        ) {
          if (
            snapshot.sequence <= previous.sequence &&
            snapshot.revision <= previous.revision
          ) {
            return { state: previous, effect: "none" };
          }
          return stale(previous, "The snapshot counters conflicted with local state.");
        }
      } else {
        return incompatible(previous, "The server sent a snapshot out of sequence.");
      }
      return {
        state: {
          phase: "live",
          hello: previous.hello,
          view: snapshot.state,
          sequence: snapshot.sequence,
          revision: snapshot.revision,
          lastPatch: null,
          reason: null,
        },
        effect: "none",
      };
    }
    case "state_patch": {
      const patch = message.patch;
      if (previous.phase === "stale") {
        return { state: previous, effect: "request_snapshot" };
      }
      if (previous.phase !== "live" || previous.view === null) {
        return stale(previous, "A patch arrived before a current snapshot.");
      }
      if (patch.sequence <= previous.sequence) {
        const duplicateLast =
          previous.lastPatch !== null &&
          previous.lastPatch.sequence === patch.sequence &&
          previous.lastPatch.baseRevision === patch.base_revision &&
          previous.lastPatch.revision === patch.revision &&
          previous.lastPatch.fingerprint === canonicalJson(patch.changes);
        if (
          duplicateLast ||
          (patch.sequence < previous.sequence && patch.revision <= previous.revision) ||
          (previous.lastPatch === null &&
            patch.sequence === previous.sequence &&
            patch.revision === previous.revision)
        ) {
          return { state: previous, effect: "none" };
        }
        return stale(previous, "A replayed patch conflicted with current state.");
      }
      if (patch.sequence !== previous.sequence + 1) {
        return stale(previous, "A state patch sequence gap was detected.");
      }
      if (patch.base_revision !== previous.revision) {
        return stale(previous, "A state patch used an unexpected base revision.");
      }
      let view = previous.view;
      for (const change of patch.changes) {
        view = applyStateChange(view, change);
      }
      if (!snapshotIdentityIsValid(view)) {
        return incompatible(
          previous,
          "A state patch violated protocol identity invariants.",
        );
      }
      return {
        state: {
          ...previous,
          phase: "live",
          view,
          sequence: patch.sequence,
          revision: patch.revision,
          lastPatch: {
            sequence: patch.sequence,
            baseRevision: patch.base_revision,
            revision: patch.revision,
            fingerprint: canonicalJson(patch.changes),
          },
          reason: null,
        },
        effect: "none",
      };
    }
    default:
      return assertNever(message, "server message");
  }
}
