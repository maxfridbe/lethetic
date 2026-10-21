/** Real IndexedDB transactions, migration, binary round trips and failed saves. */
export async function testSessionStorage(evaluate: (expression: string) => Promise<unknown>): Promise<void> {
  await evaluate(`(async () => {
    const {openBrowserStorage}=await import('./src/session-storage.js');
    const {SessionStore,SESSION_KEY}=await import('./src/sessions.js');
    const {encodeBytes,decodeBytes,FILE_BYTES,emptyWorkspace}=await import('./src/workspace.js');
    const {BrowserFiles}=await import('./src/files.js');
    const {FileClient}=await import('./web/src/files/client.js');
    const check=(ok,message)=>{if(!ok)throw Error(message)};
    const memory=()=>{const values=new Map();return {get length(){return values.size},key:i=>[...values.keys()][i]??null,getItem:k=>values.get(k)??null,setItem:(k,v)=>values.set(k,v),removeItem:k=>values.delete(k),clear:()=>values.clear()}};
    const prefs=memory(),name='test-sessions-'+crypto.randomUUID();
    const original={id:crypto.randomUUID(),name:null,label:'Migrated',updatedAt:new Date().toISOString(),blocks:[],messages:[],inputs:[],system:'keep',connection:null,python:true,theme:'dracula',workspace:{version:1,directories:['empty'],files:[{path:'old.bin',data:'AP8=',modifiedAt:123}]}};
    new SessionStore(prefs).save(original);
    let disk=await openBrowserStorage(prefs,name),store=new SessionStore(disk);
    check(prefs.getItem(SESSION_KEY)===null,'Legacy data not removed after migration');
    check(JSON.stringify(store.list()[0])===JSON.stringify(new SessionStore({getItem:()=>JSON.stringify({version:1,sessions:[original]}),setItem(){}}).list()[0]),'Migration changed session');
    const bytes=new Uint8Array(FILE_BYTES);for(let i=0;i<bytes.length;i++)bytes[i]=i%256;
    const large={...original,label:'Large',workspace:{version:1,directories:['empty'],files:[{path:'large.bin',data:encodeBytes(bytes),modifiedAt:456}]}};
    store.save(large); check(disk.pending,'Save claimed durable before transaction');
    store.save({...large,label:'Latest'}); await disk.flush();
    check(!disk.pending && disk.status.includes('saved'),'Save did not commit');
    disk.close(); disk=await openBrowserStorage(prefs,name);store=new SessionStore(disk);
    check(store.list()[0].label==='Latest','Queued latest save lost');
    const restored=store.list()[0].workspace.files[0];
    check(restored.modifiedAt===456 && restored.data===large.workspace.files[0].data,'Large binary bytes/date changed on reopen');
    const rawDb=await new Promise((resolve,reject)=>{const r=indexedDB.open(name);r.onsuccess=()=>resolve(r.result);r.onerror=()=>reject(r.error)});
    const raw=await new Promise((resolve,reject)=>{const tx=rawDb.transaction('sessions'),r=tx.objectStore('sessions').get('all');tx.oncomplete=()=>resolve(r.result);tx.onabort=()=>reject(tx.error)});
    check(raw.sessions[0].workspace.files[0].data instanceof Uint8Array,'Database stored encoded text instead of binary');
    check(raw.sessions[0].workspace.files[0].data.length===FILE_BYTES,'Binary storage has encoding overhead');rawDb.close();
    let workspace=store.list()[0].workspace;
    const files=new BrowserFiles();files.bindWorkspace(()=>structuredClone(workspace),value=>{workspace=value});
    const download=new Uint8Array(await (await files.request('/api/files/download','large.bin',new AbortController().signal)).arrayBuffer());
    check(download.length===bytes.length && download.every((byte,i)=>byte===bytes[i]),'Large file download changed bytes');
    files.save('large.txt','x'.repeat(3*1024*1024));
    const client=new FileClient((endpoint,path,signal)=>files.request(endpoint,path,signal),FILE_BYTES);
    check((await client.read('large.txt',new AbortController().signal)).content.length===3*1024*1024,'Browser text viewer retained 2 MiB limit');
    const defaultClient=new FileClient((endpoint,path,signal)=>files.request(endpoint,path,signal));
    let rejected=false;try{await defaultClient.read('large.txt',new AbortController().signal)}catch{rejected=true}check(rejected,'Native text viewer limit changed');
    // Simulate a synchronous quota failure inside the transaction callback.
    const put=IDBObjectStore.prototype.put;
    IDBObjectStore.prototype.put=function(){throw new DOMException('Test quota exceeded','QuotaExceededError')};
    try {
      store.save({...large,label:'Unsaved quota edit'});
      let failed=false;try{await disk.flush()}catch{failed=true}
      check(failed && disk.pending && disk.status.includes('not saved'),'Failed commit was reported as durable');
    } finally {IDBObjectStore.prototype.put=put}
    const old=await openBrowserStorage(prefs,name);
    check(new SessionStore(old).list()[0].label==='Latest','Aborted transaction replaced previous save');old.close();
    await disk.flush();check(!disk.pending,'Retry did not save pending edit');
    // Separate tab cannot overwrite a later revision with its stale cache.
    const stale=await openBrowserStorage(prefs,name);
    store.save({...large,label:'Winner'});await disk.flush();
    new SessionStore(stale).save({...large,label:'Stale'});
    let conflict=false;try{await stale.flush()}catch{conflict=true}
    check(conflict && stale.status.includes('Another tab'),'Cross-tab overwrite was not prevented');
    const verify=await openBrowserStorage(prefs,name);
    check(new SessionStore(verify).list()[0].label==='Winner','Conflicting save overwrote current data');verify.close();
    // Allow the stale connection to close by discarding only its in-memory edits.
    // This test connection intentionally remains open until the fixture navigates away.
    store.clear();await disk.flush();disk.close();
    // An old client may write a legacy record after migration; never erase that unrelated data.
    new SessionStore(prefs).save(original);
    const empty=await openBrowserStorage(prefs,name);check(new SessionStore(empty).list().length===0,'Clear was not persisted');empty.close();
    check(prefs.getItem(SESSION_KEY)!==null,'Existing database erased a later legacy save');
    const broken=memory();broken.setItem(SESSION_KEY,'broken');let bad=false;
    try{await openBrowserStorage(broken,name+'-broken')}catch{bad=true}
    check(bad && broken.getItem(SESSION_KEY)==='broken','Bad migration replaced existing data');
  })()`);
}
