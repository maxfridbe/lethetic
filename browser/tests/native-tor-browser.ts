/** Exercise the actual native-only Connections controls without public Tor availability. */
export async function testNativeTorControls(evaluate: (source: string) => Promise<unknown>): Promise<void> {
  await evaluate(`(async()=>{
    const {LocalTransport}=await import('./src/local-transport.js');
    const {createConnectionsDialog}=await import('./src/connections-dialog.js');
    const {browserHeaderStatus}=await import('./src/status-view.js');
    const prior=Object.getOwnPropertyDescriptor(window,'__LETHETIC_HOST_TOKEN__');
    Object.defineProperty(window,'__LETHETIC_HOST_TOKEN__',{configurable:true,value:'test-native'});
    const records=new Map(),storage={getItem:key=>records.get(key)??null,setItem:(key,value)=>records.set(key,value)};
    const events={socketOpened(){},authenticationEpochChanged(){},transportStatus(){},protocolFault(){},serverMessage(){},commandResponse(){}};
    const transport=new LocalTransport(events,{storage,connections:[],mode:'dotnet-wasm',nugetAccess:true});
    let editor;
    const check=(ok,message)=>{if(!ok)throw Error(message)};
    try {
      transport.start();const id=transport.state.session.session_id;
      editor=createConnectionsDialog(transport,storage);editor.open();
      const field=name=>editor.dialog.querySelector('[data-field="'+name+'"]');
      const policy=field('native-network-mode');check(policy && !policy.disabled,'Native network policy is unavailable');
      check(field('browser-network-route').hidden,'Executable exposed the browser relay controls');
      policy.value='tor';policy.dispatchEvent(new Event('change'));
      check(transport.networkMode==='tor' && transport.networkAccess,'Tor selection not applied');
      check(field('nuget').disabled && !field('nuget').checked,'Tor allowed direct NuGet downloads');
      check(field('permissions').textContent.includes('Tor only'),'Tor route not visible');
      check(transport.state.session.session_id===id,'Network change replaced session');
      check(browserHeaderStatus(transport,'Projects')==='App · WASM · .NET · TOR only · Directory: Projects','Native runtime/directory/Tor summary is incorrect');
      transport.setMode('python');
      check(browserHeaderStatus(transport,null)==='App · WASM · PY · TOR only · Virtual directory','Python/virtual status did not update');
      policy.value='off';policy.dispatchEvent(new Event('change'));check(!transport.networkAccess,'Off kept networking');
      policy.value='direct';policy.dispatchEvent(new Event('change'));check(transport.networkAccess && transport.networkMode==='direct','Explicit direct selection failed');
    } finally {
      editor?.destroy();transport.dispose();
      if(prior)Object.defineProperty(window,'__LETHETIC_HOST_TOKEN__',prior);else delete window.__LETHETIC_HOST_TOKEN__;
    }
  })()`);
}
