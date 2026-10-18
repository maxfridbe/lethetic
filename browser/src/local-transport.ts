import { WFE_PROTOCOL_SCHEMA_SHA256, WFE_PROTOCOL_VERSION } from "../../web/src/generated/contracts.js";
import type { CommandOutcome, ICommandRequest, PanelId, WebAppSnapshot, WebCommand } from "../../web/src/generated/contracts.js";
import type { ApplicationTransport, BrowserTransportEvents } from "../../web/src/transport.js";
import { createRuntime, DEFAULT_PROMPT } from "./sdk.js";
import type { Connection, LetheticRuntime, RuntimeEvent, Tool } from "./sdk.js";
import { createPythonTool } from "./python.js";
import type { PythonTool } from "./python.js";
import { block, initialState, NO_LOSS } from "./local-state.js";

export interface LocalOptions {
  connections: Connection[];
  python?: boolean;
  tools?: Tool[];
  systemPrompt?: string;
  pyodideIndexURL?: string;
  onConnectionChanged?: (index: number) => void;
}

/** Drives the existing remote-mode UI using the same typed commands and snapshots. */
export class LocalTransport implements ApplicationTransport {
  readonly state: WebAppSnapshot = initialState();
  pendingCount = 0;
  lastSendFailure: string | null = null;
  selected = 0;
  pythonEnabled: boolean;
  #revision = 0;
  #closed = false;
  #runtime: LetheticRuntime | null = null;
  #python: PythonTool | null = null;
  #operation: AbortController | null = null;
  #history: string[] = [];
  #system: string;
  #requestCount = 0;

  constructor(readonly events: BrowserTransportEvents, readonly options: LocalOptions) {
    this.pythonEnabled = options.python ?? false;
    this.#system = options.systemPrompt ?? DEFAULT_PROMPT;
  }

  get busy(): boolean { return this.#operation !== null; }
  get connection(): Connection | undefined { return this.options.connections[this.selected]; }

  start(): void {
    this.events.socketOpened();
    this.events.serverMessage({ type: "hello", hello: { protocol_version: WFE_PROTOCOL_VERSION,
      minimum_protocol_version: WFE_PROTOCOL_VERSION, server_name: "lethetic", sequence: 0, revision: 0,
      schema_sha256: WFE_PROTOCOL_SCHEMA_SHA256, capabilities: { state_patches: true, request_replay: true,
        session_names: true, exact_tool_approval: true, read_only_files: false } } });
    this.publish();
  }

  publish(): void {
    if (this.#closed) return;
    this.#revision++;
    const connection = this.connection;
    this.state.models = this.options.connections.map((item, index) => ({ model_id: String(index), label: item.name,
      model_name: item.model, transport: item.provider === "openai" ? "open_ai_chat_completions" : "claude_code_proxy",
      available: true, selected: index === this.selected }));
    this.state.status.model_label = connection?.model ?? "No model";
    this.state.status.provider_label = connection?.name ?? "Configure a connection";
    this.state.status.provider_transport = connection?.provider === "anthropic" ? "claude_code_proxy" : "open_ai_chat_completions";
    this.state.status.python.profile = this.pythonEnabled ? "python_only" : "general";
    this.state.status.tool_use = this.pythonEnabled ? "Pyodide · browser worker" : "Browser tools";
    this.state.status.visible_block_count = this.state.blocks.blocks.length;
    this.events.serverMessage({ type: "state_snapshot", snapshot: { protocol_version: WFE_PROTOCOL_VERSION,
      revision: this.#revision, sequence: this.#revision, state: structuredClone(this.state) } });
  }

  setConnections(connections: Connection[], selected: number): void {
    this.#requireIdle();
    this.options.connections = connections;
    this.selected = selected;
    this.reset();
    this.publish();
  }

  reset(): void {
    this.#requireIdle();
    this.#runtime?.dispose(); this.#runtime = null;
    this.#python?.reset(); this.#python = null;
    this.state.blocks = { blocks: [], omitted_before: 0, truncated: false };
    this.state.status.stop_reason = "Ready";
    this.state.overlay = { active_panel: null, data: null };
  }

  send(command: WebCommand, expectedRevision: number): ICommandRequest | null {
    if (this.#closed) return null;
    const request = { ...command, id: crypto.randomUUID(), expected_revision: expectedRevision };
    this.pendingCount++;
    queueMicrotask(() => {
      if (this.#closed) return;
      this.pendingCount--;
      try {
        const outcome = this.#execute(command);
        this.publish();
        this.events.commandResponse({ id: request.id, result: { status: "ok", revision: this.#revision, outcome } }, request);
      } catch (error) {
        this.publish();
        this.events.commandResponse({ id: request.id, result: { status: "error", error: {
          code: this.state.overlay.active_panel === "confirmation" ? "confirmation_required" : "bad_request",
          message: error instanceof Error ? error.message : String(error), current_revision: null, retryable: false } } }, request);
      }
    });
    return request;
  }

  requestSnapshot(revision: number): ICommandRequest | null { return this.send({ type: "request_snapshot" }, revision); }
  noteHello(): void { /* No network handshake is required for the local runtime. */ }
  markSynchronized(): void {
    this.events.transportStatus({ phase: "live", label: "Browser · WASM", attempt: 0, retryInMilliseconds: null });
  }
  markStale(): void { /* The next synchronous snapshot restores state. */ }
  async requestFile(): Promise<Response> { throw new Error("Native files are unavailable in browser mode."); }
  closePermanently(): void { this.dispose(); }
  dispose(): void {
    this.#closed = true; this.#operation?.abort(); this.#runtime?.dispose(); this.#python?.reset();
  }

  #requireIdle(): void { if (this.busy) throw new Error("Stop the current turn first."); }

  #execute(command: WebCommand): CommandOutcome {
    if ("session_id" in command && command.session_id !== this.state.session.session_id) throw new Error("Session changed.");
    switch (command.type) {
      case "send_prompt":
        this.#requireIdle();
        if (!this.connection) throw new Error("Open Connections and save a model endpoint first.");
        if (!command.prompt.trim()) throw new Error("Enter a prompt.");
        this.#operation = new AbortController();
        this.#history.push(command.prompt);
        this.state.blocks.blocks.push(block("user", command.prompt, "You"));
        this.state.activity = { kind: "processing", fully_idle: false, cancellable: true,
          cancel_id: crypto.randomUUID(), progress_percent: null };
        void this.#run(command.prompt, this.#operation.signal);
        return { type: "prompt_accepted", session_id: this.state.session.session_id };
      case "stop": this.#operation?.abort(new DOMException("Turn stopped.", "AbortError")); break;
      case "request_snapshot": return { type: "snapshot_queued" };
      case "dismiss_overlay": this.state.overlay = { active_panel: null, data: null }; break;
      case "select_theme":
        if (!this.state.themes.some((theme) => theme.theme_id === command.theme_id)) throw new Error("Unknown theme.");
        for (const theme of this.state.themes) theme.selected = theme.theme_id === command.theme_id;
        break;
      case "select_model": {
        this.#requireIdle();
        const selected = Number(command.model_id);
        if (!Number.isInteger(selected) || !this.options.connections[selected]) throw new Error("Unknown connection.");
        this.selected = selected; this.reset(); this.options.onConnectionChanged?.(selected); break;
      }
      case "rename_session":
        this.state.session.display_name = command.name;
        this.state.overlay = { active_panel: null, data: null };
        return { type: "session_renamed", session_id: this.state.session.session_id, display_name: command.name };
      case "select_history_entry": {
        const entry = this.#history[Number(command.entry_id)];
        if (entry === undefined) throw new Error("Unknown history entry.");
        return { type: "history_entry_selected", session_id: this.state.session.session_id,
          entry_id: command.entry_id, editor_content: entry };
      }
      case "set_agent_mode":
        this.#requireIdle();
        if (!["general", "python"].includes(command.mode_id)) throw new Error("Unknown agent mode.");
        this.pythonEnabled = command.mode_id === "python"; this.reset(); break;
      case "select_system_prompt": this.#openSystemPrompt(); break;
      case "save_system_prompt":
        this.#requireIdle(); this.reset(); this.#system = command.content; this.#openSystemPrompt(); break;
      case "clear_context": case "delete_python_runtime":
        this.#requireIdle();
        if (!command.confirmed || this.state.overlay.data?.type !== "confirmation" ||
            command.confirmation_id !== this.state.overlay.data.confirmation.confirmation_id ||
            command.type !== this.state.overlay.data.confirmation.action) {
          this.state.overlay = { active_panel: "confirmation", data: { type: "confirmation", confirmation: {
            confirmation_id: crypto.randomUUID(), action: command.type, session_id: this.state.session.session_id,
            title: "Start fresh?", message: "This clears the current chat and Python variables. Encrypted saved code is kept." } } };
          throw new Error("Review the confirmation dialog.");
        }
        this.reset(); break;
      case "new_session": this.#requireIdle(); this.reset(); this.#history = [];
        this.state.session.session_id = crypto.randomUUID();
        return { type: "session_created", session_id: this.state.session.session_id };
      case "invoke_command": return this.#invoke(command.command_id);
      default: throw new Error("This command requires the native runtime.");
    }
    return { type: "applied" };
  }

  #invoke(id: string): CommandOutcome {
    const panels: Record<string, PanelId> = { hotkeys: "hotkeys", themes: "themes", models: "models",
      "input-history": "input_history", "name-session": "name_session", "agent-mode": "agent_mode", "system-prompt": "system_prompt" };
    const panel = panels[id];
    if (panel) {
      this.state.overlay = { active_panel: panel, data: null };
      if (panel === "hotkeys") this.state.overlay.data = { type: "hotkeys", shortcuts: [
        { keys: "Ctrl+P", label: "Command palette" }, { keys: "Ctrl+P → H", label: "Hotkeys" },
        { keys: "Ctrl+P → D", label: "Debugger" }, { keys: "Enter", label: "Send prompt" },
        { keys: "Shift+Enter", label: "New line" }, { keys: "Esc, Esc", label: "Stop turn" } ] };
      if (panel === "input_history") this.state.overlay.data = { type: "input_history", has_more: false,
        entries: this.#history.map((label, index) => ({ entry_id: String(index), label })) };
      if (panel === "name_session") this.state.overlay.data = { type: "name_session", current_name: this.state.session.display_name };
      if (panel === "agent_mode") this.state.overlay.data = { type: "agent_modes", modes: [
        { mode_id: "general", label: "Chat", selected: !this.pythonEnabled, enabled: !this.busy, disabled_reason: null },
        { mode_id: "python", label: "Python · Pyodide", selected: this.pythonEnabled, enabled: !this.busy, disabled_reason: null } ] };
      if (panel === "system_prompt") this.#openSystemPrompt();
      return { type: "panel_opened", panel };
    }
    if (id === "toggle-debugger") this.state.debugger.open = !this.state.debugger.open;
    else if (id === "clear-ui") this.state.blocks = { blocks: [], omitted_before: 0, truncated: false };
    else if (id === "agent-general" || id === "python-permissive") {
      this.#requireIdle(); this.pythonEnabled = id === "python-permissive"; this.reset();
    } else throw new Error("Unsupported browser command.");
    return { type: "applied" };
  }

  #openSystemPrompt(): void {
    this.state.overlay = { active_panel: "system_prompt", data: { type: "system_prompts",
      prompts: [{ prompt_id: "browser", label: "Browser prompt", selected: true }],
      editor_content: this.#system, content_truncated: false } };
  }

  async #run(prompt: string, signal: AbortSignal): Promise<void> {
    try {
      if (!this.#runtime) {
        const connection = this.connection;
        if (!connection) throw new Error("No connection selected.");
        const tools = [...(this.options.tools ?? [])];
        if (this.pythonEnabled) {
          this.#python = createPythonTool({ indexURL: this.options.pyodideIndexURL }); tools.push(this.#python);
        }
        const runtime = await createRuntime({ connection, tools, systemPrompt: this.#system, onEvent: (event) => this.#event(event) });
        if (this.#closed) { runtime.dispose(); return; }
        this.#runtime = runtime;
      }
      await this.#runtime.send(prompt, signal);
      this.state.status.stop_reason = "Ready";
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      this.state.blocks.blocks.push({ ...block("text", message + "\nIncomplete turn removed from model history; tool effects may remain.", "Turn interrupted"), success: false });
      this.state.status.stop_reason = message;
    } finally {
      this.#operation = null;
      this.state.activity = { kind: "idle", fully_idle: true, cancellable: false, cancel_id: null, progress_percent: null };
      this.publish();
    }
  }

  #event(event: RuntimeEvent): void {
    switch (event.type) {
      case "request":
        this.#requestCount++;
        this.state.usage.session.request_count = String(this.#requestCount);
        this.state.activity.kind = "processing"; this.state.status.stop_reason = `Thinking · request ${event.round}`; break;
      case "assistant": this.state.blocks.blocks.push(block("markdown", event.text, "Lethetic")); break;
      case "tool_start": case "tool_end": {
        const content = event.type === "tool_start" ? JSON.stringify(event.call.arguments, null, 2) : event.output;
        const item = block(event.type === "tool_start" ? "tool_call" : "tool_result", content, event.call.name);
        item.tool = { kind: event.type === "tool_start" ? "call" : "result", tool_name: event.call.name, payload: content, payload_loss: NO_LOSS };
        this.state.blocks.blocks.push(item); this.state.activity.kind = "executing_tool";
        this.state.status.stop_reason = event.type === "tool_start" ? `Running ${event.call.name}` : "Tool complete";
        break;
      }
    }
    this.publish();
  }
}
