import type {
  ActivityKind,
  ApprovalDecision,
  CommandId,
  CommandView,
  ConfirmationView,
  DestructiveActionView,
  LspAction,
  PanelDataView,
  PanelId,
  PendingApprovalView,
  ProjectionLossView,
  RenderBlockView,
  ThemeColorsView,
  ThemeView,
  UiIconId,
  WebAppSnapshot,
  WebCommand,
} from "../generated/contracts.js";
import {
  COMMAND_ORDER,
  THEME_CATALOG,
} from "../generated/contracts.js";
import type {
  ChatFollowScheduler,
  ChatScrollMetrics,
} from "../chat-follow.js";
import type { ActivitySpinnerKind } from "../icons.js";
import { MAX_JSON_SEGMENTS, inspectJson } from "../json.js";
import { isMarkdownBlockKind } from "../markdown.js";
import type { RemoteState } from "../protocol.js";
import { assertNever, isSafeCssColor } from "../safety.js";

export const CHAT_WINDOW_SIZE = 180;
export const CHAT_OVERSCAN = 24;
export const CHAT_ROW_ESTIMATE = 112;
export const MAX_DRAFT_LENGTH = 1_000_000;
export const MAX_PROMPT_BYTES = 128 * 1024;
export const MAX_SYSTEM_PROMPT_NAME_BYTES = 256;
export const MAX_SYSTEM_PROMPT_BYTES = 256 * 1024;
export const MAX_ANSWER_BYTES = 64 * 1024;

const UTF8_ENCODER = new TextEncoder();

export const CHAT_FOLLOW_SCHEDULER: ChatFollowScheduler = {
  now: () => globalThis.performance.now(),
  setTimeout: (callback, delayMilliseconds) =>
    globalThis.setTimeout(callback, delayMilliseconds),
  clearTimeout: (handle) => globalThis.clearTimeout(handle),
  requestAnimationFrame: (callback) =>
    globalThis.requestAnimationFrame(() => callback()),
  cancelAnimationFrame: (handle) => globalThis.cancelAnimationFrame(handle),
};

export const PANEL_TITLES = {
  command_palette: "Command palette",
  hotkeys: "Hotkeys",
  themes: "Themes",
  input_history: "Input history",
  loop_detection: "Loop detection",
  system_prompt: "System prompts",
  sessions: "Sessions",
  name_session: "Name session",
  latest_files: "Latest files",
  models: "Models",
  lsp_servers: "LSP servers",
  agent_mode: "Agent mode",
  tool_approval: "Tool approval",
  ask_user: "Question",
  confirmation: "Confirmation",
} as const satisfies Readonly<Record<PanelId, string>>;

export const THEME_VARIABLES = {
  output_fg: "--theme-output-fg",
  input_fg: "--theme-input-fg",
  highlight_fg: "--theme-highlight-fg",
  system_fg: "--theme-system-fg",
  thought_fg: "--theme-thought-fg",
  tool_fg: "--theme-tool-fg",
  success_fg: "--theme-success-fg",
  error_fg: "--theme-error-fg",
  warning_fg: "--theme-warning-fg",
  json_key_fg: "--theme-json-key-fg",
  json_val_fg: "--theme-json-val-fg",
  input_bg: "--theme-input-bg",
  thought_bg: "--theme-thought-bg",
  tool_bg: "--theme-tool-bg",
  terminal_bg: "--theme-terminal-bg",
} as const satisfies Readonly<Record<keyof ThemeColorsView, string>>;

export type ConfirmableType =
  | "delete_session"
  | "wipe_sessions"
  | "save_system_prompt"
  | "clear_context"
  | "delete_python_runtime"
  | "quit";
export type ConfirmableCommand = Extract<WebCommand, { type: ConfirmableType }>;
export type AffirmativeApprovalDecision = Exclude<ApprovalDecision, "deny">;

export function chatScrollMetrics(element: HTMLElement): ChatScrollMetrics {
  return {
    scrollTop: element.scrollTop,
    scrollHeight: element.scrollHeight,
    clientHeight: element.clientHeight,
  };
}

export function isChatScrollKey(event: KeyboardEvent): boolean {
  if (event.altKey || event.ctrlKey || event.metaKey) {
    return false;
  }
  return [
    "ArrowDown",
    "ArrowUp",
    "End",
    "Home",
    "PageDown",
    "PageUp",
    " ",
  ].includes(event.key);
}

export function utf8Length(value: string): number {
  return UTF8_ENCODER.encode(value).byteLength;
}

export function inputValue(event: Event): string {
  const target = event.currentTarget;
  if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement) {
    return target.value;
  }
  return "";
}

export function isEditableTarget(target: EventTarget | null): boolean {
  return (
    target instanceof HTMLInputElement ||
    target instanceof HTMLTextAreaElement ||
    target instanceof HTMLSelectElement ||
    (target instanceof HTMLElement && target.isContentEditable)
  );
}

export function sessionLabel(snapshot: WebAppSnapshot): string {
  return snapshot.session.display_name ?? snapshot.session.fallback_label;
}

export function choiceLabel(choice: {
  readonly display_name: string | null;
  readonly fallback_label: string;
}): string {
  return choice.display_name ?? choice.fallback_label;
}

export function activityLabel(kind: ActivityKind): string {
  switch (kind) {
    case "idle":
      return "Idle";
    case "loading_session":
      return "Loading session";
    case "awaiting_approval":
      return "Awaiting tool approval";
    case "executing_tool":
      return "Executing tool";
    case "awaiting_answer":
      return "Awaiting your answer";
    case "processing":
      return "Processing";
    case "managing_lsp":
      return "Managing LSP server";
    default:
      return assertNever(kind, "activity kind");
  }
}

export function activitySpinnerKind(
  kind: ActivityKind,
  live: boolean,
): ActivitySpinnerKind | null {
  if (!live) {
    return null;
  }
  switch (kind) {
    case "processing":
      return "regular";
    case "executing_tool":
    case "managing_lsp":
      return "tool";
    case "idle":
    case "loading_session":
    case "awaiting_approval":
    case "awaiting_answer":
      return null;
    default:
      return assertNever(kind, "activity spinner kind");
  }
}

export function blockLabel(block: RenderBlockView): string {
  switch (block.kind) {
    case "text":
      return "Assistant";
    case "user":
      return "You";
    case "thought":
      return "Thought";
    case "markdown":
      return "Assistant";
    case "tool_call":
      return block.tool?.tool_name ?? "Tool call";
    case "tool_result":
      return block.tool?.tool_name ?? "Tool result";
    case "divider":
      return "Divider";
    case "formulating":
      return "Formulating";
    case "truncation":
      return "Truncated content";
    default:
      return assertNever(block.kind, "block kind");
  }
}

export function lspActionLabel(action: LspAction): string {
  switch (action) {
    case "install":
      return "Install";
    case "enable":
      return "Enable";
    case "disable":
      return "Disable";
    case "cancel_install":
      return "Cancel install";
    default:
      return assertNever(action, "LSP action");
  }
}

export function lspActionIcon(action: LspAction): UiIconId {
  switch (action) {
    case "install":
      return "download";
    case "enable":
      return "check";
    case "disable":
      return "power";
    case "cancel_install":
      return "stop";
    default:
      return assertNever(action, "LSP action icon");
  }
}

export function approvalLabel(decision: ApprovalDecision): string {
  switch (decision) {
    case "approve_once":
      return "Approve once";
    case "approve_always":
      return "Always allow tools";
    case "deny":
      return "Deny";
    default:
      return assertNever(decision, "approval decision");
  }
}

export function approvalIcon(decision: ApprovalDecision): UiIconId {
  switch (decision) {
    case "approve_once":
      return "check";
    case "approve_always":
      return "shield-check";
    case "deny":
      return "close";
    default:
      return assertNever(decision, "approval icon");
  }
}

export function panelData<T extends PanelDataView["type"]>(
  snapshot: WebAppSnapshot,
  type: T,
): Extract<PanelDataView, { type: T }> | null {
  const data = snapshot.overlay.data;
  if (data?.type !== type) {
    return null;
  }
  return data as Extract<PanelDataView, { type: T }>;
}

export function commandForId(
  snapshot: WebAppSnapshot,
  id: CommandId,
): CommandView | null {
  return snapshot.commands.find((command) => command.id === id) ?? null;
}

export function orderedCommands(snapshot: WebAppSnapshot): CommandView[] {
  const commands: CommandView[] = [];
  for (const id of COMMAND_ORDER) {
    const command = commandForId(snapshot, id);
    if (command !== null) {
      commands.push(command);
    }
  }
  return commands;
}

export function filteredCommands(
  snapshot: WebAppSnapshot,
  query: string,
): CommandView[] {
  const normalized = query.trim().toLocaleLowerCase();
  return orderedCommands(snapshot).filter((command) =>
    `${command.label} ${command.id}`.toLocaleLowerCase().includes(normalized),
  );
}

export function orderedThemes(snapshot: WebAppSnapshot): ThemeView[] {
  const runtime = new Map(snapshot.themes.map((theme) => [theme.theme_id, theme]));
  const themes: ThemeView[] = [];
  for (const catalogTheme of THEME_CATALOG) {
    themes.push(runtime.get(catalogTheme.theme_id) ?? catalogTheme);
    runtime.delete(catalogTheme.theme_id);
  }
  for (const theme of snapshot.themes) {
    if (runtime.has(theme.theme_id)) {
      themes.push(theme);
      runtime.delete(theme.theme_id);
    }
  }
  return themes;
}

export function safeThemeColor(
  value: string,
  fallbackKey: keyof ThemeColorsView,
): string {
  return isSafeCssColor(value) ? value : THEME_CATALOG[0].colors[fallbackKey];
}

export function snapshotOverlaySignature(
  snapshot: WebAppSnapshot | null,
): string | null {
  const overlay = snapshot?.overlay;
  if (overlay === undefined || snapshot === null) {
    return null;
  }
  const panel = overlay.active_panel;
  if (panel === null) {
    return null;
  }
  if (panel === "confirmation") {
    const data = overlay.data;
    return data?.type === "confirmation"
      ? `${panel}:${data.confirmation.confirmation_id}`
      : `${panel}:missing`;
  }
  if (panel === "tool_approval") {
    return `${panel}:${snapshot.pending_approval?.approval_id ?? "missing"}`;
  }
  if (panel === "ask_user") {
    return `${panel}:${snapshot.pending_question?.form_id ?? "missing"}`;
  }
  return `${panel}:${overlay.data?.type ?? "none"}`;
}

export function overlaySignature(remote: RemoteState): string | null {
  return snapshotOverlaySignature(remote.view);
}

export function approvalHasHiddenContent(
  approval: Pick<PendingApprovalView, "preview_redacted" | "preview_truncated">,
): boolean {
  return approval.preview_redacted || approval.preview_truncated;
}

export function approvalUsesJsonHighlighting(
  approval: Pick<PendingApprovalView, "preview_truncated" | "tool_name">,
): boolean {
  return approval.tool_name !== "python" && !approval.preview_truncated;
}

export function projectionLossIsTruncated(
  loss: ProjectionLossView,
): boolean {
  return loss.truncation !== null;
}

export function projectionLossIsLossy(loss: ProjectionLossView): boolean {
  return loss.filtered || loss.redacted || loss.truncation !== null;
}

export function projectionLossMessages(
  losses: readonly ProjectionLossView[],
): string[] {
  return [
    losses.some((loss) => loss.filtered)
      ? "WFE intentionally omitted fields outside this browser-safe projection."
      : null,
    losses.some((loss) => loss.redacted)
      ? "WFE replaced sensitive values before browser delivery."
      : null,
    losses.some((loss) => loss.truncation === "size_limit")
      ? "WFE shortened this value to a fixed browser limit; the omitted tail is not shown."
      : null,
    losses.some((loss) => loss.truncation === "invalid_source")
      ? "WFE replaced an invalid or incomplete value with a safe projection."
      : null,
  ].filter((message): message is string => message !== null);
}

export function toolBlockUsesJsonHighlighting(
  block: Pick<RenderBlockView, "tool">,
): boolean {
  return (
    block.tool !== null && !projectionLossIsTruncated(block.tool.payload_loss)
  );
}

export function toolResultUsesAutoRendering(
  block: Pick<RenderBlockView, "kind" | "tool">,
): boolean {
  return block.kind === "tool_result" && block.tool?.kind === "result";
}

export function blockContentIsLossy(
  block: Pick<RenderBlockView, "content_loss" | "tool">,
): boolean {
  return (
    projectionLossIsLossy(block.content_loss) ||
    (block.tool !== null && projectionLossIsLossy(block.tool.payload_loss))
  );
}

export function markdownBlockUsesJsonHighlighting(
  block: Pick<RenderBlockView, "content_loss" | "tool">,
): boolean {
  return (
    !projectionLossIsTruncated(block.content_loss) &&
    (block.tool === null || !projectionLossIsTruncated(block.tool.payload_loss))
  );
}

export function chatJsonSegmentAllocations(
  blocks: readonly RenderBlockView[],
): number[] {
  const allocations = blocks.map(() => 0);
  const inspections = blocks.map((block) => {
    if (
      block.tool === null ||
      projectionLossIsTruncated(block.tool.payload_loss)
    ) {
      return null;
    }
    return inspectJson(block.tool.payload);
  });
  let remaining = MAX_JSON_SEGMENTS;

  const allocateExact = (kind: "call" | "result"): void => {
    for (let index = blocks.length - 1; index >= 0; index -= 1) {
      const block = blocks[index];
      const inspection = inspections[index];
      if (
        block?.tool?.kind !== kind ||
        inspection?.status !== "valid" ||
        inspection.segments > remaining
      ) {
        continue;
      }
      allocations[index] = inspection.segments;
      remaining -= inspection.segments;
    }
  };

  allocateExact("call");
  allocateExact("result");

  const markdownCandidates: number[] = [];
  for (let index = 0; index < blocks.length; index += 1) {
    const block = blocks[index];
    if (block === undefined) {
      continue;
    }
    if (
      isMarkdownBlockKind(block.kind) &&
      !projectionLossIsTruncated(block.content_loss)
    ) {
      markdownCandidates.push(index);
      continue;
    }
    if (
      block.kind === "tool_result" &&
      block.tool?.kind === "result" &&
      !projectionLossIsTruncated(block.tool.payload_loss) &&
      inspections[index]?.status === "invalid"
    ) {
      markdownCandidates.push(index);
    }
  }

  if (remaining === 0 || markdownCandidates.length === 0) {
    return allocations;
  }
  const share = Math.floor(remaining / markdownCandidates.length);
  let extra = remaining % markdownCandidates.length;
  for (let offset = markdownCandidates.length - 1; offset >= 0; offset -= 1) {
    const index = markdownCandidates[offset];
    if (index === undefined) {
      continue;
    }
    const allocation = share + (extra > 0 ? 1 : 0);
    allocations[index] = allocation;
    if (extra > 0) {
      extra -= 1;
    }
  }
  return allocations;
}

export function approvalConfirmationKey(
  revision: number,
  approval: PendingApprovalView,
): string {
  return JSON.stringify([
    revision,
    approval.session_id,
    approval.approval_id,
    approval.tool_call_id,
    approval.preview,
    approval.preview_redacted,
    approval.preview_truncated,
    approval.allowed_decisions,
  ]);
}

export function panelInstanceKey(
  panel: PanelId,
  snapshot: WebAppSnapshot | null,
): string {
  if (snapshot === null) {
    return panel;
  }
  if (panel === "tool_approval" && snapshot.pending_approval !== null) {
    const approval = snapshot.pending_approval;
    return JSON.stringify([
      panel,
      approval.session_id,
      approval.approval_id,
      approval.tool_call_id,
    ]);
  }
  if (panel === "ask_user" && snapshot.pending_question !== null) {
    const question = snapshot.pending_question;
    return JSON.stringify([
      panel,
      question.session_id,
      question.form_id,
      question.tool_call_id,
    ]);
  }
  if (
    panel === "confirmation" &&
    snapshot.overlay.data?.type === "confirmation"
  ) {
    return JSON.stringify([
      panel,
      snapshot.overlay.data.confirmation.confirmation_id,
    ]);
  }
  return panel;
}

export function hiddenApprovalConfirmationMessage(
  decision: AffirmativeApprovalDecision,
  approval: Pick<PendingApprovalView, "preview_redacted" | "preview_truncated">,
): string {
  const hidden = [
    approval.preview_redacted ? "sensitive values were replaced" : null,
    approval.preview_truncated ? "the preview has an unseen tail" : null,
  ]
    .filter((item): item is string => item !== null)
    .join(" and ");
  const scope =
    decision === "approve_always"
      ? "Allow this exact current call and all later tool calls under the current execution policy without another preview"
      : "Approve this exact current call once";
  return `${scope} even though ${hidden}.`;
}

export function confirmationActionFor(
  command: ConfirmableCommand,
): DestructiveActionView {
  switch (command.type) {
    case "delete_session":
      return "delete_session";
    case "wipe_sessions":
      return "wipe_sessions";
    case "save_system_prompt":
      return "overwrite_system_prompt";
    case "clear_context":
      return "clear_context";
    case "delete_python_runtime":
      return "delete_python_runtime";
    case "quit":
      return "quit";
    default:
      return assertNever(command, "confirmable command");
  }
}

export function confirmationMatches(
  command: ConfirmableCommand | null,
  confirmation: ConfirmationView,
): boolean {
  if (command === null || confirmationActionFor(command) !== confirmation.action) {
    return false;
  }
  switch (command.type) {
    case "delete_session":
    case "save_system_prompt":
    case "clear_context":
    case "delete_python_runtime":
      return confirmation.session_id === command.session_id;
    case "wipe_sessions":
    case "quit":
      return confirmation.session_id === null;
    default:
      return assertNever(command, "confirmation match");
  }
}

export function confirmedCommand(
  command: ConfirmableCommand,
  confirmationId: string,
): ConfirmableCommand {
  switch (command.type) {
    case "delete_session":
      return { ...command, confirmed: true, confirmation_id: confirmationId };
    case "wipe_sessions":
      return { ...command, confirmed: true, confirmation_id: confirmationId };
    case "save_system_prompt":
      return {
        ...command,
        confirmed_overwrite: true,
        confirmation_id: confirmationId,
      };
    case "clear_context":
      return { ...command, confirmed: true, confirmation_id: confirmationId };
    case "delete_python_runtime":
      return { ...command, confirmed: true, confirmation_id: confirmationId };
    case "quit":
      return { ...command, confirmed: true, confirmation_id: confirmationId };
    default:
      return assertNever(command, "confirmation payload");
  }
}

export function chatWindowStartForScroll(
  count: number,
  scrollTop: number,
  following: boolean,
): number {
  if (following) {
    return Math.max(0, count - CHAT_WINDOW_SIZE);
  }
  return Math.max(
    0,
    Math.min(
      Math.floor(scrollTop / CHAT_ROW_ESTIMATE) - CHAT_OVERSCAN,
      Math.max(0, count - CHAT_WINDOW_SIZE),
    ),
  );
}

export function rebaseChatWindowStart(
  start: number,
  previous: WebAppSnapshot,
  current: WebAppSnapshot,
): number {
  const previousPrefix =
    previous.blocks.omitted_before - (previous.blocks.truncated ? 1 : 0);
  const currentPrefix =
    current.blocks.omitted_before - (current.blocks.truncated ? 1 : 0);
  const rebased = start + previousPrefix - currentPrefix;
  return Math.max(
    0,
    Math.min(rebased, Math.max(0, current.blocks.blocks.length - 1)),
  );
}

export type OverlayFocusTransition = "none" | "opened" | "closed";

export function overlayFocusTransition(
  previousKey: string | null,
  currentKey: string | null,
): OverlayFocusTransition {
  if (currentKey !== null && currentKey !== previousKey) {
    return "opened";
  }
  if (currentKey === null && previousKey !== null) {
    return "closed";
  }
  return "none";
}
