// Native CDP-pipe driver. No npm packages, browser libraries, or existing profile.
import { spawn } from "node:child_process";
import { writeFile, stat } from "node:fs/promises";
import { resolve } from "node:path";

let input = "";
for await (const chunk of process.stdin) input += chunk;
const config = JSON.parse(input);
input = "";
let stage = "browser launch";
let browser;
let closed = false;
let session;
let counter = 0;
const pending = new Map();
const listeners = [];
const sleep = (ms) => new Promise((done) => setTimeout(done, ms));
const requests = [];
const sentFrames = [];
const violations = [];
const pageErrors = [];
let csp = null;
let permissions = null;
let workerError = false;

function check(value, label) { if (!value) throw new Error(label); }
function rpc(method, params = {}, target = session) {
  const id = ++counter;
  return new Promise((resolve, reject) => {
    const deadline = setTimeout(() => { pending.delete(id); reject(new Error("CDP deadline")); }, 15_000);
    pending.set(id, { resolve, reject, deadline });
    browser.stdio[3].write(JSON.stringify({ id, method, params, ...(target ? { sessionId: target } : {}) }) + "\0");
  });
}
async function evaluate(expression) {
  const result = await rpc("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true, userGesture: true });
  check(!result.exceptionDetails, "page evaluation failed");
  return result.result.value;
}
async function until(expression, label, timeout = 30_000) {
  stage = label;
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (await evaluate(expression)) return;
    await sleep(100);
  }
  throw new Error("browser condition deadline");
}
async function click(selector) {
  check(await evaluate(`(() => { const item = document.querySelector(${JSON.stringify(selector)}); if (!item || item.disabled) return false; item.click(); return true; })()`), "button unavailable");
}
async function clickText(text, selector = "button") {
  check(await evaluate(`(() => { const item = [...document.querySelectorAll(${JSON.stringify(selector)})].find(item => item.textContent.trim() === ${JSON.stringify(text)}); if (!item || item.disabled) return false; item.click(); return true; })()`), "button unavailable");
}
async function downloaded(filename) {
  stage = "download completion";
  const deadline = Date.now() + 15_000;
  while (Date.now() < deadline) {
    try { if ((await stat(resolve(config.downloads, filename))).isFile()) return; } catch {}
    await sleep(100);
  }
  throw new Error("download unavailable");
}
async function layout() {
  return evaluate(`(() => {
    const pane = document.querySelector('.files-pane').getBoundingClientRect();
    const chat = document.querySelector('#chat-scroll').getBoundingClientRect();
    const editor = document.querySelector('#files-monaco').getBoundingClientRect();
    const composer = document.querySelector('.composer').getBoundingClientRect();
    return pane.height >= 150 && pane.bottom <= chat.top + 1 && editor.width >= 120 &&
      editor.height >= 50 && chat.height > 10 && chat.bottom <= composer.top + 1 && composer.bottom <= innerHeight;
  })()`);
}
async function screenshot() {
  const result = await rpc("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
  await writeFile(config.screenshot, Buffer.from(result.data, "base64"));
}
async function cleanup() {
  if (!browser || closed) return;
  try { await rpc("Browser.close", {}, null); } catch {}
  for (let i = 0; i < 60 && !closed; i++) await sleep(50);
  if (!closed) browser.kill("SIGTERM");
  for (let i = 0; i < 40 && !closed; i++) await sleep(50);
  if (!closed) browser.kill("SIGKILL");
  for (const record of pending.values()) { clearTimeout(record.deadline); record.reject(new Error("browser closed")); }
  pending.clear();
}

try {
  const environment = { ...process.env, HOME: config.browserHome,
    XDG_CONFIG_HOME: resolve(config.browserHome, "config"), XDG_CACHE_HOME: resolve(config.browserHome, "cache") };
  browser = spawn(config.browser, ["--headless=new", `--user-data-dir=${config.profile}`,
    "--remote-debugging-pipe", "--ignore-certificate-errors", "--no-first-run", "--no-default-browser-check",
    "--disable-background-networking", "--disable-component-update", "--disable-sync", "--disable-extensions",
    "--window-size=1280,1000", "about:blank"], { env: environment, stdio: ["ignore", "ignore", "pipe", "pipe", "pipe"] });
  browser.stderr.on("data", () => {});
  browser.on("exit", () => { closed = true; });
  let buffered = "";
  browser.stdio[4].setEncoding("utf8");
  browser.stdio[4].on("data", (chunk) => {
    buffered += chunk;
    for (;;) {
      const end = buffered.indexOf("\0");
      if (end < 0) break;
      const message = JSON.parse(buffered.slice(0, end));
      buffered = buffered.slice(end + 1);
      if (message.id) {
        const entry = pending.get(message.id);
        if (!entry) continue;
        clearTimeout(entry.deadline);
        pending.delete(message.id);
        if (message.error) entry.reject(new Error("CDP command rejected")); else entry.resolve(message.result);
      } else {
        for (const listener of listeners) listener(message);
      }
    }
  });
  const target = await rpc("Target.createTarget", { url: "about:blank" });
  const attached = await rpc("Target.attachToTarget", { targetId: target.targetId, flatten: true });
  session = attached.sessionId;
  await rpc("Page.enable");
  await rpc("Runtime.enable");
  await rpc("Network.enable");
  await rpc("Browser.setDownloadBehavior", { behavior: "allow", downloadPath: config.downloads, eventsEnabled: true }, null);
  await rpc("Browser.grantPermissions", { origin: config.origin, permissions: ["clipboardReadWrite", "clipboardSanitizedWrite"] }, null);
  await rpc("Emulation.setDeviceMetricsOverride", { width: 1280, height: 1000, deviceScaleFactor: 1, mobile: false });
  listeners.push((event) => {
    if (event.sessionId !== session) return;
    if (event.method === "Network.requestWillBeSent") {
      const { url, method } = event.params.request;
      const parsed = new URL(url);
      // Never retain URL fragments, request headers, bodies or authentication proofs.
      requests.push({ origin: parsed.origin, path: parsed.pathname, method });
    }
    if (event.method === "Network.responseReceived" && event.params.type === "Document") {
      const headers = event.params.response.headers;
      csp = headers["content-security-policy"] ?? headers["Content-Security-Policy"];
      permissions = headers["permissions-policy"] ?? headers["Permissions-Policy"];
    }
    if (event.method === "Network.webSocketFrameSent") {
      const payload = event.params.response.payloadData;
      sentFrames.push({ prompt: payload.includes('"send_prompt"'), fileContent: payload.includes("browser-fixture-marker") || payload.includes("__fileExecuted") });
    }
    if (event.method === "Runtime.exceptionThrown") pageErrors.push(event.params.exceptionDetails.text);
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
  check(typeof csp === "string" && csp.includes("script-src 'self'") && !csp.includes("unsafe-eval") && !csp.includes("blob:"), "script policy weakened");
  check(permissions.includes("clipboard-read=()"), "clipboard read policy weakened");
  check(await evaluate("location.hash === ''"), "bootstrap fragment retained");
  if (!config.enabled) {
    stage = "disabled capability";
    check(!csp.includes("unsafe-inline") && permissions.includes("clipboard-write=()"), "disabled policy weakened");
    check(await evaluate("document.querySelector('#files-toggle') === null"), "disabled file controls visible");
    check(!requests.some((request) => request.path.includes("monaco")), "disabled editor loaded");
    await screenshot();
  } else {
    check(csp.includes("style-src 'self' 'unsafe-inline'") && csp.includes("worker-src 'self'"), "editor policy missing");
    check(permissions.includes("clipboard-write=(self)"), "copy path policy missing");
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
  violations.push(...await evaluate("window.__cspViolations"));
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
