import { buttonIconHtml } from "../../web/src/icons.js";
import { createDialog, errorMessage, field } from "./dialog.js";
import type { LocalTransport } from "./local-transport.js";
import type { WireRequest, WireResponse } from "./dotnet.js";

type Preview = { reload(): Promise<void>; navigate(path: string): Promise<void>; destroy(): void };
type PreviewFactory = (options: { iframe: HTMLIFrameElement; port: number; path: string;
  request(value: WireRequest): Promise<WireResponse>; onNavigate(path: string): void }) => Promise<Preview>;

export function createSitesDialog(transport: LocalTransport): { open(): void; refresh(): void; clear(): void; destroy(): void } {
  const dialog = createDialog("Interactive sites", `
    <p class="browser-note">Local Python and C# sites. Files created here stay in this session.</p>
    <p class="browser-note" data-field="site-network" role="status"></p>
    <div class="browser-site-toolbar">
      <label>Site <select data-field="site-port" aria-label="Virtual port"></select></label>
      <label>Path <input data-field="site-path" value="/" placeholder="/index.html"></label>
      <button type="button" data-field="site-open">${buttonIconHtml("preview")}Open</button>
      <button type="button" data-field="site-reload">${buttonIconHtml("refresh")}Reload</button>
      <button type="button" data-field="site-stop">${buttonIconHtml("stop")}Stop site</button>
    </div>
    <p data-field="site-status" role="status"></p>
    <iframe hidden data-field="site-frame" title="Interactive session site" sandbox="allow-scripts allow-forms" referrerpolicy="no-referrer"></iframe>
    <details class="browser-note"><summary>Preview support</summary><p>Classic JavaScript, CSS, images and fetch to Python or C# routes work here. For external APIs, use pyodide.http.pyfetch in an async Python handler, or HttpClient in a C# route with Network access enabled. Direct external requests from preview JavaScript, ES modules and WebSockets are unavailable. Files created by requests appear in Files.</p></details>`);
  dialog.classList.add("browser-sites");
  const select = field<HTMLSelectElement>(dialog, "site-port");
  const path = field<HTMLInputElement>(dialog, "site-path");
  const frame = field<HTMLIFrameElement>(dialog, "site-frame");
  const status = (message: string): void => { field(dialog, "site-status").textContent = message; };
  const button = (name: string): HTMLButtonElement => field(dialog, `site-${name}`);
  let preview: Preview | null = null, activeSession = "", activePort = 0, opening = false, generation = 0;
  let queue: Promise<unknown> = Promise.resolve();
  function clear(): void {
    generation++; preview?.destroy(); preview = null; frame.removeAttribute("src"); frame.hidden = true;
    activeSession = ""; activePort = 0; opening = false;
  }
  function refresh(): void {
    if (!dialog.open) return;
    const language = transport.pythonEnabled ? "Python" : "C#";
    field(dialog, "site-network").textContent = transport.networkAccess
      ? `${language} Network access: enabled · ${transport.dotnetRelay ? "HTTP relay" : "direct requests; destination must allow CORS"}. Preview JavaScript can fetch this site's ${language} routes.`
      : `${language} Network access: disabled. External API calls are blocked. Enable Network access in Connections → Runtime & security, then run the site file again.`;
    const sites = transport.sites;
    if (activeSession && (activeSession !== transport.state.session.session_id || !transport.siteAccess || !sites.some(site => site.port === activePort))) clear();
    const choices = sites.map(site => String(site.port));
    if (Array.from(select.options).map(option => option.value).join() !== choices.join()) {
      const previous = select.value;
      select.replaceChildren(...sites.map(site => new Option(`:${site.port} · ${site.webRoot ?? (transport.pythonEnabled ? "Python routes" : "C# routes")}`, String(site.port))));
      if (choices.includes(previous)) select.value = previous;
    }
    button("open").disabled = opening || transport.busy || !sites.length;
    button("reload").disabled = opening || transport.busy || !preview;
    button("stop").disabled = opening || transport.busy || !sites.length;
    if (!transport.siteAccess) status("Interactive sites are off. Enable them in Connections → Runtime & security.");
    else if (!sites.length) status(transport.pythonEnabled
      ? "No site is running. Run a .py file that calls lethetic_wire.listen."
      : "No site is running. Run a .cs or .csproj that calls DotnetDide.Wire.ListenAsync.");
  }
  async function openPreview(): Promise<void> {
    if (opening || transport.busy || !select.value) return;
    clear(); opening = true;
    const mine = generation, sessionId = transport.state.session.session_id, port = Number(select.value);
    activeSession = sessionId; activePort = port;
    status("Opening site…"); refresh();
    try {
      const sdk = await import(new URL("../../runtimes/dotnetdide/dotnetdide.js", import.meta.url).href) as { createWirePreview: PreviewFactory };
      if (mine !== generation) return;
      const created = await sdk.createWirePreview({ iframe: frame, port, path: path.value || "/",
        onNavigate: value => { path.value = value; },
        request: value => {
          // Serialize parallel fetches from a page while keeping model turns exclusive.
          const next = queue.catch(() => {}).then(async () => {
            if (mine !== generation || !dialog.open) throw new Error("Preview closed.");
            const response = await transport.requestSite(value, sessionId);
            if (mine === generation) status(`${value.method} ${value.path} → ${response.status}${response.error ? " · " + response.error : ""}`);
            return response;
          });
          queue = next; return next;
        },
      });
      if (mine !== generation) created.destroy(); else { preview = created; frame.hidden = false; status(`Running on virtual port ${port}`); }
    } catch (error) { if (mine === generation) { clear(); status(errorMessage(error)); } }
    finally { if (mine === generation) opening = false; refresh(); }
  }
  button("open").onclick = () => { void openPreview(); };
  path.onkeydown = event => { if (event.key === "Enter") { event.preventDefault(); void openPreview(); } };
  button("reload").onclick = () => { void openPreview(); };
  button("stop").onclick = () => {
    const port = Number(select.value); clear();
    void transport.stopSite(port).then(() => status(`Stopped virtual port ${port}`))
      .catch(error => status(errorMessage(error))).finally(refresh);
  };
  dialog.addEventListener("close", clear);
  return {
    open(): void { if (!dialog.open) dialog.showModal(); refresh(); if (select.value && !preview) void openPreview(); },
    refresh, clear,
    destroy(): void { clear(); dialog.remove(); },
  };
}
