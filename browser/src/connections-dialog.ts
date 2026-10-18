import { validateConnection, saveConnections } from "./connection.js";
import { fetchModels } from "./models.js";
import type { AvailableModel } from "./models.js";
import { createRuntime } from "./sdk.js";
import type { Connection, LetheticRuntime } from "./sdk.js";
import type { LocalTransport } from "./local-transport.js";
import { createDialog, errorMessage, field } from "./dialog.js";

export function createConnectionsDialog(transport: LocalTransport, storage?: Storage): {
  dialog: HTMLDialogElement; open(): void; destroy(): void;
} {
  const dialog = createDialog("Connections", `
    <p>Connect directly to an OpenAI-compatible or Anthropic endpoint. Keys stay in memory; only connection metadata is saved.</p>
    <label>Saved connections<select data-field="connections"></select></label>
    <form data-field="connection-form" class="browser-connection-form">
      <label>Name<input data-field="name" required maxlength="100" autocomplete="off"></label>
      <label>Protocol<select data-field="provider"><option value="openai">OpenAI-compatible</option><option value="anthropic">Anthropic Messages</option></select></label>
      <label>Endpoint URL<input data-field="endpoint" type="url" required placeholder="https://your-server/v1/chat/completions" autocomplete="off" spellcheck="false"></label>
      <label>Model<input data-field="model" required autocomplete="off" spellcheck="false"></label>
      <label>API key (optional, memory only)<input data-field="key" type="password" autocomplete="off" spellcheck="false"></label>
      <div class="panel-toolbar"><button class="primary-button" type="submit">Save connection</button><button type="button" data-field="remove">Remove</button><button type="button" data-field="test">Test saved connection</button><button type="button" data-field="fetch-models">Fetch models</button></div>
      <section class="browser-model-picker" data-field="model-picker" hidden>
        <label>Filter models<input data-field="model-filter" type="search" placeholder="Search model IDs or names" autocomplete="off"></label>
        <label>Available models<select data-field="model-list" size="6"></select></label>
        <p data-field="model-count" role="status"></p>
        <button type="button" data-field="use-model">Use selected model</button>
      </section>
    </form>
    <label class="browser-python"><input data-field="python" type="checkbox"> Enable Python tools (Pyodide)</label>
    <p>Python runs without per-call approval, with browser network access and session files synced to Files. Stop resets its variables. Changing connections or Python mode starts a new chat.</p>
    <p>Endpoints must permit this page’s origin through CORS. HTTPS pages require HTTPS endpoints.</p>
    <p data-field="status" role="status"></p>`);
  const input = (name: string): HTMLInputElement => field<HTMLInputElement>(dialog, name);
  const select = field<HTMLSelectElement>(dialog, "connections");
  let selected = transport.connection ? transport.selected : -1;
  let testRuntime: LetheticRuntime | null = null;
  let operation: AbortController | null = null;
  let modelOperation: AbortController | null = null;
  let models: AvailableModel[] = [];
  const modelList = field<HTMLSelectElement>(dialog, "model-list");
  const cancelModels = (): void => {
    modelOperation?.abort(); modelOperation = null; models = [];
    field<HTMLButtonElement>(dialog, "fetch-models").disabled = false;
    field(dialog, "model-picker").hidden = true;
    modelList.replaceChildren(); input("model-filter").value = "";
  };
  const filterModels = (): void => {
    const query = input("model-filter").value.trim().toLowerCase();
    const matching = models.filter(model => `${model.id} ${model.name}`.toLowerCase().includes(query));
    const chosen = modelList.value || input("model").value;
    modelList.replaceChildren(...matching.map(model => new Option(model.name === model.id ? model.id : `${model.name} — ${model.id}`, model.id)));
    modelList.value = matching.some(model => model.id === chosen) ? chosen : matching[0]?.id ?? "";
    field(dialog, "model-count").textContent = `${matching.length} of ${models.length} models`;
    field<HTMLButtonElement>(dialog, "use-model").disabled = matching.length === 0;
  };
  input("model-filter").addEventListener("input", filterModels);
  field(dialog, "use-model").addEventListener("click", () => {
    if (modelList.value) { input("model").value = modelList.value; status("Model selected. Save the connection to use it."); }
  });
  for (const name of ["endpoint", "key", "provider"]) field(dialog, name).addEventListener("input", cancelModels);
  field(dialog, "fetch-models").addEventListener("click", () => {
    cancelModels();
    const controller = new AbortController(); modelOperation = controller;
    field<HTMLButtonElement>(dialog, "fetch-models").disabled = true;
    status("Fetching models…");
    const timer = setTimeout(() => controller.abort(new Error("Model list request timed out.")), 30_000);
    void (async () => {
      try {
        const fetched = await fetchModels({ endpoint: input("endpoint").value.trim(),
          provider: field<HTMLSelectElement>(dialog, "provider").value as Connection["provider"],
          apiKey: input("key").value }, controller.signal);
        if (modelOperation !== controller) return;
        controller.signal.throwIfAborted();
        models = fetched; filterModels(); field(dialog, "model-picker").hidden = false;
        status(models.length ? "Models fetched. Filter and select a model, then save the connection." : "No models returned. You can enter a model manually.");
      } catch (error) {
        if (modelOperation === controller) status(`${errorMessage(error)} Check endpoint access and CORS; manual model entry is available.`);
      } finally {
        clearTimeout(timer);
        if (modelOperation === controller) {
          modelOperation = null; field<HTMLButtonElement>(dialog, "fetch-models").disabled = false;
        }
      }
    })();
  });
  const status = (text: string): void => { field(dialog, "status").textContent = text; };
  const fill = (): void => {
    cancelModels();
    const connection = transport.options.connections[selected];
    input("name").value = connection?.name ?? "";
    input("endpoint").value = connection?.endpoint ?? "";
    input("model").value = connection?.model ?? "";
    input("key").value = connection?.apiKey ?? "";
    field<HTMLSelectElement>(dialog, "provider").value = connection?.provider ?? "openai";
    input("python").checked = transport.pythonEnabled;
    field<HTMLButtonElement>(dialog, "remove").disabled = selected < 0;
    field<HTMLButtonElement>(dialog, "test").disabled = selected < 0 || operation !== null;
  };
  const refresh = (): void => {
    select.replaceChildren(new Option("New connection", ""));
    transport.options.connections.forEach((connection, index) => select.add(new Option(connection.name, String(index))));
    select.value = selected < 0 ? "" : String(selected); fill();
  };
  const persist = (): void => {
    if (!storage) return;
    try { saveConnections(storage, transport.options.connections); }
    catch { status("Connection saved for this tab; browser storage is unavailable."); }
  };
  field(dialog, "connection-form").addEventListener("submit", (event) => {
    event.preventDefault();
    try {
      const connection = validateConnection({ name: input("name").value.trim(),
        endpoint: input("endpoint").value.trim(), model: input("model").value,
        provider: field<HTMLSelectElement>(dialog, "provider").value as Connection["provider"], apiKey: input("key").value });
      const connections = [...transport.options.connections];
      if (connections.some((other, index) => other.name === connection.name && index !== selected)) throw new Error("Choose a unique connection name.");
      const index = selected < 0 ? connections.length : selected;
      connections[index] = connection; transport.setConnections(connections, index); selected = index;
      refresh(); status("Connection saved. Close this dialog to start chatting."); persist();
    } catch (error) { status(errorMessage(error)); }
  });
  select.addEventListener("change", () => {
    try {
      const index = select.value === "" ? -1 : Number(select.value);
      if (index >= 0) transport.setConnections(transport.options.connections, index);
      selected = index; fill(); status("");
    } catch (error) { status(errorMessage(error)); }
  });
  field(dialog, "remove").addEventListener("click", () => {
    try {
      const connections = transport.options.connections.filter((_item, index) => index !== selected);
      transport.setConnections(connections, 0); selected = connections.length ? 0 : -1;
      refresh(); status("Connection removed."); persist();
    } catch (error) { status(errorMessage(error)); }
  });
  input("python").addEventListener("change", () => {
    try {
      if (transport.busy) throw new Error("Stop the current turn first.");
      transport.setPythonMode(input("python").checked);
    } catch (error) { input("python").checked = transport.pythonEnabled; status(errorMessage(error)); }
  });
  field(dialog, "test").addEventListener("click", () => {
    if (operation) return;
    operation = new AbortController(); const signal = operation.signal;
    field<HTMLButtonElement>(dialog, "test").disabled = true; status("Testing saved connection…");
    void (async () => {
      try {
        const connection = transport.options.connections[selected];
        if (!connection) throw new Error("Save a connection first.");
        testRuntime = await createRuntime({ connection, timeoutMs: 30_000 });
        await testRuntime.send("Reply with the single word Connected.", signal);
        status("Connected. The model returned a valid response.");
      } catch (error) { status(errorMessage(error)); }
      finally { testRuntime?.dispose(); testRuntime = null; operation = null; field<HTMLButtonElement>(dialog, "test").disabled = selected < 0; }
    })();
  });
  dialog.addEventListener("close", () => { operation?.abort(); cancelModels(); });
  return { dialog, open(): void {
    selected = transport.connection ? transport.selected : -1; refresh(); status("");
    if (!dialog.open) dialog.showModal();
  }, destroy(): void { cancelModels(); operation?.abort(); testRuntime?.dispose(); dialog.remove(); } };
}
