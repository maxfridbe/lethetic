/** Real released .NET WASM, not a worker fixture. Runs under the site's actual CSP/subpath. */
export async function testDotnetBrowser(evaluate: (expression: string) => Promise<unknown>, previewInput: (expression: string, full?: boolean) => Promise<unknown>, captureSite?: () => Promise<void>): Promise<void> {
  await evaluate(`(async () => {
    // Apply the shipped policy before loading the runtime in the embedding fixture.
    const html=await (await fetch('./index.html')).text();
    const shipped=new DOMParser().parseFromString(html,'text/html');
    const policy=shipped.querySelector('meta[http-equiv="Content-Security-Policy"]');
    if(!policy)throw Error('Standalone CSP missing');
    document.head.append(policy.cloneNode(true));
    document.querySelector('link[href="./guide.css"]')?.remove();
    const styles=document.createElement('link');styles.rel='stylesheet';styles.href='./styles.css';
    await new Promise((resolve,reject)=>{styles.onload=resolve;styles.onerror=reject;document.head.append(styles)});
    await document.fonts.ready;
    const {createDotnetTool}=await import('./src/sdk.js');
    const {DOTNET_PROGRAM_EXAMPLE,DOTNET_PROJECT_EXAMPLE}=await import('./src/prompts.js');
    const run=(tool,code)=>tool.execute({code},new AbortController().signal);
    const tool=createDotnetTool();
    try {
      await run(tool,'File.WriteAllText("answer.txt", "42"); int answer = 40;');
      if((await run(tool,'answer + 2')).trim()!=='42')throw Error('.NET state failed');
      if(!tool.snapshotWorkspace().files.some(f=>f.path==='answer.txt'))throw Error('.NET files missing');
      // Execute the exact examples injected into the model prompt in real WASM.
      for(const example of [DOTNET_PROGRAM_EXAMPLE,DOTNET_PROJECT_EXAMPLE]) {
        const output=await run(tool,example);
        if(!output.includes('Build succeeded') || output.includes('Build failed'))throw Error('Prompt example failed: '+output);
      }
      const exampleFiles=tool.snapshotWorkspace().files;
      const json=exampleFiles.find(f=>f.path==='demo/output/result.json');
      const answerFile=exampleFiles.find(f=>f.path==='report/answer.txt');
      if(!json || JSON.parse(atob(json.data)).answer!==42 || !answerFile || atob(answerFile.data)!=='42')throw Error('Prompt example outputs missing');
      for(const path of ['demo/Program.cs','demo/bin/Program.dll','report/Report.csproj','report/Answer.cs','report/bin/Report.dll']) {
        if(!exampleFiles.some(f=>f.path===path))throw Error('Prompt example artifact missing: '+path);
      }
      for(const name of ['images','audio']) {
        const response=await fetch('./runtimes/dotnetdide/examples/'+name+'.cs');
        if(!response.ok)throw Error('Missing media example: '+name);
        const source=await response.text();
        const result=await run(tool,source);
        if(!result.includes('Created media/'))throw Error('Media generation failed: '+result);
      }
      globalThis.dotnetGeneratedWorkspace=tool.snapshotWorkspace();
      const packageDenied=await run(tool,'await DotnetDide.Nuget.Load("Newtonsoft.Json", "13.0.4")');
      if(!packageDenied.includes('disabled'))throw Error('NuGet should default off: '+packageDenied);
      const denied=await run(tool,'await Dotnetdide.Net.GetStringAsync("'+location.origin+'/v1/models")');
      if(!denied.includes('disabled'))throw Error('Network was not denied: '+denied);
      const edit=tool.snapshotWorkspace();edit.files.push({path:'upload.txt',data:btoa('from UI')});tool.setWorkspace(edit);
      if(!(await run(tool,'File.ReadAllText("upload.txt")')).includes('from UI'))throw Error('Uploaded file unavailable');
      const large=await run(tool,'File.WriteAllBytes("large.bin", new byte[3 * 1024 * 1024]); Console.WriteLine(new FileInfo("large.bin").Length);');
      if(!large.includes('3145728') || !tool.snapshotWorkspace().files.some(f=>f.path==='large.bin' && f.data.length===4194304))throw Error('Large .NET checkpoint failed: '+large);
      const controller=new AbortController();
      const pending=tool.execute({code:'while(true) {}'},controller.signal);
      setTimeout(()=>controller.abort(),150);let stopped=false;try{await pending}catch{stopped=true}
      if(!stopped)throw Error('.NET stop failed');
      if((await run(tool,'File.ReadAllText("answer.txt")')).trim()!=='42')throw Error('Files not restored after stop');
      if((await run(tool,'new FileInfo("large.bin").Length')).trim()!=='3145728')throw Error('Large .NET file not restored');
      if(!(await run(tool,'answer')).includes('CS0103'))throw Error('Variables survived worker termination');
    } finally {tool.reset()}
    const sitesTool=createDotnetTool({siteAccess:true});
    try {
      const {DOTNET_SITE_EXAMPLE}=await import('./src/prompts.js');
      const create='Directory.CreateDirectory("promptsite"); File.WriteAllText("promptsite/Program.cs", '+JSON.stringify(DOTNET_SITE_EXAMPLE)+'); var siteRun=await Dotnet.Run("promptsite/Program.cs"); Console.WriteLine(siteRun.Success);';
      const output=await run(sitesTool,create);
      if(!output.includes('True') || !sitesTool.sites().some(site=>site.port===8080))throw Error('Site prompt example failed: '+output);
      const response=await sitesTool.requestSite({port:8080,method:'GET',path:'/api/hello'},new AbortController().signal);
      if(response.status!==200 || !atob(response.body).includes('Hello from C#'))throw Error('Compiled prompt route failed: '+JSON.stringify(response));
    } finally {sitesTool.reset()}
    const online=createDotnetTool({networkAccess:true});
    try { if(!(await run(online,'await Dotnetdide.Net.GetStringAsync("'+location.origin+'/v1/models")')).includes('deepseek/test'))throw Error('Enabled .NET network failed'); }
    finally {online.reset()}
    const packages=createDotnetTool({nugetAccess:true,networkAccess:false});
    try {
      const loaded=await run(packages,'await DotnetDide.Nuget.Load("Newtonsoft.Json", "13.0.4")');
      if(!loaded.includes('13.0.4'))throw Error('Browser package load failed: '+loaded);
      if(!(await run(packages,'Newtonsoft.Json.JsonConvert.SerializeObject(new { answer = 42 })')).includes('"answer":42'))throw Error('Loaded package unavailable');
      if(!(await run(packages,'await Dotnetdide.Net.GetStringAsync("'+location.origin+'/v1/models")')).includes('disabled'))throw Error('NuGet granted general network access');
    } finally {packages.reset()}
  })()`);

  await evaluate(`(async () => {
    const {mountLethetic}=await import('./src/embed.js');
    localStorage.removeItem('lethetic.sessions.v1');
    const root=document.createElement('div');root.style.height='900px';document.body.replaceChildren(root);
    const options={storage:localStorage,credentialStorage:sessionStorage,connections:[{name:'Dotnet test',provider:'openai',endpoint:location.origin+'/v1/chat/completions',model:'test'}]};
    let view=mountLethetic(root,options);
    view.destroy();
    const {SessionStore}=await import('./src/sessions.js');
    const store=new SessionStore(localStorage), initial=store.list()[0];
    store.save({...initial,workspace:globalThis.dotnetGeneratedWorkspace});
    view=mountLethetic(root,{...options,sessionRouting:false});
    const wait=async predicate=>{for(let i=0;i<1200;i++){if(predicate())return;await new Promise(resolve=>setTimeout(resolve,50))}throw Error('Timed out: '+root.innerText.slice(-2000))};
    const change=(selector,value)=>{const el=document.querySelector(selector);if(el.type==='checkbox')el.checked=value;else el.value=value;el.dispatchEvent(new Event('change'))};
    const saved=()=>JSON.parse(localStorage.getItem('lethetic.sessions.v1')).sessions[0];
    const send=async text=>{const input=root.querySelector('#composer-input');input.value=text;input.dispatchEvent(new Event('input',{bubbles:true}));root.querySelector('form.composer').requestSubmit();await wait(()=>root.innerText.includes('C# says:'));await wait(()=>!!root.querySelector('.activity-idle'));};
    root.querySelector('[data-browser-action=connections]').click();
    const id=saved().id;change('[data-field=mode]','dotnet-wasm');
    if(document.querySelector('[data-field=nuget]').checked)throw Error('NuGet should start disabled');
    change('[data-field=nuget]',true);if(saved().id!==id || !saved().nugetAccess || saved().networkAccess)throw Error('NuGet changed session or network permission');
    if(document.querySelector('[data-field=network]').checked)throw Error('Network should start disabled');
    change('[data-field=network]',true);if(saved().id!==id || !saved().networkAccess)throw Error('Permission changed session');
    change('[data-field=network]',false);
    change('[data-field=network-route]','relay');
    const endpoint=document.querySelector('[data-field=relay-endpoint]'),token=document.querySelector('[data-field=relay-token]');
    endpoint.value='socks5://localhost:9050';document.querySelector('[data-field=apply-route]').click();
    if(!document.querySelector('.browser-connections [data-field=status]').textContent.includes('SOCKS') || saved().dotnetRelayEndpoint)throw Error('Direct SOCKS route was accepted');
    endpoint.value='https://relay.example/relay';endpoint.dispatchEvent(new Event('input'));
    token.value='private-relay-token-for-browser-test';token.dispatchEvent(new Event('input'));
    document.querySelector('[data-field=apply-route]').click();
    if(saved().dotnetRelayEndpoint!==endpoint.value || saved().id!==id || localStorage.getItem('lethetic.sessions.v1').includes(token.value))throw Error('Relay persistence leaked token or changed session');
    if(!document.querySelector('[data-field=permissions]').textContent.includes('token present'))throw Error('Relay token state missing');
    // Stay offline for the C# file-generation turn: no live proxy is required for local computation.
    document.querySelector('dialog[open]').close();
    await send('create dotnet files');
    await wait(()=>saved().workspace.files.some(f=>f.path==='Program.cs'));
    if(!root.querySelector('.python-tool-code code')?.textContent.includes('Dotnet.Run'))throw Error('C# call not rendered as parsed code');
    view.destroy();view=mountLethetic(root,{...options,sessionRouting:false});
    root.querySelector('[data-browser-action=connections]').click();
    if(!document.querySelector('[data-field=nuget]').checked)throw Error('NuGet permission not restored');
    if(document.querySelector('[data-field=mode]').value!=='dotnet-wasm' || saved().id!==id || saved().networkAccess)throw Error('.NET session settings not restored');
    if(document.querySelector('[data-field=relay-token]').value!=='private-relay-token-for-browser-test' || document.querySelector('[data-field=network-route]').value!=='relay')throw Error('Relay not restored with the existing session');
    document.querySelector('dialog[open]').close();
    if(!saved().workspace.files.some(f=>f.path==='bin/Program.dll'))throw Error('Compiled artifacts not saved');
    root.querySelector('#files-toggle').click();
    await wait(()=>!!root.querySelector('.files-entry[title="Program.cs"]'));
    root.querySelector('.files-entry[title="Program.cs"]').click();
    await wait(()=>!!root.querySelector('.monaco-editor'));
    await document.fonts.ready;
    globalThis.dotnetTestView=view;
  })()`);
  await evaluate(`(async()=>{
    const folder=document.querySelector('.files-entry[title="media"]');
    if(!folder)throw Error('.NET media folder absent from Files');folder.click();
  })()`);
  for(const [name,tag] of [['gradient.png','img'],['gradient.jpg','img'],['tone.wav','audio'],['tone.opus','audio']]) {
    await evaluate(`(async()=>{
      const wait=async p=>{for(let i=0;i<200;i++){if(p())return;await new Promise(r=>setTimeout(r,50))}throw Error('Preview did not open')};
      await wait(()=>document.querySelector('.files-entry[title="media/${name}"]'));
      document.querySelector('.files-entry[title="media/${name}"]').click();
      await wait(()=>document.querySelector('.files-inline-preview')?.srcdoc.includes('<${tag}'));
      document.querySelector('[data-preview-expand]').click();
    })()`);
    const decoded=await previewInput(`(async()=>{
      const item=document.querySelector('${tag}');
      if('${tag}'==='img'){await item.decode();return item.naturalWidth===64 && item.naturalHeight===32}
      if(item.readyState<1)await new Promise((resolve,reject)=>{item.addEventListener('loadedmetadata',resolve,{once:true});item.addEventListener('error',reject,{once:true});setTimeout(()=>reject(Error('Audio timeout')),15000)});
      return item.controls && item.duration>0.24 && item.duration<0.3;
    })()`);
    if(!decoded)throw Error('Generated .NET file did not preview: '+name);
    await evaluate("document.querySelector('.browser-preview[open]').close()");
  }

  await evaluate(`(async()=>{
    globalThis.wireWait=async predicate=>{for(let end=Date.now()+60000;Date.now()<end;){if(predicate())return;await new Promise(r=>setTimeout(r,50))}throw Error('Wire UI timed out: '+document.body.innerText.slice(-2500))};
    const wait=globalThis.wireWait;
    globalThis.wireSaved=()=>JSON.parse(localStorage.getItem('lethetic.sessions.v1')).sessions[0];
    const saved=globalThis.wireSaved, id=saved().id;
    // Existing connection/session remains intact. Run a project directly through Files.
    document.querySelector('[data-browser-action=connections]').click();
    const permission=document.querySelector('[data-field=sites]');
    if(permission.checked)throw Error('Interactive sites must default off');
    permission.checked=true;permission.dispatchEvent(new Event('change'));
    if(saved().id!==id || !saved().siteAccess || saved().networkAccess)throw Error('Sites grant changed session or network');
    document.querySelector('dialog[open]').close();
    const source=await (await fetch('./runtimes/dotnetdide/examples/site.cs')).text();
    const create=async(path,content)=>{
      document.querySelector('[data-file-action=new]').click();
      document.querySelector('[data-field=file-path]').value=path;
      document.querySelector('[data-field=file-content]').value=content;
      document.querySelector('[data-field=file-form]').requestSubmit();
      await wait(()=>document.querySelector('.files-path')?.textContent.includes(path));
    };
    await create('wireapp/Program.cs',source);
    if(!document.querySelector('[data-file-action=run] .ui-icon'))throw Error('C# Run icon missing');
    await create('wireapp/Site.csproj','<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><OutputType>Exe</OutputType><TargetFramework>net11.0</TargetFramework></PropertyGroup></Project>');
    document.querySelector('[data-file-action=run]').click();
    await wait(()=>document.querySelector('[data-browser-action=sites]')?.textContent.includes('(1)'));
    await wait(()=>!!document.querySelector('.activity-idle'));
    if(saved().id!==id || !saved().workspace.files.some(file=>file.path==='wireapp/bin/Site.dll'))throw Error('Project Run lost files/session');
    if(!document.body.innerText.includes('Site ready on virtual port'))throw Error('Run output missing');
    document.querySelector('[data-browser-action=sites]').click();
    await wait(()=>document.querySelector('[data-field=site-frame]')?.src.includes('wire-preview-host.html'));
    const networkNote=document.querySelector('[data-field=site-network]').textContent;
    if(!networkNote.includes('disabled') || !networkNote.includes('run the site file again'))throw Error('Sites must explain disabled outbound HTTP and restart');
    await new Promise(r=>setTimeout(r,300));
  })()`);
  const interacted=await previewInput(`(async()=>{
    const wait=async predicate=>{for(let end=Date.now()+10000;Date.now()<end;){if(predicate())return;await new Promise(r=>setTimeout(r,50))}throw Error('Site interaction timeout')};
    await wait(()=>!!document.querySelector('#hello'));
    document.querySelector('#hello').click();await wait(()=>document.querySelector('#answer').textContent.includes('IServer'));
    document.querySelector('input').value='Lethetic request checkpoint';document.querySelector('#note').requestSubmit();
    await wait(()=>document.querySelector('#saved').textContent.includes('Saved note.txt'));
    let opaque=false;try{parent.document.body}catch{opaque=true}
    return opaque;
  })()`);
  if(!interacted)throw Error('Interactive site could access parent');
  await captureSite?.();
  await evaluate(`(async()=>{
    const wait=globalThis.wireWait,saved=globalThis.wireSaved,id=saved().id;
    await wait(()=>saved().workspace.files.some(file=>file.path==='wireapp/note.txt' && atob(file.data)==='Lethetic request checkpoint'));
    document.querySelector('[data-field=site-stop]').click();
    await wait(()=>document.querySelector('[data-browser-action=sites]')?.textContent.includes('(0)'));
    document.querySelector('dialog[open]').close();
    // The single-file button also compiles; the old virtual port has been released.
    document.querySelector('.files-entry[title="wireapp/Program.cs"]').click();
    await wait(()=>document.querySelector('.files-path')?.textContent.includes('Program.cs'));
    document.querySelector('[data-file-action=run]').click();
    await wait(()=>document.querySelector('[data-browser-action=sites]')?.textContent.includes('(1)'));
    await wait(()=>!!document.querySelector('.activity-idle'));
    document.querySelector('[data-browser-action=connections]').click();
    const permission=document.querySelector('[data-field=sites]');permission.checked=false;permission.dispatchEvent(new Event('change'));
    if(saved().siteAccess || saved().id!==id || !saved().workspace.files.some(file=>file.path==='wireapp/note.txt'))throw Error('Revocation lost session/files');
    if(!document.querySelector('[data-browser-action=sites]').textContent.includes('(0)'))throw Error('Revocation kept site running');
    const network=document.querySelector('[data-field=network]');network.checked=true;network.dispatchEvent(new Event('change'));
    if(!saved().networkAccess || saved().id!==id)throw Error('Network permission lost session');
    document.querySelector('dialog[open]').close();
    document.querySelector('[data-browser-action=sites]').click();
    const route=saved().dotnetRelayEndpoint ? 'HTTP relay' : 'direct requests';
    if(!document.querySelector('[data-field=site-network]').textContent.includes('enabled · '+route))throw Error('Sites network status is stale');
    document.querySelector('dialog[open]').close();
    document.querySelector('[data-browser-action=connections]').click();
    network.checked=false;network.dispatchEvent(new Event('change'));
    document.querySelector('dialog[open]').close();
  })()`);

}
