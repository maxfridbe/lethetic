import { downloadBlob } from "../../web/src/download.js";
import { NativeDirectory, nativeAvailable, resetNativeWork } from "./native-host.js";
import { lifecycleDiagnostics } from "./lifecycle.js";
import { browserCommandVisible } from "./local-state.js";
import { FILE_BYTES } from "./workspace.js";
import { BrowserSessionStorage } from "./session-storage.js";
export { openBrowserStorage } from "./session-storage.js";
import { createTurnLimitsDialog } from "./turn-limits-dialog.js";
import { createSitesDialog } from "./sites-dialog.js";
import { createFileTools } from "./file-tools.js";
import { createFileUpload } from "./file-upload.js";
import { PinnedStats } from "./pinned-stats.js";
import { sessionDebugBundle, downloadSessionBundle } from "./session-export.js";
import { uiIcon } from "../../web/src/icons.js";
import { h } from "../../web/lib/snabbdom/build/index.js";
import { SpaApplication } from "../../web/src/app.js";
import type { Connection, Tool } from "./sdk.js";
import { loadConnections, restoreConnectionKeys, validateConnection } from "./connection.js";
import { LocalTransport } from "./local-transport.js";
import { createConnectionsDialog } from "./connections-dialog.js";
import { codeSnippets } from "./code-snippets.js";
import { browserHeaderStatus, renderBrowserHeaderStatus, connectionIndicator, renderBrowserStatus } from "./status-view.js";
import { canPreviewFile, canRenderFile, workspacePreview, copyCode, createFilePreview, downloadCode, snippetName } from "./file-actions.js";
import { sessionRoute } from "./sessions.js";
import { BrowserFiles } from "./files.js";
import { createFileEditor } from "./file-editor.js";

export interface EmbedOptions {
  /** Defer initial connection setup until activate(), for hosts displaying a startup overlay. */
  startInBackground?: boolean;
  connections?: Connection[];
  /** Optional device-local persistence for connections, sessions and files. */
  storage?: Storage;
  /** Optional tab-scoped key storage; use sessionStorage to survive reloads. */
  credentialStorage?: Storage;
  tools?: Tool[];
  systemPrompt?: string;
  python?: boolean;
  mode?: import("./sessions.js").AgentMode;
  /** Python and .NET code network access; default false. Model connections are unaffected. */
  networkMode?: import("./network-route.js").NativeNetworkMode;
  networkAccess?: boolean;
  /** Direct NuGet.org managed package downloads and cache reuse; default false. */
  nugetAccess?: boolean;
  /** Python or ASP.NET virtual sites in an isolated preview; default false. */
  siteAccess?: boolean;
  /** Optional HTTP relay for both Python and .NET (legacy option name). Only its endpoint is saved with the session; token uses credentialStorage. */
  dotnetRelay?: import("./network-route.js").DotnetRelay | null;
  pyodideIndexURL?: string;
  /** Retained for compatibility; the shared remote-mode UI is already compact. */
  compact?: boolean;
  /** Enable #/sessions/<id>, #/latest and #/new. Off by default in embedded hosts. */
  sessionRouting?: boolean;
}

/** Mount the same SpaApplication used by native remote mode, backed by local WASM.
 * The shared UI uses document-level keyboard and focus handling: mount one per document.
 */
export function mountLethetic(root: HTMLElement, options: EmbedOptions = {}): { initialize(onProgress: (message: string) => void): Promise<void>; activate(): void; destroy(): void } {
  resetNativeWork();
  if (nativeAvailable()) globalThis.addEventListener("pagehide", resetNativeWork);
  let connections = options.connections?.map(validateConnection) ??
    (options.storage ? loadConnections(options.storage) : []);
  if (options.credentialStorage) connections = restoreConnectionKeys(options.credentialStorage, connections);
  const files = new BrowserFiles(options.storage);
  const directory = new NativeDirectory();
  let choosingDirectory = false;
  const pinnedStats = new PinnedStats(options.storage);
  let editor: ReturnType<typeof createFileEditor> | null = null;
  const preview = createFilePreview({ plainText: () => plainText, onPlainText: value => { plainText = value; transport?.publish(); } });
  let sites: ReturnType<typeof createSitesDialog> | null = null;
  let limits: ReturnType<typeof createTurnLimitsDialog> | null = null;
  let settings: ReturnType<typeof createConnectionsDialog> | null = null;
  let transport: LocalTransport | null = null;
  let fileStatus = "";
  const sourceViews = new Set<string>();
  let plainText = false;
  let renderedPreview: { path: string; data: string; document: string | null } | null = null;
  let exportStatus = "";
  let exporting = false;
  const upload = createFileUpload(files, () => transport ? { id: transport.state.session.session_id, busy: transport.busy } : null,
    message => { if (message) fileStatus = message; transport?.publish(); });
  const fileTools = createFileTools(files, () => transport?.publish());
  const mount = document.createElement("div"); root.replaceChildren(mount);
  const snippetActions = (snippet: { code: string; language: string }) => h("div.browser-code-actions", [
    h("button", { attrs: { type: "button", "data-browser-action": "save-code" }, on: { click: () => editor?.open(snippetName(snippet), snippet.code) } }, [uiIcon("save"), "Save to Session"]),
    h("button", { attrs: { type: "button", "data-browser-action": "preview" }, on: { click: () => preview.open(snippet.code, snippet.language, snippetName(snippet)) } }, [uiIcon("preview"), "Preview"]),
    h("button", { attrs: { type: "button", "data-browser-action": "download" }, on: { click: () => { void downloadCode(snippet.code, snippetName(snippet)).then(saved => {
      fileStatus = saved ? "Download ready." : "Download cancelled."; transport?.publish();
    }).catch(error => { fileStatus = `Download failed: ${String(error)}`; transport?.publish(); }); } } }, [uiIcon("download"), "Download"]),
    h("button", { attrs: { type: "button", "data-browser-action": "copy" }, on: { click: (event: Event) => {
      const button = event.currentTarget as HTMLButtonElement;
      void copyCode(snippet.code).then(() => { button.title = "Contents copied"; })
        .catch((error: unknown) => { fileStatus = String(error); transport?.publish(); });
    } } }, [uiIcon("copy"), "Copy"]),
  ]);
  const application = new SpaApplication(mount, {
    ...(options.storage ? {personaStorage: options.storage} : {}),
    paletteCommandVisible: command => browserCommandVisible(command.id, transport?.mode ?? "general"),
    paletteCommands: () => [{ id: "local:turn-limits", label: "Turn limits", description: "Timeout, maximum model/tool rounds and transient retries for this session", icon: "command", behavior: "open_panel", accelerator: null, enabled: true, disabled_reason: null, run: () => limits?.open() }, { id: "local:compact-session", label: "Compact session context", description: "Summarize model context; keep this session, transcript, files and runtime state", icon: "command", behavior: "execute", accelerator: null,
      enabled: transport?.canCompact ?? false, disabled_reason: transport?.busy ? "Finish or stop the running turn first" : "Send a message with a configured connection first",
      run: () => { void transport?.compact().catch(error => { if (transport) { transport.persistenceStatus = `Compaction: ${String(error)}`; transport.publish(); } }); } }],
    headerStatus: () => browserHeaderStatus(transport, directory.name),
    headerStatusContent: () => renderBrowserHeaderStatus(transport, directory.name),
    status: () => renderBrowserStatus(transport, pinnedStats, () => transport?.publish()),
    promptPlaceholder: "Message your model…",
    systemPromptDetails: () => transport?.promptDetails ?? null,
    sessionsNotice: () => [transport?.persistenceStatus, exportStatus].filter(Boolean).join(" "),
    sessionActions: (id) => h("span.browser-code-actions", [options.storage instanceof BrowserSessionStorage ? h("button", {
      attrs: { type: "button", "data-session-database": id, title: "Download this session and its files as a SQLite database" },
      props: { disabled: exporting }, on: { click: async () => {
        if (exporting) return;
        exporting = true; exportStatus = "Preparing session database…"; transport?.publish();
        try {
          const bytes = (options.storage as BrowserSessionStorage).exportDatabase(id);
          const saved = await downloadBlob(new Blob([new Uint8Array(bytes)], {type: "application/vnd.sqlite3"}), `lethetic-session-${id}.db`);
          exportStatus = saved ? "Session database saved. Connection keys are excluded." : "Export cancelled.";
        } catch (error) { exportStatus = `Export failed: ${String(error)}`; }
        finally { exporting = false; transport?.publish(); }
      } }
    }, [uiIcon("download"), "Database (.db)"]) : null, h("button", { attrs: { type: "button", "data-session-export": id, title: "Download this session, its files and debug information as .tar.gz" },
      props: { disabled: exporting }, on: { click: () => {
        if (!transport || exporting) return;
        exporting = true; exportStatus = "Preparing debug bundle…"; transport.publish();
        void (async () => {
          try {
            const session = transport!.sessionForExport(id);
            const current = id === transport!.state.session.session_id;
            const bundle = await sessionDebugBundle(session, {
              browser: navigator.userAgent, language: navigator.language, viewport: { width: innerWidth, height: innerHeight },
              current, busy: current && transport!.busy, lifecycle: lifecycleDiagnostics(),
              ...(current ? { status: transport!.state.status, activity: transport!.state.activity, debugger: transport!.state.debugger } : {}),
            }, [...transport!.relaySecrets, ...transport!.options.connections.flatMap(connection => connection.apiKey ? [connection.apiKey] : [])]);
            const saved = await downloadSessionBundle(bundle, id); exportStatus = saved ? "Debug bundle saved. Connection keys are excluded." : "Export cancelled.";
          } catch (error) { exportStatus = `Export failed: ${String(error)}`; }
          finally { exporting = false; transport?.publish(); }
        })();
      } } }, [uiIcon("download"), "Export debug (.tar.gz)"])]),
    files: { local: true, viewBytes: FILE_BYTES, nativeText: () => plainText, count: () => files.snapshotWorkspace().files.length, entries: entries => fileTools.sort(entries), entryTitle: path => fileTools.title(path), scope: options.storage ? "Session files · synced with code runtime" : "Session files · this tab only", rootLabel: "Session files",
      previewContent: state => {
        const file = state.selectedPath ? files.get(state.selectedPath) : undefined;
        if (!file || sourceViews.has(file.path)) return null;
        if (renderedPreview?.path !== file.path || renderedPreview.data !== file.data) {
          renderedPreview = { path: file.path, data: file.data, document: canRenderFile(file) ? workspacePreview(file) : null };
        }
        return renderedPreview.document === null ? null : h("iframe", { key: file.path, attrs: { class: "files-inline-preview", title: `Preview: ${file.path}`, sandbox: "", referrerpolicy: "no-referrer" }, props: { srcdoc: renderedPreview.document } });
      },
      expandRendered: state => {
        const file = state.selectedPath ? files.get(state.selectedPath) : undefined;
        if (!file || !canRenderFile(file) || sourceViews.has(file.path)) return false;
        preview.openFile(file, fileTools.sortFiles().filter(canPreviewFile), true); return true;
      },
      actions: state => h("span.browser-file-actions", [upload.render(state.directory), fileTools.render(),
        nativeAvailable() ? h("button", { attrs: { type: "button", title: directory.status }, props: { disabled: !!transport?.busy || choosingDirectory }, on: { click: () => {
          if (transport?.busy || choosingDirectory) return;
          choosingDirectory = true;
          void (async () => {
            try {
              if (directory.name) await directory.revoke();
              else {
                // Import into the current session explicitly; switching never happens during a turn.
                if (!confirm("Open a real folder? Its files will replace this session's virtual files. Changes will be saved to that folder. Hidden files and links are excluded. Disk files are never deleted.")) return;
                const session = transport?.state.session.session_id;
                const workspace = await directory.choose();
                if (transport?.busy || session !== transport?.state.session.session_id) { await directory.revoke(); throw new Error("Session changed while selecting a directory; choose it again when idle."); }
                if (workspace) files.replaceWorkspace(workspace);
              }
              fileStatus = directory.status;
            } catch (error) { fileStatus = String(error); }
            finally { choosingDirectory = false; transport?.publish(); }
          })();
        } } }, [uiIcon("folder"), directory.name ? `Folder: ${directory.name} · Disconnect` : "Virtual · Open folder"]) : null]),
      contentActions: (state) => h("span.browser-file-actions", [
        state.selectedPath && /\.(cs|csproj|py)$/i.test(state.selectedPath) ? h("button", {
          attrs: { type: "button", "data-file-action": "run", title: /\.py$/i.test(state.selectedPath) ? "Run this file in this session's Python worker" : "Compile and run this file in this session's .NET worker" },
          props: { disabled: transport?.busy }, on: { click: () => {
            if (state.selectedPath) void (/\.py$/i.test(state.selectedPath) ? transport?.runPythonFile(state.selectedPath) : transport?.runDotnetFile(state.selectedPath))?.catch(error => { fileStatus = String(error); transport?.publish(); });
          } },
        }, [uiIcon("run"), "Run"]) : null,
        !state.preview || (canRenderFile(files.get(state.preview.path)) && !sourceViews.has(state.preview.path)) ? null : h("button", {
          attrs: { type: "button", "data-file-action": "text-mode", title: plainText ? "Use Monaco with syntax highlighting and line numbers" : "Use plain text for native selection", "aria-pressed": String(plainText) },
          on: { click: () => { plainText = !plainText; transport?.publish(); } },
        }, [uiIcon(plainText ? "code" : "file"), plainText ? "Monaco" : "Plain text"]),
        !state.selectedPath || !state.preview || !canRenderFile(files.get(state.selectedPath)) ? null : h("button", { attrs: { type: "button", "data-file-action": "source", "aria-pressed": String(sourceViews.has(state.selectedPath)), title: "Switch between rendered preview and selectable source text" }, on: { click: () => {
          if (state.selectedPath) { if (!sourceViews.delete(state.selectedPath)) sourceViews.add(state.selectedPath); transport?.publish(); }
        } } }, [uiIcon(sourceViews.has(state.selectedPath ?? "") ? "preview" : "code"), sourceViews.has(state.selectedPath ?? "") ? "Rendered" : "Source"]),
        h("button", { attrs: { type: "button", "data-file-action": "new" }, props: { disabled: transport?.busy }, on: { click: () => editor?.open(state.directory ? state.directory + "/untitled.txt" : "untitled.txt", "") } }, [uiIcon("add"), "New file"]),
        h("button", { attrs: { type: "button", "data-file-action": "edit" }, props: { disabled: !state.preview || transport?.busy }, on: { click: () => { if (state.preview) editor?.open(state.preview.path, state.preview.content, true); } } }, [uiIcon("edit"), "Edit"]),
        h("button", { attrs: { type: "button", "data-file-action": "preview" }, props: { disabled: !canPreviewFile(state.selectedPath ? files.get(state.selectedPath) : undefined) }, on: { click: () => {
          try { const file = state.selectedPath ? files.get(state.selectedPath) : undefined; if (file) preview.openFile(file, fileTools.sortFiles().filter(canPreviewFile), false, sourceViews.has(file.path)); }
          catch (error) { fileStatus = String(error); transport?.publish(); }
        } } }, [uiIcon("preview"), "Preview"]),

        h("button", { attrs: { type: "button", "data-file-action": "delete" }, props: { disabled: !state.selectedPath || transport?.busy }, on: { click: () => {
          if (state.selectedPath && confirm(`Delete ${state.selectedPath} from this session?`)) {
            try { files.remove(state.selectedPath); } catch (error) { fileStatus = String(error); transport?.publish(); }
          }
        } } }, [uiIcon("trash"), "Delete"]),
        h("span", { attrs: { role: "status" } }, fileStatus),
      ]) },
    headerActions: () => { sites?.refresh(); const connection = connectionIndicator(transport); return h("div", { attrs: { class: "browser-header-actions" } }, [
      h("button", { attrs: { type: "button", "data-browser-action": "connections", "data-connection-state": connection.state, title: connection.title, "aria-label": `Connections — ${connection.title}` }, on: { click: () => settings?.open() } }, [uiIcon("connection"), "Connections"]),
      h("button", { attrs: { type: "button", "data-browser-action": "sessions" }, on: { click: () => transport?.send({ type: "invoke_command", command_id: "sessions" }, 0) } }, [uiIcon("history"), "Sessions"]),
      transport?.mode === "dotnet-wasm" || transport?.pythonEnabled ? h("button", { attrs: { type: "button", "data-browser-action": "sites", title: "Open an interactive session site" }, on: { click: () => sites?.open() } }, [uiIcon("site"), `Sites (${transport.sites.length})`]) : null,
    ]); },
    codeActions: (code, language) => snippetActions({ code, language: language ?? "text" }),
    blockActions: (block) => block.tool?.kind === "call" && ["python", "dotnet"].includes(block.tool.tool_name)
      ? h("div", codeSnippets(block).map(snippetActions)) : null,
    externalDialogOpen: () => !!document.querySelector("dialog[open]"),
  });
  let initializing = true;
  let routeError = "";
  let initialSession = "latest";
  if (options.sessionRouting) { try { initialSession = sessionRoute(location.hash); } catch (error) { routeError = String(error); initialSession = "new"; } }
  transport = new LocalTransport(application, { ...options, connections, initialSession, files,
    onFilesChanged: paths => {
      application.refreshFiles(paths);
      if (directory.name) void directory.sync(files.snapshotWorkspace()).then(() => { fileStatus = directory.status; transport?.publish(); })
        .catch(error => { fileStatus = `Folder save failed: ${String(error)}`; transport?.publish(); });
    },
    onSessionChanged: (id) => {
      void directory.revoke().catch(error => { fileStatus = String(error); transport?.publish(); });
      application.resetFiles(); sourceViews.clear(); preview.clear(); sites?.clear(); if (editor?.dialog.open) editor.dialog.close(); fileStatus = "";
      if (!options.sessionRouting) return;
      const hash = `#/sessions/${id}`;
      if (hash !== location.hash) {
        const replace = initializing || !location.hash || ["#/new", "#/latest"].includes(location.hash);
        history[replace ? "replaceState" : "pushState"](null, "", location.pathname + location.search + hash);
      }
    } });
  limits = createTurnLimitsDialog(transport);
  sites = createSitesDialog(transport);
  settings = createConnectionsDialog(transport, options.storage, options.credentialStorage);
  editor = createFileEditor(files, (path) => { void application.openFile(path); });
  const persistence = options.storage instanceof BrowserSessionStorage ? options.storage : null;
  const notice = document.createElement("div"); notice.className = "browser-persistence"; notice.setAttribute("role", "status");
  const noticeText = document.createElement("span"), retry = document.createElement("button");
  retry.type = "button"; retry.textContent = "Retry save";
  retry.addEventListener("click", () => { void persistence?.flush().catch(() => {}); });
  notice.append(noticeText, retry);
  if (persistence) root.prepend(notice);
  const saving = (): void => {
    notice.hidden = !persistence?.pending; noticeText.textContent = persistence?.status ?? "";
    retry.hidden = !persistence?.status.startsWith("Session not saved:"); transport?.publish();
  };
  const unsubscribeStorage = persistence?.subscribe(saving);
  const protectUnsaved = (event: BeforeUnloadEvent): void => { if (persistence?.pending) { event.preventDefault(); event.returnValue = ""; } };
  if (persistence) globalThis.addEventListener("beforeunload", protectUnsaved);
  application.attachTransport(transport); transport.start(); saving();
  initializing = false;
  if (routeError) { transport.persistenceStatus = routeError; transport.publish(); }
  const navigate = (): void => {
    try { transport?.openSession(sessionRoute(location.hash)); }
    catch (error) {
      if (transport) {
        transport.persistenceStatus = error instanceof Error ? error.message : String(error);
        history.replaceState(null, "", location.pathname + location.search + `#/sessions/${transport.state.session.session_id}`);
      }
    }
    transport?.publish();
  };
  if (options.sessionRouting) globalThis.addEventListener("hashchange", navigate);
  const activate = (): void => { if (!connections.length) settings?.open(); else document.getElementById("composer-input")?.focus(); };
  if (!options.startInBackground) activate();
  return { initialize: onProgress => transport!.initialize(onProgress), activate, destroy(): void { globalThis.removeEventListener("pagehide", resetNativeWork); resetNativeWork(); void directory.revoke().catch(() => {}); unsubscribeStorage?.(); globalThis.removeEventListener("beforeunload", protectUnsaved); upload.destroy(); globalThis.removeEventListener("hashchange", navigate); transport?.dispose(); application.dispose(); settings?.destroy(); limits?.destroy(); sites?.destroy(); editor?.destroy(); preview.destroy(); root.replaceChildren(); } };
}
