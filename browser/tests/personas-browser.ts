/** Exercise the real palette, persona switches, imports/exports, SVG isolation, and provider prompt. */
export async function testPersonas(evaluate: (source: string) => Promise<unknown>, capture?: () => Promise<void>): Promise<void> {
  await evaluate(`(async()=>{
    const {mountLethetic}=await import('./src/embed.js');
    const {BUILTIN_PERSONAS}=await import('./web/src/personas/builtins.js');
    const {parsePersona}=await import('./web/src/personas/schema.js');
    const {PersonaEffects}=await import('./web/src/personas/effects.js');
    const root=document.createElement('div');root.style.height='900px';document.body.replaceChildren(root);
    const records=new Map(),storage={getItem:key=>records.get(key)??null,setItem:(key,value)=>records.set(key,value)};
    const originalFetch=window.fetch,originalPlay=PersonaEffects.prototype.play,originalUnlock=PersonaEffects.prototype.unlock;
    const played=[];PersonaEffects.prototype.play=function(sound){played.push(sound)};PersonaEffects.prototype.unlock=async function(){};
    let view,stream;const requests=[];
    const assert=(ok,message)=>{if(!ok)throw Error(message)};
    const wait=async check=>{for(let i=0;i<400;i++){if(check())return;await new Promise(resolve=>setTimeout(resolve,20));}throw Error('Persona UI condition timed out')};
    const endpoint=location.origin+'/persona-provider';
    window.fetch=(url,init)=>String(url)===endpoint ? (requests.push(JSON.parse(init.body)),Promise.resolve(new Response(new ReadableStream({start(c){stream=c}}),{headers:{'Content-Type':'text/event-stream'}}))) : originalFetch(url,init);
    const options={storage,systemPrompt:'Preserve this base prompt exactly.',connections:[{name:'Persona test',provider:'openai',endpoint,model:'test'}]};
    const open=()=>{root.querySelector('[aria-label="Open command palette"]').click();const button=root.querySelector('[id="palette-command-local:personas"]');assert(button,'Personas command missing');assert(root.querySelector('[id="palette-command-local:persona-import"]') && root.querySelector('[id="palette-command-local:persona-export"]'),'Persona import/export palette shortcuts missing');button.click();};
    const field=name=>document.querySelector('[data-persona-field="'+name+'"]');
    const choose=id=>{field('select').value=id;field('select').dispatchEvent(new Event('change'))};
    const toggle=(name,on)=>{field(name).checked=on;field(name).dispatchEvent(new Event('change'))};
    const close=()=>field('close').click();
    const submit=()=>{const input=root.querySelector('#composer-input');input.value='Return JSON only.';input.dispatchEvent(new Event('input',{bubbles:true}));root.querySelector('form.composer').requestSubmit()};
    const finish=()=>{const enc=new TextEncoder();stream.enqueue(enc.encode('data: '+JSON.stringify({choices:[{delta:{content:'{"ok":true}'},finish_reason:'stop'}]})+'\\n\\n'+'data: [DONE]\\n\\n'));stream.close();stream=null};
    try {
      for(const p of BUILTIN_PERSONAS)assert(parsePersona(JSON.stringify(p)).id===p.id,'Built-in cannot roundtrip');
      const badSvg=['<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>','<svg xmlns="http://www.w3.org/2000/svg" onload="alert(1)"/>','<svg xmlns="http://www.w3.org/2000/svg"><image href="https://tracker.invalid/pixel"/></svg>','<svg xmlns="http://www.w3.org/2000/svg"><circle style="fill:red"/></svg>'];
      for(const svg of badSvg){let rejected=false;try{parsePersona(JSON.stringify({...BUILTIN_PERSONAS[0],avatar:{svg,events:{}}}))}catch{rejected=true}assert(rejected,'Unsafe avatar accepted')}
      view=mountLethetic(root,options);open();
      assert(field('select').options.length===5,'Missing built-in personas');assert(field('select').value==='sassy' && !field('audio').checked,'Defaults changed');
      const id=JSON.parse(records.get('lethetic.sessions.v1')).sessions[0].id;
      choose('skippy');assert(root.querySelector('[data-persona="skippy"]'),'Companion did not switch');
      field('preview').click();assert(field('preview-image').getAnimations().length>0,'SVG preview animation missing');
      assert(field('prompt').textContent.includes('narration only: Skippy') && field('prompt').textContent.includes('code, generated files'),'Prompt boundary missing');
      toggle('audio',true);close();submit();await wait(()=>requests.length===1 && !!stream);
      assert(root.querySelector('.thinking-line').textContent.includes('Magnificence'),'Persona quips did not replace sassy lines');
      const system=requests[0].messages.find(m=>m.role==='system').content;
      assert(system.startsWith(options.systemPrompt) && system.includes('Skippy the Magnificent') && system.includes('narration only'),'Provider did not receive persona section');
      assert(requests[0].messages.at(-1).content==='Return JSON only.','Persona contaminated user message');
      const avatar=root.querySelector('#persona-avatar');await avatar.decode();assert(avatar.naturalWidth>0,'Persona SVG failed to decode');assert(avatar.tagName==='IMG' && getComputedStyle(avatar.parentElement).pointerEvents==='none','Avatar is interactive markup');
      finish();await wait(()=>root.querySelector('.activity-idle'));await wait(()=>played.length>=2);
      assert(JSON.parse(records.get('lethetic.sessions.v1')).sessions[0].id===id,'Persona replaced session');
      open();choose('hal9000');close();submit();await wait(()=>requests.length===2 && !!stream);
      const next=requests[1].messages.find(m=>m.role==='system').content;
      assert(next.includes('HAL9000') && !next.includes('Skippy the Magnificent'),'Old persona accumulated');
      assert(requests[1].messages.filter(m=>m.role==='user').length===2,'Persona erased history');finish();await wait(()=>root.querySelector('.activity-idle'));
      open();toggle('enabled',false);close();assert(!root.querySelector('#persona-avatar'),'Disabled companion remains');
      const before=played.length;submit();await wait(()=>requests.length===3 && !!stream);
      assert(!root.querySelector('.thinking-line'),'Disabled quips remain');assert(!requests[2].messages.find(m=>m.role==='system').content.includes('lethetic-persona:start'),'Disabled narration remains');finish();await wait(()=>root.querySelector('.activity-idle'));assert(played.length===before,'Disabled persona played sound');
      view.destroy();view=mountLethetic(root,options);open();assert(field('select').value==='hal9000' && !field('enabled').checked,'Selection or switches lost on reload');
      // Real file input import, including malformed input that must not change selection.
      const imported={...BUILTIN_PERSONAS[2],id:'custom-helper',name:'Custom Helper'};
      const upload=source=>{const transfer=new DataTransfer();transfer.items.add(new File([source],'helper.persona.json',{type:'application/json'}));field('file').files=transfer.files;field('file').dispatchEvent(new Event('change'))};
      upload(JSON.stringify(imported));await wait(()=>field('select').value==='custom-helper');assert(!field('enabled').checked,'Import enabled disabled features');
      upload('{bad');await wait(()=>field('status').textContent.includes('SyntaxError'));assert(field('select').value==='custom-helper','Bad import replaced persona');
      const create=URL.createObjectURL,click=HTMLAnchorElement.prototype.click;let exported;
      URL.createObjectURL=blob=>{exported=blob;return create(blob)};HTMLAnchorElement.prototype.click=function(){};
      try {field('export').click();await wait(()=>!!exported);assert(parsePersona(await exported.text()).id==='custom-helper','Export did not roundtrip')} finally {URL.createObjectURL=create;HTMLAnchorElement.prototype.click=click}
      toggle('enabled',true);toggle('avatar',false);assert(!root.querySelector('#persona-avatar'),'Avatar toggle ineffective');toggle('quips',false);toggle('narration',false);assert(field('prompt').textContent.includes('disabled'),'Narration toggle ineffective');
      field('remove').click();assert(field('select').value==='sassy','Deleting custom persona did not restore default');
      toggle('avatar',true);toggle('quips',true);toggle('narration',true);toggle('audio',false);choose('skippy');
      globalThis.personaScreenshotView=view;view=null;
    } finally {view?.destroy();window.fetch=originalFetch;PersonaEffects.prototype.play=originalPlay;PersonaEffects.prototype.unlock=originalUnlock;}
  })()`);
  try { await capture?.(); }
  finally { await evaluate("globalThis.personaScreenshotView?.destroy();delete globalThis.personaScreenshotView"); }
}
