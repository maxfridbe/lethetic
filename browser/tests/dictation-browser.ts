/** Real shared composer with a deterministic native bridge. Never opens a test machine's mic. */
export async function testDictation(evaluate: (source: string) => Promise<unknown>, capture?: () => Promise<void>, android = false): Promise<void> {
  await evaluate(`(async()=>{
    const {mountLethetic}=await import('./src/embed.js');
    const root=document.createElement('div');root.style.height='900px';document.body.replaceChildren(root);
    const names=['__LETHETIC_DESKTOP__','__LETHETIC_ANDROID__','__LETHETIC_HOST_TOKEN__','__TAURI__'];
    const prior=names.map(name=>Object.getOwnPropertyDescriptor(window,name));
    const preference=localStorage.getItem('lethetic.dictation.v1');localStorage.removeItem('lethetic.dictation.v1');
    const calls=[];let finish,view,fail=false,chatRequests=0;
    const originalFetch=window.fetch;
    const assert=(ok,message)=>{if(!ok)throw Error(message)};
    const wait=async check=>{for(let i=0;i<300;i++){if(check())return;await new Promise(r=>setTimeout(r,20))}throw Error('Dictation UI condition timed out')};
    const host={core:{invoke:async(command,args)=>{
      assert(args.token==='dictation-test-capability','Native capability missing');calls.push({command,args});
      if(command.endsWith('|prepare_dictation'))return;
      if(command.endsWith('|dictation_status'))return {ready:true,model:'base',level:.55};
      if(command.endsWith('|start_dictation')){if(fail)throw 'Microphone permission denied';return;}
      if(command.endsWith('|finish_dictation'))return new Promise(resolve=>{finish=resolve});
      if(command.endsWith('|cancel_dictation'))return;
      // Native lifecycle, directory and network reset calls are unrelated to microphone capture.
      if(command.endsWith('|set_working') || command.endsWith('|revoke_network') || command.endsWith('|stop_forwarded_sites') || command.endsWith('|cancel_download') || command.endsWith('|cancel_speech') || command.endsWith('|revoke_directory'))return;
      throw Error('Unexpected native command '+command);
    }}};
    for(const [name,value] of [['__LETHETIC_DESKTOP__',${!android}],['__LETHETIC_ANDROID__',${android}],['__LETHETIC_HOST_TOKEN__','dictation-test-capability'],['__TAURI__',host]])Object.defineProperty(window,name,{configurable:true,value});
    window.fetch=async(url,init)=>{
      if(String(url)===location.origin+'/dictation-chat'){chatRequests++;return new Response('data: '+JSON.stringify({choices:[{delta:{content:'Received.'},finish_reason:'stop'}]})+'\\n\\ndata: [DONE]\\n\\n',{headers:{'Content-Type':'text/event-stream'}})}
      return originalFetch(url,init);
    };
    const button=()=>root.querySelector('[data-dictation-send]');
    const input=()=>root.querySelector('#composer-input');
    const pointer=type=>button().dispatchEvent(new PointerEvent(type,{bubbles:true,button:0,pointerId:1,isPrimary:true,pointerType:${JSON.stringify(android ? 'touch' : 'mouse')}}));
    // Synthetic pointer tests use the same handlers; native capture is tested separately.
    const pointerCapture=HTMLElement.prototype.setPointerCapture;
    HTMLElement.prototype.setPointerCapture=function(){};
    try {
      view=mountLethetic(root,{connections:[{name:'Dictation',provider:'openai',endpoint:location.origin+'/dictation-chat',model:'test'}]});
      await wait(()=>button() && root.querySelector('.dictation-status').textContent.includes('Hold Send to dictate locally'));
      assert(!button().disabled,'Empty draft must still allow holding Send');
      assert(calls.find(c=>c.command.endsWith('|prepare_dictation')).args.model===${JSON.stringify(android ? 'tiny' : 'base')},'Wrong default speech model');
      assert(getComputedStyle(button()).touchAction==='none','Hold gesture must not become a page scroll');
      input().value='Existing draft';input().dispatchEvent(new Event('input',{bubbles:true}));
      pointer('pointerdown');await new Promise(r=>setTimeout(r,350));await wait(()=>button().textContent.includes('Listening'));
      await wait(()=>root.querySelector('meter'));
      assert(!chatRequests,'Holding Send submitted the existing draft');
      pointer('pointerup');button().click();await wait(()=>finish);
      finish('new spoken words');await wait(()=>input().value==='Existing draft new spoken words');
      assert(!chatRequests,'Releasing Send submitted the transcript');
      assert(calls.some(c=>c.command.endsWith('|start_dictation')) && calls.some(c=>c.command.endsWith('|finish_dictation')),'Native capture was not used');
      pointer('pointerdown');await new Promise(r=>setTimeout(r,350));await wait(()=>button().textContent.includes('Listening'));
      window.dispatchEvent(new Event('blur'));await wait(()=>button().textContent.trim().endsWith('Send'));
      assert(input().value==='Existing draft new spoken words','Cancellation altered the draft');
      pointer('pointerdown');await new Promise(r=>setTimeout(r,350));await wait(()=>button().textContent.includes('Listening'));
      pointer('pointercancel');button().click();await wait(()=>button().textContent.trim().endsWith('Send'));
      assert(!chatRequests,'Cancelled touch gesture submitted the draft');
      fail=true;pointer('pointerdown');await new Promise(r=>setTimeout(r,350));await wait(()=>root.querySelector('.dictation-status').textContent.includes('permission denied'));
      pointer('pointerup');button().click();assert(!chatRequests,'Mic error accidentally sent the draft');fail=false;
      // Space works without a pointer and a late result after Escape must be discarded.
      button().dispatchEvent(new KeyboardEvent('keydown',{key:' ',bubbles:true}));await new Promise(r=>setTimeout(r,350));await wait(()=>button().textContent.includes('Listening'));
      finish=null;button().dispatchEvent(new KeyboardEvent('keyup',{key:' ',bubbles:true}));await wait(()=>finish);
      window.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true}));finish('cancelled text');await new Promise(r=>setTimeout(r,30));
      assert(!input().value.includes('cancelled'),'Cancelled inference changed the draft');
      // A normal short click still submits exactly once.
      pointer('pointerdown');pointer('pointerup');button().click();await wait(()=>chatRequests===1);
      await wait(()=>!button().disabled);
      root.querySelector('[aria-label="Open command palette"]').click();
      root.querySelector('[id="palette-command-local:dictation"]').click();
      assert(document.querySelector('.dictation-dialog[open]'),'Dictation settings missing');
      assert(document.querySelector('.dictation-dialog').textContent.includes('never uploaded'),'Local privacy explanation missing');
      if (${android}) assert(document.querySelector('.dictation-dialog').textContent.includes('Microphone permission'),'Android permission help missing');
      globalThis.dictationTestView=view;
    } finally {
      window.fetch=originalFetch;HTMLElement.prototype.setPointerCapture=pointerCapture;
      for(let i=0;i<names.length;i++){if(prior[i])Object.defineProperty(window,names[i],prior[i]);else delete window[names[i]];}
      if(preference===null)localStorage.removeItem('lethetic.dictation.v1');else localStorage.setItem('lethetic.dictation.v1',preference);
      if(!globalThis.dictationTestView)view?.destroy();
    }
  })()`);
  if (capture) await capture();
  await evaluate("globalThis.dictationTestView.destroy(); delete globalThis.dictationTestView;");
}
