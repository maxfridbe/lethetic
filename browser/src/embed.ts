import { h } from "../../web/lib/snabbdom/build/index.js";
import { SpaApplication } from "../../web/src/app.js";
import type { Connection, Tool } from "./sdk.js";
import { loadConnections, validateConnection } from "./connection.js";
import { LocalTransport } from "./local-transport.js";
import { createConnectionsDialog } from "./connections-dialog.js";
import { codeSnippets } from "./code-snippets.js";
import { renderBrowserStatus } from "./status-view.js";
import { copyCode, createFilePreview, downloadCode, snippetName } from "./file-actions.js";
import { sessionRoute } from "./sessions.js";
import { BrowserFiles } from "./files.js";
import { createFileEditor } from "./file-editor.js";

export interface EmbedOptions {
  connections?: Connection[];
  /** Optional device-local persistence for connections, sessions and files. */
  storage?: Storage;
  tools?: Tool[];
  systemPrompt?: string;
  python?: boolean;
  pyodideIndexURL?: string;
  /** Retained for compatibility; the shared remote-mode UI is already compact. */
  compact?: boolean;
  /** Enable #/sessions/<id>, #/latest and #/new. Off by default in embedded hosts. */
  sessionRouting?: boolean;
}

/** Mount the same SpaApplication used by native remote mode, backed by local WASM.
 * The shared UI uses document-level keyboard and focus handling: mount one per document.
 */
export function mountLethetic(root: HTMLElement, options: EmbedOptions = {}): { destroy(): void } {
  const connections = options.connections?.map(validateConnection) ??
    (options.storage ? loadConnections(options.storage) : []);
  const files = new BrowserFiles(options.storage);
  let editor: ReturnType<typeof createFileEditor> | null = null;
  const preview = createFilePreview();
  let settings: ReturnType<typeof createConnectionsDialog> | null = null;
  let transport: LocalTransport | null = null;
  let fileStatus = "";
  const mount = document.createElement("div"); root.replaceChildren(mount);
  const application = new SpaApplication(mount, {
    status: () => renderBrowserStatus(transport),
    promptPlaceholder: "Message your model…",
    sessionsNotice: () => transport?.persistenceStatus ?? "",
    files: { local: true, scope: options.storage ? "Local files · saved on this device" : "Local files · this tab only", rootLabel: "Local files",
      actions: (state) => h("span.browser-file-actions", [
        h("button", { attrs: { type: "button", "data-file-action": "new" }, on: { click: () => editor?.open(state.directory ? state.directory + "/untitled.txt" : "untitled.txt", "") } }, "New file"),
        h("button", { attrs: { type: "button", "data-file-action": "edit" }, props: { disabled: !state.preview }, on: { click: () => { if (state.preview) editor?.open(state.preview.path, state.preview.content, true); } } }, "Edit"),
        h("button", { attrs: { type: "button", "data-file-action": "preview" }, props: { disabled: !state.preview }, on: { click: () => { if (state.preview) preview.open(state.preview.content, "", state.preview.path); } } }, "Preview"),
        h("button", { attrs: { type: "button", "data-file-action": "copy" }, props: { disabled: !state.preview }, on: { click: () => {
          if (state.preview) void copyCode(state.preview.content).then(() => { fileStatus = "Contents copied."; transport?.publish(); }).catch((error: unknown) => { fileStatus = String(error); transport?.publish(); });
        } } }, "Copy contents"),
        h("button", { attrs: { type: "button", "data-file-action": "delete" }, props: { disabled: !state.preview }, on: { click: () => {
          if (state.preview && confirm(`Delete ${state.preview.path} from this device?`)) {
            try { files.remove(state.preview.path); application.refreshFiles(); } catch (error) { fileStatus = String(error); transport?.publish(); }
          }
        } } }, "Delete"),
        h("span", { attrs: { role: "status" } }, fileStatus),
      ]) },
    headerActions: () => h("div", { attrs: { class: "browser-header-actions" } }, [
      h("button", { attrs: { type: "button", "data-browser-action": "connections" }, on: { click: () => settings?.open() } }, "Connections"),
      h("button", { attrs: { type: "button", "data-browser-action": "sessions" }, on: { click: () => transport?.send({ type: "invoke_command", command_id: "sessions" }, 0) } }, "Sessions"),
    ]),
    blockActions: (block) => {
      const snippets = codeSnippets(block);
      return snippets.length ? h("div", [
        ...snippets.map((snippet, index) => h("div.browser-code-actions", [
          h("button", { attrs: { type: "button", "data-browser-action": "save-code" }, on: { click: () => editor?.open(snippetName(snippet), snippet.code) } }, snippets.length === 1 ? "Save to Files" : `Save code ${index + 1} to Files`),
          h("button", { attrs: { type: "button", "data-browser-action": "preview" }, on: { click: () => preview.open(snippet.code, snippet.language, snippetName(snippet)) } }, "Preview"),
          h("button", { attrs: { type: "button", "data-browser-action": "download" }, on: { click: () => downloadCode(snippet.code, snippetName(snippet)) } }, "Download"),
          h("button", { attrs: { type: "button", "data-browser-action": "copy" }, on: { click: () => {
            void copyCode(snippet.code).then(() => { fileStatus = "Contents copied."; transport?.publish(); })
              .catch((error: unknown) => { fileStatus = error instanceof Error ? error.message : String(error); transport?.publish(); });
          } } }, "Copy contents"),
        ])), h("span", { attrs: { role: "status" } }, fileStatus),
      ]) : null;
    },
    externalDialogOpen: () => !!document.querySelector("dialog[open]"),
  });
  let initializing = true;
  let routeError = "";
  let initialSession = "latest";
  if (options.sessionRouting) { try { initialSession = sessionRoute(location.hash); } catch (error) { routeError = String(error); initialSession = "new"; } }
  transport = new LocalTransport(application, { ...options, connections, initialSession, files,
    onSessionChanged: (id) => {
      if (!options.sessionRouting) return;
      const hash = `#/sessions/${id}`;
      if (hash !== location.hash) {
        const replace = initializing || !location.hash || ["#/new", "#/latest"].includes(location.hash);
        history[replace ? "replaceState" : "pushState"](null, "", location.pathname + location.search + hash);
      }
    } });
  settings = createConnectionsDialog(transport, options.storage);
  editor = createFileEditor(files, (path) => { void application.openFile(path); });
  application.attachTransport(transport); transport.start();
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
  if (!connections.length) settings.open();
  return { destroy(): void { globalThis.removeEventListener("hashchange", navigate); transport?.dispose(); application.dispose(); settings?.destroy(); editor?.destroy(); preview.destroy(); root.replaceChildren(); } };
}
