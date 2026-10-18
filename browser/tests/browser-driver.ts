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
    return !document.querySelector('[data-field=send]').disabled;
  })()`), "connection could not be saved");
  check(await evaluate("!JSON.stringify(localStorage).includes('secret-never-persisted')"), "key persisted");
  await evaluate("document.querySelector('[data-field=test]').click()");
  await until("document.querySelector('[role=status]').textContent.includes('valid response')");
  await evaluate(`document.querySelector('[data-field=prompt]').value='hello'; document.querySelector('[data-field=composer]').requestSubmit()`);
  await until("document.querySelector('[data-field=transcript]').textContent.includes('Hello from the browser test')");
  check(await evaluate("performance.getEntriesByType('resource').some(r=>r.name.includes('/lethetic/wasm/lethetic_wasm_bg.wasm'))"), "WASM was not loaded from the project subpath");
  const image = await rpc("Page.captureScreenshot", { format: "png" });
  await writeFile(config.screenshot, Buffer.from(String(image["data"]), "base64"));
  await rpc("Emulation.setDeviceMetricsOverride", { width: 390, height: 844, deviceScaleFactor: 1, mobile: true });
  check(await evaluate("document.documentElement.scrollWidth <= innerWidth"), "mobile layout overflows");
  await rpc("Emulation.clearDeviceMetricsOverride");
  await rpc("Page.reload");
  await until("!!document.querySelector('[data-field=name]')");
  check(await evaluate("document.querySelector('[data-field=name]').value==='Browser test' && document.querySelector('[data-field=key]').value===''"), "reload did not preserve metadata / forget key");
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
  check(await evaluate(`(async () => {
    const {mountLethetic}=await import('./src/embed.js');
    const container=document.createElement('div');document.body.append(container);
    const view=mountLethetic(container,{compact:true,python:true,pyodideIndexURL:location.origin+'/pyodide/',
      connections:[{name:'Embed',provider:'openai',endpoint:location.origin+'/v1/chat/completions',model:'test'}]});
    container.querySelector('[data-field=prompt]').value='calculate';
    container.querySelector('[data-field=composer]').requestSubmit();
    for(let i=0;i<200;i++) {
      if(container.textContent.includes('Python says: 42')) {view.destroy();container.remove();return true}
      await new Promise(resolve=>setTimeout(resolve,25));
    }
    view.destroy();container.remove();return false;
  })()`), "embedded UI Python/model tool loop failed");
  if (config.pyodide) {
    const actual = await evaluate(`(async () => {
      const {createPythonTool}=await import('./src/sdk.js');
      const python=createPythonTool();
      const run=code=>python.execute({code},new AbortController().signal);
      try {
        await run('x = 40');
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
