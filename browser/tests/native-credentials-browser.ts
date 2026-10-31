/** Real voice controls + native credential protocol, recreated with fresh in-memory state. */
export async function testNativeCredentialRestart(evaluate: (source: string) => Promise<unknown>): Promise<void> {
  await evaluate(`(async()=>{
    const {openNativeCredentials,flushCredentials}=await import('./web/src/native-credentials.js');
    const {PersonaStore}=await import('./web/src/personas/store.js');
    const {PersonaSpeechControls}=await import('./web/src/personas/speech-controls.js');
    const {saveConnectionKeys,saveConnections,loadConnections,restoreConnectionKeys}=await import('./src/connection.js');
    const names=['__LETHETIC_ANDROID__','__LETHETIC_HOST_TOKEN__','__TAURI__'];const previous=names.map(n=>globalThis[n]);
    const disk={},preferences=new Map();let sent,controls;
    const store={getItem:k=>preferences.get(k)??null,setItem:(k,v)=>preferences.set(k,v)};
    const check=(ok,message)=>{if(!ok)throw Error(message)};
    const wait=async check=>{for(let i=0;i<200;i++){if(check())return;await new Promise(r=>setTimeout(r,10))}throw Error('Credential restart condition timed out')};
    globalThis.__LETHETIC_ANDROID__=true;globalThis.__LETHETIC_HOST_TOKEN__='vault-test';
    globalThis.__TAURI__={core:{invoke:async(command,args)=>{
      check(args.token==='vault-test','Vault lacked main-window authorization');
      if(command.endsWith('|load_credentials'))return {...disk};
      if(command.endsWith('|save_credentials')){if(args.value===null)delete disk[args.key];else disk[args.key]=args.value;return {};}
      if(command.endsWith('|response_audio')){if(args.operation==='start')sent=args.payload;return {active:false};}
      throw Error('Unexpected native command');
    }}};
    const root=document.createElement('div');document.body.append(root);
    const field=name=>root.querySelector('[data-speech="'+name+'"]');
    const mount=()=>{controls=new PersonaSpeechControls(new PersonaStore(store),()=>{});controls.mount(root)};
    try {
      let vault=await openNativeCredentials();
      const connection={name:'Saved account',provider:'openai',endpoint:'https://models.example/v1/chat/completions',model:'test',apiKey:'saved-model-key'};
      saveConnections(store,[connection]);saveConnectionKeys(vault,[connection]);await flushCredentials(vault);
      mount();field('enabled').checked=true;field('endpoint').value='http://brainiac-nvidia:8000/v1/audio/speech';field('key').value='saved-voice-key';field('save').click();
      await wait(()=>field('status').textContent.includes('Voice saved'));
      check(field('key-status').textContent.includes('API key present'),'Saved voice key missing before restart');
      controls.dispose();root.replaceChildren();
      vault=await openNativeCredentials();
      check(restoreConnectionKeys(vault,loadConnections(store))[0].apiKey==='saved-model-key','Model key lost on restart');
      mount();check(field('key').value==='','Restored secret exposed in the input');
      check(field('key-status').textContent.includes('API key present'),'Voice key lost on restart');
      check(field('key').placeholder.includes('Saved key present'),'Blank field did not explain saved key');
      field('test').click();await wait(()=>!!sent);
      check(sent.requests[0].headers.Authorization==='Bearer saved-voice-key','Restored voice key not sent to native playback');
      check(![...preferences.values()].join('').includes('saved-voice-key'),'Voice key leaked to preferences');
      check(![...preferences.values()].join('').includes('saved-model-key'),'Model key leaked to preferences');
      field('clear').click();await flushCredentials(vault);controls.dispose();root.replaceChildren();
      await openNativeCredentials();mount();check(field('key-status').textContent.includes('No API key'),'Cleared key returned after restart');
    } finally {
      controls?.dispose();root.remove();
      names.forEach((name,i)=>{if(previous[i]===undefined)delete globalThis[name];else globalThis[name]=previous[i]});
    }
  })()`);
}
