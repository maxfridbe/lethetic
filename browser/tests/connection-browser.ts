/** Exercise the actual Connections dialog and WASM chat path with synthetic credentials only. */
export async function testConnectionCredentials(evaluate: (source: string) => Promise<unknown>): Promise<void> {
  await evaluate(`(async () => {
    const dialog=document.querySelector('.browser-connections[open]');
    const field=name=>dialog.querySelector('[data-field="'+name+'"]');
    const key=field('key'), originalKey=key.value, session=location.hash;
    const saved=localStorage.getItem('lethetic.connections.v1'), keys=sessionStorage.getItem('lethetic.connection-keys.v1');
    const originalFetch=window.fetch, requests=[];
    const assert=(ok,message)=>{if(!ok)throw Error(message);};
    const wait=async check=>{for(let i=0;i<200;i++){if(check())return;await new Promise(resolve=>setTimeout(resolve,25));}throw Error('Credential check timed out');};
    window.fetch=(url,options)=>{
      if(/\\/v1\\/(models|chat\\/completions)$/.test(new URL(String(url),location.href).pathname)){
        requests.push({url:String(url),headers:new Headers(options?.headers)});
        return Promise.resolve(String(url).endsWith('/models')?Response.json({data:[{id:'test'}]}):
          Response.json({choices:[{finish_reason:'stop',message:{role:'assistant',content:'Connected'}}]}));
      }
      return originalFetch(url,options);
    };
    try {
      // Silent value changes (for example autofill) must be revalidated at action time.
      key.value=location.href;
      field('test').click();
      assert(field('key-state').textContent.includes('URL or session link'),'No clear invalid-key explanation');
      assert(key.getAttribute('aria-invalid')==='true' && field('test').disabled && field('save').disabled,'Invalid key still usable');
      assert(requests.length===0,'Session URL was sent by Test connection');
      key.value=originalKey;key.dispatchEvent(new Event('input'));
      key.value=location.href;
      field('fetch-models').click();
      await wait(()=>field('status').textContent.includes('URL or session link'));
      assert(requests.length===0,'Session URL was sent by Fetch models');
      field('connection-form').dispatchEvent(new Event('submit',{bubbles:true,cancelable:true}));
      assert(localStorage.getItem('lethetic.connections.v1')===saved && sessionStorage.getItem('lethetic.connection-keys.v1')===keys,'Invalid draft replaced saved connection');
      assert(location.hash===session,'Invalid key created a different session');
      key.value='  '+originalKey+'  ';key.dispatchEvent(new Event('input'));
      field('test').click();
      await wait(()=>!field('test').disabled);
      assert(requests.at(-1)?.headers.get('Authorization')==='Bearer '+originalKey,'Test used a different credential');
      field('fetch-models').click();
      await wait(()=>!field('fetch-models').disabled);
      assert(requests.at(-1)?.url.endsWith('/models') && requests.at(-1)?.headers.get('Authorization')==='Bearer '+originalKey,'Model discovery used a different credential');
      const {createRuntime}=await import('./src/sdk.js');
      const connection=JSON.parse(saved)[0];
      const runtime=await createRuntime({connection:{...connection,apiKey:'  '+originalKey+'  '}});
      try{await runtime.send('chat credential regression');}finally{runtime.dispose();}
      assert(requests.at(-1)?.headers.get('Authorization')==='Bearer '+originalKey,'Chat used a different credential');
      assert(requests.length===4 && requests.every(request=>request.headers.get('Authorization')==='Bearer '+originalKey),'Unexpected credential request count or model-discovery credential');
      assert(location.hash===session,'Testing changed the active session');
    } finally {
      window.fetch=originalFetch;key.value=originalKey;key.dispatchEvent(new Event('input'));
    }
  })()`);
}

export async function testConnectionLimits(evaluate: (source: string) => Promise<unknown>): Promise<void> {
  await evaluate(`(async()=>{
    const {LocalTransport}=await import('./src/local-transport.js');
    const {createConnectionsDialog}=await import('./src/connections-dialog.js');
    const {SessionStore}=await import('./src/sessions.js');
    const check=(ok,message)=>{if(!ok)throw Error(message)};
    const wait=async f=>{for(let i=0;i<400;i++){if(f())return;await new Promise(r=>setTimeout(r,20));}throw Error('Connection limits timed out')};
    const connection={name:'Limits test',provider:'openai',endpoint:'https://openrouter.ai/api/v1/chat/completions',model:'vendor/model',apiKey:'limits-test-key'};
    const original=fetch;let failMetadata=false,unavailable=false,chatCalls=0,staleResolve;
    const values=new Map(),storage={getItem:k=>values.get(k)??null,setItem:(k,v)=>values.set(k,v)};
    globalThis.fetch=async(url,init)=>{
      const path=new URL(String(url)).pathname;
      check(new Headers(init?.headers).get('Authorization')==='Bearer limits-test-key','Limits lookup omitted key');
      if(path.endsWith('/endpoints'))return Response.json({data:{endpoints:[{name:'Provider',context_length:100000,max_prompt_tokens:80000,max_completion_tokens:8000}]}});
      if(path.endsWith('/models')){
        if(unavailable)return new Response('',{status:503});
        if(failMetadata)return new Promise(resolve=>{staleResolve=resolve});
        return Response.json({data:[{id:'vendor/model',context_length:128000,top_provider:{max_completion_tokens:16000}}]});
      }
      const body=JSON.parse(init.body);check(body.max_tokens===8000,'Test ignored provider output cap');chatCalls++;
      return Response.json({choices:[{finish_reason:'stop',message:{role:'assistant',content:'Connected'}}]});
    };
    const transport=new LocalTransport({socketOpened(){},authenticationEpochChanged(){},transportStatus(){},protocolFault(){},commandResponse(){},serverMessage(){}},{connections:[connection],storage});
    let editor;
    try {
      transport.start();const id=transport.state.session.session_id;
      check(transport.turnLimits.contextTokens===131072 && transport.turnLimits.maxOutputTokens===16384,'Fourfold defaults missing');
      editor=createConnectionsDialog(transport);editor.open();const field=n=>editor.dialog.querySelector('[data-field="'+n+'"]');
      await wait(()=>!field('test').disabled && chatCalls===1);
      check(field('connection-context').value==='80000','Test did not populate context with provider cap and output reservation');
      check(field('connection-output').value==='8000','Test did not cap output');
      check(field('connection-model-limits').textContent.includes('128,000'),'Model capacity not shown');
      check(field('connection-timeout').value==='10' && field('connection-rounds').value==='32','Operational defaults changed');
      check(transport.turnLimits.contextTokens===131072,'Test silently changed saved budget');
      field('connection-limits-form').requestSubmit();
      check(transport.turnLimits.contextTokens===80000 && transport.turnLimits.maxOutputTokens===8000,'Apply did not persist budgets');
      check(transport.state.session.session_id===id && transport.sessionForExport(id).messages.length===0,'Testing or Apply replaced session/history');
      check(new SessionStore(storage).list()[0].turnLimits.contextTokens===80000,'Limits not saved');
      field('connection-default-limits').click();check(field('connection-context').value==='131072' && field('connection-output').value==='16384','Defaults button did not restore new budgets');
      unavailable=true;field('test').click();await wait(()=>!field('test').disabled && chatCalls===2);
      check(field('connection-model-limits').textContent.includes('unavailable'),'Missing metadata failure status');unavailable=false;
      failMetadata=true;field('test').click();await wait(()=>!!staleResolve);
      field('model').value='vendor/other';field('model').dispatchEvent(new Event('input'));
      staleResolve(Response.json({data:[{id:'vendor/model',context_length:999999}]}));await new Promise(r=>setTimeout(r,50));
      check(field('connection-context').value==='131072' && !field('connection-model-limits').textContent.includes('999,999'),'Stale metadata overwrote editor');
      check(chatCalls===2,'Cancelled test still called chat');
    }finally{editor?.destroy();transport.dispose();globalThis.fetch=original;}
  })()`);
}

/** App restart/autofill must recover the existing chat, without an anonymous provider request. */
export async function testMissingConnectionKey(evaluate: (source: string) => Promise<unknown>): Promise<void> {
  await evaluate(`(async()=>{
    const {LocalTransport}=await import('./src/local-transport.js');
    const {createConnectionsDialog}=await import('./src/connections-dialog.js');
    const check=(ok,message)=>{if(!ok)throw Error(message)};
    const wait=async f=>{for(let i=0;i<400;i++){if(f())return;await new Promise(r=>setTimeout(r,20));}throw Error('Autofill recovery timed out')};
    const records=new Map(),keys=new Map();
    const store=m=>({getItem:k=>m.get(k)??null,setItem:(k,v)=>m.set(k,v)});
    const storage=store(records),credentials=store(keys);
    const connection={name:'Restart test',provider:'openai',endpoint:'https://openrouter.ai/api/v1/chat/completions',model:'vendor/model'};
    const original=fetch;let requests=0;
    globalThis.fetch=async(url,init)=>{
      // Background model-capacity discovery may read OpenRouter's public catalog without a key.
      if(String(url).endsWith('/models'))return Response.json({data:[]});
      requests++;
      check(new Headers(init?.headers).get('Authorization')==='Bearer restored-test-key','Anonymous or stale credential sent');
      return Response.json({choices:[{finish_reason:'stop',message:{role:'assistant',content:'Connected'}}]});
    };
    const events={socketOpened(){},authenticationEpochChanged(){},transportStatus(){},protocolFault(){},commandResponse(){},serverMessage(){}};
    const transport=new LocalTransport(events,{connections:[connection],storage,credentialStorage:credentials});
    let editor;
    try {
      transport.start();const id=transport.state.session.session_id;
      editor=createConnectionsDialog(transport,storage,credentials);editor.open();
      const field=n=>editor.dialog.querySelector('[data-field="'+n+'"]');
      await wait(()=>field('status').textContent.includes('API key missing'));
      check(requests===0,'Opening connections sent anonymous test');
      check(field('active-summary').textContent.includes('No saved key'),'Missing saved key hidden');
      // A password manager may not dispatch input/change.
      field('key').value='restored-test-key';
      await wait(()=>!field('save').disabled && field('key-state').textContent.includes('not saved for chat'));
      field('test').click();await wait(()=>field('status').textContent.includes('draft only'));
      check(transport.connection.apiKey===undefined,'Testing silently saved credential');
      field('use').click();await wait(()=>field('status').textContent.includes('valid response'));
      check(transport.connection.apiKey==='restored-test-key','Save & use did not update active credential');
      check(field('active-summary').textContent.includes('Saved key present'),'Saved credential status incorrect');
      check(transport.state.session.session_id===id,'Credential recovery replaced chat');
      check(!JSON.stringify([...records]).includes('restored-test-key'),'Credential leaked to session store');
      check(JSON.stringify([...keys]).includes('restored-test-key'),'Credential not retained for reload');
      editor.dialog.close();await new Promise(r=>setTimeout(r,30));editor.open();await wait(()=>field('status').textContent.includes('valid response'));
      check(field('key').value==='restored-test-key','Reopening connections lost key');
    } finally {editor?.destroy();transport.dispose();globalThis.fetch=original;}
  })()`);
}
