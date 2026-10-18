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
  await until("!!document.querySelector('dialog[open] [data-field=passphrase]')");
  await evaluate(`document.querySelector('[data-field=passphrase]').value='browser test passphrase';
    document.querySelector('[data-field=unlock-form]').requestSubmit()`);
  await until("!document.querySelector('[data-field=editor]').hidden");
  check(await evaluate("document.querySelector('[data-field=code]').value===\"print('private snippet')\""), "code capture failed");
  await evaluate(`document.querySelector('[data-field=filename]').value='private-analysis.py';
    document.querySelector('[data-field=save-form]').requestSubmit()`);
  await until("document.querySelector('dialog[open] [role=status]').textContent.includes('Saved encrypted')");
  check(await evaluate(`!JSON.stringify(localStorage).includes('private snippet') &&
    !JSON.stringify(localStorage).includes('private-analysis.py') &&
    !JSON.stringify(localStorage).includes('browser test passphrase')`), "plaintext code or passphrase persisted");
  await evaluate("document.querySelector('dialog[open]').close()");
  await until("document.querySelector('[data-field=editor]').hidden");
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
  await evaluate(`document.querySelector('dialog[open]').close(); document.querySelector('[data-browser-action=code]').click();
    document.querySelector('[data-field=passphrase]').value='wrong password'; document.querySelector('[data-field=unlock-form]').requestSubmit()`);
  await until("document.querySelector('dialog[open] [role=status]').textContent.includes('Unable to unlock')");
  check(await evaluate("document.querySelector('[data-field=editor]').hidden"), "wrong passphrase unlocked library");
  await evaluate(`document.querySelector('[data-field=passphrase]').value='browser test passphrase';
    document.querySelector('[data-field=unlock-form]').requestSubmit()`);
  await until("!document.querySelector('[data-field=editor]').hidden");
  check(await evaluate(`(() => {
    const files=document.querySelector('[data-field=files]'); files.selectedIndex=1; files.dispatchEvent(new Event('change'));
    return document.querySelector('[data-field=filename]').value==='private-analysis.py' &&
      document.querySelector('[data-field=code]').value===\"print('private snippet')\";
  })()`), "encrypted code did not survive reload");
  await evaluate("document.querySelector('[data-field=delete]').click()");
  await until("document.querySelector('dialog[open] [role=status]').textContent.includes('deleted')");
  await evaluate("document.querySelector('dialog[open]').close()");
  // Exercise the actual Worker bridge with a local dependency fixture, then with real Pyodide if requested.
  process.stdout.write("Encrypted code reload, wrong-passphrase rejection, and deletion passed.\n");
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
