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
  throw new Error(`Condition failed: ${expression}\n${await evaluate("document.body.innerText.slice(-5000)")}`);
}

try {
  const target = await rpc("Target.createTarget", { url: "about:blank" });
  const attached = await rpc("Target.attachToTarget", { targetId: target["targetId"], flatten: true });
  session = String(attached["sessionId"]);
  await rpc("Page.enable"); await rpc("Runtime.enable");
  await rpc("Page.navigate", { url: `${config.origin}/lethetic/` });
  await until("!!document.querySelector('[data-field=name]')");
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
  check(await evaluate("!!document.querySelector('[data-field=active-summary] .is-valid') && document.querySelector('[data-field=active-summary]').textContent.includes('Key present')"), "active connection or key presence is unclear");
  const connectionSession = await evaluate("location.hash");
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
  await evaluate(`globalThis.processingFetch=fetch;
    globalThis.fetch=async (...args)=>{if(String(args[0]).endsWith('/v1/chat/completions'))await new Promise(resolve=>{globalThis.releaseProcessing=resolve}); return globalThis.processingFetch(...args)};
    document.querySelector('dialog[open]').close();
    const prompt=document.getElementById('composer-input'); prompt.value='hello';
    prompt.dispatchEvent(new Event('input',{bubbles:true})); document.querySelector('form.composer').requestSubmit()`);
  await until("typeof globalThis.releaseProcessing==='function' && !!document.querySelector('.activity-label.is-processing')");
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
  check(await evaluate("document.querySelector('.browser-preview[open] iframe').srcdoc.includes(\"print('private snippet')\")"), "editor preview lost unsaved contents");
  await evaluate("document.querySelector('.browser-preview[open]').close()");
  await evaluate(`document.querySelector('[data-field=file-path]').value='reports/analysis.py';
    document.querySelector('[data-field=file-form]').requestSubmit()`);
  await until("document.querySelector('.files-path')?.textContent.includes('reports/analysis.py') && !document.querySelector('[data-file-action=preview]').disabled");
  check(await evaluate("!!document.querySelector('.files-preview .files-content-actions [data-file-action=preview]')"), "file actions are not beside the content viewer");
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
  await evaluate("document.querySelector('.browser-status').open=true; document.querySelectorAll('[data-stat-pin][aria-pressed=false]').forEach(button=>button.click()); document.querySelector('.browser-status').open=false");
  check(await evaluate("document.querySelectorAll('[data-pinned-stat]').length===14"), "stats did not pin");
  const originalHash = await evaluate("location.hash");
  await evaluate("document.querySelector('#files-toggle').click()");
  await until("!!document.querySelector('.files-entry[title=reports]')");
  await evaluate("document.querySelector('.files-entry[title=reports]').click()");
  await until("!!document.querySelector('.files-entry[title=\"reports/analysis.py\"]')");
  await evaluate("document.querySelector('.files-entry[title=\"reports/analysis.py\"]').click()");
  await until("document.querySelector('.files-path small')?.textContent.includes('B')");
  await until("!!document.querySelector('#files-monaco[data-editor-ready=true]')");
  await until("!document.querySelector('.toast')");
  await evaluate("document.querySelector('[data-files-dock=left]').click()");
  await evaluate("new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))");
  check(await evaluate("(()=>{const files=document.querySelector('.files-pane').getBoundingClientRect(), list=document.querySelector('.files-browser').getBoundingClientRect(), editor=document.querySelector('.files-preview').getBoundingClientRect(), chat=document.querySelector('#chat-scroll').getBoundingClientRect();return files.right<=chat.left+1 && list.bottom<=editor.top+1 && editor.width>200})()"), "left dock did not stack files above the viewer beside chat");
  check(await evaluate("!!document.querySelector('.files-entry .file-type-icon[title=Python]')"), "Python file icon missing");
  const leftImage=await rpc("Page.captureScreenshot",{format:"png"});
  await writeFile(config.screenshot.replace('.png','-files-left.png'),Buffer.from(String(leftImage['data']),'base64'));
  await evaluate("document.querySelector('[data-files-dock=top]').click()");
  await evaluate("new Promise(resolve=>requestAnimationFrame(resolve))");
  check(await evaluate("document.querySelector('.files-pane').getBoundingClientRect().bottom<=document.querySelector('#chat-scroll').getBoundingClientRect().top+1"), "top dock did not restore above chat");
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
  check(await evaluate("document.querySelector('.files-pane').getBoundingClientRect().bottom<=document.querySelector('#chat-scroll').getBoundingClientRect().top+1 && document.documentElement.scrollWidth<=innerWidth"), "left dock did not fall back to top on mobile");
  await evaluate("document.querySelector('#files-toggle').click()");
  check(await evaluate("(()=>{const bar=document.querySelector('.browser-usage-summary');return bar.clientHeight>40 && bar.clientHeight<=parseFloat(getComputedStyle(bar).lineHeight)*3+8})()"), "pinned stats did not wrap within three lines");
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
  check(await evaluate("document.querySelectorAll('[data-pinned-stat]').length===14"), "pins did not survive reload");
  check(await evaluate("document.querySelector('.files-pane').dataset.dock==='left'"), "Files dock did not survive reload");
  await evaluate("document.querySelector('[data-browser-action=connections]').click()");
  check(await evaluate("document.querySelector('[data-field=name]').value==='Browser test' && document.querySelector('[data-field=key]').value==='secret-never-persisted'"), "reload did not preserve connection metadata and tab key");
  await evaluate("document.querySelector('[data-field=key]').value='updated-tab-key'; document.querySelector('[data-field=connection-form]').requestSubmit()");
  check(await evaluate("location.hash") === originalHash, "re-entering a key created a different session");
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
  await evaluate(`globalThis.exportObjectURL=URL.createObjectURL; globalThis.exportAnchorClick=HTMLAnchorElement.prototype.click;
    URL.createObjectURL=blob=>{globalThis.exportedBundle=blob;return globalThis.exportObjectURL(blob)};
    HTMLAnchorElement.prototype.click=function(){globalThis.exportedName=this.download};
    document.querySelector('[data-session-export]').click()`);
  await until("globalThis.exportedName?.endsWith('.tar.gz')");
  const debugArchive = await evaluate("(async()=>btoa(String.fromCharCode(...new Uint8Array(await globalThis.exportedBundle.arrayBuffer()))))()");
  await writeFile(config.screenshot.replace('.png','-session.tar.gz'),Buffer.from(String(debugArchive),'base64'));
  check(await evaluate("location.hash")===originalHash,"export changed the current session");
  await evaluate("URL.createObjectURL=globalThis.exportObjectURL; HTMLAnchorElement.prototype.click=globalThis.exportAnchorClick");
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
  await evaluate("document.querySelector('[data-browser-action=connections]').click(); document.querySelector('[data-field=python]').click()");
  check(await evaluate("location.hash") === originalHash, "enabling Python created a new session");
  check(await evaluate("document.querySelector('.app-shell').textContent.includes('Context restored')"), "enabling Python cleared history");
  await evaluate("document.querySelector('dialog[open]').close()");
  await rpc("Page.reload");
  await until("!!document.querySelector('[data-field=name]')");
  await evaluate("document.querySelector('[data-browser-action=connections]').click()");
  check(await evaluate("document.querySelector('[data-field=python]').checked && document.querySelector('[data-field=key]').value==='updated-tab-key'"), "Python or updated credentials did not survive reload");
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
  await evaluate("document.querySelector('[data-field=python]').click(); document.querySelector('dialog[open]').close()");
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
  await rpc("Page.reload");
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
  await rpc("Page.navigate", { url: `${config.origin}/lethetic/integration.html` });
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
      const first=records()[0].id;
      if(!records()[0].workspace.files.some(file=>file.path==='outputs/chart.png'))throw Error('Python files were not persisted');
      await wait(()=>!!root.querySelector('.files-entry[title=outputs]'));
      root.querySelector('.files-entry[title=outputs]').click();
      await wait(()=>!!root.querySelector('.files-entry[title="outputs/chart.png"]'));
      if(!root.querySelector('.files-entry[title="outputs/empty"]'))throw Error('Empty directory not visible');
      root.querySelector('.files-entry[title="outputs/chart.png"]').click();
      await wait(()=>!root.querySelector('[data-file-action=preview]').disabled);
      root.querySelector('[data-file-action=preview]').click();
      if(!document.querySelector('[data-field=preview-frame]').srcdoc.includes('data:image/png;base64,'))throw Error('PNG preview unavailable');
      document.querySelector('dialog[open]').close();
      root.querySelector('.files-entry[title="outputs/data.csv"]').click();
      await wait(()=>!root.querySelector('[data-file-action=edit]').disabled);
      root.querySelector('[data-file-action=edit]').click();
      document.querySelector('[data-field=file-content]').value='edited in Files';
      document.querySelector('[data-field=file-form]').requestSubmit();
      if(document.querySelector('dialog[open]'))throw Error('File editor stayed open: '+document.querySelector('dialog[open]').textContent);
      await send('read file','read: edited in Files');
      view.destroy(); view=mountLethetic(root,options);
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
  process.stdout.write('Python workspace creation, Files editing, image preview, reopen, isolation and deletion passed.\n');
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
        const error=await run('from pathlib import Path\\nPath("saved.bin").write_bytes(bytes([0,255]))\\nraise ValueError("expected-test-error")');
        const controller=new AbortController();
        const loop=python.execute({code:'from pathlib import Path\\nPath("saved.bin").write_bytes(b"unfinished")\\nwhile True: pass'},controller.signal);
        setTimeout(()=>controller.abort(),100);
        let stopped=false;try{await loop}catch{stopped=true}
        const restarted=await run('print("x" in globals())');
        const restoredFile=await run('from pathlib import Path\\nprint(Path("saved.bin").read_bytes().hex())');
        return {value,error:error.includes('expected-test-error'),stopped,restarted,restoredFile};
      } finally {python.reset()}
    })()`);
    check(JSON.stringify(actual) === JSON.stringify({ value: "42\n", error: true, stopped: true, restarted: "False\n", restoredFile: "00ff\n" }), `real Pyodide failed: ${JSON.stringify(actual)}`);
  }
  process.stdout.write(`Browser acceptance passed${config.pyodide ? " (including real Pyodide)" : ""}.\n`);
} finally {
  for (const request of pending.values()) clearTimeout(request.timer);
  pending.clear();
  browser.kill("SIGKILL");
}
