import { testNativeFileDownloads, testNativeSessionDownloads } from "./native-download-browser.js";
import { testSessionStorage } from "./session-storage-browser.js";
import { testDotnetBrowser } from "./dotnet-browser.js";
import { testPythonNetwork } from "./python-network-browser.js";
import { testPythonSites } from "./python-sites-browser.js";
import { testConnectionCredentials, testConnectionLimits, testMissingConnectionKey } from "./connection-browser.js";
import { testStreamingControls } from "./streaming-browser.js";
import { testConnectionThinking } from "./thinking-browser.js";
import { spawn } from "node:child_process";
import { writeFile } from "node:fs/promises";
import { PREVIEW_WEBM } from "./media-fixture.js";
import { navigateDocument } from "./navigation.js";
import type { CdpListener } from "./navigation.js";

interface Config { browser: string; profile: string; origin: string; pyodide: boolean; screenshot: string }
process.stdin.setEncoding("utf8");
let input = "";
for await (const chunk of process.stdin) input += chunk;
const config = JSON.parse(input) as Config;
const browser = spawn(config.browser, ["--headless=new", "--password-store=basic", "--use-mock-keychain", "--no-sandbox", "--remote-debugging-pipe",
  `--user-data-dir=${config.profile}`, "--no-first-run", "--disable-background-networking",
  "--disable-component-update", "--disable-sync", "--window-size=1440,1100", "about:blank"],
{ stdio: ["ignore", "ignore", "pipe", "pipe", "pipe"] });
const pending = new Map<number, { resolve: (value: Record<string, unknown>) => void;
  reject: (error: Error) => void; timer: ReturnType<typeof setTimeout>; failure: Error }>();
let counter = 0;
let session: string | undefined;
let buffered = "";
let browserErrors = "";
const eventListeners = new Set<CdpListener>();
browser.stderr?.setEncoding("utf8");
browser.stderr?.on("data", (chunk) => { browserErrors = (browserErrors + chunk).slice(-3000); });
browser.on("exit", () => {
  for (const request of pending.values()) { clearTimeout(request.timer); request.reject(new Error(browserErrors)); }
  pending.clear();
});
const pipe = browser.stdio[4];
if (!pipe) throw new Error("No CDP pipe");
pipe.setEncoding("utf8");
pipe.on("data", (chunk) => {
  buffered += chunk;
  while (buffered.includes("\0")) {
    const end = buffered.indexOf("\0");
    const message = JSON.parse(buffered.slice(0, end)) as Record<string, unknown>;
    buffered = buffered.slice(end + 1);
    if (message["method"]) { for (const listener of eventListeners) listener(message); }
    const request = pending.get(Number(message["id"]));
    if (!request) continue;
    pending.delete(Number(message["id"])); clearTimeout(request.timer);
    if (message["error"]) {
      request.failure.message += `: ${JSON.stringify(message["error"])}`;
      request.reject(request.failure);
    }
    else request.resolve(message["result"] as Record<string, unknown>);
  }
});
function rpc(method: string, params: Record<string, unknown> = {}): Promise<Record<string, unknown>> {
  const id = ++counter;
  const failure = new Error(`CDP ${method} failed`);
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => { pending.delete(id); reject(new Error(`CDP timed out: ${method}`)); }, 180_000);
    pending.set(id, { resolve, reject, timer, failure });
    browser.stdio[3]?.write(JSON.stringify({ id, method, params, sessionId: session }) + "\0");
  });
}
async function evaluate(expression: string): Promise<unknown> {
  const response = await rpc("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (response["exceptionDetails"]) throw new Error(JSON.stringify(response["exceptionDetails"]));
  return (response["result"] as Record<string, unknown>)["value"];
}
async function navigate(method: "Page.navigate" | "Page.reload", params: Record<string, unknown> = {}): Promise<void> {
  if (!session) throw new Error("No page session for navigation");
  await navigateDocument(rpc, listener => {
    eventListeners.add(listener); return () => { eventListeners.delete(listener); };
  }, session, method, params);
}
function check(value: unknown, message: string): asserts value { if (!value) throw new Error(message); }
async function until(expression: string): Promise<void> {
  for (let attempt = 0; attempt < 200; attempt++) {
    if (await evaluate(expression)) return;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(`Condition failed: ${expression}\n${await evaluate("document.body.innerText.slice(-5000)")}`);
}


async function waitForStartup(): Promise<void> {
  for (let attempt = 0; attempt < 3600; attempt++) {
    if (await evaluate("!document.querySelector('#startup-splash') && !!document.querySelector('.app-shell')")) {
      check(await evaluate("performance.now()>=2000"), "splash ended before two seconds");
      return;
    }
    const error = await evaluate("document.querySelector('#startup-splash')?.dataset.failed==='true' ? document.querySelector('#splash-progress').textContent : ''");
    if (error) throw new Error('Startup failed: ' + String(error));
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  throw new Error('Startup did not finish');
}

try {
  const target = await rpc("Target.createTarget", { url: "about:blank" });
  const attached = await rpc("Target.attachToTarget", { targetId: target["targetId"], flatten: true });
  session = String(attached["sessionId"]);
  await rpc("Page.enable"); await rpc("Runtime.enable");
  await rpc("Page.setLifecycleEventsEnabled", {enabled: true});
  // Match the packaged Tauri WebView's hardening, including every lazy Monaco import.
  await rpc("Page.addScriptToEvaluateOnNewDocument", {source: "Object.freeze(Object.prototype);"});
  // Hold the actual WASM download to verify that elapsed time alone never hides the popup.
  const startupGate = await rpc("Page.addScriptToEvaluateOnNewDocument", {source: `
    {const original=globalThis.fetch;const gate=new Promise(resolve=>globalThis.releaseStartupWasm=resolve);
    globalThis.fetch=async (...args)=>{const url=args[0] instanceof Request?args[0].url:String(args[0]);
      if(url.includes('lethetic_wasm_bg.wasm'))await gate;return original(...args);};}`});
  await navigate("Page.navigate", { url: `${config.origin}/lethetic/` });
  await until("!!document.querySelector('#startup-splash:modal')");
  check(await evaluate("document.querySelector('#startup-splash').getAttribute('aria-busy')==='true'"), "startup did not announce loading");
  check(await evaluate("(()=>{const splash=document.querySelector('#startup-splash');splash.click();return splash.open && !splash.dispatchEvent(new Event('cancel',{cancelable:true}))})()"), "loading splash dismissed before readiness");
  // Session hydration waits for WASM; all startup status belongs in the existing splash.
  await until("document.querySelector('#splash-progress')?.textContent==='Loading saved sessions…'");
  check(await evaluate("(()=>{const r=document.querySelector('#startup-splash').getBoundingClientRect();return Math.abs(r.width-innerWidth*.7)<2 && Math.abs(r.height-innerHeight*.7)<2})()"), "splash must occupy 70% of the viewport dimensions");
  check(await evaluate("!document.querySelector('#startup-splash button') && document.querySelector('#browser-root').childElementCount===0"), "startup added a continue button or a duplicate loading screen behind the splash");
  const splashImage = await rpc("Page.captureScreenshot", { format: "png" });
  await writeFile(config.screenshot.replace('.png','-splash.png'), Buffer.from(String(splashImage['data']), 'base64'));
  await rpc("Emulation.setDeviceMetricsOverride", { width: 390, height: 844, deviceScaleFactor: 1, mobile: true });
  await rpc("Emulation.setEmulatedMedia", { features: [{ name: "prefers-reduced-motion", value: "reduce" }] });
  await evaluate("new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))");
  check(await evaluate("document.querySelector('#startup-splash').getAnimations({subtree:true}).length===0"), "splash ignored reduced-motion preference");
  check(await evaluate("document.querySelector('.splash-brand').getBoundingClientRect().width<=document.querySelector('#startup-splash').clientWidth-24"), "splash wordmark overflowed mobile viewport");
  const splashMobile = await rpc("Page.captureScreenshot", { format: "png" });
  await writeFile(config.screenshot.replace('.png','-splash-mobile.png'), Buffer.from(String(splashMobile['data']), 'base64'));
  await rpc("Emulation.clearDeviceMetricsOverride");
  await rpc("Emulation.setEmulatedMedia", { features: [] });
  await evaluate("new Promise(resolve=>setTimeout(resolve,Math.max(0,2100-performance.now())))");
  check(await evaluate("document.querySelector('#startup-splash')?.open"), "popup hid before WASM finished loading");
  await evaluate("globalThis.releaseStartupWasm()");
  await rpc("Page.removeScriptToEvaluateOnNewDocument", {identifier: startupGate['identifier']});
  await waitForStartup();
  await until("!!document.querySelector('[data-field=name]')");
  check(await evaluate("performance.now()>=2000 && !document.querySelector('#startup-splash') && !document.querySelector('[aria-busy=true]')"), "splash ended early or remained over the ready application");
  await evaluate(`document.querySelector('[data-field=endpoint]').value=location.origin+'/v1/chat/completions';
    document.querySelector('[data-field=key]').value='secret-never-persisted';
    document.querySelector('[data-field=fetch-models]').click()`);
  await until("document.querySelector('[data-field=model-list]').options.length===3");
  check(await evaluate("document.querySelector('[data-field=model]').value===''"), "discovery changed model before selection");
  await evaluate(`{ const filter=document.querySelector('[data-field=model-filter]'); filter.value='DEEPSEEK'; filter.dispatchEvent(new Event('input')); }`);
  check(await evaluate("document.querySelector('[data-field=model-list]').options.length===1"), "model list did not filter");
  await evaluate("document.querySelector('[data-field=use-model]').click()");
  check(await evaluate("document.querySelector('[data-field=model]').value==='deepseek/test'"), "model selection failed");
  await evaluate(`{ const filter=document.querySelector('[data-field=model-filter]'); filter.value='no-such-model'; filter.dispatchEvent(new Event('input')); }`);
  check(await evaluate("document.querySelector('[data-field=use-model]').disabled && document.querySelector('[data-field=model-count]').textContent==='0 of 3 models'"), "empty model filter was not handled");
  // A stale in-flight list must not populate a different connection's picker.
  await evaluate(`globalThis.originalFetch=fetch; globalThis.modelRequests=[];
    globalThis.fetch=(url,options)=>String(url).endsWith('/v1/models') ? new Promise(resolve=>globalThis.modelRequests.push({resolve,signal:options.signal})) : globalThis.originalFetch(url,options);
    document.querySelector('[data-field=fetch-models]').click();
    document.querySelector('[data-field=endpoint]').dispatchEvent(new Event('input'));
    globalThis.modelRequests[0].resolve(Response.json({data:[{id:'stale'}]}));`);
  check(await evaluate("document.querySelector('[data-field=model-picker]').hidden && globalThis.modelRequests[0].signal.aborted"), "stale model discovery survived endpoint change");
  await evaluate("globalThis.fetch=globalThis.originalFetch");
  check(await evaluate(`(() => {
    const set = (name, value) => document.querySelector('[data-field='+name+']').value=value;
    set('name','Browser test'); set('endpoint',location.origin+'/v1/chat/completions');
    set('model','test'); set('key','secret-never-persisted');
    document.querySelector('[data-field=connection-form]').requestSubmit();
    return document.querySelector('dialog[open] [data-field=status]').textContent.includes('Connection saved');
  })()`), "connection could not be saved");
  check(await evaluate("!JSON.stringify(localStorage).includes('secret-never-persisted')"), "key persisted");
  await evaluate("document.querySelector('[data-field=test]').click()");
  await until("document.querySelector('dialog[open] [data-field=status]').textContent.includes('valid response')");
  check(await evaluate("!!document.querySelector('[data-field=active-summary] .is-valid') && document.querySelector('[data-field=active-summary]').textContent.includes('Saved key present')"), "active connection or key presence is unclear");
  const connectionSession = await evaluate("location.hash");
  await testConnectionCredentials(evaluate);
  await evaluate("document.querySelector('[data-field=duplicate]').click()");
  check(await evaluate("document.querySelector('[data-field=name]').value==='Browser test copy' && document.querySelector('[data-field=key]').value==='secret-never-persisted'"), "duplicate lost connection key");
  check(await evaluate("document.querySelector('[data-field=active-summary] strong').textContent==='Browser test'"), "duplicating activated the copy");
  await evaluate("document.querySelector('[data-field=use]').click()");
  check(await evaluate("document.querySelector('[data-field=active-summary] strong').textContent==='Browser test copy'"), "Use did not activate the copy");
  check(await evaluate("location.hash")===connectionSession,"switching connections changed session");
  await evaluate(`globalThis.connectionFetch=fetch;
    globalThis.fetch=(url,options)=>String(url).endsWith('/v1/chat/completions') ? Promise.resolve(new Response('Invalid key',{status:401})) : globalThis.connectionFetch(url,options);
    document.querySelector('[data-field=key]').value='bad-test-key'; document.querySelector('[data-field=key]').dispatchEvent(new Event('input'));
    document.querySelector('[data-field=connection-form]').requestSubmit()`);
  await until("!!document.querySelector('[data-field=active-summary] .is-invalid')");
  const connectionImage=await rpc("Page.captureScreenshot",{format:"png"});
  await writeFile(config.screenshot.replace('.png','-connections.png'),Buffer.from(String(connectionImage['data']),'base64'));
  await rpc("Emulation.setDeviceMetricsOverride",{width:390,height:844,deviceScaleFactor:1,mobile:true});
  check(await evaluate("document.querySelector('.browser-connections .overlay-body').scrollWidth<=document.querySelector('.browser-connections .overlay-body').clientWidth+1"), "connections overflow on a phone");
  const connectionMobile=await rpc("Page.captureScreenshot",{format:"png"});
  await writeFile(config.screenshot.replace('.png','-connections-mobile.png'),Buffer.from(String(connectionMobile['data']),'base64'));
  await rpc("Emulation.clearDeviceMetricsOverride");
  await evaluate("globalThis.fetch=globalThis.connectionFetch; document.querySelector('[data-connection-index=\"0\"] [data-connection-action=use]').click()");
  await until("!!document.querySelector('[data-field=active-summary] .is-valid')");
  await evaluate("document.querySelector('[data-connection-index=\"0\"] [data-connection-action=edit]').click()");
  check(await evaluate("document.querySelector('[data-field=key]').value==='secret-never-persisted' && !!document.querySelector('[data-field=active-summary] .is-valid')"), "switching back lost the original key or validation");
  check(await evaluate("document.querySelector('[data-field=name]').value==='Browser test' && document.querySelector('[data-field=endpoint]').value===location.origin+'/v1/chat/completions'"),"saved name/endpoint did not populate");
  await evaluate("for(const name of ['name','endpoint']){const input=document.querySelector('[data-field='+name+']');input.value='';input.dispatchEvent(new Event('input'))} document.querySelector('[data-connection-index=\"0\"] [data-connection-action=edit]').click()");
  check(await evaluate("document.querySelector('[data-field=name]').value==='Browser test' && document.querySelector('[data-field=endpoint]').value===location.origin+'/v1/chat/completions'"),"stale blank draft replaced saved fields on Edit");
  await evaluate("document.querySelector('[data-field=name]').value='Renamed connection'; document.querySelector('[data-field=endpoint]').value=location.origin+'/alternate/v1/chat/completions';document.querySelector('[data-field=connection-form]').requestSubmit();document.querySelector('[data-connection-index=\"1\"] [data-connection-action=edit]').click();document.querySelector('[data-connection-index=\"0\"] [data-connection-action=edit]').click()");
  check(await evaluate("document.querySelector('[data-field=name]').value==='Renamed connection' && document.querySelector('[data-field=endpoint]').value.endsWith('/alternate/v1/chat/completions') && document.querySelector('[data-field=key]').value==='secret-never-persisted'"),"edited name, endpoint or key was lost");
  await evaluate("document.querySelector('[data-field=name]').value='Browser test';document.querySelector('[data-field=endpoint]').value=location.origin+'/v1/chat/completions';document.querySelector('[data-field=connection-form]').requestSubmit()");

  await evaluate(`globalThis.processingFetch=fetch;
    globalThis.processingStarted=false;
    const processingGate=new Promise(resolve=>{globalThis.releaseProcessing=resolve});
    globalThis.fetch=async (...args)=>{if(String(args[0]).endsWith('/v1/chat/completions')){globalThis.processingStarted=true;await processingGate} return globalThis.processingFetch(...args)};
    document.querySelector('dialog[open]').close();
    const prompt=document.getElementById('composer-input'); prompt.value='hello';
    prompt.dispatchEvent(new Event('input',{bubbles:true})); document.querySelector('form.composer').requestSubmit()`);
  await until("globalThis.processingStarted && !!document.querySelector('.activity-label.is-processing')");
  check(await evaluate("(()=>{const spinner=getComputedStyle(document.querySelector('.activity-spinner')),label=getComputedStyle(document.querySelector('.activity-label'));return parseFloat(spinner.fontSize)===2*parseFloat(label.fontSize) && label.animationName==='processing-shimmer' && label.backgroundImage.includes('gradient')})()"), "processing indicator size or gradient missing");
  const processingImage=await rpc("Page.captureScreenshot",{format:"png"});
  await writeFile(config.screenshot.replace('.png','-processing.png'),Buffer.from(String(processingImage['data']),'base64'));
  await rpc("Emulation.setEmulatedMedia", {features:[{name:"prefers-reduced-motion",value:"reduce"}]});
  check(await evaluate("getComputedStyle(document.querySelector('.activity-label')).animationName==='none'"), "processing gradient ignores reduced motion");
  await rpc("Emulation.setEmulatedMedia", {features:[]});
  await evaluate("globalThis.fetch=globalThis.processingFetch; globalThis.releaseProcessing()");
  await until("document.querySelector('.app-shell').textContent.includes('Hello from the browser test')");
  await until("document.querySelector('.browser-usage-summary').textContent.includes('1,200 in')");
  check(await evaluate("document.querySelector('.browser-status').textContent.includes('1,024') && !document.querySelector('.transport-level')"), "provider cache usage or browser status failed");
  check(await evaluate("document.querySelector('[data-pinned-stat=context]').textContent.includes('1,200 / 131,072')"), "context must use latest provider input and session input budget");
  for (const stat of ["contextMax", "outputBudget", "requestUsage", "thinking", "files", "blocks", "timeout", "rounds", "retries"]) {
    check(await evaluate(`!!document.querySelector('[data-stat-pin=${stat}]')`), `Missing CLI-style stat ${stat}`);
  }
  check(await evaluate("performance.getEntriesByType('resource').some(r=>r.name.includes('/lethetic/browser/wasm/lethetic_wasm_bg.wasm'))"), "WASM was not loaded from the project subpath");
  await evaluate(`Object.defineProperty(navigator,'clipboard',{configurable:true,value:{writeText:async text=>{globalThis.copiedTurn=text}}});
    document.querySelector('.block-user [data-copy-turn]').click()`);
  await until("globalThis.copiedTurn==='hello'");
  await evaluate("document.querySelector('.block-markdown [data-copy-turn]').click()");
  await until("globalThis.copiedTurn.includes('Hello from the browser test') && globalThis.copiedTurn.includes('private snippet')");
  check(await evaluate("[...document.querySelectorAll('article.chat-block')].every(block=>!!block.querySelector('.chat-block-actions [data-copy-turn]'))"), "a turn is missing Copy contents");
  // These are the original remote UI controls, driven by the local transport.
  await evaluate("document.querySelector('[aria-label=\"Open command palette\"]').click()");
  await until("!!document.querySelector('#palette-command-themes')");
  await evaluate("document.querySelector('#palette-command-themes').click()");
  await until("!!document.querySelector('.theme-choice')");
  await evaluate("document.querySelector('.theme-choice:not(.is-selected)').click()");
  await evaluate("[...document.querySelectorAll('.theme-choice')].find(button=>button.textContent.includes('Dracula')).click()");
  await evaluate("document.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true}))");
  await evaluate("document.querySelector('[aria-label=\"Open command palette\"]').click(); document.querySelector('#palette-command-toggle-debugger').click()");
  await until("!!document.querySelector('.has-debugger')");
  await evaluate("document.querySelector('[aria-label=\"Open command palette\"]').click(); document.querySelector('#palette-command-toggle-debugger').click()");
  check(await evaluate("!!document.querySelector('.markdown-code-block [data-browser-action=save-code]') && !!document.querySelector('.markdown-code-block [data-browser-action=copy] .ui-icon')"), "snippet actions are not beside their code box");
  await evaluate("document.querySelector('[data-browser-action=save-code]').click()");
  await until("!!document.querySelector('dialog[open] [data-field=file-content]')");
  check(await evaluate("document.querySelector('[data-field=file-content]').value===\"print('private snippet')\""), "code capture failed");
  await evaluate(`Object.defineProperty(navigator,'clipboard',{configurable:true,value:{writeText:async text=>{globalThis.copiedText=text}}});
    document.querySelector('[data-field=copy-content]').click()`);
  await until("globalThis.copiedText===\"print('private snippet')\"");
  await evaluate("document.querySelector('[data-field=preview-content]').click()");
  await until("!!document.querySelector('.browser-preview[open]')");
  await until("document.querySelector('.browser-preview[open] [data-field=preview-monaco] .view-lines')?.textContent.includes('private')");
  check(await evaluate("document.querySelector('.browser-preview[open] [data-field=preview-frame]').hidden"), "code preview did not use Monaco");
  await evaluate("document.querySelector('.browser-preview[open]').close()");
  await evaluate(`document.querySelector('[data-field=file-path]').value='reports/analysis.py';
    document.querySelector('[data-field=file-form]').requestSubmit()`);
  await until("document.querySelector('.files-path')?.textContent.includes('reports/analysis.py') && !document.querySelector('[data-file-action=preview]').disabled");
  check(await evaluate("!!document.querySelector('.files-preview .files-content-actions [data-file-action=preview]')"), "file actions are not beside the content viewer");
  await until("document.querySelector('#files-monaco')?.dataset.editorReady==='true'");
  await until("document.querySelector('#files-monaco .view-lines')?.textContent.includes('private')");
  check(await evaluate("!!document.querySelector('#files-monaco .line-numbers') && !document.querySelector('.files-native-text')"), "Monaco is not the default code preview");
  check(await evaluate(`(async () => {
    const api=await import('./web/lib/monaco/vs/editor/editor.api.js');
    const model=api.editor.getModels()[0];
    const thread=globalThis.MonacoEnvironment.getWorker('workerMain.js','editorWorkerService');
    let replies=0; thread.addEventListener('message',()=>replies++);
    const worker=api.editor.createWebWorker({worker:thread});
    try { await worker.withSyncedResources([model.uri]); return replies>0 && Object.isFrozen(Object.prototype); }
    finally { worker.dispose(); }
  })()`), "Monaco module worker failed with native prototype protection on a local HTTP origin");
  await evaluate("document.querySelector('[data-file-action=text-mode]').click()");
  check(await evaluate("(()=>{const text=document.querySelector('.files-native-text');text.focus();text.setSelectionRange(0,text.value.length);return text.selectionEnd>10 && text.value.includes(\"print('private snippet')\")})()"), "native multiline text selection failed");
  await evaluate("document.querySelector('[data-preview-expand]').click()");
  check(await evaluate("(()=>{const r=document.querySelector('.files-preview').getBoundingClientRect();return r.width>=innerWidth-2 && r.height>=innerHeight-2})()"), "Files preview did not expand");
  await evaluate("document.querySelector('[data-preview-expand]').click()");

  await evaluate("document.querySelector('[data-file-action=text-mode]').click()");
  await until("document.querySelector('#files-monaco')?.dataset.editorReady==='true'");

  await evaluate("document.querySelector('[data-preview-expand]').click()");
  check(await evaluate("!!document.querySelector('.files-pane.is-full-preview #files-monaco .line-numbers')"), "expanded file pane lost Monaco");
  await evaluate("document.querySelector('[data-preview-expand]').click()");

  check(await evaluate("!document.querySelector('[data-field=passphrase]') && !document.querySelector('[data-browser-action=code]')"), "encrypted library remains visible");
  await evaluate(`Object.defineProperty(navigator,'clipboard',{configurable:true,value:{writeText:async text=>{globalThis.copiedText=text}}});
    document.querySelector('[data-file-action=copy]').click()`);
  await until("globalThis.copiedText===\"print('private snippet')\"");
  await evaluate(`globalThis.originalAnchorClick=HTMLAnchorElement.prototype.click;
    globalThis.originalObjectURL=URL.createObjectURL;
    URL.createObjectURL=function(blob){globalThis.downloadTextPromise=blob.text();return globalThis.originalObjectURL(blob)};
    HTMLAnchorElement.prototype.click=function(){globalThis.downloadName=this.download};
    [...document.querySelectorAll('.files-actions button')].find(b=>b.title==='Download the selected file (up to 32 MiB)').click()`);
  await until("!!globalThis.downloadTextPromise");
  check(await evaluate("globalThis.downloadTextPromise") === "print('private snippet')", "download changed file contents");
  check(await evaluate("globalThis.downloadName") === "analysis.py", "download filename changed");
  await evaluate("HTMLAnchorElement.prototype.click=globalThis.originalAnchorClick; URL.createObjectURL=globalThis.originalObjectURL");
  await testNativeFileDownloads(evaluate, until);
  await evaluate("document.querySelector('[data-file-action=preview]').click()");
  await until("!!document.querySelector('.browser-preview[open]')");
  check(await evaluate("document.querySelector('.browser-preview[open] h2').textContent === 'reports/analysis.py'"), "preview title omitted filename");
  await evaluate("document.querySelector('[data-field=preview-expand]').click()");
  check(await evaluate("(()=>{const r=document.querySelector('.browser-preview[open]').getBoundingClientRect();return r.width>=innerWidth-2 && r.height>=innerHeight-2})()"), "preview did not expand to window");
  await evaluate("document.querySelector('[data-field=preview-expand]').click()");
  await until("document.querySelector('.browser-preview[open] [data-field=preview-monaco] .view-lines')?.textContent.includes('private')");
  check(await evaluate(`(() => { const host=document.querySelector('.browser-preview[open] [data-field=preview-monaco]');
    return !host.hidden && !!host.querySelector('.line-numbers') && host.getBoundingClientRect().height>100 && host.querySelectorAll('.view-line span[class*=mtk]').length>1; })()`), "dialog preview lost Monaco highlighting or sizing");
  await evaluate("document.querySelector('.browser-preview[open] [data-field=preview-expand]').click(); globalThis.previewEditor=document.querySelector('.browser-preview[open] .monaco-editor')");
  check(await evaluate("!document.querySelector('.browser-preview[open] [data-field=preview-monaco]').hidden && !!previewEditor"), "expanded preview lost Monaco");
  await evaluate("document.querySelector('.browser-preview[open] [data-field=preview-text-mode]').click()");
  check(await evaluate(`(() => {const dialog=document.querySelector('.browser-preview[open]'), text=dialog.querySelector('[data-field=preview-plain-text]');
    text.focus();text.setSelectionRange(0,text.value.length);const name=dialog.querySelector('h2').textContent;
    text.dispatchEvent(new KeyboardEvent('keydown',{key:'ArrowRight',bubbles:true}));
    return !text.hidden && text.selectionEnd>10 && text.value.includes("print('private snippet')") && dialog.querySelector('h2').textContent===name;})()`), "plain text selection or arrow keys switched files");
  await evaluate("document.querySelector('.browser-preview[open] [data-field=preview-text-mode]').click()");
  await until("document.querySelector('.browser-preview[open] [data-field=preview-monaco] .view-lines')?.textContent.includes('private')");
  await evaluate("document.querySelector('.browser-preview[open] [data-field=preview-expand]').click()");
  check(await evaluate(`(async () => {
    const {previewDocument}=await import('./src/file-actions.js');
    const doc=previewDocument('<h1>Hello</h1><script>parent.hacked=true<\/script><img src="https://example.com/leak" onerror="evil()"><a href="https://example.com">leave</a><form><input></form>','html','test.html');
    const parsed=new DOMParser().parseFromString(doc,'text/html');
    return !!parsed.querySelector('h1') && !parsed.querySelector('script,form,[onerror],[href],[src]') && doc.includes("default-src 'none'") && !globalThis.hacked;
  })()`), "preview sanitizer allowed active content");
  await evaluate("document.querySelector('dialog[open]').close(); document.querySelector('#files-toggle').click()");
  await evaluate(`document.querySelector('#files-toggle').click(); document.querySelector('[data-file-action=new]').click();
    document.querySelector('[data-field=file-path]').value='reports/preview.html';
    document.querySelector('[data-field=file-content]').value='<h1>Rendered local HTML</h1><script>parent.hacked=true<\\/script>';
    document.querySelector('[data-field=file-form]').requestSubmit()`);
  await until("document.querySelector('.files-path')?.textContent.includes('reports/preview.html') && !document.querySelector('[data-file-action=preview]').disabled");
  check(await evaluate("document.querySelector('.files-inline-preview')?.srcdoc.includes('Rendered local HTML')"),"HTML did not preview automatically");
  await evaluate("document.querySelector('[data-file-action=source]').click()");
  await until("document.querySelector('#files-monaco')?.dataset.editorReady==='true'");
  await until("document.querySelector('#files-monaco .view-lines')?.textContent.includes('Rendered')");
  check(await evaluate("!document.querySelector('.files-inline-preview')"),"HTML source toggle failed");
  await evaluate("document.querySelector('[data-file-action=source]').click();document.querySelector('[data-file-action=preview]').click()");
  await until("!!document.querySelector('.browser-preview[open]')");
  // Resolve the open dialog's exact iframe; other inline/closed frames may coexist.
  await evaluate("document.querySelector('.browser-preview[open] [data-field=preview-source]').click()");
  await until("document.querySelector('.browser-preview[open] [data-field=preview-monaco] .view-lines')?.textContent.includes('Rendered')");
  check(await evaluate("document.querySelector('.browser-preview[open] [data-field=preview-frame]').hidden && !globalThis.hacked"), "HTML source did not use safe Monaco preview");
  await evaluate("document.querySelector('.browser-preview[open] [data-field=preview-source]').click()");
  check(await evaluate("!document.querySelector('.browser-preview[open] [data-field=preview-frame]').hidden"), "Rendered view did not restore");
  const previewInput = async (expression: string, full = true): Promise<unknown> => {
    const interactive = await evaluate("!!document.querySelector('.browser-sites[open] iframe')");
    if (!interactive) await until(`document.querySelector('.browser-preview[open] [data-field=preview-frame]').dataset.navigationReady==='${String(full)}'`);
    const selector = interactive ? '.browser-sites[open] iframe' : '.browser-preview[open] iframe';
    const handle=await rpc('Runtime.evaluate',{expression:`document.querySelector(${JSON.stringify(selector)})`});
    const node=await rpc('DOM.describeNode',{objectId:(handle['result'] as {objectId:string}).objectId});
    const frameId=(node['node'] as {frameId:string}).frameId;
    const targets=await rpc('Target.getTargets');
    const frame=(targets['targetInfos'] as {type:string;targetId:string}[]).find(target=>target.type==='iframe' && target.targetId===frameId);
    if(frame) {
      const attached=await rpc('Target.attachToTarget',{targetId:frame.targetId,flatten:true});
      const parentSession=session;session=String(attached['sessionId']);
      try{return await evaluate(expression)}finally{session=parentSession}
    }
    const world=await rpc('Page.createIsolatedWorld',{frameId,worldName:'preview-navigation-test'});
    const result=await rpc('Runtime.evaluate',{contextId:world['executionContextId'],expression,returnByValue:true,awaitPromise:true});
    check(!result['exceptionDetails'],JSON.stringify(result['exceptionDetails']));
    return (result['result'] as {value:unknown}).value;
  };
  const previewText=await previewInput("document.querySelector('h1')?.textContent",false);
  check(previewText === 'Rendered local HTML', 'HTML preview did not render: ' + JSON.stringify(previewText));
  check(!await evaluate('globalThis.hacked'), 'preview executed a script');
  await evaluate("document.querySelector('[data-field=preview-expand]').click()");
  await previewInput("dispatchEvent(new KeyboardEvent('keydown',{key:'ArrowRight',bubbles:true}))");
  await until("document.querySelector('.browser-preview[open] h2').textContent==='reports/analysis.py'");
  await evaluate("document.querySelector('.browser-preview[open]').dispatchEvent(new KeyboardEvent('keydown',{key:'ArrowLeft',bubbles:true}))");
  await until("document.querySelector('.browser-preview[open] h2').textContent==='reports/preview.html'");
  await new Promise(resolve=>setTimeout(resolve,100));
  const selectionState=await previewInput("(()=>{const range=document.createRange();range.selectNodeContents(document.querySelector('h1'));const selection=getSelection();selection.removeAllRanges();selection.addRange(range);const state={text:selection.toString(),ranges:selection.rangeCount,collapsed:selection.isCollapsed};dispatchEvent(new KeyboardEvent('keydown',{key:'ArrowRight',bubbles:true}));return state})()");
  check((selectionState as {ranges:number;collapsed:boolean}).ranges>0 && !(selectionState as {collapsed:boolean}).collapsed,"preview text selection was not established: "+JSON.stringify(selectionState));
  check(await evaluate("document.querySelector('.browser-preview[open] h2').textContent==='reports/preview.html'"),"selection was replaced by navigation: "+JSON.stringify(selectionState));
  await previewInput("(()=>{getSelection().removeAllRanges();const t=new Touch({identifier:1,target:document.body,clientX:300,clientY:100});dispatchEvent(new TouchEvent('touchstart',{touches:[t]}));const end=new Touch({identifier:1,target:document.body,clientX:100,clientY:105});dispatchEvent(new TouchEvent('touchend',{changedTouches:[end]}))})()");
  await until("document.querySelector('.browser-preview[open] h2').textContent==='reports/analysis.py'");
  await evaluate("document.querySelector('[data-field=preview-previous]').click()");
  await until("document.querySelector('.browser-preview[open] h2').textContent==='reports/preview.html'");
  const fullImage=await rpc('Page.captureScreenshot',{format:'png'});
  await writeFile(config.screenshot.replace('.png','-fullscreen.png'),Buffer.from(String(fullImage['data']),'base64'));

  await evaluate("document.querySelector('dialog[open]').close(); globalThis.confirm=()=>true; document.querySelector('[data-file-action=delete]').click()");
  await until("!document.querySelector('.files-entry[title=\"reports/preview.html\"]')");
  await evaluate("document.querySelector('#files-toggle').click()");
  await evaluate("document.querySelector('.browser-status').open=true; document.querySelectorAll('[data-stat-pin][aria-pressed=false]').forEach(button=>button.click()); document.querySelector('.browser-status').open=false");
  check(await evaluate("document.querySelectorAll('[data-pinned-stat]').length===document.querySelectorAll('[data-stat-pin]').length && document.querySelectorAll('[data-pinned-stat]').length>=30"), "stats did not pin");
  const originalHash = await evaluate("location.hash");
  await evaluate("document.querySelector('#files-toggle').click()");
  await until("!!document.querySelector('.files-entry[title=reports]')");
  await evaluate("document.querySelector('.files-entry[title=reports]').click()");
  await until("!!document.querySelector('.files-entry[title=\"reports/analysis.py\"]')");
  await evaluate("document.querySelector('.files-entry[title=\"reports/analysis.py\"]').click()");
  await until("document.querySelector('.files-path small')?.textContent.includes('B')");
  await until("document.querySelector('#files-monaco')?.dataset.editorReady==='true'");
  await until("!document.querySelector('.toast')");
  check(await evaluate("document.querySelectorAll('.files-dock-controls button').length===1"), "dock should use one toggle");
  await evaluate("document.querySelector('[data-files-dock=left]').click()");
  await evaluate("new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))");
  check(await evaluate("(()=>{const files=document.querySelector('.files-pane').getBoundingClientRect(), list=document.querySelector('.files-browser').getBoundingClientRect(), editor=document.querySelector('.files-preview').getBoundingClientRect(), chat=document.querySelector('#chat-scroll').getBoundingClientRect();return files.right<=chat.left+1 && list.bottom<=editor.top+1 && editor.width>200})()"), "left dock did not stack files above the viewer beside chat");
  check(await evaluate("!!document.querySelector('.files-entry .file-type-icon[title=Python]')"), "Python file icon missing");
  const oldWidth = Number(await evaluate("document.querySelector('.files-pane').getBoundingClientRect().width"));
  const grip = await evaluate("(()=>{const r=document.querySelector('.files-outer-splitter').getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2}})()") as {x:number;y:number};
  await rpc('Input.dispatchMouseEvent',{type:'mousePressed',x:grip.x,y:grip.y,button:'left',clickCount:1});
  await rpc('Input.dispatchMouseEvent',{type:'mouseMoved',x:grip.x+100,y:grip.y,button:'left',buttons:1});
  await rpc('Input.dispatchMouseEvent',{type:'mouseReleased',x:grip.x+100,y:grip.y,button:'left',clickCount:1});
  check(Number(await evaluate("document.querySelector('.files-pane').getBoundingClientRect().width"))>oldWidth+50,"Files edge did not resize");
  await evaluate("document.querySelector('.files-body > .files-splitter').dispatchEvent(new KeyboardEvent('keydown',{key:'ArrowDown',bubbles:true}))");
  check(await evaluate("!!document.querySelector('.files-body').style.getPropertyValue('--files-list-height')"),"list/preview split did not resize");

  const leftImage=await rpc("Page.captureScreenshot",{format:"png"});
  await writeFile(config.screenshot.replace('.png','-files-left.png'),Buffer.from(String(leftImage['data']),'base64'));
  await evaluate("document.querySelector('[data-files-dock=top]').click()");
  await evaluate("new Promise(resolve=>requestAnimationFrame(resolve))");
  check(await evaluate("document.querySelector('.files-pane').getBoundingClientRect().bottom<=document.querySelector('#chat-scroll').getBoundingClientRect().top+1"), "top dock did not restore above chat");
  const topHeight = Number(await evaluate("document.querySelector('.files-pane').getBoundingClientRect().height"));
  const topGrip = await evaluate("(()=>{const r=document.querySelector('.files-outer-splitter').getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2}})()") as {x:number;y:number};
  await rpc('Input.dispatchMouseEvent',{type:'mousePressed',x:topGrip.x,y:topGrip.y,button:'left',clickCount:1});
  await rpc('Input.dispatchMouseEvent',{type:'mouseMoved',x:topGrip.x,y:topGrip.y+70,button:'left',buttons:1});
  await rpc('Input.dispatchMouseEvent',{type:'mouseReleased',x:topGrip.x,y:topGrip.y+70,button:'left',clickCount:1});
  check(Number(await evaluate("document.querySelector('.files-pane').getBoundingClientRect().height"))>topHeight+50,"horizontal Files edge did not resize");
  const draggedHeight = Number(await evaluate("document.querySelector('.files-pane').getBoundingClientRect().height"));
  await evaluate("document.querySelector('.files-outer-splitter').dispatchEvent(new KeyboardEvent('keydown',{key:'ArrowUp',bubbles:true}))");
  check(Number(await evaluate("document.querySelector('.files-pane').getBoundingClientRect().height"))<draggedHeight-10,"horizontal Files edge keyboard resize failed");
  const filesImage = await rpc("Page.captureScreenshot", { format: "png" });
  await writeFile(config.screenshot.replace('.png','-files.png'), Buffer.from(String(filesImage["data"]), "base64"));
  await evaluate("document.querySelector('[data-files-dock=left]').click()");
  await evaluate("document.querySelector('#files-toggle').click()");
  check(await evaluate("(()=>{const rail=document.querySelector('.files-pane').getBoundingClientRect(), chat=document.querySelector('#chat-scroll').getBoundingClientRect(), toggle=document.querySelector('#files-toggle').getBoundingClientRect();return rail.width<=32 && document.querySelector('.files-pane').scrollWidth<=rail.width && rail.height>rail.width && rail.right<=chat.left+1 && Math.abs(rail.top-chat.top)<1 && toggle.height>toggle.width})()"), "collapsed left Files bar is not vertical beside chat");
  await evaluate("document.querySelector('[data-files-dock=top]').click()");
  check(await evaluate("document.querySelector('.files-pane').getBoundingClientRect().bottom<=document.querySelector('#chat-scroll').getBoundingClientRect().top+1"), "collapsed top dock did not restore above chat");
  await evaluate("document.querySelector('[data-files-dock=left]').click()");
  const image = await rpc("Page.captureScreenshot", { format: "png" });
  await writeFile(config.screenshot, Buffer.from(String(image["data"]), "base64"));
  await rpc("Emulation.setDeviceMetricsOverride", { width: 390, height: 844, deviceScaleFactor: 1, mobile: true });
  check(await evaluate("document.documentElement.scrollWidth <= innerWidth"), "mobile layout overflows");
  await evaluate("document.querySelector('#files-toggle').click()");
  check(await evaluate("(()=>{const files=document.querySelector('.files-pane').getBoundingClientRect(), list=document.querySelector('.files-browser').getBoundingClientRect(), editor=document.querySelector('.files-preview').getBoundingClientRect(), chat=document.querySelector('#chat-scroll').getBoundingClientRect();return files.right<=chat.left+1 && list.bottom<=editor.top+1 && editor.height>50 && chat.width>100 && document.documentElement.scrollWidth<=innerWidth})()"), "mobile left dock did not keep files above preview beside chat");
  const mobileWidth = Number(await evaluate("document.querySelector('.files-pane').getBoundingClientRect().width"));
  const mobileGrip = await evaluate("(()=>{const r=document.querySelector('.files-outer-splitter').getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2}})()") as {x:number;y:number};
  await rpc('Input.dispatchTouchEvent',{type:'touchStart',touchPoints:[mobileGrip]});
  await rpc('Input.dispatchTouchEvent',{type:'touchMove',touchPoints:[{x:mobileGrip.x+35,y:mobileGrip.y}]});
  await rpc('Input.dispatchTouchEvent',{type:'touchEnd',touchPoints:[]});
  check(Number(await evaluate("document.querySelector('.files-pane').getBoundingClientRect().width"))>mobileWidth+20,"mobile left dock did not resize by touch");
  await until("!!document.querySelector('.files-entry[title=reports]')");
  await evaluate("document.querySelector('.files-entry[title=reports]').click()");
  await until("!!document.querySelector('.files-entry[title=\"reports/analysis.py\"]')");
  await evaluate("document.querySelector('.files-entry[title=\"reports/analysis.py\"]').click()");
  await until("document.querySelector('#files-monaco .view-lines')?.textContent.includes('private')");
  check(await evaluate("document.querySelector('#files-monaco').getBoundingClientRect().height>60"), "mobile left dock hid the Monaco preview under its controls");
  const mobileFiles = await rpc("Page.captureScreenshot", { format: "png" });
  await writeFile(config.screenshot.replace('.png','-mobile-left.png'), Buffer.from(String(mobileFiles['data']), 'base64'));
  for (const width of [320, 600, 844, 390]) {
    await rpc("Emulation.setDeviceMetricsOverride", { width, height: 844, deviceScaleFactor: 1, mobile: true });
    await evaluate("new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))");
    check(await evaluate("document.documentElement.scrollWidth<=innerWidth && document.querySelector('.files-pane').getBoundingClientRect().right<=document.querySelector('#chat-scroll').getBoundingClientRect().left+1 && document.querySelector('#files-monaco').getBoundingClientRect().height>60"), `left dock or Monaco overflowed at ${width}px`);
  }
  await evaluate("document.querySelector('[data-files-dock=top]').click()");
  check(await evaluate("document.querySelector('.files-pane').getBoundingClientRect().bottom<=document.querySelector('#chat-scroll').getBoundingClientRect().top+1"), "mobile top dock toggle did not restore Files above chat");
  await evaluate("document.querySelector('[data-files-dock=left]').click()");
  await evaluate("document.querySelector('#files-toggle').click()");
  check(await evaluate("(()=>{const bar=document.querySelector('.browser-usage-summary');return bar.clientHeight>40 && bar.clientHeight<=parseFloat(getComputedStyle(bar).lineHeight)*3+8})()"), "pinned stats did not wrap within three lines");
  await rpc("Emulation.setSafeAreaInsetsOverride", {insets:{top:24,bottom:34,left:0,right:0}});
  const safeControls = await evaluate(`(async () => {
    await new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)));
    const controls=[...document.querySelectorAll('.session-header button,.browser-status>summary')];
    const visible=controls.filter(button=>button.getBoundingClientRect().height>0);
    return visible.length>0 && visible.every(button=>{
      const r=button.getBoundingClientRect();
      const hit=document.elementFromPoint(r.x+r.width/2,r.y+r.height/2);
      return r.top>=24 && r.bottom<=innerHeight-34 && (hit===button || button.contains(hit));
    });
  })()`);
  check(safeControls, "mobile system bars obscure header/status controls or intercept their hit targets");
  await rpc("Emulation.setSafeAreaInsetsOverride", {insets:{top:0,bottom:0,left:0,right:0}});
  const viewportResult = await evaluate(`(async () => {
    const {fitViewport}=await import('./src/viewport.js');
    const root=document.getElementById('browser-root');
    document.querySelector('#composer-input').focus();
    await new Promise(resolve=>setTimeout(resolve,150));
    const viewport=Object.assign(new EventTarget(),{height:400,offsetTop:34,scale:1});
    const cleanup=fitViewport(root,viewport);
    viewport.dispatchEvent(new Event('resize'));
    await new Promise(resolve=>requestAnimationFrame(resolve));
    const prompt=document.querySelector('#composer-input').getBoundingClientRect();
    const send=document.querySelector('.composer button').getBoundingClientRect();
    const visible=prompt.top>=34 && prompt.bottom<=434 && send.bottom<=434 && send.top>=34;
    viewport.height=844;viewport.offsetTop=0;viewport.dispatchEvent(new Event('resize'));
    const restored=root.style.height==='844px' && root.style.top==='0px';
    cleanup();window.dispatchEvent(new Event('resize'));
    document.querySelector('#composer-input').blur();
    return {visible,restored,prompt:prompt.toJSON(),send:send.toJSON()};
  })()`);
  check((viewportResult as {visible:boolean;restored:boolean}).visible && (viewportResult as {restored:boolean}).restored,
    "keyboard viewport obscures prompt or Send, or fails to restore: " + JSON.stringify(viewportResult));
  const mobileImage = await rpc("Page.captureScreenshot", { format: "png" });
  await writeFile(config.screenshot.replace('.png','-mobile.png'), Buffer.from(String(mobileImage['data']), 'base64'));
  await rpc("Emulation.clearDeviceMetricsOverride");
  process.stdout.write("Shared UI, WASM chat, code capture, and mobile layout passed.\n");
  await until("!document.querySelector('.browser-persistence:not([hidden])')");
  await evaluate("globalThis.beforeSessionReload=true");
  await navigate("Page.reload");
  await until("!!document.querySelector('#startup-splash')");
  await waitForStartup();
  await until("!globalThis.beforeSessionReload && !!document.querySelector('[data-field=name]')");
  await until("!!document.querySelector('[data-field=name]')");
  check(await evaluate("document.querySelectorAll('[data-pinned-stat]').length===document.querySelectorAll('[data-stat-pin]').length && document.querySelectorAll('[data-pinned-stat]').length>=30"), "pins did not survive reload: " + JSON.stringify(await evaluate("({pins:localStorage.getItem('lethetic.pinned-stats.v2'),pinned:document.querySelectorAll('[data-pinned-stat]').length,available:document.querySelectorAll('[data-stat-pin]').length,status:document.querySelector('.browser-persistence')?.textContent})")));
  check(await evaluate("document.querySelector('.files-pane').dataset.dock==='left'"), "Files dock did not survive reload");
  await evaluate("document.querySelector('[data-browser-action=connections]').click()");
  check(await evaluate("document.querySelector('[data-field=name]').value==='Browser test' && document.querySelector('[data-field=key]').value==='secret-never-persisted'"), "reload did not preserve connection metadata and tab key");
  await evaluate("document.querySelector('[data-field=key]').value='updated-tab-key'; document.querySelector('[data-field=connection-form]').requestSubmit()");
  check(await evaluate("location.hash") === originalHash, "re-entering a key created a different session");
  await evaluate("document.querySelector('dialog[open]').close()");
  check(await evaluate("location.hash") === originalHash, "refresh switched sessions");
  await until("document.querySelector('.app-shell').textContent.includes('Hello from the browser test')");
  check(await evaluate("document.querySelector('.browser-usage-summary').textContent.includes('1,200 in') && document.querySelector('[data-pinned-stat=context]').textContent.includes('1,200 / 131,072')"), "usage/context did not survive reload");
  await evaluate("document.querySelector('#files-toggle').click()");
  await until("!!document.querySelector('.files-entry[title=reports]')");
  await evaluate("document.querySelector('.files-entry[title=reports]').click()");
  await until("!!document.querySelector('.files-entry[title=\"reports/analysis.py\"]')");
  await evaluate("document.querySelector('.files-entry[title=\"reports/analysis.py\"]').click()");
  await until("document.querySelector('[data-file-action=edit]')?.disabled===false");
  await evaluate("document.querySelector('[data-file-action=edit]').click()");
  check(await evaluate("document.querySelector('[data-field=file-content]').value") === "print('private snippet')", "saved file did not survive reload");
  await evaluate("document.querySelector('dialog[open]').close(); document.querySelector('#files-toggle').click(); document.querySelector('[data-browser-action=sessions]').click()");
  await until("!!document.querySelector('.sessions-panel')");
  check(await evaluate("document.querySelector('.sessions-panel').textContent.includes('hello')"), "chat missing in shared Sessions screen");
  await evaluate(`globalThis.exportObjectURL=URL.createObjectURL; globalThis.exportAnchorClick=HTMLAnchorElement.prototype.click;
    URL.createObjectURL=blob=>{globalThis.exportedBundle=blob;return globalThis.exportObjectURL(blob)};
    HTMLAnchorElement.prototype.click=function(){globalThis.exportedName=this.download};
    document.querySelector('[data-session-export]').click()`);
  await until("globalThis.exportedName?.endsWith('.tar.gz')");
  const debugArchive = await evaluate("(async()=>btoa(String.fromCharCode(...new Uint8Array(await globalThis.exportedBundle.arrayBuffer()))))()");
  await writeFile(config.screenshot.replace('.png','-session.tar.gz'),Buffer.from(String(debugArchive),'base64'));
  check(await evaluate("location.hash")===originalHash,"export changed the current session");
  await evaluate("document.querySelector('[data-session-database]').click()");
  await until("globalThis.exportedName?.endsWith('.db')");
  const databaseImage = await evaluate("(async()=>{const bytes=new Uint8Array(await globalThis.exportedBundle.arrayBuffer());let text='';for(let i=0;i<bytes.length;i+=8192)text+=String.fromCharCode(...bytes.subarray(i,i+8192));return btoa(text)})()");
  await writeFile(config.screenshot.replace('.png','-session.db'),Buffer.from(String(databaseImage),'base64'));

  await evaluate("URL.createObjectURL=globalThis.exportObjectURL; HTMLAnchorElement.prototype.click=globalThis.exportAnchorClick");
  await testNativeSessionDownloads(evaluate, until);
  await evaluate("document.querySelector('.sessions-panel .primary-button').click()");
  await until("!document.querySelector('.sessions-panel')");
  const newHash = await evaluate("location.hash");
  check(newHash !== originalHash, "new session reused old id");
  await evaluate(`location.hash=${JSON.stringify(originalHash)}`);
  await until("document.querySelector('.app-shell').textContent.includes('Hello from the browser test')");
  await evaluate(`const prompt=document.querySelector('#composer-input'); prompt.value='remember earlier';
    prompt.dispatchEvent(new Event('input',{bubbles:true})); document.querySelector('form.composer').requestSubmit()`);
  await until("document.querySelector('.app-shell').textContent.includes('Context restored')");
  await until("document.querySelector('.browser-status').textContent.includes('2,528')");
  await evaluate("document.querySelector('[data-browser-action=connections]').click(); document.querySelector('[data-field=mode]').value='python'; document.querySelector('[data-field=mode]').dispatchEvent(new Event('change'))");
  check(await evaluate("location.hash") === originalHash, "enabling Python created a new session");
  check(await evaluate("document.querySelector('.app-shell').textContent.includes('Context restored')"), "enabling Python cleared history");
  await evaluate("document.querySelector('dialog[open]').close()");
  await until("!document.querySelector('.browser-persistence:not([hidden])')");
  await evaluate("globalThis.beforeSessionReload=true");
  await navigate("Page.reload");
  await until("!!document.querySelector('#startup-splash')");
  await waitForStartup();
  await until("!globalThis.beforeSessionReload && !!document.querySelector('[data-field=name]')");
  await until("!!document.querySelector('[data-field=name]')");
  await evaluate("document.querySelector('[data-browser-action=connections]').click()");
  check(await evaluate("document.querySelector('[data-field=mode]').value==='python' && document.querySelector('[data-field=key]').value==='updated-tab-key'"), "Python or updated credentials did not survive reload");
  check(await evaluate("location.hash") === originalHash, "Python session changed on refresh");
  await evaluate("document.querySelector('dialog[open]').close(); document.querySelector('[aria-label=\"Open command palette\"]').click(); document.querySelector('#palette-command-system-prompt').click()");
  await until("!!document.querySelector('#prompt-injected')");
  check(await evaluate("document.querySelector('#prompt-injected').readOnly && document.querySelector('#prompt-injected').value.includes('Because you are in Python mode') && JSON.parse(document.querySelector('#prompt-tools').value)[0].name==='python'"), "Python prompt additions are missing");
  await evaluate("globalThis.testPromptSuffix=document.querySelector('#prompt-injected').value; const input=document.querySelector('#prompt-content'); input.value='Solve the task carefully.'; input.dispatchEvent(new Event('input',{bubbles:true})); document.querySelector('form.prompt-editor').requestSubmit()");
  await until("document.querySelector('#prompt-content')?.value==='Solve the task carefully.'");
  check(await evaluate("location.hash") === originalHash, "prompt edit switched sessions");
  check(await evaluate("document.querySelector('.app-shell').textContent.includes('Context restored')"), "prompt edit cleared history");
  const promptImage=await rpc("Page.captureScreenshot",{format:"png"});
  await writeFile(config.screenshot.replace('.png','-prompt.png'),Buffer.from(String(promptImage['data']),'base64'));
  await evaluate("document.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true})); document.querySelector('[data-browser-action=connections]').click()");
  await evaluate("document.querySelector('[data-field=mode]').value='general'; document.querySelector('[data-field=mode]').dispatchEvent(new Event('change')); document.querySelector('dialog[open]').close()");
  await evaluate("document.querySelector('[aria-label=\"Open command palette\"]').click(); document.querySelector('#palette-command-system-prompt').click()");
  await until("!!document.querySelector('#prompt-injected')");
  check(await evaluate("!document.querySelector('#prompt-injected').value.includes('Because you are in Python mode') && document.querySelector('#prompt-content').value==='Solve the task carefully.' && document.querySelector('#prompt-tools').value==='[]'"), "chat prompt retained Python instructions or lost edits");
  await evaluate("document.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true}))");
  await evaluate("location.hash='#/latest'");
  await until(`location.hash===${JSON.stringify(originalHash)}`);
  await evaluate("location.hash='#/new'");
  await until(`location.hash.startsWith('#/sessions/') && location.hash!==${JSON.stringify(originalHash)}`);
  check(!await evaluate("document.querySelector('.app-shell').textContent.includes('Context restored')"), "new route retained old transcript");
  const freshHash = await evaluate("location.hash");
  await until("!document.querySelector('.browser-persistence:not([hidden])')");
  await evaluate("globalThis.beforeSessionReload=true");
  await navigate("Page.reload");
  await until("!!document.querySelector('#startup-splash')");
  await waitForStartup();
  await until("!globalThis.beforeSessionReload && !!document.querySelector('[data-field=name]')");
  await until("!!document.querySelector('[data-browser-action=sessions]')");
  check(await evaluate("location.hash") === freshHash, "empty new session did not survive refresh");
  process.stdout.write("Files, sandbox previews, copy/download, session reload/context, and routes passed.\n");
  // Exercise the actual Worker bridge with a local dependency fixture, then with real Pyodide if requested.
  const legacyPython = await evaluate(`(async () => {
    const worker=new Worker(new URL('./browser/src/python-worker.js',location.href),{type:'module'});
    let id=0;
    const run=code=>new Promise((resolve,reject)=>{
      const requestId=++id;
      const timeout=setTimeout(()=>reject(Error('Legacy worker timed out')),10000);
      worker.onmessage=event=>{if(event.data.id===requestId){clearTimeout(timeout);resolve(event.data)}};
      worker.onerror=()=>{clearTimeout(timeout);reject(Error('Legacy worker failed'))};
      // v0.3 clients omit workspace. An open tab can load a newer worker after deploy.
      worker.postMessage({id:requestId,code,indexURL:location.origin+'/pyodide/'});
    });
    try {
      const created=await run('# create-session-files');
      const read=await run('# read-session-file');
      return {created:created.output,read:read.output,files:read.workspace?.files.length};
    } finally {worker.terminate();}
  })()`);
  check(JSON.stringify(legacyPython)===JSON.stringify({created:'files created\n',read:'read: value\n42\n\n',files:2}),
    'Older open tab cannot use the updated worker: '+JSON.stringify(legacyPython));
  process.stdout.write('Older client / updated worker compatibility passed.\n');
  const pythonResult = await evaluate(`(async () => {
    const {createPythonTool}=await import('./src/sdk.js');
    const python=createPythonTool({indexURL:location.origin+'/pyodide/',timeoutMs:10000});
    try {
      const output=await python.execute({code:'print(42)'},new AbortController().signal);
      const controller=new AbortController();
      const hanging=python.execute({code:'while True: pass'},controller.signal);
      setTimeout(()=>controller.abort(),100);
      let stopped=false; try{await hanging}catch{stopped=true}
      const restarted=await python.execute({code:'print(42)'},new AbortController().signal);
      return {output,stopped,restarted};
    } finally {python.reset()}
  })()`);
  check(JSON.stringify(pythonResult) === JSON.stringify({ output: "42\n", stopped: true, restarted: "42\n" }), "Python worker lifecycle failed");
  process.stdout.write("Python worker lifecycle passed.\n");
  await navigate("Page.navigate", { url: `${config.origin}/lethetic/integration.html` });
  await until("document.title==='Lethetic integration guide'");
  check(await evaluate(`(async () => {
    const {mountLethetic}=await import('./src/embed.js');
    const container=document.createElement('div');document.body.replaceChildren(container);
    const view=mountLethetic(container,{compact:true,python:true,systemPrompt:'Test editable prompt',pyodideIndexURL:location.origin+'/pyodide/',
      connections:[{name:'Embed',provider:'openai',endpoint:location.origin+'/v1/chat/completions',model:'test'}]});
    const {injectedInstructions}=await import('./src/prompts.js');
    const originalFetch=globalThis.fetch; let sentPrompt=false;
    globalThis.fetch=(url,options)=>{
      if(String(url).endsWith('/v1/chat/completions')) {
        const request=JSON.parse(options.body);
        if(request.messages[0].content!=='Test editable prompt'+String.fromCharCode(10,10)+injectedInstructions(true))throw Error('Displayed Python instructions differ from sent system prompt');
        if(request.tools[0].function.name!=='python')throw Error('Python definition not sent');
        sentPrompt=true;
      }
      return originalFetch(url,options);
    };
    const prompt=container.querySelector('#composer-input'); prompt.value='calculate';
    prompt.dispatchEvent(new Event('input',{bubbles:true})); container.querySelector('form.composer').requestSubmit();
    for(let i=0;i<200;i++) {
      if(container.textContent.includes('Python says: 42')) {
        const captured=!!container.querySelector('[data-browser-action=save-code]');
        const counted=container.querySelector('.browser-status').textContent.includes('2,528');
        view.destroy(); const cleaned=!document.querySelector('dialog') && !container.childNodes.length;
        globalThis.fetch=originalFetch; container.remove();return captured && cleaned && counted && sentPrompt;
      }
      await new Promise(resolve=>setTimeout(resolve,25));
    }
    globalThis.fetch=originalFetch; view.destroy();container.remove();return false;
  })()`), "embedded UI Python/model tool loop failed");
  process.stdout.write("Embedded shared UI and Python tool loop passed.\n");
  await testSessionStorage(evaluate);
  process.stdout.write("IndexedDB migration, 16 MiB binary files, quota failure/retry and cross-tab conflict passed.\n");
  check(await evaluate(`(async () => {
    const {mountLethetic}=await import('./src/embed.js');
    const root=document.createElement('div'); document.body.replaceChildren(root);
    for(const mode of ['general','python','dotnet-wasm']) {
      const view=mountLethetic(root,{mode,connections:[{name:'Palette',provider:'openai',endpoint:location.origin+'/v1/chat/completions',model:'test'}]});
      root.querySelector('[aria-label="Open command palette"]').click();
      const reset=root.querySelector('#palette-command-delete-python-runtime');
      if(Boolean(reset)!==(mode==='python') || root.querySelector('#palette-command-python-strict') || !root.querySelector('#palette-command-agent-mode') || !document.getElementById('palette-command-local:compact-session'))throw Error('Wrong command visibility in '+mode);
      view.destroy();
    }
    return true;
  })()`), "mode-specific command palette filtering failed");
  const workspaceResult = await evaluate(`(async () => {
    const {mountLethetic}=await import('./src/embed.js');
    localStorage.removeItem('lethetic.sessions.v1'); localStorage.removeItem('lethetic.files.v1');
    const root=document.createElement('div'); root.style.height='900px'; document.body.replaceChildren(root);
    const options={storage:localStorage,python:true,
      pyodideIndexURL:${config.pyodide ? 'undefined' : "location.origin+'/pyodide/'"},
      connections:[{name:'Workspace test',provider:'openai',endpoint:location.origin+'/v1/chat/completions',model:'test'}]};
    let view=mountLethetic(root,options);
    const wait=async predicate=>{for(let i=0;i<${config.pyodide ? 2400 : 200};i++){if(predicate())return;await new Promise(r=>setTimeout(r,50));}throw Error('Workspace UI condition timed out: '+predicate.toString()+'; '+root.textContent.slice(-2500))};
    const send=async (prompt,answer)=>{
      const input=root.querySelector('#composer-input');input.value=prompt;input.dispatchEvent(new Event('input',{bubbles:true}));
      root.querySelector('form.composer').requestSubmit();
      await wait(()=>root.textContent.includes('Python says: '+answer));
      await wait(()=>!!root.querySelector('.activity-idle'));
    };
    const records=()=>JSON.parse(localStorage.getItem('lethetic.sessions.v1')).sessions;
    try {
      root.querySelector('#files-toggle').click();
      await send('create files','files created');
      const pythonBlock=[...root.querySelectorAll('.block-tool_call')].at(-1);
      const parsedCode=pythonBlock.querySelector('.python-tool-code code').textContent;
      if(!parsedCode.includes('from pathlib import Path') || !parsedCode.includes('\\n') || !pythonBlock.querySelector('.code-keyword'))throw Error('Python tool code was not parsed/highlighted');
      Object.defineProperty(navigator,'clipboard',{configurable:true,value:{writeText:async text=>{globalThis.pythonCopied=text}}});
      pythonBlock.querySelector('[data-copy-turn]').click();await wait(()=>globalThis.pythonCopied===parsedCode);
      pythonBlock.querySelector('button[data-python-view=original]').click();
      if(pythonBlock.querySelector('.python-tool-code'))throw Error('Original did not restore JSON');
      pythonBlock.querySelector('[data-copy-turn]').click();await wait(()=>globalThis.pythonCopied!==parsedCode);
      if(JSON.parse(globalThis.pythonCopied).code!==parsedCode)throw Error('Original copy was not exact tool payload');
      pythonBlock.querySelector('button[data-python-view=parsed]').click();
      if(pythonBlock.querySelector('.python-tool-code code').textContent!==parsedCode)throw Error('Parsed toggle changed code');

      const first=records()[0].id;
      if(!records()[0].workspace.files.some(file=>file.path==='outputs/chart.png'))throw Error('Python files were not persisted');
      await wait(()=>!!root.querySelector('.files-entry[title=outputs]'));
      root.querySelector('.files-entry[title=outputs]').click();
      await wait(()=>!!root.querySelector('.files-entry[title="outputs/chart.png"]'));
      if(!root.querySelector('.files-entry[title="outputs/empty"]'))throw Error('Empty directory not visible');
      const uploadInput=root.querySelector('[data-file-upload]');
      uploadInput.click=()=>{}; root.querySelector('[data-file-action=upload]').click();
      const selection=new DataTransfer();
      selection.items.add(new File(['a,b\\n1,2\\n'],'upload.csv',{type:'text/csv'}));
      selection.items.add(new File([new Uint8Array([0,255,1])],'raw.bin'));
      uploadInput.files=selection.files; uploadInput.dispatchEvent(new Event('change',{bubbles:true}));
      await wait(()=>!!root.querySelector('.files-entry[title="outputs/upload.csv"]') && !!root.querySelector('.files-entry[title="outputs/raw.bin"]'));
      if(records()[0].workspace.files.find(file=>file.path==='outputs/raw.bin')?.data!=='AP8B')throw Error('Uploaded binary bytes changed');
      await send('read uploads','uploaded: a,b\\n1,2 / 00ff01');
      if(!root.querySelector('.files-breadcrumbs').textContent.includes('Session files ('+records()[0].workspace.files.length+')'))throw Error('Missing session file count');
      const sort=root.querySelector('[aria-label="Sort session files"]'); sort.value='size'; sort.dispatchEvent(new Event('change',{bubbles:true}));
      const originalURL=URL.createObjectURL, originalClick=HTMLAnchorElement.prototype.click;
      try {
        URL.createObjectURL=blob=>{globalThis.__zipBlob=blob;return originalURL(blob)};
        HTMLAnchorElement.prototype.click=function(){};
        root.querySelector('[data-file-action=download-all]').click();
      } finally { URL.createObjectURL=originalURL; HTMLAnchorElement.prototype.click=originalClick; }
      globalThis.__sessionZip=Array.from(new Uint8Array(await globalThis.__zipBlob.arrayBuffer()));


      root.querySelector('.files-entry[title="outputs/chart.png"]').click();
      await wait(()=>!root.querySelector('[data-file-action=preview]').disabled);
      if(!root.querySelector('.files-inline-preview')?.srcdoc.includes('data:image/png;base64,'))throw Error('PNG did not preview automatically');
      const stableFrame=root.querySelector('.files-inline-preview'), stableRow=root.querySelector('.files-entry[title="outputs/chart.png"]');
      let removedPreview=false;
      const observeRemovals=records=>{for(const record of records)for(const node of record.removedNodes)if(node===stableFrame || node.contains?.(stableFrame))removedPreview=true;};
      const observer=new MutationObserver(observeRemovals);observer.observe(root.querySelector('#files-body'),{childList:true,subtree:true});
      root.querySelector('[data-file-action=upload]').click();
      const unrelated=new DataTransfer();unrelated.items.add(new File(['unrelated update'],'incremental.txt'));
      const incrementalInput=root.querySelector('[data-file-upload]');incrementalInput.files=unrelated.files;incrementalInput.dispatchEvent(new Event('change',{bubbles:true}));
      await wait(()=>!!root.querySelector('.files-entry[title="outputs/incremental.txt"]'));
      observeRemovals(observer.takeRecords());observer.disconnect();
      if(removedPreview || stableFrame!==root.querySelector('.files-inline-preview') || stableRow!==root.querySelector('.files-entry[title="outputs/chart.png"]'))throw Error('Unrelated file update replaced the preview or existing row');
      root.querySelector('[data-preview-expand]').click();
      if(!document.querySelector('.browser-preview.is-full-preview[open]'))throw Error('Image Expand did not open full preview');
      if(!document.querySelector('[data-field=preview-frame]').srcdoc.includes('data:image/png;base64,'))throw Error('PNG preview unavailable');
      document.querySelector('dialog[open]').close();
      root.querySelector('.files-entry[title="outputs/data.csv"]').click();
      await wait(()=>!root.querySelector('[data-file-action=edit]').disabled);
      root.querySelector('[data-file-action=edit]').click();
      document.querySelector('[data-field=file-content]').value='edited in Files';
      document.querySelector('[data-field=file-form]').requestSubmit();
      if(document.querySelector('dialog[open]'))throw Error('File editor stayed open: '+document.querySelector('dialog[open]').textContent);
      await send('read file','read: edited in Files');
      const beforeCompact=records()[0];
      root.querySelector('[aria-label="Open command palette"]').click();
      const search=root.querySelector('#palette-search');search.value='compact';search.dispatchEvent(new Event('input',{bubbles:true}));
      if(!root.querySelector('[id="palette-command-local:compact-session"]'))throw Error('Compaction missing from palette search');
      root.querySelector('#palette-list').dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true}));
      await wait(()=>records()[0].blocks.some(block=>block.title==='Context compacted'));
      if(records()[0].id!==first || records()[0].messages.length!==2 || records()[0].blocks.length<=beforeCompact.blocks.length)throw Error('Compaction did not preserve session/transcript or replace context');
      if(JSON.stringify(records()[0].workspace)!==JSON.stringify(beforeCompact.workspace))throw Error('Compaction changed files');
      await send('restore files','restored: edited in Files');
      view.destroy(); view=mountLethetic(root,options);
      if(!records()[0].workspace.files.some(file=>file.path==='outputs/upload.csv') || records()[0].workspace.files.find(file=>file.path==='outputs/raw.bin')?.data!=='AP8B')throw Error('Uploads lost on reopen');
      await send('restore files','restored: edited in Files');
      root.querySelector('[data-browser-action=sessions]').click();
      await wait(()=>!!root.querySelector('.sessions-panel .primary-button'));
      root.querySelector('.sessions-panel .primary-button').click();
      await wait(()=>!root.querySelector('.sessions-panel'));
      if(records()[0].workspace.files.length)throw Error('New session inherited old files');
      await send('list files','workspace empty: true');
      root.querySelector('[data-browser-action=sessions]').click();
      await wait(()=>!!root.querySelector('.sessions-panel'));
      [...root.querySelectorAll('.choice-row')].find(button=>button.textContent.includes('create files')).click();
      await wait(()=>!root.querySelector('.sessions-panel'));
      if(records()[0].id!==first)throw Error('Wrong session resumed');
      await send('delete file','file deleted');
      if(records()[0].workspace.files.some(file=>file.path==='outputs/data.csv'))throw Error('Python deletion did not sync');
      root.querySelector('#files-toggle').click();
      await wait(()=>!!root.querySelector('.files-entry[title=outputs]'));
      root.querySelector('.files-entry[title=outputs]').click();
      await wait(()=>!!root.querySelector('.files-entry[title="outputs/chart.png"]'));
      if(root.querySelector('.files-entry[title="outputs/data.csv"]'))throw Error('Deleted file remains visible');
      return true;
    } finally {view.destroy();root.remove();}
  })()`);
  check(workspaceResult, 'session workspace UI failed');
  await writeFile(config.screenshot.replace('.png','-files.zip'), Uint8Array.from(await evaluate('globalThis.__sessionZip') as number[]));
  process.stdout.write('Python workspace creation, Files editing, image preview, reopen, isolation and deletion passed.\n');
  await evaluate(`(async()=>{
    const {mountLethetic}=await import('./src/embed.js');
    const {SessionStore}=await import('./src/sessions.js');
    const {encodeBytes}=await import('./src/workspace.js');
    const bytes=new Uint8Array(1644), data=new DataView(bytes.buffer);
    const word=(offset,text)=>bytes.set(new TextEncoder().encode(text),offset);
    word(0,'RIFF');data.setUint32(4,1636,true);word(8,'WAVE');word(12,'fmt ');data.setUint32(16,16,true);
    data.setUint16(20,1,true);data.setUint16(22,1,true);data.setUint32(24,8000,true);data.setUint32(28,16000,true);
    data.setUint16(32,2,true);data.setUint16(34,16,true);word(36,'data');data.setUint32(40,1600,true);
    const store=new SessionStore(localStorage),saved=store.list()[0];
    saved.workspace={version:1,directories:[],files:[{path:'clip.wav',data:encodeBytes(bytes)},{path:'clip.webm',data:${JSON.stringify(PREVIEW_WEBM)}}]};store.save(saved);
    const root=document.createElement('div');root.style.height='900px';document.body.replaceChildren(root);
    globalThis.mediaView=mountLethetic(root,{storage:localStorage,connections:[{name:'Media',provider:'openai',endpoint:location.origin+'/v1/chat/completions',model:'test'}]});
    root.querySelector('#files-toggle').click();
  })()`);
  for(const [name,tag] of [['clip.wav','audio'],['clip.webm','video']]) {
    await until(`!!document.querySelector('.files-entry[title="${name}"]')`);
    await evaluate(`document.querySelector('.files-entry[title="${name}"]').click()`);
    await until(`!!document.querySelector('.files-inline-preview')?.srcdoc.includes('<${tag}')`);
    check(await evaluate("!document.querySelector('.files-inline-preview').srcdoc.includes('autoplay')"),'media unexpectedly autoplays');
    await evaluate("document.querySelector('[data-preview-expand]').click()");
    check(await previewInput(`(async()=>{const media=document.querySelector('${tag}'); if(media.readyState<1)await new Promise((resolve,reject)=>{media.addEventListener('loadedmetadata',resolve,{once:true});media.addEventListener('error',()=>reject(Error('${name} failed to load: '+media.error?.message)),{once:true});setTimeout(()=>reject(Error('${name} metadata timed out: ready='+media.readyState+', network='+media.networkState+', error='+media.error?.message)),15000)});return media.controls && media.readyState>=1 && !media.autoplay})()`),'media did not decode');
    await evaluate("document.querySelector('.browser-preview[open]').close()");
  }
  await evaluate('globalThis.mediaView.destroy()');
  process.stdout.write('Automatic audio/video previews decoded in the browser with native controls.\n');
  await testDotnetBrowser(evaluate, previewInput, async () => {
    const image = await rpc("Page.captureScreenshot", {format:"png"});
    await writeFile(config.screenshot.replace('.png','-sites.png'),Buffer.from(String(image['data']),'base64'));
    await rpc("Emulation.setDeviceMetricsOverride", {width:390,height:844,deviceScaleFactor:1,mobile:true});
    check(await evaluate("document.querySelector('.browser-sites .overlay-body').scrollWidth<=document.querySelector('.browser-sites .overlay-body').clientWidth+1"), "Interactive sites overflow on mobile");
    const mobile = await rpc("Page.captureScreenshot", {format:"png"});
    await writeFile(config.screenshot.replace('.png','-sites-mobile.png'),Buffer.from(String(mobile['data']),'base64'));
    await rpc("Emulation.clearDeviceMetricsOverride");
  });
  process.stdout.write("Released .NET runtime: files, state, cancellation, permissions, C# file/project Run, interactive sites, request checkpoints and UI reload passed.\n");
  const dotnetScreenshot = await rpc("Page.captureScreenshot", { format: "png" });
  await writeFile(config.screenshot.replace(".png", "-dotnet.png"), Buffer.from(String(dotnetScreenshot["data"]), "base64"));
  await evaluate("document.querySelector('[data-browser-action=connections]').click()");
  await evaluate("document.querySelector('.browser-runtime').scrollIntoView({block:'start'})");
  const securityScreenshot = await rpc("Page.captureScreenshot", { format: "png" });
  await writeFile(config.screenshot.replace(".png", "-dotnet-security.png"), Buffer.from(String(securityScreenshot["data"]), "base64"));
  await rpc("Emulation.setDeviceMetricsOverride", { width: 390, height: 844, deviceScaleFactor: 1, mobile: true });
  check(await evaluate("document.documentElement.scrollWidth <= innerWidth"), ".NET settings overflow on mobile");
  await evaluate("document.querySelector('.browser-runtime').scrollIntoView({block:'start'})");
  const mobileDotnet = await rpc("Page.captureScreenshot", { format: "png" });
  await writeFile(config.screenshot.replace(".png", "-dotnet-mobile.png"), Buffer.from(String(mobileDotnet["data"]), "base64"));
  await evaluate("globalThis.dotnetTestView.destroy()");
  await testConnectionThinking(evaluate);
  await testStreamingControls(evaluate);
  await testConnectionLimits(evaluate);
  await testMissingConnectionKey(evaluate);
  process.stdout.write("Thinking preferences, transcript, tool continuation and reload passed.\n");
  if (config.pyodide) {
    await evaluate(`(async () => {
      const {createPythonTool}=await import('./src/sdk.js');
      globalThis.testPython=createPythonTool();
      return await globalThis.testPython.execute({code:'x = 40'},new AbortController().signal);
    })()`);
    process.stdout.write("Real Pyodide initialized.\n");
    const actual = await evaluate(`(async () => {
      const python=globalThis.testPython;
      const run=code=>python.execute({code},new AbortController().signal);
      try {
        const {PYODIDE_CHECK,PYTHON_PROGRAM_EXAMPLE}=await import('./src/prompts.js');
        const probe=await run(PYODIDE_CHECK);
        if(!probe.includes("'pyodide_available': True") || !probe.includes("'running_in_pyodide': True"))throw Error('Prompt Pyodide detection failed: '+probe);
        const program=await run(PYTHON_PROGRAM_EXAMPLE);
        if(program.trim()!=='42' || !python.snapshotWorkspace().files.some(file=>file.path==='solution.py'))throw Error('Prompt example did not create a downloadable program: '+program);
        const value=await run('print(x + 2)');
        const large=await run('from pathlib import Path\\nPath("large.bin").write_bytes(bytes(3 * 1024 * 1024))\\nprint(Path("large.bin").stat().st_size)');
        if(large.trim()!=='3145728' || !python.snapshotWorkspace().files.some(f=>f.path==='large.bin' && f.data.length===4194304))throw Error('Large Python file did not checkpoint: '+large);
        const error=await run('from pathlib import Path\\nPath("saved.bin").write_bytes(bytes([0,255]))\\nraise ValueError("expected-test-error")');
        const controller=new AbortController();
        const loop=python.execute({code:'from pathlib import Path\\nPath("saved.bin").write_bytes(b"unfinished")\\nwhile True: pass'},controller.signal);
        setTimeout(()=>controller.abort(),100);
        let stopped=false;try{await loop}catch{stopped=true}
        const restarted=await run('print("x" in globals())');
        if((await run('from pathlib import Path\\nprint(Path("large.bin").stat().st_size)')).trim()!=='3145728')throw Error('Large Python file did not restore');
        const restoredFile=await run('from pathlib import Path\\nprint(Path("saved.bin").read_bytes().hex())');
        return {value,error:error.includes('expected-test-error'),stopped,restarted,restoredFile};
      } finally {python.reset()}
    })()`);
    check(JSON.stringify(actual) === JSON.stringify({ value: "42\n", error: true, stopped: true, restarted: "False\n", restoredFile: "00ff\n" }), `real Pyodide failed: ${JSON.stringify(actual)}`);
    await testPythonNetwork(evaluate);
    process.stdout.write("Real Pyodide network denial, packages, relay methods, site routes and no-fallback passed.\n");
    await testPythonSites(evaluate, previewInput, async () => {
      const mobile=await rpc("Page.captureScreenshot",{format:"png",captureBeyondViewport:false});
      await writeFile(config.screenshot.replace(".png","-python-sites-mobile.png"),Buffer.from(String(mobile["data"]),"base64"));
      await rpc("Emulation.setDeviceMetricsOverride",{width:1280,height:1000,deviceScaleFactor:1,mobile:false});
      await evaluate("new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))");
      const desktop=await rpc("Page.captureScreenshot",{format:"png",captureBeyondViewport:false});
      await writeFile(config.screenshot.replace(".png","-python-sites.png"),Buffer.from(String(desktop["data"]),"base64"));
    });
    process.stdout.write("Real Pyodide sites: async HTTP, Flask WSGI, .py Run, iframe forms, checkpoints and revocation passed.\n");
  }
  check(await evaluate(`(async()=>{
    const {createConnectionsDialog}=await import('./src/connections-dialog.js');
    const {LocalTransport}=await import('./src/local-transport.js');
    Object.defineProperty(window,'__LETHETIC_HOST_TOKEN__',{configurable:true,value:'native-ui-test'});
    const transport=new LocalTransport({socketOpened(){},authenticationEpochChanged(){},transportStatus(){},protocolFault(){},serverMessage(){},commandResponse(){}},{connections:[]});
    const view=createConnectionsDialog(transport);
    try {
      view.open();
      const route=view.dialog.querySelector('[data-field=browser-network-route]');
      const hidden=route.hidden && route.getBoundingClientRect().height===0;
      const message=view.dialog.innerText.includes('No proxy setup or destination CORS required');
      return hidden && message;
    } finally {view.destroy();transport.dispose();delete window.__LETHETIC_HOST_TOKEN__;}
  })()`), 'native Connections should hide proxy settings and explain automatic HTTP');
  process.stdout.write(`Browser acceptance passed${config.pyodide ? " (including real Pyodide)" : ""}.\n`);
} finally {
  for (const request of pending.values()) clearTimeout(request.timer);
  pending.clear();
  browser.kill("SIGKILL");
}
