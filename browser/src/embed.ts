import { h } from "../../web/lib/snabbdom/build/index.js";
import { SpaApplication } from "../../web/src/app.js";
import type { Connection, Tool } from "./sdk.js";
import { loadConnections, validateConnection } from "./connection.js";
import { LocalTransport } from "./local-transport.js";
import { createConnectionsDialog } from "./connections-dialog.js";
import { createVaultDialog } from "./vault-dialog.js";
import { codeSnippets } from "./code-snippets.js";
import { renderBrowserStatus } from "./status-view.js";

export interface EmbedOptions {
  connections?: Connection[];
  /** Optional persistence for connection metadata and the encrypted code library. */
  storage?: Storage;
  tools?: Tool[];
  systemPrompt?: string;
  python?: boolean;
  pyodideIndexURL?: string;
  /** Retained for compatibility; the shared remote-mode UI is already compact. */
  compact?: boolean;
}

/** Mount the same SpaApplication used by native remote mode, backed by local WASM.
 * The shared UI uses document-level keyboard and focus handling: mount one per document.
 */
export function mountLethetic(root: HTMLElement, options: EmbedOptions = {}): { destroy(): void } {
  const connections = options.connections?.map(validateConnection) ??
    (options.storage ? loadConnections(options.storage) : []);
  const vault = createVaultDialog(options.storage);
  let settings: ReturnType<typeof createConnectionsDialog> | null = null;
  let transport: LocalTransport | null = null;
  const mount = document.createElement("div"); root.replaceChildren(mount);
  const application = new SpaApplication(mount, {
    status: () => renderBrowserStatus(transport),
    promptPlaceholder: "Message your model…",
    headerActions: () => h("div", { attrs: { class: "browser-header-actions" } }, [
      h("button", { attrs: { type: "button", "data-browser-action": "connections" }, on: { click: () => settings?.open() } }, "Connections"),
      h("button", { attrs: { type: "button", "data-browser-action": "code" }, on: { click: () => vault.open() } }, "Code library"),
    ]),
    blockActions: (block) => {
      const snippets = codeSnippets(block);
      return snippets.length ? h("div", { attrs: { class: "browser-code-actions" } }, snippets.map((snippet, index) =>
        h("button", { attrs: { type: "button", "data-browser-action": "save-code" },
          on: { click: () => vault.open(snippet) } }, snippets.length === 1 ? "Save code encrypted" : `Save code ${index + 1} encrypted`))) : null;
    },
    externalDialogOpen: () => vault.dialog.open || (settings?.dialog.open ?? false),
  });
  transport = new LocalTransport(application, { ...options, connections });
  settings = createConnectionsDialog(transport, options.storage);
  application.attachTransport(transport); transport.start();
  if (!connections.length) settings.open();
  return { destroy(): void { transport?.dispose(); application.dispose(); settings?.destroy(); vault.destroy(); root.replaceChildren(); } };
}
