import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";
import { createRuntime } from "../src/sdk.js";
import { LocalTransport } from "../src/local-transport.js";
import { parseServerMessage } from "../../web/src/protocol.js";

await test("discovered limits cap real WASM requests without replacing session budgets or history", async () => {
  const connection = {name:"Router",provider:"openai" as const,endpoint:"https://openrouter.ai/api/v1/chat/completions",model:"vendor/model",apiKey:"test-key"};
  (await createRuntime({connection,wasm:await readFile(new URL('../wasm/lethetic_wasm_bg.wasm',import.meta.url))})).dispose();
  const original = globalThis.fetch;
  let requests=0, modelContext=10000, modelOutput=2000;
  const values=new Map<string,string>();
  const storage={getItem:(key:string)=>values.get(key)??null,setItem:(key:string,value:string)=>{values.set(key,value)}} as Storage;
  const transport=new LocalTransport({socketOpened(){},authenticationEpochChanged(){},transportStatus(){},protocolFault(){},commandResponse(){},
    serverMessage(message){assert.ok(parseServerMessage(JSON.stringify(message)).ok)}},{connections:[connection],storage});
  try {
    globalThis.fetch=async(url,init)=>{
      if(String(url).endsWith('/models'))return Response.json({data:[{id:'vendor/model',context_length:modelContext,top_provider:{max_completion_tokens:modelOutput}}]});
      if(String(url).endsWith('/endpoints'))return Response.json({data:{endpoints:[]}});
      const body=JSON.parse(String(init?.body)); assert.equal(body.max_tokens,modelOutput); requests++;
      if(requests===2)assert.ok(body.messages.some((m:{content:string})=>m.content==='first answer'));
      return Response.json({choices:[{finish_reason:'stop',message:{role:'assistant',content:'first answer'}}],usage:{prompt_tokens:1200,completion_tokens:20,total_tokens:1220}});
    };
    transport.start(); const id=transport.state.session.session_id;
    await transport.modelLimits.ensure(connection);
    assert.equal(transport.effectiveTurnLimits.contextTokens,8000); assert.equal(transport.turnLimits.contextTokens,131072);
    for(let turn=0;turn<2;turn++){
      transport.send({type:'send_prompt',session_id:id,prompt:'hello'},0); await Promise.resolve();
      for(let n=0;transport.busy && n<100;n++)await new Promise(resolve=>setTimeout(resolve,5));
      assert.equal(transport.busy,false); assert.equal(transport.state.status.stop_reason,'Ready');
    }
    assert.equal(requests,2); assert.equal(transport.state.session.session_id,id);
    assert.equal(transport.state.status.context_tokens,'1200'); assert.equal(transport.state.status.context_limit_tokens,'8000');
    assert.equal(transport.state.status.context_source,'server_usage'); assert.equal(transport.state.status.request_usage?.total_tokens,'1220');
    transport.openSession(id); assert.equal(transport.metrics.context.tokens,1200);
    assert.equal(transport.turnLimits.contextTokens,131072);
    assert.equal(transport.turnLimits.autoOutput,true);
    modelContext=1048576;modelOutput=943718;
    await transport.refreshModelLimits();
    assert.equal(transport.effectiveTurnLimits.maxOutputTokens,943718);
    transport.send({type:'send_prompt',session_id:id,prompt:'continue'},0);await Promise.resolve();
    for(let n=0;transport.busy && n<100;n++)await new Promise(resolve=>setTimeout(resolve,5));
    assert.equal(transport.busy,false);assert.equal(transport.state.status.stop_reason,'Ready');
    assert.equal(requests,3);
    transport.openSession(id);assert.equal(transport.turnLimits.autoOutput,true);

  } finally {transport.dispose();globalThis.fetch=original;}
});
