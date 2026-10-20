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
      assert(requests.length===3,'Unexpected credential request count');
      assert(location.hash===session,'Testing changed the active session');
    } finally {
      window.fetch=originalFetch;key.value=originalKey;key.dispatchEvent(new Event('input'));
    }
  })()`);
}
