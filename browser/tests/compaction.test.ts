import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";
import { createRuntime } from "../src/sdk.js";
import { summarizeContext } from "../src/compaction.js";
import { LocalTransport } from "../src/local-transport.js";
import { SessionStore } from "../src/sessions.js";
import { BrowserFiles } from "../src/files.js";

await test("compaction uses bounded passes, retains carried context and checks cancellation", async () => {
  const controller=new AbortController(), requests:string[]=[], progress:number[]=[];
  let clears=0;
  const summary=await summarizeContext({clear(){clears++}, async send(prompt){requests.push(prompt);return 'retained '+requests.length}},
    [{role:'user',content:'x'.repeat(100000)}],controller.signal,current=>progress.push(current));
  assert.equal(summary,'retained 3'); assert.equal(clears,3); assert.deepEqual(progress,[1,2,3]);
  assert.ok(requests[1]!.includes('retained 1')); assert.ok(requests.every(prompt=>prompt.length<49000));
  await assert.rejects(()=>summarizeContext({clear(){},async send(){return ''}},[{}],controller.signal,()=>{}),/empty/);
  controller.abort(); await assert.rejects(()=>summarizeContext({clear(){throw Error('should not run')},async send(){return ''}},[{}],controller.signal,()=>{}));
});

await test("compaction commits context in place, preserves files and transcript, and rolls back failures", async () => {
  const connection={name:'Test',provider:'openai' as const,endpoint:'https://example.com/v1/chat/completions',model:'test',apiKey:'secret'};
  const wasm=await readFile(new URL('../wasm/lethetic_wasm_bg.wasm',import.meta.url));
  (await createRuntime({connection,wasm})).dispose();
  const values=new Map<string,string>(); let quota=false;
  const storage={getItem:(key:string)=>values.get(key)??null,setItem:(key:string,value:string)=>{if(quota)throw Error('quota');values.set(key,value)}} as Storage;
  const id=crypto.randomUUID(), messages=[{role:'user',content:'remember requirement'},{role:'assistant',content:'remembered'}];
  new SessionStore(storage).save({id,name:'Work',label:'Work',updatedAt:new Date().toISOString(),blocks:[],messages,inputs:['remember requirement'],system:'custom system',python:true,theme:'dracula',connection});
  const files=new BrowserFiles(storage);
  const transport=new LocalTransport({socketOpened(){},authenticationEpochChanged(){},transportStatus(){},protocolFault(){},serverMessage(){},commandResponse(){}},{connections:[connection],storage,files});
  transport.start();files.save('keep.txt','preserved');
  const fetch=globalThis.fetch;
  try {
    globalThis.fetch=async (_url,init)=>{
      const body=JSON.parse(String(init?.body)); assert.ok(body.messages[0].content.includes('compact'));
      assert.ok(!body.tools?.length); assert.ok(body.messages[1].content.includes('remember requirement'));
      return Response.json({choices:[{finish_reason:'stop',message:{role:'assistant',content:'Keep the requirement and keep.txt.'}}]});
    };
    await transport.compact();
    assert.equal(transport.state.session.session_id,id); assert.equal(transport.pythonEnabled,true);
    assert.equal(files.list()[0]?.content,'preserved');
    const saved=new SessionStore(storage).list()[0]!;
    assert.equal(saved.messages.length,2); assert.ok(JSON.stringify(saved.messages).includes('Keep the requirement'));
    assert.equal(saved.blocks.at(-1)?.title,'Context compacted'); assert.equal(saved.system,'custom system');
    globalThis.fetch=async()=>new Response('',{status:500});
    await assert.rejects(()=>transport.compact(),/HTTP 500/);
    assert.deepEqual(new SessionStore(storage).list()[0]!.messages,saved.messages);
    globalThis.fetch=async()=>{quota=true;return Response.json({choices:[{finish_reason:'stop',message:{role:'assistant',content:'replacement'}}]})};
    await assert.rejects(()=>transport.compact(),/quota/); quota=false;
    assert.deepEqual(new SessionStore(storage).list()[0]!.messages,saved.messages);
    globalThis.fetch=async()=>{transport.send({type:'stop',session_id:transport.state.session.session_id,cancel_id:transport.state.activity.cancel_id!},0);await Promise.resolve();return Response.json({choices:[{finish_reason:'stop',message:{role:'assistant',content:'cancelled'}}]})};
    await assert.rejects(()=>transport.compact());
    assert.deepEqual(new SessionStore(storage).list()[0]!.messages,saved.messages);
    assert.equal(transport.busy,false);
  } finally {globalThis.fetch=fetch;transport.dispose()}
});
