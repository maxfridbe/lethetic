import { DEFAULT_TURN_LIMITS, parseTurnLimits, LOOP_MODES, parseLoopMode } from "./turn-limits.js";
import type { TurnLimits } from "./turn-limits.js";
import { RelayKeys, relayEndpoint, relayToken } from "./network-route.js";
import { connectionMetadata } from "./connection.js";
import type { DotnetRelay } from "./network-route.js";
import { COMPACTION_PROMPT, summarizeContext } from "./compaction.js";
import { WFE_PROTOCOL_SCHEMA_SHA256, WFE_PROTOCOL_VERSION } from "../../web/src/generated/contracts.js";
import type { CommandOutcome, ICommandRequest, PanelId, WebAppSnapshot, WebCommand } from "../../web/src/generated/contracts.js";
import type { ApplicationTransport, BrowserTransportEvents } from "../../web/src/transport.js";
import { createRuntime, DEFAULT_PROMPT } from "./sdk.js";
import type { Connection, LetheticRuntime, RuntimeEvent, Tool } from "./sdk.js";
import { effectiveSystemPrompt, injectedInstructions, toolDefinitions } from "./prompts.js";
import { createDotnetTool } from "./dotnet.js";
import type { DotnetTool, WireRequest, WireResponse, WireSite } from "./dotnet.js";
import type { AgentMode } from "./sessions.js";
import { createPythonTool } from "./python.js";
import type { PythonTool } from "./python.js";
import { emptyWorkspace, dateWorkspace } from "./workspace.js";
import type { Workspace } from "./workspace.js";
import { block, initialState, NO_LOSS } from "./local-state.js";
import { BrowserMetrics } from "./metrics.js";
import type { SavedSession } from "./sessions.js";
import { SessionStore } from "./sessions.js";
import type { BrowserFiles } from "./files.js";
import type { FileEndpoint } from "../../web/src/files/protocol.js";

export interface LocalOptions {
  connections: Connection[];
  python?: boolean;
  mode?: AgentMode;
  networkAccess?: boolean; nugetAccess?: boolean; siteAccess?: boolean;
  dotnetRelay?: DotnetRelay | null;
  credentialStorage?: Storage;
  tools?: Tool[];
  systemPrompt?: string;
  pyodideIndexURL?: string;
  onConnectionChanged?: (index: number) => void;
  storage?: Storage;
  initialSession?: string;
  onSessionChanged?: (id: string) => void;
  files?: BrowserFiles;
  onFilesChanged?: () => void;
}

/** Drives the existing remote-mode UI using the same typed commands and snapshots. */
export class LocalTransport implements ApplicationTransport {
  readonly state: WebAppSnapshot = initialState();
  readonly metrics = new BrowserMetrics();
  #ticker: ReturnType<typeof setInterval> | null = null;
  #streamPublish: ReturnType<typeof setTimeout> | null = null;
  pendingCount = 0;
  lastSendFailure: string | null = null;
  selected = 0;
  mode: AgentMode;
  networkAccess: boolean;
  nugetAccess: boolean;
  siteAccess: boolean;
  #relayEndpoint: string | null = null;
  readonly #relayKeys: RelayKeys;
  get dotnetRelay(): DotnetRelay | null { return this.#relayEndpoint ? { endpoint: this.#relayEndpoint, token: this.#relayKeys.get(this.#relayEndpoint) } : null; }
  get relaySecrets(): string[] { return this.#relayKeys.values(); }
  get pythonEnabled(): boolean { return this.mode === "python"; }
  #revision = 0;
  #closed = false;
  #runtime: LetheticRuntime | null = null;
  #python: PythonTool | null = null;
  #dotnet: DotnetTool | null = null;
  #operation: AbortController | null = null;
  #history: string[] = [];
  #system: string;
  loopDetection = "combined";
  turnLimits: TurnLimits = {...DEFAULT_TURN_LIMITS};
  #requestCount = 0;
  #responseStart = 0;
  #streamBlocks = new Map<"thinking" | "assistant", number>();
  readonly #sessions: SessionStore;
  #savedMessages: unknown[] = [];
  #workspace = emptyWorkspace();
  #legacyWorkspace = emptyWorkspace();
  persistenceStatus = "Sessions stay in this tab; storage is not enabled.";

  constructor(readonly events: BrowserTransportEvents, readonly options: LocalOptions) {
    this.mode = options.mode ?? (options.python ? "python" : "general");
    this.networkAccess = options.networkAccess ?? false;
    this.nugetAccess = options.nugetAccess ?? false;
    this.siteAccess = options.siteAccess ?? false;
    this.#relayKeys = new RelayKeys(options.credentialStorage);
    if (options.dotnetRelay) {
      this.#relayEndpoint = relayEndpoint(options.dotnetRelay.endpoint);
      if (options.dotnetRelay.token) this.#relayKeys.set(this.#relayEndpoint, relayToken(options.dotnetRelay.token));
    }
    this.#system = options.systemPrompt ?? DEFAULT_PROMPT;
    this.#sessions = new SessionStore(options.storage);
    if (options.files) {
      // Retain the old global record; import a copy only for pre-workspace sessions.
      try { this.#legacyWorkspace = options.files.snapshotWorkspace(); } catch { /* Leave damaged legacy data untouched. */ }
      options.files.bindWorkspace(() => structuredClone(this.#workspace), (workspace) => this.#editWorkspace(workspace));
    }
  }

  get promptDetails(): { instructions: string; tools: string } {
    return { instructions: injectedInstructions(this.mode, this.networkAccess, !!this.#relayEndpoint, this.nugetAccess, this.siteAccess),
      tools: JSON.stringify(toolDefinitions(this.mode, this.options.tools), null, 2) };
  }

  get busy(): boolean { return this.#operation !== null; }
  get connection(): Connection | undefined { return this.options.connections[this.selected]; }

  start(): void {
    this.events.socketOpened();
    this.events.serverMessage({ type: "hello", hello: { protocol_version: WFE_PROTOCOL_VERSION,
      minimum_protocol_version: WFE_PROTOCOL_VERSION, server_name: "lethetic", sequence: 0, revision: 0,
      schema_sha256: WFE_PROTOCOL_SCHEMA_SHA256, capabilities: { state_patches: true, request_replay: true,
        session_names: true, exact_tool_approval: true, read_only_files: !!this.options.files } } });
    try { this.openSession(this.options.initialSession ?? "latest"); }
    catch (error) { this.persistenceStatus = error instanceof Error ? error.message : String(error); }
    this.publish();
  }

  publish(): void {
    if (this.#streamPublish !== null) { clearTimeout(this.#streamPublish); this.#streamPublish = null; }
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
    this.state.status.tool_use = this.pythonEnabled ? `Pyodide · ${this.networkAccess ? this.#relayEndpoint ? "HTTP relay" : "direct network" : "network off"}` : this.mode === "dotnet-wasm" ? `.NET 11 · WASM · ${this.networkAccess ? this.#relayEndpoint ? "HTTP relay" : "direct network" : "network off"}` : "Browser tools";
    this.state.status.context_limit_tokens = String(this.turnLimits.contextTokens);
    this.state.status.visible_block_count = this.state.blocks.blocks.length;
    this.events.serverMessage({ type: "state_snapshot", snapshot: { protocol_version: WFE_PROTOCOL_VERSION,
      revision: this.#revision, sequence: this.#revision, state: structuredClone(this.state) } });
  }

  get canCompact(): boolean { return !this.busy && !!this.connection && this.#savedMessages.length > 0; }

  async compact(): Promise<void> {
    this.#requireIdle();
    const connection = this.connection;
    if (!connection) throw new Error("Choose a connection before compacting.");
    this.#saveSession(true);
    if (!this.#savedMessages.length) throw new Error("There is no model context to compact yet.");
    const original = this.sessionForExport(this.state.session.session_id);
    const controller = new AbortController(); this.#operation = controller;
    this.state.overlay = { active_panel: null, data: null };
    this.state.activity = { kind: "processing", fully_idle: false, cancellable: true, cancel_id: crypto.randomUUID(), progress_percent: 0 };
    this.state.status.stop_reason = "Compacting context…"; this.metrics.begin(); this.publish();
    let summarizer: LetheticRuntime | null = null;
    try {
      summarizer = await createRuntime({ connection, ...this.turnLimits, systemPrompt: COMPACTION_PROMPT, tools: [], onEvent: event => {
        if (event.type === "request" || event.type === "response") { this.metrics.event(event); this.publish(); }
      } });
      const summary = await summarizeContext(summarizer, original.messages, controller.signal, (part, total) => {
        this.state.status.stop_reason = `Compacting context · ${part}/${total}`;
        this.state.activity.progress_percent = Math.round((part - 1) / total * 100); this.publish();
      });
      controller.signal.throwIfAborted();
      const messages = [{role:"user",content:`Context from earlier in this session:\n\n${summary}`},
        {role:"assistant",content:"I will continue using this context and the existing session files."}];
      summarizer.restore(messages);
      const blocks = [...this.state.blocks.blocks, block("markdown", summary, "Context compacted")];
      // Persist before replacing working context; quota failures leave the original intact.
      this.#sessions.save({...original, messages, blocks, updatedAt:new Date().toISOString(), metrics:this.metrics.snapshot()});
      this.#runtime?.dispose(); this.#runtime = null; this.#savedMessages = messages;
      this.state.blocks.blocks = blocks; this.state.status.stop_reason = "Context compacted";
    } catch (error) {
      this.state.status.stop_reason = `Compaction stopped: ${error instanceof Error ? error.message : String(error)}. Original context kept.`;
      throw error;
    } finally {
      summarizer?.dispose(); this.metrics.finish(); this.#operation = null;
      this.state.activity = { kind: "idle", fully_idle: true, cancellable: false, cancel_id: null, progress_percent: null };
      if (!this.#closed) this.#saveSession(); this.publish();
    }
  }

  setTurnLimits(value: TurnLimits): void {
    this.#requireIdle();
    const limits = parseTurnLimits(value);
    this.#saveSession(true);
    const previous = this.turnLimits;
    this.turnLimits = limits;
    try { this.#saveSession(true); }
    catch (error) { this.turnLimits = previous; throw error; }
    this.#runtime?.dispose(); this.#runtime = null;
    this.publish();
  }

  setConnections(connections: Connection[], selected: number): void {
    this.#requireIdle();
    const before = this.connection, after = connections[selected];
    this.#saveSession(true);
    if (before?.endpoint !== after?.endpoint || before?.provider !== after?.provider ||
        before?.model !== after?.model || before?.apiKey !== after?.apiKey || before?.thinking !== after?.thinking) {
      this.#runtime?.dispose(); this.#runtime = null;
    }
    this.options.connections = connections;
    this.selected = selected;
    this.#saveSession();
    this.publish();
  }

  setPythonMode(enabled: boolean): void { this.setMode(enabled ? "python" : "general"); }

  setMode(mode: AgentMode): void {
    this.#requireIdle();
    if (!["general", "python", "dotnet-wasm"].includes(mode)) throw new Error("Unknown agent mode.");
    if (mode === this.mode) return;
    this.#saveSession(true);
    this.#runtime?.dispose(); this.#runtime = null;
    this.#python?.reset(); this.#python = null;
    this.#dotnet?.reset(); this.#dotnet = null;
    this.mode = mode;
    this.#saveSession(); this.publish();
  }

  setNetworkAccess(enabled: boolean): void {
    this.#requireIdle();
    if (typeof enabled !== "boolean") throw new Error("Network access must be a boolean.");
    if (enabled === this.networkAccess) return;
    this.#saveSession(true);
    this.#runtime?.dispose(); this.#runtime = null;
    this.#dotnet?.reset(); this.#dotnet = null;
    this.#python?.reset(); this.#python = null;
    this.networkAccess = enabled;
    this.#saveSession(); this.publish();
  }

  setNugetAccess(enabled: boolean): void {
    this.#requireIdle();
    if (typeof enabled !== "boolean") throw new Error("NuGet package access must be a boolean.");
    if (enabled === this.nugetAccess) return;
    this.#saveSession(true);
    this.#runtime?.dispose(); this.#runtime = null;
    this.#dotnet?.reset(); this.#dotnet = null;
    this.nugetAccess = enabled;
    this.#saveSession(); this.publish();
  }

  setSiteAccess(enabled: boolean): void {
    this.#requireIdle();
    if (typeof enabled !== "boolean") throw new Error("Interactive sites permission must be a boolean.");
    if (enabled === this.siteAccess) return;
    this.#saveSession(true);
    this.#runtime?.dispose(); this.#runtime = null;
    this.#dotnet?.reset(); this.#dotnet = null;
    this.#python?.reset(); this.#python = null;
    this.siteAccess = enabled;
    this.#saveSession(); this.publish();
  }

  get sites(): WireSite[] { return (this.pythonEnabled ? this.#python : this.#dotnet)?.sites() ?? []; }

  #pythonTool(): PythonTool {
    return this.#python ??= createPythonTool({ indexURL: this.options.pyodideIndexURL, siteAccess: this.siteAccess, networkAccess: this.networkAccess, relay: this.dotnetRelay,
      workspace: this.#workspace, onWorkspace: workspace => this.#checkpointWorkspace(workspace) });
  }

  #dotnetTool(): DotnetTool {
    return this.#dotnet ??= createDotnetTool({ networkAccess: this.networkAccess, nugetAccess: this.nugetAccess,
      siteAccess: this.siteAccess, relay: this.dotnetRelay, workspace: this.#workspace,
      onWorkspace: workspace => this.#checkpointWorkspace(workspace) });
  }

  async runDotnetFile(path: string): Promise<void> {
    this.#requireIdle();
    if (!/\.(cs|csproj)$/i.test(path) || !this.#workspace.files.some(file => file.path === path)) throw new Error("Select a session .cs or .csproj file to run.");
    this.setMode("dotnet-wasm");
    await this.#executeCode(`var fileRun = await Dotnet.Run(${JSON.stringify(path)});
if (!fileRun.Success) throw new Exception(fileRun.Error?.Message ?? $"Program failed with exit code {fileRun.ExitCode}. See compilation output above.");`, `Run ${path}`);
  }

  async runPythonFile(path: string): Promise<void> {
    this.#requireIdle();
    if (!/\.py$/i.test(path) || !this.#workspace.files.some(file => file.path === path)) throw new Error("Select a session .py file to run.");
    this.setMode("python");
    await this.#executeCode(`await __import__("lethetic_wire").run_file(${JSON.stringify("/home/pyodide/" + path)})`, `Run ${path}`);
  }

  async stopSite(port: number): Promise<void> {
    this.#requireIdle();
    if (!Number.isInteger(port) || !this.sites.some(site => site.port === port)) throw new Error("Site is no longer running.");
    await this.#executeCode(this.pythonEnabled ? `__import__("lethetic_wire").stop(${port})` : `DotnetDide.Wire.Stop(${port});`, `Stop virtual port ${port}`);
  }

  async #executeCode(code: string, title: string): Promise<void> {
    const controller = new AbortController(); this.#operation = controller;
    this.state.activity = { kind: "processing", fully_idle: false, cancellable: true, cancel_id: crypto.randomUUID(), progress_percent: null };
    this.state.status.stop_reason = title; this.publish();
    try {
      const output = await (this.pythonEnabled ? this.#pythonTool() : this.#dotnetTool()).execute({ code }, controller.signal);
      this.state.blocks.blocks.push(block("text", String(output), title));
      this.state.status.stop_reason = "Ready";
    } catch (error) {
      this.state.blocks.blocks.push({ ...block("text", String(error), title), success: false });
      this.state.status.stop_reason = String(error); throw error;
    } finally {
      this.#operation = null;
      this.state.activity = { kind: "idle", fully_idle: true, cancellable: false, cancel_id: null, progress_percent: null };
      if (!this.#closed) this.#saveSession(); this.publish();
    }
  }

  async requestSite(request: WireRequest, sessionId: string): Promise<WireResponse> {
    this.#requireIdle();
    const tool = this.pythonEnabled ? this.#python : this.mode === "dotnet-wasm" ? this.#dotnet : null;
    if (sessionId !== this.state.session.session_id || !this.siteAccess || !tool) throw new Error("This session's interactive site is no longer available.");
    const controller = new AbortController(); this.#operation = controller;
    this.state.activity = { kind: "processing", fully_idle: false, cancellable: true, cancel_id: crypto.randomUUID(), progress_percent: null };
    this.state.status.stop_reason = `Site :${request.port} · ${request.method} ${request.path}`; this.publish();
    try { return await tool.requestSite(request, controller.signal); }
    finally { this.#operation = null;
      this.state.activity = { kind: "idle", fully_idle: true, cancellable: false, cancel_id: null, progress_percent: null };
      this.state.status.stop_reason = "Ready";
      if (!this.#closed) this.#saveSession(); this.publish(); }
  }

  setDotnetRelay(value: DotnetRelay | null): void {
    this.#requireIdle();
    const endpoint = value ? relayEndpoint(value.endpoint) : null;
    const token = value ? relayToken(value.token) : "";
    if (endpoint === this.#relayEndpoint && (!endpoint || token === this.#relayKeys.get(endpoint))) return;
    this.#saveSession(true);
    if (endpoint) this.#relayKeys.set(endpoint, token);
    this.#runtime?.dispose(); this.#runtime = null;
    this.#dotnet?.reset(); this.#dotnet = null;
    this.#python?.reset(); this.#python = null;
    this.#relayEndpoint = endpoint;
    this.#saveSession(); this.publish();
  }

  reset(): void {
    this.#requireIdle();
    this.#runtime?.dispose(); this.#runtime = null;
    this.#python?.reset(); this.#python = null;
    this.#dotnet?.reset(); this.#dotnet = null;
    this.metrics.reset(); this.#requestCount = 0;
    this.#savedMessages = [];
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
        if (!this.busy) this.#saveSession();
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
  async requestFile(endpoint: FileEndpoint, path: string, signal: AbortSignal): Promise<Response> {
    if (!this.options.files) throw new Error("Files are unavailable.");
    return this.options.files.request(endpoint, path, signal);
  }
  closePermanently(): void { this.dispose(); }
  dispose(): void {
    if (this.#ticker !== null) clearInterval(this.#ticker);
    if (this.#streamPublish !== null) clearTimeout(this.#streamPublish);
    this.#closed = true; this.#operation?.abort(); this.#runtime?.dispose(); this.#python?.reset(); this.#dotnet?.reset();
  }

  #requireIdle(): void { if (this.busy) throw new Error("Stop the current turn first."); }

  #execute(command: WebCommand): CommandOutcome {
    if ("session_id" in command && command.type !== "resume_session" && command.type !== "delete_session" &&
        command.session_id !== this.state.session.session_id) throw new Error("Session changed.");
    switch (command.type) {
      case "send_prompt":
        this.#requireIdle();
        if (!this.connection) throw new Error("Open Connections and save a model endpoint first.");
        if (!command.prompt.trim()) throw new Error("Enter a prompt.");
        this.#operation = new AbortController();
        this.metrics.begin();
        this.#ticker = setInterval(() => this.publish(), 1000);
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
        this.setConnections(this.options.connections, selected); this.options.onConnectionChanged?.(selected); break;
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
        if (!["general", "python", "dotnet-wasm"].includes(command.mode_id)) throw new Error("Unknown agent mode.");
        this.setMode(command.mode_id as AgentMode); break;
      case "select_system_prompt": this.#openSystemPrompt(); break;
      case "save_system_prompt":
        this.#requireIdle(); this.#saveSession(true);
        this.#runtime?.dispose(); this.#runtime = null;
        this.#system = command.content; this.#openSystemPrompt(); break;
      case "clear_context": case "delete_python_runtime":
        this.#requireIdle();
        if (!command.confirmed || this.state.overlay.data?.type !== "confirmation" ||
            command.confirmation_id !== this.state.overlay.data.confirmation.confirmation_id ||
            command.type !== this.state.overlay.data.confirmation.action) {
          this.state.overlay = { active_panel: "confirmation", data: { type: "confirmation", confirmation: {
            confirmation_id: crypto.randomUUID(), action: command.type, session_id: this.state.session.session_id,
            title: "Start fresh?", message: "This clears the current chat and Python variables. Local files are kept." } } };
          throw new Error("Review the confirmation dialog.");
        }
        this.reset(); break;
      case "resume_session": this.openSession(command.session_id);
        return { type: "session_loaded", session_id: this.state.session.session_id };
      case "delete_session": case "wipe_sessions": {
        this.#requireIdle();
        const target = command.type === "delete_session" ? command.session_id : null;
        const confirmation = this.state.overlay.data;
        if (!command.confirmed || confirmation?.type !== "confirmation" ||
            command.confirmation_id !== confirmation.confirmation.confirmation_id ||
            confirmation.confirmation.action !== command.type || confirmation.confirmation.session_id !== target) {
          this.state.overlay = { active_panel: "confirmation", data: { type: "confirmation", confirmation: {
            confirmation_id: crypto.randomUUID(), action: command.type, session_id: target,
            title: "Delete saved sessions?", message: target ? "Delete this session and its workspace files from this device?" : "Delete every saved chat and its workspace files from this device?" } } };
          throw new Error("Review the confirmation dialog.");
        }
        if (target) this.#sessions.remove(target); else this.#sessions.clear();
        if (!target || target === this.state.session.session_id) this.#newSession();
        this.state.overlay = { active_panel: "sessions", data: null };
        return target ? { type: "session_deleted", session_id: target } : { type: "sessions_wiped" };
      }
      case "new_session": this.#requireIdle(); this.#saveSession(true); this.#newSession();
        return { type: "session_created", session_id: this.state.session.session_id };
      case "set_loop_detection": {
        this.#requireIdle();
        const mode = parseLoopMode(command.mode_id);
        this.#saveSession(true);
        const previous = this.loopDetection; this.loopDetection = mode;
        try { this.#saveSession(true); } catch (error) { this.loopDetection = previous; throw error; }
        this.#runtime?.dispose(); this.#runtime = null;
        this.#invoke("loop-detection"); break;
      }
      case "invoke_command": return this.#invoke(command.command_id);
      default: throw new Error("This command requires the native runtime.");
    }
    return { type: "applied" };
  }

  #invoke(id: string): CommandOutcome {
    const panels: Record<string, PanelId> = { hotkeys: "hotkeys", themes: "themes", models: "models",
      sessions: "sessions", "loop-detection": "loop_detection",
      "input-history": "input_history", "name-session": "name_session", "agent-mode": "agent_mode", "system-prompt": "system_prompt" };
    const panel = panels[id];
    if (panel) {
      this.state.overlay = { active_panel: panel, data: null };
      if (panel === "loop_detection") this.state.overlay.data = { type: "loop_modes", modes: LOOP_MODES.map(([mode_id, label]) => ({mode_id, label, selected: mode_id === this.loopDetection, enabled: !this.busy, disabled_reason: this.busy ? "Stop the current turn first" : null})) };
      if (panel === "hotkeys") this.state.overlay.data = { type: "hotkeys", shortcuts: [
        { keys: "Ctrl+P", label: "Command palette" }, { keys: "Ctrl+P → H", label: "Hotkeys" },
        { keys: "Ctrl+P → D", label: "Debugger" }, { keys: "Enter", label: "Send prompt" },
        { keys: "Shift+Enter", label: "New line" }, { keys: "Esc, Esc", label: "Stop turn" } ] };
      if (panel === "input_history") this.state.overlay.data = { type: "input_history", has_more: false,
        entries: this.#history.map((label, index) => ({ entry_id: String(index), label })) };
      if (panel === "name_session") this.state.overlay.data = { type: "name_session", current_name: this.state.session.display_name };
      if (panel === "agent_mode") this.state.overlay.data = { type: "agent_modes", modes: [
        { mode_id: "general", label: "Chat", selected: this.mode === "general", enabled: !this.busy, disabled_reason: null },
        { mode_id: "python", label: "Python · Pyodide", selected: this.pythonEnabled, enabled: !this.busy, disabled_reason: null },
        { mode_id: "dotnet-wasm", label: ".NET 11 · WASM", selected: this.mode === "dotnet-wasm", enabled: !this.busy, disabled_reason: null } ] };
      if (panel === "system_prompt") this.#openSystemPrompt();
      return { type: "panel_opened", panel };
    }
    if (id === "toggle-debugger") this.state.debugger.open = !this.state.debugger.open;
    else if (id === "clear-ui") this.state.blocks = { blocks: [], omitted_before: 0, truncated: false };
    else if (id === "agent-general" || id === "python-permissive") {
      this.setPythonMode(id === "python-permissive");
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
          tools.push(this.#pythonTool());
        }
        if (this.mode === "dotnet-wasm") {
          tools.push(this.#dotnetTool());
        }
        const runtime = await createRuntime({ connection, ...this.turnLimits, loopDetection: this.loopDetection, tools, systemPrompt: effectiveSystemPrompt(this.#system, this.mode, this.networkAccess, !!this.#relayEndpoint, this.nugetAccess, this.siteAccess), onEvent: (event) => this.#event(event) });
        if (this.#closed) { runtime.dispose(); return; }
        try { runtime.restore(this.#savedMessages); } catch (error) { runtime.dispose(); throw error; }
        this.#runtime = runtime;
      }
      await this.#runtime.send(prompt, signal);
      this.state.status.stop_reason = "Ready";
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      this.state.blocks.blocks.push({ ...block("text", message + "\nIncomplete turn removed from model history; tool effects may remain.", "Turn interrupted"), success: false });
      this.state.status.stop_reason = message;
    } finally {
      if (this.#ticker !== null) clearInterval(this.#ticker);
      this.#ticker = null; this.metrics.finish();
      this.#operation = null;
      this.state.activity = { kind: "idle", fully_idle: true, cancellable: false, cancel_id: null, progress_percent: null };
      if (!this.#closed) this.#saveSession();
      this.publish();
    }
  }

  #newSession(): void {
    this.loopDetection = "combined";
    this.turnLimits = {...DEFAULT_TURN_LIMITS};
    this.reset(); this.#history = []; this.#workspace = emptyWorkspace();
    this.networkAccess = this.options.networkAccess ?? false;
    this.nugetAccess = this.options.nugetAccess ?? false;
    this.siteAccess = this.options.siteAccess ?? false;
    this.#relayEndpoint = this.options.dotnetRelay ? relayEndpoint(this.options.dotnetRelay.endpoint) : null;
    this.state.session = { session_id: crypto.randomUUID(), display_name: null, fallback_label: "New chat" };
    this.options.onSessionChanged?.(this.state.session.session_id);
  }

  openSession(route: string): void {
    this.#requireIdle();
    const saved = this.#sessions.list();
    const session = route === "latest" ? saved[0] : saved.find((item) => item.id === route);
    if (route !== "latest" && route !== "new" && !session) throw new Error("Session not found on this device. Open Sessions or #/new.");
    if (this.state.blocks.blocks.some((item) => item.kind === "user")) this.#saveSession(true);
    if (session && route !== "new") {
      this.reset();
      this.state.session = { session_id: session.id, display_name: session.name, fallback_label: session.label };
      this.state.blocks.blocks = session.blocks; this.#savedMessages = session.messages; this.#history = session.inputs;
      this.#workspace = structuredClone(session.workspace ?? this.#legacyWorkspace);
      this.loopDetection = session.loopDetection ?? "combined";
      this.turnLimits = {...(session.turnLimits ?? DEFAULT_TURN_LIMITS)};
      this.#system = session.system; this.mode = session.mode ?? (session.python ? "python" : "general");
      this.networkAccess = session.networkAccess ?? false;
      this.nugetAccess = session.nugetAccess ?? false;
      this.siteAccess = session.siteAccess ?? false;
      this.#relayEndpoint = session.dotnetRelayEndpoint ?? null;
      if (session.metrics) this.metrics.restore(session.metrics);
      for (const theme of this.state.themes) theme.selected = theme.theme_id === session.theme;
      if (session.connection) {
        const matches = (item: Connection): boolean => item.endpoint === session.connection?.endpoint &&
          item.model === session.connection?.model && item.provider === session.connection?.provider;
        let index = this.options.connections.findIndex(item => item.name === session.connection?.name && matches(item));
        if (index < 0) {
          const candidates = this.options.connections.map((item, index) => ({item,index})).filter(({item}) => matches(item));
          if (candidates.length === 1) index = candidates[0]!.index;
        }
        if (index < 0) { index = this.options.connections.length; this.options.connections.push(session.connection); }
        this.selected = index;
      }
      this.options.onSessionChanged?.(session.id);
    } else this.#newSession();
    this.#saveSession();
  }

  sessionForExport(id: string): SavedSession {
    if (id === this.state.session.session_id) this.#saveSession();
    const saved = this.#sessions.list().find(item => item.id === id);
    if (!saved) throw new Error("Session not found on this device.");
    // Include current diagnostics/transcript even during a running or failed turn.
    return structuredClone(id === this.state.session.session_id ? { ...saved,
      blocks: this.state.blocks.blocks, metrics: this.metrics.snapshot(), workspace: this.#workspace } : saved);
  }

  #saveSession(mustSave = false): void {
    if (this.busy) return;
    try {
      const connection = this.connection;
      this.#savedMessages = this.#runtime?.history() ?? this.#savedMessages;
      const label = this.#history[0]?.slice(0, 100) || "New chat";
      this.state.session.fallback_label = label;
      this.#sessions.save({ id: this.state.session.session_id, name: this.state.session.display_name, label,
        updatedAt: new Date().toISOString(), blocks: this.state.blocks.blocks, messages: this.#savedMessages,
        inputs: this.#history, system: this.#system, python: this.pythonEnabled, mode: this.mode, networkAccess: this.networkAccess, nugetAccess: this.nugetAccess, siteAccess: this.siteAccess, dotnetRelayEndpoint: this.#relayEndpoint, workspace: this.#workspace,
        metrics: this.metrics.snapshot(), turnLimits: this.turnLimits, loopDetection: this.loopDetection,
        theme: this.state.themes.find((theme) => theme.selected)?.theme_id ?? "dracula",
        connection: connection ? connectionMetadata(connection) : null });
      if (this.options.storage) this.persistenceStatus = "Chats and files are saved unencrypted on this device. API keys are excluded from saved chats. Session files sync with the selected code runtime. Variables reset on reload, stop, mode/permission changes or session switch.";
      this.state.sessions = { sessions: this.#sessions.list().map((item) => ({ session_id: item.id,
        display_name: item.name, fallback_label: item.label, selected: item.id === this.state.session.session_id })), has_more: false };
    } catch (error) {
      this.persistenceStatus = `Session not saved: ${error instanceof Error ? error.message : String(error)}`;
      if (mustSave) throw error;
    }
  }

  #editWorkspace(value: Workspace): void {
    this.#requireIdle();
    const previous = this.#workspace; this.#workspace = dateWorkspace(value, previous);
    try { this.#saveSession(true); }
    catch (error) { this.#workspace = previous; this.publish(); throw error; }
    this.#python?.setWorkspace(this.#workspace);
    this.#dotnet?.setWorkspace(this.#workspace);
    this.options.onFilesChanged?.(); this.publish();
  }

  #checkpointWorkspace(workspace: Workspace): void {
    if (this.#closed) return;
    this.#workspace = dateWorkspace(workspace, this.#workspace);
    // Save files after each completed Python call, even while the model turn continues.
    // Keep the last complete model history; restoring files must not replay partial turns.
    try {
      const saved = this.#sessions.list().find((session) => session.id === this.state.session.session_id);
      if (!saved) throw new Error("No saved session for this workspace.");
      this.#sessions.save({ ...saved, workspace: this.#workspace, updatedAt: new Date().toISOString() });
    } catch (error) {
      this.persistenceStatus = `Workspace not saved: ${error instanceof Error ? error.message : String(error)}`;
      throw error;
    } finally { this.options.onFilesChanged?.(); this.publish(); }
  }

  #event(event: RuntimeEvent): void {
    this.metrics.event(event);
    switch (event.type) {
      case "request":
        this.#responseStart = this.state.blocks.blocks.length; this.#streamBlocks.clear();
        this.#requestCount++;
        this.state.usage.session.request_count = String(this.#requestCount);
        this.state.activity.kind = "processing"; this.state.status.stop_reason = `Thinking · request ${event.round}`; break;
      case "retry":
        this.state.status.stop_reason = `Retry ${event.attempt} in ${Math.ceil(event.delayMs / 1000)}s · ${event.error}`; break;
      case "response_reset":
        this.state.blocks.blocks.splice(this.#responseStart); this.#streamBlocks.clear(); break;
      case "thinking_delta": case "assistant_delta": case "thinking": case "assistant": {
        const thinking = event.type === "thinking" || event.type === "thinking_delta";
        const key = thinking ? "thinking" : "assistant", index = this.#streamBlocks.get(key);
        const item = index === undefined ? undefined : this.state.blocks.blocks[index];
        if (item) item.content = event.type.endsWith("_delta") ? item.content + event.text : event.text;
        else {
          this.#streamBlocks.set(key, this.state.blocks.blocks.length);
          this.state.blocks.blocks.push(block(thinking ? "thought" : "markdown", event.text, thinking ? "Thinking" : "Lethetic"));
        }
        break;
      }
      case "tool_start": case "tool_end": {
        const content = event.type === "tool_start" ? JSON.stringify(event.call.arguments, null, 2) : event.output;
        // Shared tool blocks keep their text only in tool.payload.
        const item = block(event.type === "tool_start" ? "tool_call" : "tool_result", "", event.call.name);
        item.tool = { kind: event.type === "tool_start" ? "call" : "result", tool_name: event.call.name, payload: content, payload_loss: NO_LOSS };
        this.state.blocks.blocks.push(item); this.state.activity.kind = "executing_tool";
        this.state.status.stop_reason = event.type === "tool_start" ? `Running ${event.call.name}` : "Tool complete";
        break;
      }
    }
    if (event.type === "assistant_delta" || event.type === "thinking_delta") {
      // Coalesce token bursts; final events flush immediately through publish().
      if (this.#streamPublish === null) this.#streamPublish = setTimeout(() => this.publish(), 32);
    } else this.publish();
  }
}
