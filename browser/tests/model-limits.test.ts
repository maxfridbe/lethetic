import assert from "node:assert/strict";
import test from "node:test";
import { effectiveLimits, fetchModelLimits, isOpenRouter, ModelLimitState } from "../src/model-limits.js";
import { DEFAULT_TURN_LIMITS } from "../src/turn-limits.js";
const connection = { name: "Router", provider: "openai" as const, endpoint: "https://openrouter.ai/api/v1/chat/completions", model: "vendor/model", apiKey: "private-test-key" };
const catalog = { data: [{ id: "vendor/model", context_length: 10000, top_provider: {max_completion_tokens: 2000} }] };

await test("OpenRouter model and provider limits use authenticated canonical endpoints", async () => {
  const urls: string[] = [];
  const result = await fetchModelLimits(connection, new AbortController().signal, async (url, options) => {
    urls.push(String(url)); assert.equal(new Headers(options?.headers).get("Authorization"), "Bearer private-test-key");
    assert.equal(options?.redirect,"error"); assert.equal(options?.credentials,"omit");
    return Response.json(urls.length === 1 ? catalog : { data: { endpoints: [{name:"Provider A",context_length:8000,max_prompt_tokens:7000,max_completion_tokens:1000}] } });
  });
  assert.deepEqual(urls,["https://openrouter.ai/api/v1/models","https://openrouter.ai/api/v1/models/vendor/model/endpoints"]);
  assert.equal(result.context,10000); assert.equal(result.output,2000); assert.equal(result.endpoints[0]?.context,8000);
  const limits=effectiveLimits(DEFAULT_TURN_LIMITS,result);
  assert.equal(limits.contextTokens,7000); assert.equal(limits.maxOutputTokens,1000);
  assert.equal(DEFAULT_TURN_LIMITS.contextTokens,131072);
  assert.deepEqual(effectiveLimits(DEFAULT_TURN_LIMITS,null),DEFAULT_TURN_LIMITS);
});
await test("invalid and missing limits stay unknown; provider lookup failure retains catalog", async () => {
  let calls=0;
  const value=await fetchModelLimits(connection,new AbortController().signal,async()=> ++calls===1
    ? Response.json({data:[{id:"vendor/model",context_length:-2,top_provider:{max_completion_tokens:"4096"}}]}) : new Response('',{status:404}));
  assert.equal(value.context,null); assert.equal(value.output,null); assert.deepEqual(value.endpoints,[]);
  assert.ok(!isOpenRouter({...connection,endpoint:"https://openrouter.ai.evil.test/api/v1/chat/completions"}));
  await assert.rejects(fetchModelLimits({...connection,apiKey:"Bearer key"},new AbortController().signal),/only the API key/);
  await assert.rejects(fetchModelLimits(connection,new AbortController().signal,async()=>Response.json({data:[]})),/not in/);
});
await test("metadata cache ignores a stale connection response, survives failure, and refreshes", async () => {
  let resolve!: (response: Response) => void, calls=0, changes=0;
  const state = new ModelLimitState(()=>changes++,async()=>{calls++; return new Promise<Response>(done=>{resolve=done})});
  const pending=state.ensure(connection); await Promise.resolve();
  void state.ensure(connection); assert.equal(calls,1);
  await state.ensure({...connection,endpoint:"https://example.com/v1/chat/completions"});
  resolve(Response.json(catalog)); await pending;
  assert.equal(state.value,null); assert.equal(changes,0); state.dispose();
  const failing=new ModelLimitState(()=>changes++,async()=>new Response('',{status:503}));
  await failing.ensure(connection); assert.ok(failing.status.includes('unavailable'));
  assert.equal(failing.value,null); await failing.ensure(connection,true); assert.equal(changes,2); failing.dispose();
});

await test("stopping a turn does not wait for stalled model discovery", async () => {
  const state = new ModelLimitState(()=>{},(_url,init)=>new Promise<Response>((_resolve,reject)=>init?.signal?.addEventListener("abort",()=>reject(Error("cancelled")))));
  const controller=new AbortController(); const pending=state.ready(connection,controller.signal); controller.abort();
  await assert.rejects(pending); state.dispose();
});
