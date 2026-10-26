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
    const {TraceCapture,ProviderTraceLog,parseProviderTraces}=await import('./src/provider-trace.js');
    const trace=new TraceCapture({turnId:'failed-turn',round:1,attempt:1,provider:'openai',model:'test'},'{}',[]);
    trace.chunk('data: invalid-tool-arguments',28);trace.finish('error','Invalid arguments');
    const traces=new ProviderTraceLog();traces.add(trace.value);
    const large={...original,providerTrace:traces.snapshot(),label:'Large',workspace:{version:1,directories:['empty'],files:[{path:'large.bin',data:encodeBytes(bytes),modifiedAt:456}]}};
    store.save(large); check(disk.pending,'Save claimed durable before transaction');
    store.save({...large,label:'Latest'}); await disk.flush();
    check(!disk.pending && disk.status.includes('saved'),'Save did not commit');
    disk.close(); disk=await openBrowserStorage(prefs,name);store=new SessionStore(disk);
    check(store.list()[0].label==='Latest','Queued latest save lost');
    check(JSON.stringify(store.list()[0].providerTrace)===JSON.stringify(parseProviderTraces(traces.snapshot())),'Reload lost failed trace');
    const restored=store.list()[0].workspace.files[0];
    check(restored.modifiedAt===456 && restored.data===large.workspace.files[0].data,'Large binary bytes/date changed on reopen');
    const rawDb=await new Promise((resolve,reject)=>{const r=indexedDB.open(name);r.onsuccess=()=>resolve(r.result);r.onerror=()=>reject(r.error)});
    const raw=await new Promise((resolve,reject)=>{const tx=rawDb.transaction('sessions'),r=tx.objectStore('sessions').get('session:'+original.id);tx.oncomplete=()=>resolve(r.result);tx.onabort=()=>reject(tx.error)});
    check(raw.database instanceof Uint8Array && new TextDecoder().decode(raw.database.slice(0,16))==='SQLite format 3\\0','Not a SQLite database image');
    const {contentDatabaseFactory}=await import('./src/sdk.js');
    const Database=await contentDatabaseFactory(), database=new Database(raw.database);
    const stored=database.read('browser/files/'+original.id,'large.bin');
    check(stored.length===FILE_BYTES && stored.every((byte,i)=>byte===bytes[i]),'Database changed binary file bytes');
    check(JSON.parse(database.query('SELECT outcome,response_head FROM provider_traces','[]')).rows[0][1]==='data: invalid-tool-arguments','Trace not normalized');
    database.free();rawDb.close();
    check(new TextDecoder().decode(disk.exportDatabase(original.id).slice(0,16))==='SQLite format 3\\0','Export is not SQLite');
    // A second session owns a separate image. Updating/deleting it never writes
    // or removes the first session's database.
    const second={...original,id:crypto.randomUUID(),label:'Second'};
    store.save(second);await disk.flush();
    const touched=[],observePut=IDBObjectStore.prototype.put;
    IDBObjectStore.prototype.put=function(value,key){touched.push(key);return observePut.call(this,value,key)};
    try{store.save({...second,label:'Changed second'});await disk.flush()}finally{IDBObjectStore.prototype.put=observePut}
    check(touched.includes('session:'+second.id) && !touched.includes('session:'+original.id),'Saving one session rewrote another database');
    const secondDb=new Database(disk.exportDatabase(second.id));
    check(JSON.parse(secondDb.names('browser/sessions')).join(',')===second.id,'Session database contains another chat');
    check(JSON.parse(secondDb.query('SELECT count(*) FROM provider_traces','[]')).rows[0][0]===0,'Session leaked another chat trace');secondDb.free();
    store.remove(second.id);await disk.flush();
    const afterDelete=await openBrowserStorage(prefs,name);
    check(new SessionStore(afterDelete).list().length===1 && new SessionStore(afterDelete).list()[0].id===original.id,'Deleting one database removed another');afterDelete.close();
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
    // Upgrade the previous binary IndexedDB envelope without changing its session.
    const legacyName=name+'-binary';
    const legacyDb=await new Promise((resolve,reject)=>{const r=indexedDB.open(legacyName,1);r.onupgradeneeded=()=>r.result.createObjectStore('sessions');r.onsuccess=()=>resolve(r.result);r.onerror=()=>reject(r.error)});
    await new Promise((resolve,reject)=>{const tx=legacyDb.transaction('sessions','readwrite');tx.objectStore('sessions').put({revision:4,sessions:[{...original,workspace:{...original.workspace,files:[{path:'old.bin',data:new Uint8Array([0,255]),modifiedAt:123}]}}]},'all');tx.oncomplete=resolve;tx.onabort=()=>reject(tx.error)});legacyDb.close();
    const upgraded=await openBrowserStorage(memory(),legacyName);
    check(new SessionStore(upgraded).list()[0].workspace.files[0].data==='AP8=','Binary IndexedDB migration changed bytes');upgraded.close();
    const aggregate=new Database(new Uint8Array());
    for(const session of [original,second]) {
      aggregate.put('browser/sessions',session.id,new TextEncoder().encode(JSON.stringify({...session,workspace:{...session.workspace,files:[{path:'old.bin',modifiedAt:123}]}})));
      aggregate.put('browser/files/'+session.id,'old.bin',new Uint8Array([0,255]));
    }
    const aggregateImage=aggregate.export();aggregate.free();
    const aggregateName=name+'-aggregate',aggregateDb=await new Promise((resolve,reject)=>{const r=indexedDB.open(aggregateName,1);r.onupgradeneeded=()=>r.result.createObjectStore('sessions');r.onsuccess=()=>resolve(r.result);r.onerror=()=>reject(r.error)});
    await new Promise((resolve,reject)=>{const tx=aggregateDb.transaction('sessions','readwrite');tx.objectStore('sessions').put({revision:7,database:aggregateImage},'all');tx.oncomplete=resolve;tx.onabort=()=>reject(tx.error)});aggregateDb.close();
    const split=await openBrowserStorage(memory(),aggregateName);
    check(new SessionStore(split).list().length===2,'Aggregate migration lost a session');
    for(const id of [original.id,second.id]){const db=new Database(split.exportDatabase(id));check(JSON.parse(db.names('browser/sessions')).join(',')===id,'Aggregate database was not split');db.free()}split.close();
    // Both older schemas upgrade transactionally in IndexedDB.
    for (const schemaVersion of [1,2,3]) {
    const v1=new Database(new Uint8Array());
    if(schemaVersion===1) {
    v1.query('PRAGMA foreign_keys=OFF','[]');
    for(const [table] of JSON.parse(v1.query("SELECT name FROM sqlite_schema WHERE type='view'",'[]')).rows)v1.query('DROP VIEW '+table,'[]');
    for(const [table] of JSON.parse(v1.query("SELECT name FROM sqlite_schema WHERE type='table'",'[]')).rows)v1.query('DROP TABLE '+table,'[]');
    v1.query('CREATE TABLE content_records(scope TEXT,name TEXT,ordinal INTEGER,content BLOB,PRIMARY KEY(scope,name,ordinal))','[]');
    v1.query('PRAGMA user_version=1','[]');
    const metadata={...original,workspace:{...original.workspace,files:[{path:'old.bin',modifiedAt:123}]}};
    v1.query("INSERT INTO content_records VALUES('browser/sessions',?,0,CAST(? AS BLOB))",JSON.stringify([original.id,JSON.stringify(metadata)]));
    v1.query("INSERT INTO content_records VALUES(?,'old.bin',0,X'00ff')",JSON.stringify(['browser/files/'+original.id]));
    } else {
      v1.put('browser/sessions',original.id,new TextEncoder().encode(JSON.stringify({...original,workspace:{...original.workspace,files:[{path:'old.bin',modifiedAt:123}]}})));
      v1.put('browser/files/'+original.id,'old.bin',new Uint8Array([0,255]));
      if(schemaVersion===2)v1.query('DROP TABLE provider_traces','[]');v1.query('PRAGMA user_version='+schemaVersion,'[]');
    }
    const v1image=v1.export();v1.free();
    const v1name=name+'-v'+schemaVersion,v1db=await new Promise((resolve,reject)=>{const r=indexedDB.open(v1name,1);r.onupgradeneeded=()=>r.result.createObjectStore('sessions');r.onsuccess=()=>resolve(r.result);r.onerror=()=>reject(r.error)});
    await new Promise((resolve,reject)=>{const tx=v1db.transaction('sessions','readwrite'),store=tx.objectStore('sessions');store.put({revision:1,ids:[original.id]},'all');store.put({sessionId:original.id,database:v1image},'session:'+original.id);tx.oncomplete=resolve;tx.onabort=()=>reject(tx.error)});
    const normalized=await openBrowserStorage(memory(),v1name);
    check(new SessionStore(normalized).list()[0].workspace.files[0].data==='AP8=','Relational migration changed file bytes');normalized.close();
    const persisted=await new Promise((resolve,reject)=>{const tx=v1db.transaction('sessions'),r=tx.objectStore('sessions').get('session:'+original.id);tx.oncomplete=()=>resolve(r.result.database);tx.onabort=()=>reject(tx.error)});v1db.close();
    check(new DataView(persisted.buffer,persisted.byteOffset,persisted.byteLength).getUint32(60)===4,'Relational migration was not persisted');
    const relational=new Database(persisted);
    check(JSON.parse(relational.query('SELECT session_id,system_prompt FROM sessions','[]')).rows[0][1]==='keep','Session columns missing after migration');
    check(JSON.parse(relational.query('SELECT path,size_bytes FROM session_files','[]')).rows[0][1]===2,'Normalized file query failed');relational.free();
    }
    const migrationPrefs=memory();new SessionStore(migrationPrefs).save(original);
    const savedPut=IDBObjectStore.prototype.put;
    IDBObjectStore.prototype.put=function(){throw new DOMException('Migration quota exceeded','QuotaExceededError')};
    let migrationFailed=false;
    try{await openBrowserStorage(migrationPrefs,name+'-quota-migration')}catch{migrationFailed=true}
    finally{IDBObjectStore.prototype.put=savedPut}
    check(migrationFailed && migrationPrefs.getItem(SESSION_KEY)!==null,'Failed migration removed its source');
    const retried=await openBrowserStorage(migrationPrefs,name+'-quota-migration');
    check(new SessionStore(retried).list()[0].id===original.id && migrationPrefs.getItem(SESSION_KEY)===null,'Migration retry lost data');retried.close();
    const broken=memory();broken.setItem(SESSION_KEY,'broken');let bad=false;
    try{await openBrowserStorage(broken,name+'-broken')}catch{bad=true}
    check(bad && broken.getItem(SESSION_KEY)==='broken','Bad migration replaced existing data');
  })()`);
}
