import { COMMAND_ORDER, THEME_CATALOG } from "../../web/src/generated/contracts.js";
import type { AccountingTotalsView, CommandId, CommandView, RenderBlockView, WebAppSnapshot } from "../../web/src/generated/contracts.js";

export const NO_LOSS = { filtered: false, redacted: false, truncation: null } as const;
export function block(kind: RenderBlockView["kind"], content: string, title: string): RenderBlockView {
  return { kind, content, title, content_loss: NO_LOSS, title_loss: NO_LOSS,
    tool: null, success: null, usage: null, estimated_cost: null, duration_label: null };
}
function accounting(): AccountingTotalsView {
  return { usage: { uncached_input_tokens: "0", cache_read_input_tokens: "0", cache_creation_input_tokens: "0",
    output_tokens: "0", total_input_tokens: "0", total_tokens: "0", breakdown_complete: false },
    estimated_cost: null, request_count: "0", long_context_request_count: "0", unpriced_request_count: "0",
    incomplete_usage_request_count: "0" };
}
const COMMANDS: [CommandId, string, CommandView["behavior"], string | null][] = [
  ["loop-detection", "Loop detection", "open_panel", null],
  ["hotkeys", "Hotkeys", "open_panel", "h"], ["themes", "Themes", "open_panel", "t"],
  ["models", "Models / saved connections", "open_panel", null],
  ["input-history", "Input history", "open_panel", null], ["system-prompt", "System prompt", "open_panel", null],
  ["clear-ui", "Clear UI", "execute", "c"], ["clear-context", "New chat / clear context", "confirm", null],
  ["toggle-debugger", "Debugger", "execute", "d"], ["name-session", "Name session", "open_panel", null],
  ["agent-mode", "Agent mode", "open_panel", null], ["agent-general", "Chat mode", "execute", null],
  ["python-permissive", "Python mode (Pyodide)", "execute", null],
  ["delete-python-runtime", "Reset Python session", "confirm", null],
  ["sessions", "Sessions", "open_panel", null],
];

export function initialState(): WebAppSnapshot {
  const session_id = crypto.randomUUID();
  return {
    session: { session_id, display_name: null, fallback_label: "Browser session" },
    blocks: { blocks: [block("markdown", "Connect a model using **Connections**. Save code in **Files** and reopen chats in **Sessions**.", "Lethetic")], omitted_before: 0, truncated: false },
    activity: { kind: "idle", fully_idle: true, cancellable: false, cancel_id: null, progress_percent: null },
    pending_approval: null, pending_question: null,
    commands: COMMAND_ORDER.map((id) => {
      const supported = COMMANDS.find((command) => command[0] === id);
      const label = supported?.[1] ?? id.replaceAll("-", " ");
      return { id, label, behavior: supported?.[2] ?? "execute", accelerator: supported?.[3] ?? null,
        icon: id === "themes" ? "theme" : id === "models" ? "model" : "command",
        enabled: supported !== undefined, disabled_reason: supported ? null : "Requires the native runtime",
        description: label };
    }),
    sessions: { sessions: [], has_more: false }, models: [],
    themes: THEME_CATALOG.map((theme) => ({ ...theme, colors: { ...theme.colors }, selected: theme.theme_id === "dracula" })),
    usage: { latest_turn: accounting(), session: accounting() },
    status: { stop_reason: "Ready", stop_reason_loss: NO_LOSS, model_label: "No model", provider_label: "Browser WASM",
      provider_transport: "open_ai_chat_completions", python: { profile: "general", target: null, backend: null,
        network: null, workspace_access: null, grant_count: 0, policy_source: "one_time", container: null },
      tokens_per_second: null, prompt_tokens_per_second: null, context_tokens: "0", context_limit_tokens: "0",
      context_source: "local_estimate", request_usage: null, memory_mebibytes: "0", file_count: 0,
      visible_block_count: 1, git_state: "unknown", tool_use: "", background_tasks: [],
      engine_time: "0s", tool_time: "0s", idle_time: "0s" },
    debugger: { open: false, summary: "Rust/WASM browser runtime", entries: [], omitted_before: 0 },
    overlay: { active_panel: null, data: null },
  };
}
