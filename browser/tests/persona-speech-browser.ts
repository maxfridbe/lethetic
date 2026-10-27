/** Real shared UI + Web Audio, with deterministic provider streams and no paid API calls. */
export async function testPersonaSpeech(evaluate: (source: string) => Promise<unknown>, capture?: () => Promise<void>): Promise<void> {
  await evaluate(`(async()=>{
    const {mountLethetic}=await import('./src/embed.js');
    const {BUTTON_ICONS}=await import('./web/src/icons.js');
    const root=document.createElement('div');root.style.height='900px';document.body.replaceChildren(root);
    const data=new Map(),storage={getItem:key=>data.get(key)??null,setItem:(key,value)=>data.set(key,value)};
    const originalFetch=window.fetch,createSource=AudioContext.prototype.createBufferSource;
    const requests=[],sources=[];let held,signal,view,fail=false;
    AudioContext.prototype.createBufferSource=function(){const source=createSource.call(this);sources.push(source);return source};
    const assert=(ok,message)=>{if(!ok)throw Error(message)};
    const wait=async check=>{for(let i=0;i<300;i++){if(check())return;await new Promise(r=>setTimeout(r,20))}throw Error('Speech UI condition timed out')};
    const endpoint=location.origin+'/speech-test';
    window.fetch=async(url,init)=>{
      if(String(url).startsWith(endpoint)){
        requests.push({url:String(url),headers:new Headers(init.headers),body:JSON.parse(init.body)});signal=init.signal;
        if(fail)return new Response('provider echoed secret',{status:401});
        return new Response(new ReadableStream({start(c){held=c;c.enqueue(new Uint8Array(9600));signal.addEventListener('abort',()=>{try{c.error(signal.reason)}catch{}},{once:true})}}),{headers:{'Content-Type':'application/octet-stream'}});
      }
      if(String(url)===location.origin+'/speech-chat')return new Response('data: '+JSON.stringify({choices:[{delta:{content:'Hello from the assistant.'},finish_reason:'stop'}]})+'\\n\\ndata: [DONE]\\n\\n',{headers:{'Content-Type':'text/event-stream'}});
      return originalFetch(url,init);
    };
    const field=name=>document.querySelector('[data-persona-field="'+name+'"]');
    const speech=name=>document.querySelector('[data-speech="'+name+'"]');
    const open=()=>{root.querySelector('[aria-label="Open command palette"]').click();root.querySelector('[id="palette-command-local:personas"]').click()};
    const close=()=>field('close').click();
    const select=id=>{field('select').value=id;field('select').dispatchEvent(new Event('change'))};
    try {
      view=mountLethetic(root,{storage,connections:[{name:'Speech test',provider:'openai',endpoint:location.origin+'/speech-chat',model:'test'}]});
      open();speech('enabled').checked=true;speech('endpoint').value=endpoint;speech('key').value='speech-test-secret';speech('save').click();
      assert(speech('key-status').textContent.includes('present'),'Key presence not shown');
      assert(!Array.from(data.values()).join('').includes('speech-test-secret'),'Speech secret persisted');
      const persona=JSON.parse(data.get('lethetic.personas.v1')).custom.find(p=>p.id==='sassy');
      assert(persona.speech.endpoint===endpoint && !JSON.stringify(persona).includes('speech-test-secret'),'Speech config/key export incorrect');
      close();
      const input=root.querySelector('#composer-input');input.value='Say hello.';input.dispatchEvent(new Event('input',{bubbles:true}));root.querySelector('form.composer').requestSubmit();
      await wait(()=>root.querySelector('.activity-idle') && root.querySelector('[aria-label="Play response"]'));
      assert(!root.querySelector('.chat-block-user [data-persona-speech]'),'User text got speech button');
      root.querySelector('[aria-label="Play response"]').click();
      await wait(()=>sources.length>0);
      assert(held && !signal.aborted,'Speech waited for EOF or aborted early');
      const stop=root.querySelector('[aria-label="Stop speech"]');assert(stop && stop.querySelector('.ui-icon').textContent===BUTTON_ICONS.stop,'Playing button is not square Stop');
      assert(requests[0].headers.get('Authorization')==='Bearer speech-test-secret' && requests[0].body.response_format==='pcm','OpenAI speech request incorrect');
      stop.click();assert(signal.aborted,'Stop did not abort fetch');await wait(()=>!!root.querySelector('[aria-label="Play response"]'));
      root.querySelector('[aria-label="Play response"]').click();await wait(()=>requests.length===2);
      open();select('hal9000');assert(signal.aborted,'Persona switch did not stop speech');
      select('sassy');speech('endpoint').value=endpoint+'/other';speech('save').click();speech('test').click();await wait(()=>requests.length===3);
      assert(!requests[2].headers.has('Authorization'),'Changed endpoint inherited key');speech('test').click();
      speech('provider').value='elevenlabs';speech('provider').dispatchEvent(new Event('change'));
      speech('enabled').checked=true;speech('endpoint').value=endpoint+'/{voice_id}/stream';speech('voice').value='my-voice';speech('key').value='eleven-test-secret';speech('save').click();
      speech('test').click();await wait(()=>requests.length===4 && sources.length>=3);
      assert(requests[3].url.endsWith('/my-voice/stream?output_format=pcm_24000'),'ElevenLabs URL incorrect');
      assert(requests[3].headers.get('xi-api-key')==='eleven-test-secret' && !requests[3].headers.has('Authorization'),'ElevenLabs authentication incorrect');
      held.close();held=null;await wait(()=>speech('test').textContent.includes('Test voice'));
      fail=true;speech('test').click();await wait(()=>speech('status').textContent.includes('401'));
      assert(!speech('status').textContent.includes('secret'),'Error echoed provider response');
      speech('clear').click();assert(speech('key-status').textContent.includes('No API key'),'Clear key failed');
      field('enabled').checked=false;field('enabled').dispatchEvent(new Event('change'));close();
      assert(!root.querySelector('[data-persona-speech]'),'Disabled personas still have Play buttons');
      view.destroy();view=mountLethetic(root,{storage});open();
      assert(speech('provider').value==='elevenlabs' && speech('key-status').textContent.includes('No API key'),'Voice settings not restored independently of keys');
      field('enabled').checked=true;field('enabled').dispatchEvent(new Event('change'));
      document.querySelector('.persona-speech-settings').open=true;
      speech('provider').scrollIntoView({block:'start'});
      globalThis.speechScreenshotView=view;view=null;
    } finally {view?.destroy();window.fetch=originalFetch;AudioContext.prototype.createBufferSource=createSource;}
  })()`);
  try { await capture?.(); }
  finally { await evaluate("globalThis.speechScreenshotView?.destroy();delete globalThis.speechScreenshotView"); }
}
