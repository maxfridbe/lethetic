import { spawn } from "node:child_process";
import { writeFile } from "node:fs/promises";

interface Config { browser: string; profile: string; origin: string; pyodide: boolean; screenshot: string }
process.stdin.setEncoding("utf8");
let input = "";
for await (const chunk of process.stdin) input += chunk;
const config = JSON.parse(input) as Config;
const browser = spawn(config.browser, ["--headless=new", "--no-sandbox", "--remote-debugging-pipe",
  `--user-data-dir=${config.profile}`, "--no-first-run", "--disable-background-networking",
  "--disable-component-update", "--disable-sync", "--window-size=1440,1100", "about:blank"],
{ stdio: ["ignore", "ignore", "pipe", "pipe", "pipe"] });
const pending = new Map<number, { resolve: (value: Record<string, unknown>) => void;
  reject: (error: Error) => void; timer: ReturnType<typeof setTimeout> }>();
let counter = 0;
let session: string | undefined;
let buffered = "";
let browserErrors = "";
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
    const request = pending.get(Number(message["id"]));
    if (!request) continue;
    pending.delete(Number(message["id"])); clearTimeout(request.timer);
    if (message["error"]) request.reject(new Error(JSON.stringify(message["error"])));
    else request.resolve(message["result"] as Record<string, unknown>);
  }
});
function rpc(method: string, params: Record<string, unknown> = {}): Promise<Record<string, unknown>> {
  const id = ++counter;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => { pending.delete(id); reject(new Error(`CDP timed out: ${method}`)); }, 180_000);
    pending.set(id, { resolve, reject, timer });
    browser.stdio[3]?.write(JSON.stringify({ id, method, params, sessionId: session }) + "\0");
  });
}
async function evaluate(expression: string): Promise<unknown> {
  const response = await rpc("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (response["exceptionDetails"]) throw new Error(JSON.stringify(response["exceptionDetails"]));
  return (response["result"] as Record<string, unknown>)["value"];
}
function check(value: unknown, message: string): asserts value { if (!value) throw new Error(message); }
async function until(expression: string): Promise<void> {
  for (let attempt = 0; attempt < 200; attempt++) {
    if (await evaluate(expression)) return;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(`Condition failed: ${expression}`);
}

try {
  const target = await rpc("Target.createTarget", { url: "about:blank" });
  const attached = await rpc("Target.attachToTarget", { targetId: target["targetId"], flatten: true });
  session = String(attached["sessionId"]);
  await rpc("Page.enable"); await rpc("Runtime.enable");
  await rpc("Page.navigate", { url: `${config.origin}/lethetic/` });
  await until("!!document.querySelector('[data-field=name]')");
  check(await evaluate(`(() => {
    const set = (name, value) => document.querySelector('[data-field='+name+']').value=value;
    set('name','Browser test'); set('endpoint',location.origin+'/v1/chat/completions');
    set('model','test'); set('key','secret-never-persisted');
    document.querySelector('[data-field=connection-form]').requestSubmit();
    return document.querySelector('dialog[open] [data-field=status]').textContent.includes('Connection saved');
  })()`), "connection could not be saved");
  check(await evaluate("!JSON.stringify(localStorage).includes('secret-never-persisted')"), "key persisted");
  await evaluate("document.querySelector('[data-field=test]').click()");
  await until("document.querySelector('dialog[open] [role=status]').textContent.includes('valid response')");
  await evaluate(`document.querySelector('dialog[open]').close();
    const prompt=document.getElementById('composer-input'); prompt.value='hello';
    prompt.dispatchEvent(new Event('input',{bubbles:true})); document.querySelector('form.composer').requestSubmit()`);
  await until("document.querySelector('.app-shell').textContent.includes('Hello from the browser test')");
  await until("document.querySelector('.browser-usage-summary').textContent.includes('1,200 in')");
  check(await evaluate("document.querySelector('.browser-status').textContent.includes('1,024') && !document.querySelector('.transport-level')"), "provider cache usage or browser status failed");
  check(await evaluate("performance.getEntriesByType('resource').some(r=>r.name.includes('/lethetic/browser/wasm/lethetic_wasm_bg.wasm'))"), "WASM was not loaded from the project subpath");
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
  await evaluate("document.querySelector('[data-browser-action=save-code]').click()");
  await until("!!document.querySelector('dialog[open] [data-field=file-content]')");
  check(await evaluate("document.querySelector('[data-field=file-content]').value===\"print('private snippet')\""), "code capture failed");
  await evaluate(`document.querySelector('[data-field=file-path]').value='reports/analysis.py';
    document.querySelector('[data-field=file-form]').requestSubmit()`);
  await until("document.querySelector('.files-path')?.textContent.includes('reports/analysis.py') && !document.querySelector('[data-file-action=preview]').disabled");
  check(await evaluate("!document.querySelector('[data-field=passphrase]') && !document.querySelector('[data-browser-action=code]')"), "encrypted library remains visible");
  await evaluate(`Object.defineProperty(navigator,'clipboard',{configurable:true,value:{writeText:async text=>{globalThis.copiedText=text}}});
    document.querySelector('[data-file-action=copy]').click()`);
  await until("globalThis.copiedText===\"print('private snippet')\"");
  await evaluate(`globalThis.originalAnchorClick=HTMLAnchorElement.prototype.click;
    globalThis.originalObjectURL=URL.createObjectURL;
    URL.createObjectURL=function(blob){globalThis.downloadTextPromise=blob.text();return globalThis.originalObjectURL(blob)};
    HTMLAnchorElement.prototype.click=function(){globalThis.downloadName=this.download};
    [...document.querySelectorAll('.files-actions button')].find(b=>b.textContent==='Download File').click()`);
  await until("!!globalThis.downloadTextPromise");
  check(await evaluate("globalThis.downloadTextPromise") === "print('private snippet')", "download changed file contents");
  check(await evaluate("globalThis.downloadName") === "analysis.py", "download filename changed");
  await evaluate("HTMLAnchorElement.prototype.click=globalThis.originalAnchorClick; URL.createObjectURL=globalThis.originalObjectURL; document.querySelector('[data-file-action=preview]').click()");
  await until("!!document.querySelector('.browser-preview[open]')");
  check(await evaluate(`(() => { const frame=document.querySelector('[data-field=preview-frame]');
    return frame.getAttribute('sandbox')==='' && frame.srcdoc.includes("print('private snippet')") && frame.getBoundingClientRect().height>100; })()`), "file preview is missing or not sandboxed");
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
  await evaluate("document.querySelector('[data-file-action=preview]').click()");
  await until("!!document.querySelector('.browser-preview[open]')");
  // The opaque sandbox may run in a separate renderer; attach to its CDP target.
  let previewText: unknown;
  for (let attempt = 0; attempt < 40; attempt++) {
    const targets = await rpc('Target.getTargets');
    const frameTarget = (targets['targetInfos'] as { type: string; targetId: string; url: string }[])
      .find(target => target.type === 'iframe' && target.url === 'about:srcdoc');
    if (frameTarget) {
      const attachedFrame = await rpc('Target.attachToTarget', { targetId: frameTarget.targetId, flatten: true });
      const parentSession = session; session = String(attachedFrame['sessionId']);
      try { previewText = await evaluate("document.querySelector('h1')?.textContent"); }
      finally { session = parentSession; }
      break;
    }
    const tree = await rpc('Page.getFrameTree');
    const frame = (tree['frameTree'] as { childFrames?: { frame: { id: string; url: string } }[] }).childFrames
      ?.find(child => child.frame.url === 'about:srcdoc');
    if (frame) {
      const world = await rpc('Page.createIsolatedWorld', { frameId: frame.frame.id, worldName: 'preview-test' });
      const rendered = await rpc('Runtime.evaluate', { contextId: world['executionContextId'],
        expression: "document.querySelector('h1')?.textContent", returnByValue: true });
      previewText = (rendered['result'] as { value: unknown }).value;
      if (previewText === 'Rendered local HTML') break;
    }
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  check(previewText === 'Rendered local HTML', 'HTML preview did not render: ' + JSON.stringify(previewText));
  check(!await evaluate('globalThis.hacked'), 'preview executed a script');
  await evaluate("document.querySelector('dialog[open]').close(); globalThis.confirm=()=>true; document.querySelector('[data-file-action=delete]').click()");
  await until("!document.querySelector('.files-entry[title=\"reports/preview.html\"]')");
  await evaluate("document.querySelector('#files-toggle').click()");
  const originalHash = await evaluate("location.hash");
  const image = await rpc("Page.captureScreenshot", { format: "png" });
  await writeFile(config.screenshot, Buffer.from(String(image["data"]), "base64"));
  await rpc("Emulation.setDeviceMetricsOverride", { width: 390, height: 844, deviceScaleFactor: 1, mobile: true });
  check(await evaluate("document.documentElement.scrollWidth <= innerWidth"), "mobile layout overflows");
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
  await rpc("Page.reload");
  await until("!!document.querySelector('[data-field=name]')");
  await evaluate("document.querySelector('[data-browser-action=connections]').click()");
  check(await evaluate("document.querySelector('[data-field=name]').value==='Browser test' && document.querySelector('[data-field=key]').value===''"), "reload did not preserve metadata / forget key");
  await evaluate("document.querySelector('dialog[open]').close()");
  check(await evaluate("location.hash") === originalHash, "refresh switched sessions");
  await until("document.querySelector('.app-shell').textContent.includes('Hello from the browser test')");
  check(await evaluate("document.querySelector('.browser-usage-summary').textContent.includes('1,200 in')"), "usage did not survive reload");
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
  await evaluate("location.hash='#/latest'");
  await until(`location.hash===${JSON.stringify(originalHash)}`);
  await evaluate("location.hash='#/new'");
  await until(`location.hash.startsWith('#/sessions/') && location.hash!==${JSON.stringify(originalHash)}`);
  check(!await evaluate("document.querySelector('.app-shell').textContent.includes('Context restored')"), "new route retained old transcript");
  const freshHash = await evaluate("location.hash");
  await rpc("Page.reload");
  await until("!!document.querySelector('[data-browser-action=sessions]')");
  check(await evaluate("location.hash") === freshHash, "empty new session did not survive refresh");
  process.stdout.write("Files, sandbox previews, copy/download, session reload/context, and routes passed.\n");
  // Exercise the actual Worker bridge with a local dependency fixture, then with real Pyodide if requested.
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
  await rpc("Page.navigate", { url: `${config.origin}/lethetic/integration.html` });
  await until("document.title==='Lethetic integration guide'");
  check(await evaluate(`(async () => {
    const {mountLethetic}=await import('./src/embed.js');
    const container=document.createElement('div');document.body.replaceChildren(container);
    const view=mountLethetic(container,{compact:true,python:true,pyodideIndexURL:location.origin+'/pyodide/',
      connections:[{name:'Embed',provider:'openai',endpoint:location.origin+'/v1/chat/completions',model:'test'}]});
    const prompt=container.querySelector('#composer-input'); prompt.value='calculate';
    prompt.dispatchEvent(new Event('input',{bubbles:true})); container.querySelector('form.composer').requestSubmit();
    for(let i=0;i<200;i++) {
      if(container.textContent.includes('Python says: 42')) {
        const captured=!!container.querySelector('[data-browser-action=save-code]');
        const counted=container.querySelector('.browser-status').textContent.includes('2,528');
        view.destroy(); const cleaned=!document.querySelector('dialog') && !container.childNodes.length;
        container.remove();return captured && cleaned && counted;
      }
      await new Promise(resolve=>setTimeout(resolve,25));
    }
    view.destroy();container.remove();return false;
  })()`), "embedded UI Python/model tool loop failed");
  process.stdout.write("Embedded shared UI and Python tool loop passed.\n");
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
        const value=await run('print(x + 2)');
        const error=await run('raise ValueError("expected-test-error")');
        const controller=new AbortController();
        const loop=python.execute({code:'while True: pass'},controller.signal);
        setTimeout(()=>controller.abort(),100);
        let stopped=false;try{await loop}catch{stopped=true}
        const restarted=await run('print("x" in globals())');
        return {value,error:error.includes('expected-test-error'),stopped,restarted};
      } finally {python.reset()}
    })()`);
    check(JSON.stringify(actual) === JSON.stringify({ value: "42\n", error: true, stopped: true, restarted: "False\n" }), `real Pyodide failed: ${JSON.stringify(actual)}`);
  }
  process.stdout.write(`Browser acceptance passed${config.pyodide ? " (including real Pyodide)" : ""}.\n`);
} finally {
  for (const request of pending.values()) clearTimeout(request.timer);
  pending.clear();
  browser.kill("SIGKILL");
}
