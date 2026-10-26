import { connectionLimits, connectionLimitsHtml } from "./connection-limits.js";
import { nativeAvailable } from "./native-host.js";
import { buttonIconHtml } from "../../web/src/icons.js";
import { connectionHealthIdentity as identity, validateConnection, requireChatCredential, normalizeApiKey, saveConnections, saveConnectionKeys } from "./connection.js";
import { fetchModels } from "./models.js";
import type { AvailableModel } from "./models.js";
import { createRuntime } from "./sdk.js";
import type { Connection, LetheticRuntime } from "./sdk.js";
import type { LocalTransport } from "./local-transport.js";
import { createDialog, errorMessage, field } from "./dialog.js";

export function createConnectionsDialog(transport: LocalTransport, storage?: Storage, credentialStorage?: Storage): {
  dialog: HTMLDialogElement; open(): void; destroy(): void;
} {
  const dialog = createDialog("Connections", `
    <section class="connection-active" aria-label="Current session connection" data-field="active-summary"></section>
    <div class="connection-layout">
      <section class="connection-sidebar" aria-label="Saved connections">
        <div class="connection-section-heading"><h3>Saved connections</h3><button type="button" data-field="new">${buttonIconHtml("add")}New</button></div>
        <div class="connection-cards" data-field="connections"></div>
      </section>
      <section class="connection-editor">
        <h3 data-field="editor-title">New connection</h3>
        <p class="connection-hint" data-field="edit-state" role="status"></p>
        <form data-field="connection-form" class="browser-connection-form">
          <label>Name<input data-field="name" required maxlength="100" autocomplete="off"></label>
          <label>Protocol<select data-field="provider"><option value="openai">OpenAI-compatible</option><option value="anthropic">Anthropic Messages</option></select></label>
          <label>Endpoint URL<input data-field="endpoint" type="url" required placeholder="https://your-server/v1/chat/completions" autocomplete="off" spellcheck="false"></label>
          <label>Model<input data-field="model" required autocomplete="off" spellcheck="false"></label>
          <label>Thinking · OpenRouter<select data-field="thinking"><option value="default">Model default</option><option value="enabled">On · request thinking</option><option value="disabled">Off</option></select><small>For OpenRouter or endpoints supporting its reasoning parameter. Returned reasoning appears in Thinking blocks. Some models do not expose it; others require it. Leave Model default for other endpoints.</small></label>
          <label>API key (optional)<span class="connection-key-input"><input data-field="key" name="model-api-key" type="password" autocomplete="off" autocapitalize="off" autocorrect="off" spellcheck="false" placeholder="Provider API key — not a session URL"><button type="button" data-field="show-key" aria-label="Show API key" aria-pressed="false">${buttonIconHtml("preview")}Show</button></span><small data-field="key-state" role="status"></small></label>
          <div class="panel-toolbar">
            <button class="primary-button" type="submit" data-field="save">${buttonIconHtml("save")}Save changes</button>
            <button type="button" data-field="use">${buttonIconHtml("connection")}Use this connection</button>
            <button type="button" data-field="duplicate">${buttonIconHtml("copy")}Duplicate with key</button>
            <button type="button" data-field="remove">${buttonIconHtml("trash")}Remove</button>
          </div>
          <div class="panel-toolbar">
            <button type="button" data-field="test">${buttonIconHtml("connection")}Test connection</button>
            <button type="button" data-field="fetch-models">${buttonIconHtml("refresh")}Fetch models</button>
            <span data-field="test-state" class="connection-test-state" role="status"></span>
          </div>
          <section class="browser-model-picker" data-field="model-picker" hidden>
            <label>Filter models<input data-field="model-filter" type="search" placeholder="Search model IDs or names" autocomplete="off"></label>
            <label>Available models<select data-field="model-list" size="6"></select></label>
            <p data-field="model-count" role="status"></p>
            <button type="button" data-field="use-model">${buttonIconHtml("check")}Use selected model</button>
          </section>
        </form>
      </section>
    </div>
    ${connectionLimitsHtml}
    <fieldset class="browser-runtime"><legend>Runtime &amp; security · current session</legend>
    <label>Code runtime <select data-field="mode"><option value="general">Chat</option><option value="python">Python · Pyodide</option><option value="dotnet-wasm">.NET 11 · WASM</option></select></label>
    <label${nativeAvailable() ? " hidden" : ""}><input data-field="network" type="checkbox"> Network access (Python and .NET code)</label>
    <p class="browser-note">Required for Python pyfetch and C# HttpClient calls to external APIs, including site routes and additional Python package downloads. Bundled runtime packages still load with it off. Changing this setting stops running sites; run the site file again afterward. Session files and chat are kept.</p>
    ${nativeAvailable() ? '<label>Code network access<select data-field="native-network-mode"><option value="off">Off</option><option value="direct">Direct network</option><option value="tor">Tor only · embedded Arti</option></select></label><p class="browser-note">Tor only routes Python/.NET HTTP through Tor with no direct fallback or local DNS. First use may take time to connect. Local/IP addresses and direct NuGet downloads are blocked. Model connections and bundled runtime assets keep their own routes; this is not Tor Browser.</p>' : ""}
    <label><input data-field="sites" type="checkbox"> Interactive sites (Python or ASP.NET virtual ports and isolated preview)</label>
    <label><input data-field="nuget" type="checkbox"> NuGet package access (direct downloads from NuGet.org)</label>
    <a href="${new URL("../../runtimes/dotnetdide/llm.md", import.meta.url).href}" target="_blank" rel="noopener">.NET usage guide</a>
    <a href="${new URL("../../runtimes/dotnetdide/docs/media-and-packages.md", import.meta.url).href}" target="_blank" rel="noopener">Media libraries &amp; NuGet guide</a>
    <p data-field="permissions" class="browser-note" role="status"></p>
    <div class="browser-route-controls" data-field="browser-network-route"${nativeAvailable() ? " hidden" : ""}>
    <label>HTTP route (Python and .NET) <select data-field="network-route"><option value="direct">Direct · destination CORS required</option><option value="relay">HTTP relay · optional server-side SOCKS5 / Tor</option></select></label>
    <div data-field="relay-fields" class="browser-relay-fields" hidden>
      <label>Relay endpoint<input data-field="relay-endpoint" type="url" placeholder="https://your-relay.example/relay" autocomplete="off" spellcheck="false"></label>
      <label>Relay token<input data-field="relay-token" type="password" autocomplete="off" spellcheck="false" maxlength="1024"></label>
      <small data-field="relay-key-state" role="status"></small>
      <p class="browser-note">For SOCKS5 or Tor, run the native .NET relay and configure its upstream proxy. <a href="https://github.com/maxfridbe/dotnetdide/blob/master/docs/networking.md" target="_blank" rel="noopener noreferrer">Relay &amp; Tor setup</a></p>
      <p class="browser-note">Relay: 8 MiB bodies, no browser cookies, redirects or direct fallback. Model connections and bundled runtime packages keep their own routes.</p>
    </div>
    <button type="button" data-field="apply-route">${buttonIconHtml("save")}Apply HTTP route</button>
    <p data-field="route-state" role="status" class="browser-note"></p>
    <p class="browser-note">Mode, permission and route changes keep this chat and its files; code variables restart. Direct HTTP needs destination CORS.</p>
    </div>
    ${nativeAvailable() ? '<p class="browser-note">Direct network uses the app’s HTTP client; Tor only uses its embedded Rust Tor client. No proxy setup or destination CORS required. Mode and permission changes keep chat and files; code variables restart.</p>' : ""}
    </fieldset>
    <p class="connection-hint">${credentialStorage ? "Default key storage survives refresh, but closing the tab or restarting the app can clear it. Re-enter the key and Save & use to resume the same session." : "Keys stay in memory."} Switching connections keeps this session’s history, files, and code variables.</p>
    <p data-field="status" role="status"></p>`);
  dialog.classList.add("browser-connections");
  const select = (name: string): HTMLSelectElement => field<HTMLSelectElement>(dialog, name);
  const input = (name: string): HTMLInputElement => field<HTMLInputElement>(dialog, name);
  const button = (name: string): HTMLButtonElement => field<HTMLButtonElement>(dialog, name);
  const status = (text: string): void => { field(dialog, "status").textContent = text; };
  const health = transport.connectionHealth;
  let selected = transport.connection ? transport.selected : -1;
  let operation: AbortController | null = null;
  let operationKey: string | null = null;
  let modelOperation: AbortController | null = null;
  let models: AvailableModel[] = [];
  let destroyed = false;
  let autofillTimer: ReturnType<typeof setInterval> | null = null;
  const modelList = field<HTMLSelectElement>(dialog, "model-list");
  const draft = (): Connection => ({ name: input("name").value.trim(), endpoint: input("endpoint").value.trim(),
    model: input("model").value.trim(), provider: select("provider").value as Connection["provider"], apiKey: input("key").value,
    ...(select("provider").value === "openai" && select("thinking").value !== "default" ? { thinking: select("thinking").value as Connection["thinking"] } : {}) });
  const limitsEditor = connectionLimits(dialog, transport, draft);
  const dirty = (): boolean => {
    const saved = transport.options.connections[selected], current = draft();
    return !saved || saved.name !== current.name || identity(saved) !== identity(current);
  };
  const node = (tag: string, text: string, className = ""): HTMLElement => {
    const element = document.createElement(tag); element.textContent = text; element.className = className; return element;
  };
  const healthBadge = (connection: Connection): HTMLElement => {
    let check = health.get(identity(connection));
    try { requireChatCredential(connection); } catch (error) { check = {state: "invalid", message: errorMessage(error)}; }
    const badge = node("span", "", `connection-health is-${check?.state ?? "untested"}`);
    badge.innerHTML = buttonIconHtml("connection");
    badge.append(document.createTextNode(check?.state === "valid" ? "Connected" : check?.state === "invalid" ? "Connection failed" : check?.state === "testing" ? "Testing…" : "Not tested"));
    badge.title = check?.message ?? "Test this connection to verify the endpoint, model and key.";
    return badge;
  };
  const activeSummary = (): void => {
    const summary = field(dialog, "active-summary"), active = transport.connection;
    summary.replaceChildren(node("small", "ACTIVE FOR THIS SESSION"));
    if (active) {
      summary.append(node("strong", active.name), healthBadge(active), node("span", active.model, "connection-active-model"),
        node("span", active.apiKey?.trim() ? "Saved key present" : "No saved key", "connection-key-badge"),
        node("span", `Thinking: ${active.thinking === "enabled" ? "On" : active.thinking === "disabled" ? "Off" : "Model default"}`));
    } else summary.append(node("strong", "No active connection"), node("span", "Add a connection to start chatting."));
  };
  const renderCards = (): void => {
    activeSummary();
    const list = field(dialog, "connections"); list.replaceChildren();
    transport.options.connections.forEach((connection, index) => {
      const active = index === transport.selected, editing = index === selected;
      const card = node("article", "", `connection-card${active ? " is-active" : ""}${editing ? " is-editing" : ""}`);
      card.dataset.connectionIndex = String(index);
      card.append(node("strong", connection.name), node("span", connection.model, "connection-card-model"), healthBadge(connection),
        node("small", connection.apiKey?.trim() ? "Saved key present" : "No saved key"));
      const actions = node("div", "", "connection-card-actions");
      for (const [action, label, icon] of [["use", active ? "Active" : "Use connection", active ? "check" : "connection"], ["edit", editing ? "Editing" : "Edit", "edit"]] as const) {
        const control = document.createElement("button"); control.type = "button"; control.dataset.connectionAction = action;
        control.innerHTML = buttonIconHtml(icon); control.append(document.createTextNode(label));
        control.setAttribute("aria-label", `${label}: ${connection.name}`);
        if (action === "use") { control.disabled = active || transport.busy; control.setAttribute("aria-pressed", String(active)); }
        control.addEventListener("click", () => {
          if (action === "use") activate(index);
          else { selected = index; fill(); status(""); field(dialog, "editor-title").scrollIntoView({ block: "nearest" }); input("name").focus({ preventScroll: true }); }
        });
        actions.append(control);
      }
      card.append(actions); list.append(card);
    });
    if (!transport.options.connections.length) list.append(node("p", "No saved connections yet.", "connection-hint"));
  };
  const updateEditor = (): void => {
    select("thinking").disabled = select("provider").value !== "openai";
    const current = draft(), changed = dirty(), active = selected >= 0 && selected === transport.selected;
    let valid = false;
    try { validateConnection(current); valid = true; } catch { /* Keep incomplete drafts editable. */ }
    field(dialog, "editor-title").textContent = selected < 0 ? "New connection" : `Editing ${transport.options.connections[selected]?.name ?? "connection"}`;
    field(dialog, "edit-state").textContent = transport.busy ? "A turn is running. Finish or stop it before saving or switching connections." : changed ? "Unsaved changes — save before switching connections or closing." : active ? "This connection is active for the current session." : "Saved. Use this connection to switch the current session.";
    let keyError = "";
    try { normalizeApiKey(current.apiKey); } catch (error) { keyError = errorMessage(error); }
    input("key").setCustomValidity(keyError); input("key").setAttribute("aria-invalid", String(!!keyError));
    const keyChanged = (transport.options.connections[selected]?.apiKey?.trim() ?? "") !== (current.apiKey?.trim() ?? "");
    field(dialog, "key-state").textContent = keyError || (current.apiKey?.trim() ? (keyChanged ? "Key entered · not saved for chat. Choose Save & use connection." : "Saved key present") : "No key provided · optional for endpoints without authentication");
    button("save").disabled = !changed || !valid || transport.busy;
    button("use").disabled = !valid || transport.busy || (active && !changed);
    button("use").innerHTML = buttonIconHtml(active && !changed ? "check" : "connection") + (active && !changed ? "Active connection" : changed ? "Save & use connection" : "Use this connection");
    button("duplicate").disabled = selected < 0 || transport.busy;
    button("remove").disabled = selected < 0 || transport.busy;
    button("test").disabled = !valid || operation !== null;
    button("fetch-models").disabled = !!keyError || modelOperation !== null;
    field(dialog, "test-state").replaceChildren(healthBadge(current));
    limitsEditor.describe();
    select("mode").value = transport.mode; select("mode").disabled = transport.busy;
    input("sites").checked = transport.siteAccess;
    input("sites").disabled = transport.busy || transport.mode === "general";
    input("nuget").checked = transport.nugetAccess;
    input("nuget").disabled = transport.busy || transport.mode !== "dotnet-wasm" || transport.networkMode === "tor";
    if (nativeAvailable()) {
      select("native-network-mode").value = transport.networkAccess ? transport.networkMode : "off";
      select("native-network-mode").disabled = transport.busy || transport.mode === "general";
    }
    input("network").checked = transport.networkAccess;
    input("network").disabled = transport.busy || transport.mode === "general";
    field(dialog, "permissions").textContent = transport.mode !== "general"
      ? `Session files: read/write enabled · Network: ${transport.networkAccess ? "enabled" : "disabled"} · Sites: ${transport.siteAccess ? "enabled" : "disabled"}${transport.pythonEnabled ? "" : ` · NuGet: ${transport.nugetAccess ? "enabled (NuGet.org)" : "disabled"}`} · HTTP route: ${transport.networkMode === "tor" ? nativeAvailable() ? "Tor only · embedded Arti" : "Tor unavailable · HTTP blocked" : nativeAvailable() ? "native app · direct" : transport.dotnetRelay ? transport.dotnetRelay.endpoint + (transport.dotnetRelay.token ? " · token present" : " · token required") : "direct (browser CORS)"}`
      : "Code execution is off.";
  };
  const routeEditor = (): void => {
    const relay = select("network-route").value === "relay";
    field(dialog,"relay-fields").hidden = !relay;
    for (const control of [select("network-route"), input("relay-endpoint"), input("relay-token"), button("apply-route")]) control.disabled = transport.busy || transport.mode === "general";
    const saved = transport.dotnetRelay;
    field(dialog,"relay-key-state").textContent = input("relay-token").value
      ? saved?.endpoint === input("relay-endpoint").value && saved.token === input("relay-token").value ? "Relay token present" : "Relay token entered · apply to save"
      : "Relay token required before requests can run";
  };
  const fillRoute = (): void => {
    const relay = transport.dotnetRelay;
    select("network-route").value = relay ? "relay" : "direct";
    input("relay-endpoint").value = relay?.endpoint ?? ""; input("relay-token").value = relay?.token ?? "";
    routeEditor();
  };
  const cancelModels = (): void => {
    modelOperation?.abort(); modelOperation = null; models = [];
    button("fetch-models").disabled = false; field(dialog, "model-picker").hidden = true;
    modelList.replaceChildren(); input("model-filter").value = "";
  };
  const cancelTest = (): void => {
    operation?.abort(); operation = null;
    if (operationKey && health.get(operationKey)?.state === "testing") health.delete(operationKey);
    operationKey = null; limitsEditor.reset(); transport.publish();
  };
  const fill = (): void => {
    cancelModels(); cancelTest();
    const connection = transport.options.connections[selected];
    for (const name of ["name", "endpoint", "model", "key"] as const) input(name).value = (name === "key" ? connection?.apiKey : connection?.[name]) ?? "";
    field<HTMLSelectElement>(dialog, "provider").value = connection?.provider ?? "openai";
    select("thinking").value = connection?.thinking ?? "default";
    input("key").type = "password"; button("show-key").setAttribute("aria-pressed", "false");
    button("show-key").setAttribute("aria-label", "Show API key"); button("show-key").innerHTML = buttonIconHtml("preview") + "Show";
    limitsEditor.fill(); updateEditor(); renderCards(); fillRoute();
  };
  const persist = (): void => {
    try {
      if (credentialStorage) saveConnectionKeys(credentialStorage, transport.options.connections);
      if (storage) saveConnections(storage, transport.options.connections);
    } catch { status("Changes are active in memory. Browser storage is unavailable; they may not survive refresh."); }
  };
  const activate = (index: number): void => {
    try {
      transport.setConnections(transport.options.connections, index);
      selected = index; fill();
      if (transport.connection) checkConnection(transport.connection);
      status(`Now using ${transport.connection?.name} for this session. History and files are unchanged.`);
    } catch (error) { status(errorMessage(error)); updateEditor(); renderCards(); }
  };
  const save = (use: boolean): void => {
    const form = field<HTMLFormElement>(dialog, "connection-form");
    updateEditor(); // Revalidate programmatic/autofill changes even without an input event.
    if (!form.reportValidity()) return;
    try {
      const connection = validateConnection(draft());
      const connections = [...transport.options.connections];
      if (connections.some((other, index) => other.name === connection.name && index !== selected)) throw new Error("Choose a unique connection name.");
      const index = selected < 0 ? connections.length : selected;
      connections[index] = connection;
      transport.setConnections(connections, use || !transport.connection ? index : transport.selected); selected = index;
      fill();
      if (transport.connection && selected === transport.selected) checkConnection(transport.connection);
      status(`Connection saved. ${transport.connection?.name ?? "No connection"} is active for this session.`); persist();
    } catch (error) { status(errorMessage(error)); }
  };
  field(dialog, "connection-form").addEventListener("submit", event => { event.preventDefault(); save(false); });
  button("use").addEventListener("click", () => { if (dirty()) save(true); else activate(selected); });
  button("new").addEventListener("click", () => { selected = -1; fill(); status("New connection draft. Your active connection has not changed."); input("name").focus(); });
  button("duplicate").addEventListener("click", () => {
    try {
      const source = transport.options.connections[selected]; if (!source) return;
      let name = `${source.name.slice(0, 85)} copy`, suffix = 2;
      while (transport.options.connections.some(item => item.name === name)) name = `${source.name.slice(0, 85)} copy ${suffix++}`;
      const connections = [...transport.options.connections, { ...source, name }];
      transport.setConnections(connections, transport.selected); selected = connections.length - 1;
      fill(); status(`Duplicated ${source.name}${source.apiKey ? " including its key" : ""}. ${transport.connection?.name} remains active. Edit the copy or choose Use this connection.`); persist(); input("name").focus();
    } catch (error) { status(errorMessage(error)); }
  });
  button("remove").addEventListener("click", () => {
    try {
      const connections = transport.options.connections.filter((_item, index) => index !== selected);
      const active = transport.selected === selected ? 0 : transport.selected - (selected < transport.selected ? 1 : 0);
      transport.setConnections(connections, active); selected = connections.length ? transport.selected : -1;
      fill(); status(`Connection removed. ${transport.connection ? `Now using ${transport.connection.name}.` : "No active connection."}`); persist();
    } catch (error) { status(errorMessage(error)); }
  });
  button("show-key").addEventListener("click", () => {
    const show = input("key").type === "password"; input("key").type = show ? "text" : "password";
    button("show-key").setAttribute("aria-pressed", String(show)); button("show-key").setAttribute("aria-label", show ? "Hide API key" : "Show API key");
    button("show-key").innerHTML = buttonIconHtml("preview") + (show ? "Hide" : "Show");
  });
  for (const name of ["name", "endpoint", "key", "provider", "model", "thinking"]) for (const event of ["input", "change"]) field(dialog, name).addEventListener(event, () => {
    cancelTest(); if (["endpoint", "key", "provider"].includes(name)) cancelModels(); updateEditor(); status("");
  });
  const filterModels = (): void => {
    const query = input("model-filter").value.trim().toLowerCase();
    const matching = models.filter(model => `${model.id} ${model.name}`.toLowerCase().includes(query));
    const chosen = modelList.value || input("model").value;
    modelList.replaceChildren(...matching.map(model => new Option((model.name === model.id ? model.id : `${model.name} — ${model.id}`) + (model.contextTokens ? ` · context ${model.contextTokens.toLocaleString()}` : "") + (model.maxOutputTokens ? ` · output ${model.maxOutputTokens.toLocaleString()}` : ""), model.id)));
    modelList.value = matching.some(model => model.id === chosen) ? chosen : matching[0]?.id ?? "";
    field(dialog, "model-count").textContent = `${matching.length} of ${models.length} models`;
    button("use-model").disabled = matching.length === 0;
  };
  input("model-filter").addEventListener("input", filterModels);
  button("use-model").addEventListener("click", () => {
    if (modelList.value) { input("model").value = modelList.value; cancelTest(); updateEditor(); status("Model selected. Save to apply it."); }
  });
  button("fetch-models").addEventListener("click", () => {
    cancelModels(); const controller = new AbortController(); modelOperation = controller;
    button("fetch-models").disabled = true; status("Fetching models…");
    const timer = setTimeout(() => controller.abort(new Error("Model list request timed out.")), 30_000);
    void (async () => {
      try {
        const fetched = await fetchModels(draft(), controller.signal);
        if (modelOperation !== controller || destroyed) return;
        controller.signal.throwIfAborted(); models = fetched; filterModels(); field(dialog, "model-picker").hidden = false;
        status(models.length ? "Choose a model below. Your active connection stays unchanged until you save." : "No models returned. Enter a model manually.");
      } catch (error) { if (modelOperation === controller && !destroyed) status(`${errorMessage(error)} Check endpoint access and CORS; manual model entry is available.`); }
      finally { clearTimeout(timer); if (modelOperation === controller) { modelOperation = null; updateEditor(); } }
    })();
  });
  if (nativeAvailable()) select("native-network-mode").addEventListener("change", () => {
    try { transport.setNetworkMode(select("native-network-mode").value as "off" | "direct" | "tor"); status("Network policy saved. Run your site file again; chat and files are kept."); }
    catch (error) { status(errorMessage(error)); }
    updateEditor();
  });
  select("network-route").addEventListener("change", routeEditor);
  input("relay-endpoint").addEventListener("input", () => {
    // Never carry a token to a different endpoint while editing.
    input("relay-token").value = ""; routeEditor();
  });
  input("relay-token").addEventListener("input", routeEditor);
  button("apply-route").addEventListener("click", () => {
    try {
      transport.setDotnetRelay(select("network-route").value === "relay" ? { endpoint: input("relay-endpoint").value, token: input("relay-token").value } : null);
      const message = `HTTP route saved for this session. ${transport.dotnetRelay ? transport.dotnetRelay.token ? "Relay token present." : "Enter the relay token before running network code." : "Direct browser networking selected."} Code variables restart; chat and files are kept.`;
      status(message); field(dialog,"route-state").textContent = message;
      fillRoute();
    } catch (error) { status(errorMessage(error)); field(dialog,"route-state").textContent = errorMessage(error); }
    updateEditor(); routeEditor();
  });
  select("mode").addEventListener("change", () => {
    try { transport.setMode(select("mode").value as import("./sessions.js").AgentMode); status(`Runtime updated for this session: ${transport.mode}.`); }
    catch (error) { status(errorMessage(error)); }
    updateEditor(); routeEditor();
  });
  input("network").addEventListener("change", () => {
    try { transport.setNetworkAccess(input("network").checked); status(`Code network access ${transport.networkAccess ? "enabled" : "disabled"}. Run your site file again to restart its routes with this setting.`); }
    catch (error) { status(errorMessage(error)); }
    updateEditor();
  });
  input("sites").addEventListener("change", () => {
    try { transport.setSiteAccess(input("sites").checked); status(`Interactive sites ${transport.siteAccess ? "enabled. Run a site program, then open Sites." : "disabled; running sites stopped."}`); }
    catch (error) { status(errorMessage(error)); }
    updateEditor();
  });
  input("nuget").addEventListener("change", () => {
    try { transport.setNugetAccess(input("nuget").checked); status(`.NET NuGet package access ${transport.nugetAccess ? "enabled" : "disabled"}.`); }
    catch (error) { status(errorMessage(error)); }
    updateEditor();
  });
  const checkConnection = (connection: Connection): void => {
    cancelTest();
    const controller = new AbortController(); operation = controller;
    const key = identity(connection); operationKey = key; health.set(key, {state:"testing",message:"Testing the endpoint, model and key."});
    updateEditor(); renderCards(); status(`Testing ${connection.name}…`); transport.publish();
    void (async () => {
      let runtime: LetheticRuntime | null = null;
      try {
        requireChatCredential(connection);
        await limitsEditor.discover(connection, controller.signal);
        controller.signal.throwIfAborted();
        runtime = await createRuntime({ connection, ...limitsEditor.testLimits(), timeoutMs: 30_000 });
        controller.signal.throwIfAborted();
        await runtime.send("Reply with the single word Connected.", controller.signal);
        if (operation !== controller || destroyed) return;
        health.set(key, {state:"valid",message:"The model returned a valid response."}); status(`Connected. The model returned a valid response.${dirty() ? " This tests your draft only; Save & use connection to apply it to chat." : ""}`);
      } catch (error) {
        if (operation === controller && !destroyed) { health.set(key, {state:"invalid",message:errorMessage(error)}); status(`Connection failed: ${errorMessage(error)}`); }
      } finally {
        runtime?.dispose();
        if (operation === controller) { operation = null; operationKey = null; if (health.get(key)?.state === "testing") health.delete(key); }
        if (!destroyed) { updateEditor(); renderCards(); transport.publish(); }
      }
    })();
  };
  button("test").addEventListener("click", () => {
    try { checkConnection(validateConnection(draft())); } catch (error) { status(errorMessage(error)); updateEditor(); }
  });
  const stopAutofillWatch = (): void => {
    if (autofillTimer !== null) clearInterval(autofillTimer);
    autofillTimer = null;
  };
  // Some password managers change .value without input/change events. Keep Save & use
  // available and invalidate tests against the previous draft; never save secrets implicitly.
  const watchAutofill = (): void => {
    stopAutofillWatch();
    const fingerprint = () => JSON.stringify(draft());
    let previous = fingerprint();
    autofillTimer = setInterval(() => {
      const next = fingerprint();
      if (next === previous) return;
      previous = next;
      if (operationKey !== null && operationKey !== identity(draft())) cancelTest();
      updateEditor(); renderCards();
    }, 250);
  };
  dialog.addEventListener("close", () => { stopAutofillWatch(); cancelTest(); cancelModels(); input("key").type = "password"; });
  return { dialog, open(): void {
    selected = transport.connection ? transport.selected : -1; fill(); status("");
    if (!dialog.open) dialog.showModal();
    watchAutofill();
    if (transport.connection) checkConnection(transport.connection);
  }, destroy(): void { destroyed = true; stopAutofillWatch(); cancelTest(); cancelModels(); health.clear(); dialog.remove(); } };
}
