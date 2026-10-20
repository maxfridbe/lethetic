/** Real connection editor, shared transcript, WASM tool loop and reload persistence. */
export async function testConnectionThinking(evaluate: (source: string) => Promise<unknown>): Promise<void> {
  await evaluate(`(async () => {
    const {mountLethetic}=await import('./src/embed.js');
    const root=document.createElement('div'); root.style.height='900px'; document.body.replaceChildren(root);
    const records=new Map(), keys=new Map();
    const store=map=>({getItem:key=>map.get(key)??null,setItem:(key,value)=>map.set(key,value)});
    const storage=store(records), credentialStorage=store(keys);
    const originalFetch=window.fetch, requests=[];
    const details=[{type:'reasoning.text',text:'I should check the record.',signature:'signed',index:0},
      {type:'reasoning.encrypted',data:'never-render-encrypted',id:'r1',index:1}];
    const assert=(ok,message)=>{if(!ok)throw Error(message);};
    const wait=async check=>{for(let i=0;i<400;i++){if(check())return;await new Promise(resolve=>setTimeout(resolve,25));}throw Error('Thinking check timed out: '+root.textContent.slice(-2000));};
    const field=name=>document.querySelector('.browser-connections [data-field="'+name+'"]');
    const options={storage,credentialStorage,tools:[{name:'lookup',description:'Lookup',parameters:{type:'object'},execute:()=> '42'}]};
    let view;
    window.fetch=(url,init)=>{
      if(String(url)!==location.origin+'/v1/chat/completions')return originalFetch(url,init);
      const body=JSON.parse(init.body);requests.push(body);
      assert(new Headers(init.headers).get('Authorization')==='Bearer thinking-test-key','Thinking changed credential');
      const last=body.messages.at(-1);
      let message={role:'assistant',content:'Connected'}, finish_reason='stop';
      if(last.content==='Check the record') {
        message={role:'assistant',content:null,reasoning:'I should check the record.',reasoning_details:details,
          tool_calls:[{id:'lookup-1',type:'function',function:{name:'lookup',arguments:'{}'}}]};finish_reason='tool_calls';
      } else if(last.role==='tool') {
        const returned=body.messages.at(-2).reasoning_details;
        assert(returned?.length===details.length && details.every((entry,index)=>
          Object.keys(returned[index]).length===Object.keys(entry).length && Object.keys(entry).every(key=>returned[index][key]===entry[key])),
          'Tool followup lost signed reasoning');
        message={role:'assistant',content:'The result is 42.'};
      }
      return Promise.resolve(Response.json({choices:[{finish_reason,message}]}));
    };
    try {
      view=mountLethetic(root,{...options,connections:[{name:'Thinking test',provider:'openai',endpoint:location.origin+'/v1/chat/completions',model:'test',apiKey:'thinking-test-key'}]});
      const sessions=()=>JSON.parse(records.get('lethetic.sessions.v1')).sessions;
      const originalSession=sessions()[0].id;
      root.querySelector('[data-browser-action=connections]').click();
      await wait(()=>field('status').textContent.includes('valid response'));
      assert(field('thinking').value==='default','Old connection did not default correctly');
      field('thinking').value='enabled';field('thinking').dispatchEvent(new Event('change'));
      assert(!field('save').disabled,'Thinking change was not detected');
      field('connection-form').requestSubmit();
      await wait(()=>field('status').textContent.includes('valid response'));
      assert(requests.at(-1).reasoning.enabled && requests.at(-1).reasoning.exclude===false,'Connection test did not enable thinking');
      assert(JSON.parse(records.get('lethetic.connections.v1'))[0].thinking==='enabled','Preference not saved');
      assert(field('active-summary').textContent.includes('Thinking: On'),'Active thinking setting unclear');
      document.querySelector('dialog[open]').close();
      const input=root.querySelector('#composer-input'); input.value='Check the record';input.dispatchEvent(new Event('input',{bubbles:true}));
      root.querySelector('form.composer').requestSubmit();
      await wait(()=>root.textContent.includes('The result is 42.') && !root.querySelector('#composer-input').disabled);
      assert(root.querySelectorAll('.block-thought').length===1,'Thinking duplicated or missing');
      assert(root.querySelector('.block-thought').textContent.includes('I should check the record.'),'Thinking not rendered');
      assert(!root.textContent.includes('never-render-encrypted'),'Encrypted reasoning was rendered');
      assert(requests.at(-1).reasoning.enabled,'Chat did not use enabled thinking');
      const saved=records.get('lethetic.sessions.v1');
      assert(saved.includes('reasoning_details') && saved.includes('signed'),'Reasoning not persisted with history');
      view.destroy();view=mountLethetic(root,options);
      assert(root.querySelectorAll('.block-thought').length===1,'Thinking missing after reload');
      root.querySelector('[data-browser-action=connections]').click();
      await wait(()=>field('status').textContent.includes('valid response'));
      assert(field('thinking').value==='enabled' && field('key').value==='thinking-test-key','Reload lost thinking/key');
      field('duplicate').click();
      assert(field('thinking').value==='enabled' && field('key').value==='thinking-test-key','Duplicate lost thinking/key');
      document.querySelector('[data-connection-index="0"] [data-connection-action=edit]').click();
      for(const setting of ['disabled','default']) {
        field('thinking').value=setting;field('thinking').dispatchEvent(new Event('change'));field('connection-form').requestSubmit();
        await wait(()=>field('status').textContent.includes('valid response'));
        assert(setting==='disabled' ? requests.at(-1).reasoning.enabled===false : !('reasoning' in requests.at(-1)),'Off/default request incorrect');
        assert(root.querySelectorAll('.block-thought').length===1,'Preference change replaced session history');
      }
      assert(!JSON.stringify([...records]).includes('thinking-test-key'),'Key entered persistent storage');
      assert(sessions().length===1 && sessions()[0].id===originalSession,'Thinking changes replaced the current session');
    } finally {view?.destroy();root.remove();window.fetch=originalFetch;}
  })()`);
}
