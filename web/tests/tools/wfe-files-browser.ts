// Native CDP-pipe driver. No npm packages, browser libraries, or existing profile.
// Compiled with web/tests/tsconfig.json and run by scripts/test_wfe_files_browser.py.
import { spawn, type ChildProcess } from "node:child_process";
import { stat, writeFile } from "node:fs/promises";
import { resolve } from "node:path";

interface DriverConfig {
  readonly browser: string;
  readonly profile: string;
  readonly browserHome: string;
  readonly downloads: string;
  readonly origin: string;
  url: string;
  readonly enabled: boolean;
  readonly screenshot: string;
  readonly source: string;
  readonly hostile: string;
}

interface PendingCommand {
  readonly resolve: (result: unknown) => void;
  readonly reject: (error: Error) => void;
  readonly deadline: number;
}

interface RequestRecord {
  readonly origin: string;
  readonly path: string;
  readonly method: string;
}

interface SentFrame {
  readonly prompt: boolean;
  readonly fileContent: boolean;
}

type CdpListener = (message: Record<string, unknown>) => void;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Read a nested field of an untyped CDP payload; absent paths yield undefined. */
function field(value: unknown, ...path: readonly string[]): unknown {
  let current = value;
  for (const key of path) {
    current = isRecord(current) ? current[key] : undefined;
  }
  return current;
}

function stringField(value: unknown, ...path: readonly string[]): string {
  const result = field(value, ...path);
  if (typeof result !== "string") throw new Error("unexpected CDP payload");
  return result;
}

function optionalString(value: unknown): string | null {
  return typeof value === "string" ? value : null;
}

function parseConfig(value: unknown): DriverConfig {
  const text = (key: string): string => {
    const result = field(value, key);
    if (typeof result !== "string") throw new Error("invalid driver configuration");
    return result;
  };
  const enabled = field(value, "enabled");
  if (typeof enabled !== "boolean") throw new Error("invalid driver configuration");
  return { browser: text("browser"), profile: text("profile"), browserHome: text("browserHome"),
    downloads: text("downloads"), origin: text("origin"), url: text("url"), enabled,
    screenshot: text("screenshot"), source: text("source"), hostile: text("hostile") };
}

process.stdin.setEncoding("utf8");
let input = "";
for await (const chunk of process.stdin) input += chunk;
const config = parseConfig(JSON.parse(input));
input = "";
let stage = "browser launch";
let browser: ChildProcess | null = null;
let closed = false;
let session: string | null = null;
let counter = 0;
const pending = new Map<number, PendingCommand>();
const listeners: CdpListener[] = [];
const sleep = (ms: number): Promise<void> => new Promise((done) => setTimeout(done, ms));
const requests: RequestRecord[] = [];
const sentFrames: SentFrame[] = [];
const violations: unknown[] = [];
const pageErrors: string[] = [];
// Written by the CDP listener when the application document arrives.
const documentHeaders: { csp: string | null; permissions: string | null } = { csp: null, permissions: null };
let workerError: unknown = false;

function check(value: unknown, label: string): void { if (!value) throw new Error(label); }
function rpc(method: string, params: Record<string, unknown> = {}, target: string | null = session): Promise<unknown> {
  const id = ++counter;
  return new Promise((resolve, reject) => {
    const deadline = setTimeout(() => { pending.delete(id); reject(new Error("CDP deadline")); }, 15_000);
    pending.set(id, { resolve, reject, deadline });
    const commands = browser?.stdio[3];
    if (!commands) { clearTimeout(deadline); pending.delete(id); reject(new Error("browser pipe unavailable")); return; }
    commands.write(JSON.stringify({ id, method, params, ...(target ? { sessionId: target } : {}) }) + "\0");
  });
}
async function evaluate(expression: string): Promise<unknown> {
  const result = await rpc("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true, userGesture: true });
  check(!field(result, "exceptionDetails"), "page evaluation failed");
  return field(result, "result", "value");
}
async function until(expression: string, label: string, timeout = 30_000): Promise<void> {
  stage = label;
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (await evaluate(expression)) return;
    await sleep(100);
  }
  throw new Error("browser condition deadline");
}
async function click(selector: string): Promise<void> {
  check(await evaluate(`(() => { const item = document.querySelector(${JSON.stringify(selector)}); if (!item || item.disabled) return false; item.click(); return true; })()`), "button unavailable");
}
async function clickText(text: string, selector = "button"): Promise<void> {
  check(await evaluate(`(() => { const item = [...document.querySelectorAll(${JSON.stringify(selector)})].find(item => item.textContent.trim() === ${JSON.stringify(text)}); if (!item || item.disabled) return false; item.click(); return true; })()`), "button unavailable");
}
async function downloaded(filename: string): Promise<void> {
  stage = "download completion";
  const deadline = Date.now() + 15_000;
  while (Date.now() < deadline) {
    try { if ((await stat(resolve(config.downloads, filename))).isFile()) return; } catch {}
    await sleep(100);
  }
  throw new Error("download unavailable");
}
async function layout(): Promise<unknown> {
  return evaluate(`(() => {
    const pane = document.querySelector('.files-pane').getBoundingClientRect();
    const chat = document.querySelector('#chat-scroll').getBoundingClientRect();
    const editor = document.querySelector('#files-monaco').getBoundingClientRect();
    const composer = document.querySelector('.composer').getBoundingClientRect();
    return pane.height >= 150 && pane.bottom <= chat.top + 1 && editor.width >= 120 &&
      editor.height >= 50 && chat.height > 10 && chat.bottom <= composer.top + 1 && composer.bottom <= innerHeight;
  })()`);
}
async function screenshot(): Promise<void> {
  const result = await rpc("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
  await writeFile(config.screenshot, Buffer.from(stringField(result, "data"), "base64"));
}
async function cleanup(): Promise<void> {
  if (!browser || closed) return;
  try { await rpc("Browser.close", {}, null); } catch {}
  for (let i = 0; i < 60 && !closed; i++) await sleep(50);
  if (!closed) browser.kill("SIGTERM");
  for (let i = 0; i < 40 && !closed; i++) await sleep(50);
  if (!closed) browser.kill("SIGKILL");
  for (const record of pending.values()) { clearTimeout(record.deadline); record.reject(new Error("browser closed")); }
  pending.clear();
}
function dispatch(message: Record<string, unknown>): void {
  const id = message["id"];
  if (typeof id === "number" && id !== 0) {
    const entry = pending.get(id);
    if (!entry) return;
    clearTimeout(entry.deadline);
    pending.delete(id);
    if (message["error"]) entry.reject(new Error("CDP command rejected")); else entry.resolve(message["result"]);
  } else {
    for (const listener of listeners) listener(message);
  }
}

try {
  const environment = { ...process.env, HOME: config.browserHome,
    XDG_CONFIG_HOME: resolve(config.browserHome, "config"), XDG_CACHE_HOME: resolve(config.browserHome, "cache") };
  const child = spawn(config.browser, ["--headless=new", `--user-data-dir=${config.profile}`,
    "--remote-debugging-pipe", "--ignore-certificate-errors", "--no-first-run", "--no-default-browser-check",
    "--disable-background-networking", "--disable-component-update", "--disable-sync", "--disable-extensions",
    "--window-size=1280,1000", "about:blank"], { env: environment, stdio: ["ignore", "ignore", "pipe", "pipe", "pipe"] });
  browser = child;
  child.stderr?.on("data", () => {});
  child.on("exit", () => { closed = true; });
  const responses = child.stdio[4];
  if (!responses) throw new Error("browser pipe unavailable");
  let buffered = "";
  responses.setEncoding("utf8");
  responses.on("data", (chunk) => {
    buffered += chunk;
    for (;;) {
      const end = buffered.indexOf("\0");
      if (end < 0) break;
      const message: unknown = JSON.parse(buffered.slice(0, end));
      buffered = buffered.slice(end + 1);
      if (isRecord(message)) dispatch(message);
    }
  });
  const target = await rpc("Target.createTarget", { url: "about:blank" });
  const attached = await rpc("Target.attachToTarget", { targetId: stringField(target, "targetId"), flatten: true });
  session = stringField(attached, "sessionId");
  await rpc("Page.enable");
  await rpc("Runtime.enable");
  await rpc("Network.enable");
  await rpc("Browser.setDownloadBehavior", { behavior: "allow", downloadPath: config.downloads, eventsEnabled: true }, null);
  await rpc("Browser.grantPermissions", { origin: config.origin, permissions: ["clipboardReadWrite", "clipboardSanitizedWrite"] }, null);
  await rpc("Emulation.setDeviceMetricsOverride", { width: 1280, height: 1000, deviceScaleFactor: 1, mobile: false });
  listeners.push((event) => {
    if (event["sessionId"] !== session) return;
    const method = event["method"];
    const params = event["params"];
    if (method === "Network.requestWillBeSent") {
      const parsed = new URL(stringField(params, "request", "url"));
      // Never retain URL fragments, request headers, bodies or authentication proofs.
      requests.push({ origin: parsed.origin, path: parsed.pathname, method: stringField(params, "request", "method") });
    }
    if (method === "Network.responseReceived" && field(params, "type") === "Document") {
      const headers = field(params, "response", "headers");
      documentHeaders.csp = optionalString(field(headers, "content-security-policy") ?? field(headers, "Content-Security-Policy"));
      documentHeaders.permissions = optionalString(field(headers, "permissions-policy") ?? field(headers, "Permissions-Policy"));
    }
    if (method === "Network.webSocketFrameSent") {
      const payload = stringField(params, "response", "payloadData");
      sentFrames.push({ prompt: payload.includes('"send_prompt"'), fileContent: payload.includes("browser-fixture-marker") || payload.includes("__fileExecuted") });
    }
    if (method === "Runtime.exceptionThrown") pageErrors.push(String(field(params, "exceptionDetails", "text")));
  });
  await rpc("Page.addScriptToEvaluateOnNewDocument", { source: `
    window.__fileExecuted = false; window.__fileCopies = []; window.__fileWorkers = []; window.__cspViolations = [];
    document.addEventListener('securitypolicyviolation', event => window.__cspViolations.push(event.effectiveDirective));
    const originalWrite = navigator.clipboard.writeText.bind(navigator.clipboard);
    navigator.clipboard.writeText = value => { window.__fileCopies.push(value); return originalWrite(value); };
    const OriginalWorker = window.Worker;
    window.Worker = class extends OriginalWorker {
      constructor(url, options) { super(url, options); window.__fileWorkers.push({ url: String(url), type: options?.type }); }
    };
  ` });
  stage = "secure navigation";
  await rpc("Page.navigate", { url: config.url });
  config.url = "";
  await until(`document.querySelector('#app')?.dataset.connection === 'live'`, "authenticated WFE synchronization");
  const documentCsp = documentHeaders.csp;
  const documentPermissions = documentHeaders.permissions;
  if (documentCsp === null || !documentCsp.includes("script-src 'self'") || documentCsp.includes("unsafe-eval") || documentCsp.includes("blob:")) {
    throw new Error("script policy weakened");
  }
  if (documentPermissions === null || !documentPermissions.includes("clipboard-read=()")) {
    throw new Error("clipboard read policy weakened");
  }
  check(await evaluate("location.hash === ''"), "bootstrap fragment retained");
  if (!config.enabled) {
    stage = "disabled capability";
    check(!documentCsp.includes("unsafe-inline") && documentPermissions.includes("clipboard-write=()"), "disabled policy weakened");
    check(await evaluate("document.querySelector('#files-toggle') === null"), "disabled file controls visible");
    check(!requests.some((request) => request.path.includes("monaco")), "disabled editor loaded");
    await screenshot();
  } else {
    check(documentCsp.includes("style-src 'self' 'unsafe-inline'") && documentCsp.includes("worker-src 'self'"), "editor policy missing");
    check(documentPermissions.includes("clipboard-write=(self)"), "copy path policy missing");
    stage = "file pane opening";
    await click("#files-toggle");
    await until("document.querySelector('#files-monaco')?.dataset.editorReady === 'true' && document.querySelectorAll('.files-entry').length >= 5", "Monaco initialization");
    check(await evaluate("![...document.querySelectorAll('.files-entry')].some(item => ['.git','.env','config.yml'].includes(item.title))"), "protected names disclosed");
    await click('.files-entry[title="hostile.html"]');
    await until(`(async () => { const m = await import('/lib/monaco/vs/editor/editor.api.js'); return m.editor.getModels()[0]?.getValue() === ${JSON.stringify(config.hostile)}; })()`, "hostile file rendered as text");
    check(await evaluate("window.__fileExecuted === false && document.querySelector('#files-monaco script') === null && document.querySelector('#files-monaco img') === null"), "file markup executed");
    await click('.files-entry[title="binary.bin"]');
    await until("document.querySelector('.files-preview-message')?.textContent.includes('Preview unavailable')", "binary preview refusal");
    await click('.files-entry[title="large.txt"]');
    await until("document.querySelector('.files-preview-message')?.textContent.includes('Preview unavailable')", "oversized preview refusal");
    await click('.files-entry[title="src"]');
    await until("document.querySelector('.files-entry[title=\"src/main.rs\"]') !== null", "directory navigation");
    await click('.files-entry[title="src/main.rs"]');
    await until(`(async () => { const m = await import('/lib/monaco/vs/editor/editor.api.js'); return m.editor.getModels()[0]?.getValue() === ${JSON.stringify(config.source)}; })()`, "source preview");
    check(await evaluate("(async () => { const m = await import('/lib/monaco/vs/editor/editor.api.js'); const e = m.editor.getEditors()[0]; return e.getOption(m.editor.EditorOption.readOnly) && e.getOption(m.editor.EditorOption.domReadOnly) && e.getModel().getLanguageId() === 'rust'; })()"), "editor is not read-only or language-aware");
    await evaluate("(async () => { const m = await import('/lib/monaco/vs/editor/editor.api.js'); m.editor.getEditors()[0].focus(); })()");
    await rpc("Input.insertText", { text: "UNEXPECTED_EDIT" });
    check(await evaluate(`(async () => { const m = await import('/lib/monaco/vs/editor/editor.api.js'); return m.editor.getModels()[0].getValue() === ${JSON.stringify(config.source)}; })()`), "typing changed the read-only model");
    stage = "copy path";
    await evaluate("(() => { const input = document.querySelector('#composer-input'); input.value = 'draft stays unchanged'; input.dispatchEvent(new Event('input', {bubbles:true})); })()");
    await clickText("Copy Path");
    await until("window.__fileCopies.at(-1) === 'src/main.rs'", "literal path copy");
    await until("document.querySelector('.toast')?.textContent.includes('Copied the launch-root-relative path')", "clipboard write success");
    check(await evaluate("document.querySelector('#composer-input').value === 'draft stays unchanged'"), "copy path changed draft");
    stage = "refresh selection";
    await clickText("Refresh");
    await until(`(async () => { const m = await import('/lib/monaco/vs/editor/editor.api.js'); return m.editor.getModels().length === 1 && m.editor.getModels()[0].getValue() === ${JSON.stringify(config.source)}; })()`, "refreshed source preview");
    stage = "file download";
    await clickText("Download File");
    await downloaded("main.rs");
    await until("![...document.querySelectorAll('button')].find(item => item.textContent.trim() === 'Download Folder').disabled", "download permit release");
    await clickText("Download Folder");
    await downloaded("src.zip");
    stage = "responsive pane layout";
    check(await layout(), "wide pane layout invalid");
    await rpc("Emulation.setDeviceMetricsOverride", { width: 540, height: 900, deviceScaleFactor: 1, mobile: false });
    await sleep(250);
    check(await layout(), "narrow pane layout invalid");
    await rpc("Emulation.setDeviceMetricsOverride", { width: 1280, height: 1000, deviceScaleFactor: 1, mobile: false });
    await evaluate("document.querySelector('.files-pane').style.height = '390px'");
    await sleep(250);
    check(await layout(), "resized pane layout invalid");
    await screenshot();
    stage = "same-origin module worker";
    workerError = await evaluate(`(async () => {
      const worker = window.MonacoEnvironment.getWorker('workerMain.js', 'editorWorkerService');
      let failed = false; worker.onerror = () => { failed = true; };
      await new Promise(resolve => setTimeout(resolve, 700)); worker.terminate(); return failed;
    })()`);
    check(!workerError, "module worker failed");
    check(await evaluate(`window.__fileWorkers.length > 0 && window.__fileWorkers.every(worker => new URL(worker.url).origin === location.origin && worker.type === 'module')`), "non-module or cross-origin worker");
    stage = "editor disposal";
    await click("#files-toggle");
    check(await evaluate("(async () => { const m = await import('/lib/monaco/vs/editor/editor.api.js'); return m.editor.getModels().length === 0 && m.editor.getEditors().length === 0; })()"), "editor model was retained after close");
    await click("#files-toggle");
    await until("document.querySelector('#files-monaco')?.dataset.editorReady === 'true'", "editor reopen");
  }
  stage = "network and CSP audit";
  const observed = await evaluate("window.__cspViolations");
  check(Array.isArray(observed), "page evaluation failed");
  if (Array.isArray(observed)) violations.push(...observed);
  check(violations.length === 0, "CSP violation observed");
  check(!requests.some((request) => request.origin !== config.origin), "external application network request");
  check(requests.filter((request) => request.path.startsWith("/api/files/")).every((request) => request.method === "POST"), "file operation used a non-POST endpoint");
  check(!sentFrames.some((frame) => frame.prompt || frame.fileContent), "file activity entered the prompt channel");
  check(pageErrors.length === 0, "page JavaScript exception observed");
  process.stdout.write(JSON.stringify({ browser: "Chrome for Testing", cspViolations: violations.length,
    requests: requests.length, fileRequests: requests.filter((request) => request.path.startsWith("/api/files/")).length,
    providerPrompts: 0, readOnly: config.enabled, screenshot: config.screenshot }) + "\n");
} catch (error) {
  // Keep bootstrap URLs, headers and raw browser exception payloads out of evidence.
  process.stderr.write(`Failure during ${stage}: ${error instanceof Error ? error.message : "browser check failed"}\n`);
  process.exitCode = 1;
} finally {
  await cleanup();
}
