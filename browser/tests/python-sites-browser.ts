/** Real Pyodide under the shipped CSP, including Files > Run and the opaque preview. */
export async function testPythonSites(
  evaluate: (expression: string) => Promise<unknown>,
  previewInput: (expression: string) => Promise<unknown>,
  capture?: () => Promise<void>,
): Promise<void> {
  await evaluate(`(async()=>{
    const {createPythonTool}=await import('./src/sdk.js');
    const {PYTHON_SITE_EXAMPLE}=await import('./src/prompts.js');
    const signal=()=>new AbortController().signal;
    const denied=createPythonTool();
    try {
      const output=await denied.execute({code:PYTHON_SITE_EXAMPLE},signal());
      if(!output.includes('Interactive sites are disabled') || denied.sites().length)throw Error('Site permission not enforced: '+output);
    } finally {denied.reset()}
    const tool=createPythonTool({siteAccess:true});
    const run=code=>tool.execute({code},signal());
    try {
      const started=await run(PYTHON_SITE_EXAMPLE);
      if(tool.sites()[0]?.port!==8080)throw Error('Python prompt example failed: '+started);
      const hello=await tool.requestSite({port:8080,method:'GET',path:'/api/hello'},signal());
      if(hello.status!==200 || JSON.parse(atob(hello.body)).message!=='Hello from Python')throw Error(JSON.stringify(hello));
      const saved=tool.snapshotWorkspace();
      saved.files.find(file=>file.path==='site/index.html').data=btoa('<h1>Edited live</h1>');
      tool.setWorkspace(saved);
      if(atob((await tool.requestSite({port:8080,method:'GET',path:'/'},signal())).body)!=='<h1>Edited live</h1>')throw Error('Python site missed file edit');
      const network=await run('from pyodide.http import pyfetch\\nasync def network_handler(request):\\n    response = await pyfetch('+JSON.stringify(location.origin+'/v1/models')+')\\n    return Response.json(await response.json())\\nlisten(8091, handler=network_handler)');
      if(!tool.sites().some(site=>site.port===8091))throw Error(network);
      const models=await tool.requestSite({port:8091,method:'GET',path:'/'},signal());
      if(models.status!==200 || !JSON.parse(atob(models.body)).data.length)throw Error('Async handler fetch failed: '+JSON.stringify(models));
      const installed=await run('import micropip\\nawait micropip.install("flask==3.1.2")');
      if(installed.includes('Traceback'))throw Error(installed);
      const flask=await run('from flask import Flask, request, jsonify\\napp = Flask("wire_test", root_path="/home/pyodide")\\n@app.post("/echo")\\ndef echo():\\n    value = request.get_json()\\n    return jsonify(value=value, query=request.args.get("q"))\\nlisten(8092, app)');
      if(!tool.sites().some(site=>site.port===8092))throw Error('Flask WSGI did not start: '+flask);
      const echoed=await tool.requestSite({port:8092,method:'POST',path:'/echo?q=browser',headers:{'Content-Type':['application/json']},body:btoa('{"answer":42}')},signal());
      const data=JSON.parse(atob(echoed.body));
      if(echoed.status!==200 || data.value.answer!==42 || data.query!=='browser')throw Error('Flask failed: '+JSON.stringify(echoed));
    } finally {tool.reset()}
    if(tool.sites().length)throw Error('Reset kept Python servers');
  })()`);

  await evaluate(`(async()=>{
    const {mountLethetic}=await import('./src/embed.js');
    const {SessionStore}=await import('./src/sessions.js');
    const {encodeBytes}=await import('./src/workspace.js');
    const source=await (await fetch('./python/site_example.py')).text();
    const store=new SessionStore(localStorage), saved=store.list()[0];
    saved.mode='python';saved.python=true;saved.siteAccess=true;
    saved.workspace={version:1,directories:['pyapp'],files:[{path:'pyapp/server.py',data:encodeBytes(new TextEncoder().encode(source))}]};
    store.save(saved);
    globalThis.pythonSiteSaved=()=>new SessionStore(localStorage).list().find(item=>item.id===saved.id);
    globalThis.pythonSiteWait=async predicate=>{for(let end=Date.now()+120000;Date.now()<end;){if(predicate())return;await new Promise(r=>setTimeout(r,50))}throw Error('Python site UI timeout: '+document.body.innerText.slice(-2500))};
    const root=document.createElement('div');root.style.height='900px';document.body.replaceChildren(root);
    globalThis.pythonSiteView=mountLethetic(root,{storage:localStorage,initialSession:saved.id,
      connections:[{name:'Python sites',provider:'openai',endpoint:location.origin+'/v1/chat/completions',model:'test'}]});
    if(!root.querySelector('.files-entry'))root.querySelector('#files-toggle').click();
    await pythonSiteWait(()=>!!root.querySelector('.files-entry[title=pyapp]'));
    root.querySelector('.files-entry[title=pyapp]').click();
    await pythonSiteWait(()=>!!root.querySelector('.files-entry[title="pyapp/server.py"]'));
    root.querySelector('.files-entry[title="pyapp/server.py"]').click();
    await pythonSiteWait(()=>!!root.querySelector('[data-file-action=run]'));
    if(!root.querySelector('[data-file-action=run] .ui-icon'))throw Error('Python Run icon missing');
    root.querySelector('[data-file-action=run]').click();
    await pythonSiteWait(()=>root.querySelector('[data-browser-action=sites]')?.textContent.includes('(1)'));
    await pythonSiteWait(()=>!!root.querySelector('.activity-idle'));
    root.querySelector('[data-browser-action=sites]').click();
    await pythonSiteWait(()=>document.querySelector('[data-field=site-frame]')?.src.includes('wire-preview-host.html'));
    if(!document.querySelector('[data-field=site-network]').textContent.includes('Python network: enabled'))throw Error('Python network status missing');
  })()`);
  const result = await previewInput(`(async()=>{
    const wait=async predicate=>{for(let end=Date.now()+10000;Date.now()<end;){if(predicate())return;await new Promise(r=>setTimeout(r,25))}throw Error('Python iframe interaction timeout')};
    await wait(()=>!!document.querySelector('#hello'));
    document.querySelector('#hello').click();
    await wait(()=>document.querySelector('#answer').textContent.includes('Hello browser, from Python!'));
    document.querySelector('input').value='Saved by a Python request';
    document.querySelector('#note').requestSubmit();
    await wait(()=>document.querySelector('#saved').textContent.includes('Saved note.txt'));
    let opaque=false,external=false;try{parent.document.body}catch{opaque=true}
    try{await fetch('https://example.org')}catch{external=true}
    return opaque&&external;
  })()`);
  if (!result) throw Error("Python preview sandbox failed");
  await capture?.();
  await evaluate(`(async()=>{
    await pythonSiteWait(()=>pythonSiteSaved().workspace.files.some(file=>file.path==='pyapp/note.txt'&&atob(file.data)==='Saved by a Python request'));
    document.querySelector('[data-field=site-stop]').click();
    await pythonSiteWait(()=>document.querySelector('[data-browser-action=sites]').textContent.includes('(0)'));
    document.querySelector('dialog[open]').close();
    document.querySelector('[data-file-action=run]').click();
    await pythonSiteWait(()=>document.querySelector('[data-browser-action=sites]').textContent.includes('(1)'));
    await pythonSiteWait(()=>!!document.querySelector('.activity-idle'));
    document.querySelector('[data-browser-action=connections]').click();
    const sites=document.querySelector('[data-field=sites]');
    if(sites.disabled || !sites.checked)throw Error('Python permission checkbox unavailable');
    sites.checked=false;sites.dispatchEvent(new Event('change'));
    if(pythonSiteSaved().siteAccess || !document.querySelector('[data-browser-action=sites]').textContent.includes('(0)'))throw Error('Python revocation failed');
    if(!pythonSiteSaved().workspace.files.some(file=>file.path==='pyapp/note.txt'))throw Error('Revocation discarded Python files');
    pythonSiteView.destroy();
    const {mountLethetic}=await import('./src/embed.js');
    const root=document.createElement('div');document.body.replaceChildren(root);
    const restored=mountLethetic(root,{storage:localStorage,initialSession:pythonSiteSaved().id,
      connections:[{name:'Python sites',provider:'openai',endpoint:location.origin+'/v1/chat/completions',model:'test'}]});
    if(!root.querySelector('[data-browser-action=sites]').textContent.includes('(0)'))throw Error('Reload replayed a Python site');
    if(!pythonSiteSaved().workspace.files.some(file=>file.path==='pyapp/note.txt'))throw Error('Reload lost saved Python request files');
    restored.destroy();
  })()`);
}
