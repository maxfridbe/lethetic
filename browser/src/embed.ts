import { createRuntime } from "./sdk.js";
import type { Connection, LetheticRuntime, RuntimeEvent, Tool } from "./sdk.js";
import { loadConnections, saveConnections, validateConnection } from "./connection.js";
import { createPythonTool } from "./python.js";
import type { PythonTool } from "./python.js";

export interface EmbedOptions {
  connections?: Connection[];
  /** Omit to keep configuration in memory; the standalone app passes localStorage. */
  storage?: Storage;
  tools?: Tool[];
  systemPrompt?: string;
  python?: boolean;
  pyodideIndexURL?: string;
  compact?: boolean;
}

/** Mount the complete connection editor and chat. Load styles.css alongside this module. */
export function mountLethetic(root: HTMLElement, options: EmbedOptions = {}): { destroy(): void } {
  root.classList.add("lethetic-browser");
  if (options.compact) root.classList.add("compact");
  root.innerHTML = `
    <header class="masthead"><a class="brand" href="https://github.com/maxfridbe/lethetic" target="_blank" rel="noreferrer">L<span>Lethetic</span></a><span class="edition">BROWSER EDITION · WASM</span><a href="./integration.html" target="_blank" rel="noreferrer">Embed & integrate ↗</a></header>
    <div class="workspace">
      <aside class="connections">
        <div class="eyebrow">YOUR WORKSPACE</div><h1>Connect your intelligence.</h1>
        <p class="muted">Bring a model endpoint. Work from your browser.</p>
        <label>Saved connections<select data-field="connections"><option value="">New connection</option></select></label>
        <form data-field="connection-form">
          <label>Name<input data-field="name" required maxlength="100" placeholder="My model server" autocomplete="off"></label>
          <label>Protocol<select data-field="provider"><option value="openai">OpenAI-compatible</option><option value="anthropic">Anthropic Messages</option></select></label>
          <label>Endpoint URL<input data-field="endpoint" type="url" required placeholder="https://your-server/v1/chat/completions" autocomplete="off" spellcheck="false"></label>
          <label>Model<input data-field="model" required placeholder="Model ID from your provider" autocomplete="off" spellcheck="false"></label>
          <label>API key <span class="optional">optional · memory only</span><input data-field="key" type="password" autocomplete="off" placeholder="Enter for this visit" spellcheck="false"></label>
          <div class="actions"><button type="submit" class="primary">Save connection</button><button type="button" data-field="remove">Remove</button></div>
        </form>
        <button class="test-connection" data-field="test">Test saved connection</button>
        <div class="python-setting"><label class="checkbox"><input type="checkbox" data-field="python"> Enable Python tools</label><p class="muted">Run model-generated code with Pyodide. Python can use browser networking and a temporary filesystem. First use downloads the runtime.</p></div>
        <p class="fineprint">Endpoints must allow this page’s origin through CORS. HTTPS pages require HTTPS endpoints. Connection changes start a new chat.</p>
      </aside>
      <main class="conversation">
        <div class="chat-toolbar"><div><span class="status-dot"></span><strong data-field="active">No connection selected</strong></div><div class="actions"><button data-field="export">Export</button><button data-field="clear">New chat</button></div></div>
        <div class="transcript" data-field="transcript" role="log" aria-label="Conversation" aria-live="polite">
          <div class="welcome"><div class="welcome-mark">L</div><h2>A place to think things through.</h2><p>Connect a model to start coding, exploring, or solving a problem. Enable Python for calculations and data work.</p><div class="capabilities"><span>Your model</span><span>Local WASM runtime</span><span>Optional Python</span></div></div>
        </div>
        <form class="composer" data-field="composer"><label class="sr-only">Message<textarea data-field="prompt" rows="3" placeholder="What are you working on?" required></textarea></label><div class="composer-footer"><span data-field="status" role="status">Add a connection to begin.</span><div class="actions"><button type="button" data-field="stop" hidden>Stop</button><button type="submit" data-field="send" class="primary">Send ↑</button></div></div></form>
        <p class="privacy">Requests go directly to your endpoint. Keys and chats are not stored by Lethetic.</p>
      </main>
    </div>`;
  const element = <T extends HTMLElement>(name: string): T => {
    const found = root.querySelector<T>(`[data-field="${name}"]`);
    if (!found) throw new Error(`Missing UI element: ${name}`);
    return found;
  };
  const input = (name: string): HTMLInputElement => element<HTMLInputElement>(name);
  const select = element<HTMLSelectElement>("connections");
  const transcript = element("transcript");
  const prompt = element<HTMLTextAreaElement>("prompt");
  const pythonEnabled = input("python");
  pythonEnabled.checked = options.python ?? false;
  let connections = options.connections?.map(validateConnection) ??
    (options.storage ? loadConnections(options.storage) : []);
  let selected = connections.length ? 0 : -1;
  let runtime: LetheticRuntime | null = null;
  let python: PythonTool | null = null;
  let busy = false;
  let destroyed = false;
  let testing: LetheticRuntime | null = null;
  let operation: AbortController | null = null;

  const status = (text: string): void => { if (!destroyed) element("status").textContent = text; };
  const errorText = (error: unknown): string => error instanceof Error ? error.message : String(error);
  const refreshControls = (): void => {
    for (const control of root.querySelectorAll<HTMLInputElement | HTMLButtonElement | HTMLSelectElement>(
      ".connections input, .connections button, .connections select, [data-field=clear], [data-field=export]")) {
      control.disabled = busy;
    }
    element<HTMLButtonElement>("send").disabled = busy || selected < 0;
    element<HTMLButtonElement>("remove").disabled = busy || selected < 0;
    element<HTMLButtonElement>("test").disabled = busy || selected < 0;
    element("stop").hidden = !busy;
    prompt.disabled = busy;
    element("active").textContent = connections[selected]?.name ?? "No connection selected";
    root.classList.toggle("connected", selected >= 0);
  };
  const reset = (): void => {
    runtime?.dispose(); runtime = null;
    python?.reset(); python = null;
    transcript.replaceChildren();
    status(selected >= 0 ? "Ready. Your conversation stays in this tab." : "Add a connection to begin.");
  };
  const fill = (): void => {
    const connection = connections[selected];
    input("name").value = connection?.name ?? "";
    input("endpoint").value = connection?.endpoint ?? "";
    input("model").value = connection?.model ?? "";
    input("key").value = connection?.apiKey ?? "";
    element<HTMLSelectElement>("provider").value = connection?.provider ?? "openai";
    refreshControls();
  };
  const refreshList = (): void => {
    select.replaceChildren(new Option("New connection", ""));
    connections.forEach((connection, index) => select.add(new Option(connection.name, String(index))));
    select.value = selected < 0 ? "" : String(selected);
  };
  const persist = (): void => {
    if (options.storage) {
      try { saveConnections(options.storage, connections); }
      catch { status("Connection works for this visit; browser storage is unavailable."); }
    }
  };
  const append = (role: string, text: string): void => {
    transcript.querySelector(".welcome")?.remove();
    const article = document.createElement("article");
    article.className = `message ${role === "You" ? "user-message" : ""}`;
    const title = document.createElement("h3"); title.textContent = role;
    const content = document.createElement("pre"); content.textContent = text;
    article.append(title, content); transcript.append(article);
    transcript.scrollTop = transcript.scrollHeight;
  };
  const onEvent = (event: RuntimeEvent): void => {
    if (destroyed) return;
    switch (event.type) {
      case "request": status(`Thinking · request ${event.round}`); break;
      case "assistant": append("Lethetic", event.text); break;
      case "tool_start":
        status(`Running ${event.call.name}…`);
        append(event.call.name, typeof event.call.arguments["code"] === "string" ?
          event.call.arguments["code"] : JSON.stringify(event.call.arguments, null, 2)); break;
      case "tool_end": append(`${event.call.name} output`, event.output); break;
    }
  };
  const getRuntime = async (): Promise<LetheticRuntime> => {
    if (runtime) return runtime;
    const connection = connections[selected];
    if (!connection) throw new Error("Save a connection first.");
    const tools = [...(options.tools ?? [])];
    if (pythonEnabled.checked) {
      python = createPythonTool({ indexURL: options.pyodideIndexURL });
      tools.push(python);
    }
    const created = await createRuntime({ connection, tools, onEvent, systemPrompt: options.systemPrompt });
    if (destroyed) { created.dispose(); throw new Error("View was closed."); }
    runtime = created;
    return created;
  };

  select.addEventListener("change", () => {
    selected = select.value === "" ? -1 : Number(select.value);
    reset(); fill();
  });
  element("connection-form").addEventListener("submit", (event) => {
    event.preventDefault();
    try {
      const connection = validateConnection({ name: input("name").value.trim(),
        provider: element<HTMLSelectElement>("provider").value as Connection["provider"],
        endpoint: input("endpoint").value.trim(), model: input("model").value,
        apiKey: input("key").value });
      if (connections.some((other, index) => other.name === connection.name && index !== selected)) {
        throw new Error("Choose a unique connection name.");
      }
      if (selected < 0) { selected = connections.length; connections.push(connection); }
      else connections[selected] = connection;
      reset(); refreshList(); fill(); persist();
    } catch (error) { status(errorText(error)); }
  });
  element("remove").addEventListener("click", () => {
    if (selected >= 0) connections.splice(selected, 1);
    selected = connections.length ? 0 : -1;
    reset(); refreshList(); fill(); persist();
  });
  pythonEnabled.addEventListener("change", reset);
  element("clear").addEventListener("click", reset);
  element("stop").addEventListener("click", () => {
    operation?.abort(new DOMException("Turn stopped.", "AbortError"));
  });
  element("test").addEventListener("click", () => {
    void (async () => {
      busy = true; refreshControls(); status("Testing connection…");
      operation = new AbortController();
      try {
        const connection = connections[selected];
        if (!connection) throw new Error("Save a connection first.");
        testing = await createRuntime({ connection, timeoutMs: 30_000 });
        if (destroyed) return;
        await testing.send("Reply with the single word Connected.", operation.signal);
        status("Connected. The model returned a valid response.");
      } catch (error) { status(errorText(error)); }
      finally { testing?.dispose(); testing = null; operation = null; busy = false; if (!destroyed) refreshControls(); }
    })();
  });
  element("composer").addEventListener("submit", (event) => {
    event.preventDefault();
    if (busy || !prompt.value.trim()) return;
    const text = prompt.value;
    void (async () => {
      busy = true; refreshControls(); status("Loading WASM runtime…");
      operation = new AbortController();
      try {
        const active = await getRuntime();
        append("You", text); prompt.value = "";
        await active.send(text, operation.signal);
        status("Ready.");
      } catch (error) {
        status(errorText(error));
        if (!destroyed) append("Turn interrupted", `${errorText(error)}\nIncomplete turn removed from model history. Tool effects may remain.`);
      } finally { operation = null; busy = false; if (!destroyed) { refreshControls(); prompt.focus(); } }
    })();
  });
  prompt.addEventListener("keydown", (event) => {
    if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
      event.preventDefault(); element<HTMLFormElement>("composer").requestSubmit();
    }
  });
  element("export").addEventListener("click", () => {
    const url = URL.createObjectURL(new Blob([JSON.stringify(runtime?.history() ?? [], null, 2)],
      { type: "application/json" }));
    const link = document.createElement("a"); link.href = url; link.download = "lethetic-chat.json";
    link.click(); setTimeout(() => URL.revokeObjectURL(url), 1000);
  });
  refreshList(); fill();
  if (selected >= 0) status("Ready. Re-enter your key if the endpoint requires one.");
  return { destroy(): void {
    destroyed = true; operation?.abort(); runtime?.dispose(); testing?.dispose(); python?.reset(); root.replaceChildren();
    root.classList.remove("lethetic-browser", "compact", "connected");
  } };
}
