/** Real released .NET WASM, not a worker fixture. Runs under the site's actual CSP/subpath. */
export async function testDotnetBrowser(evaluate: (expression: string) => Promise<unknown>): Promise<void> {
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
    const run=(tool,code)=>tool.execute({code},new AbortController().signal);
    const tool=createDotnetTool();
    try {
      await run(tool,'File.WriteAllText("answer.txt", "42"); int answer = 40;');
      if((await run(tool,'answer + 2')).trim()!=='42')throw Error('.NET state failed');
      if(!tool.snapshotWorkspace().files.some(f=>f.path==='answer.txt'))throw Error('.NET files missing');
      const denied=await run(tool,'await Dotnetdide.Net.GetStringAsync("'+location.origin+'/v1/models")');
      if(!denied.includes('disabled'))throw Error('Network was not denied: '+denied);
      const edit=tool.snapshotWorkspace();edit.files.push({path:'upload.txt',data:btoa('from UI')});tool.setWorkspace(edit);
      if(!(await run(tool,'File.ReadAllText("upload.txt")')).includes('from UI'))throw Error('Uploaded file unavailable');
      const controller=new AbortController();
      const pending=tool.execute({code:'while(true) {}'},controller.signal);
      setTimeout(()=>controller.abort(),150);let stopped=false;try{await pending}catch{stopped=true}
      if(!stopped)throw Error('.NET stop failed');
      if((await run(tool,'File.ReadAllText("answer.txt")')).trim()!=='42')throw Error('Files not restored after stop');
      if(!(await run(tool,'answer')).includes('CS0103'))throw Error('Variables survived worker termination');
    } finally {tool.reset()}
    const online=createDotnetTool({networkAccess:true});
    try { if(!(await run(online,'await Dotnetdide.Net.GetStringAsync("'+location.origin+'/v1/models")')).includes('deepseek/test'))throw Error('Enabled .NET network failed'); }
    finally {online.reset()}
  })()`);

  await evaluate(`(async () => {
    const {mountLethetic}=await import('./src/embed.js');
    localStorage.removeItem('lethetic.sessions.v1');
    const root=document.createElement('div');root.style.height='900px';document.body.replaceChildren(root);
    const options={storage:localStorage,connections:[{name:'Dotnet test',provider:'openai',endpoint:location.origin+'/v1/chat/completions',model:'test'}]};
    let view=mountLethetic(root,options);
    const wait=async predicate=>{for(let i=0;i<1200;i++){if(predicate())return;await new Promise(resolve=>setTimeout(resolve,50))}throw Error('Timed out: '+root.innerText.slice(-2000))};
    const change=(selector,value)=>{const el=document.querySelector(selector);if(el.type==='checkbox')el.checked=value;else el.value=value;el.dispatchEvent(new Event('change'))};
    const saved=()=>JSON.parse(localStorage.getItem('lethetic.sessions.v1')).sessions[0];
    const send=async text=>{const input=root.querySelector('#composer-input');input.value=text;input.dispatchEvent(new Event('input',{bubbles:true}));root.querySelector('form.composer').requestSubmit();await wait(()=>root.innerText.includes('C# says:'));await wait(()=>!!root.querySelector('.activity-idle'));};
    root.querySelector('[data-browser-action=connections]').click();
    const id=saved().id;change('[data-field=mode]','dotnet-wasm');
    if(document.querySelector('[data-field=network]').checked)throw Error('Network should start disabled');
    change('[data-field=network]',true);if(saved().id!==id || !saved().networkAccess)throw Error('Permission changed session');
    change('[data-field=network]',false);document.querySelector('dialog[open]').close();
    await send('create dotnet files');
    await wait(()=>saved().workspace.files.some(f=>f.path==='Program.cs'));
    if(!root.querySelector('.python-tool-code code')?.textContent.includes('Dotnet.Run'))throw Error('C# call not rendered as parsed code');
    view.destroy();view=mountLethetic(root,{...options,sessionRouting:false});
    root.querySelector('[data-browser-action=connections]').click();
    if(document.querySelector('[data-field=mode]').value!=='dotnet-wasm' || saved().id!==id || saved().networkAccess)throw Error('.NET session settings not restored');
    document.querySelector('dialog[open]').close();
    if(!saved().workspace.files.some(f=>f.path==='bin/Program.dll'))throw Error('Compiled artifacts not saved');
    root.querySelector('#files-toggle').click();
    await wait(()=>!!root.querySelector('.files-entry[title="Program.cs"]'));
    root.querySelector('.files-entry[title="Program.cs"]').click();
    await wait(()=>!!root.querySelector('.monaco-editor'));
    await document.fonts.ready;
    globalThis.dotnetTestView=view;
  })()`);
}
