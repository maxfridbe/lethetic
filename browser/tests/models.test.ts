import assert from "node:assert/strict";
import test from "node:test";
import { fetchModels } from "../src/models.js";

const signal = new AbortController().signal;
await test("model discovery preserves proxy prefixes, uses draft credentials, and normalizes models", async () => {
  const models = await fetchModels({provider:"openai",endpoint:"https://example.com/proxy/v1/chat/completions",apiKey:"memory-key"}, signal,
    async (url, options) => {
      assert.equal(url,"https://example.com/proxy/v1/models");
      assert.equal(new Headers(options?.headers).get("Authorization"),"Bearer memory-key");
      assert.equal(options?.credentials,"omit"); assert.equal(options?.redirect,"error");
      assert.equal(options?.signal,signal);
      return Response.json({data:[{id:"z"},{id:"a",name:"Alpha"},{id:"z"},{id:""},null,{}]});
    });
  assert.deepEqual(models,[{id:"a",name:"Alpha"},{id:"z",name:"z"}]);
});
await test("Anthropic discovery authenticates and traverses model pages", async () => {
  let calls = 0;
  const models = await fetchModels({provider:"anthropic",endpoint:"https://example.com/gateway/v1/messages",apiKey:"key"}, signal,
    async (url, options) => {
      const headers = new Headers(options?.headers); const parsed = new URL(String(url));
      assert.equal(parsed.pathname,"/gateway/v1/models");
      assert.equal(headers.get("x-api-key"),"key"); assert.equal(headers.get("Authorization"),null);
      assert.equal(headers.get("anthropic-version"),"2023-06-01");
      assert.equal(headers.get("anthropic-dangerous-direct-browser-access"),"true");
      if (calls++ === 0) {
        assert.equal(parsed.searchParams.get("after_id"),null);
        return Response.json({data:[{id:"one",display_name:"First"}],has_more:true,last_id:"one"});
      }
      assert.equal(parsed.searchParams.get("after_id"),"one");
      return Response.json({data:[{id:"two",display_name:"Second"}],has_more:false});
    });
  assert.equal(calls,2); assert.deepEqual(models,[{id:"one",name:"First"},{id:"two",name:"Second"}]);
});
await test("discovery reports HTTP/format errors, rejects unsafe URLs, and supports cancellation", async () => {
  const connection = {provider:"openai" as const,endpoint:"https://example.com/v1/chat/completions"};
  await assert.rejects(fetchModels(connection,signal,async()=>new Response("secret",{status:401})),/HTTP 401/);
  await assert.rejects(fetchModels(connection,signal,async()=>Response.json({error:"bad"})),/did not return a model list/);
  await assert.rejects(fetchModels({...connection,endpoint:"http://example.com/v1/chat/completions"},signal),/HTTPS/);
  await assert.rejects(fetchModels({...connection,endpoint:"https://user:pass@example.com/v1/chat/completions"},signal),/credentials/);
  await assert.rejects(fetchModels({...connection,endpoint:"https://example.com/custom"},signal),/full chat/);
  const controller = new AbortController(); controller.abort();
  await assert.rejects(fetchModels(connection,controller.signal,async(_url,options)=>{options?.signal?.throwIfAborted(); return Response.json({data:[]});}),/abort/i);
  await assert.rejects(fetchModels({...connection,provider:"anthropic",endpoint:"https://example.com/v1/messages"},signal,
    async()=>Response.json({data:[],has_more:true,last_id:"same"})),/pagination/);
});

await test("model picker retains advertised context and output limits", async () => {
  const models = await fetchModels({provider:"openai",endpoint:"https://openrouter.ai/api/v1/chat/completions"},signal,
    async()=>Response.json({data:[{id:"vendor/model",context_length:100000,top_provider:{max_completion_tokens:8192}},{id:"bad",context_length:"unknown",top_provider:{max_completion_tokens:-1}}]}));
  assert.deepEqual(models,[{id:"bad",name:"bad"},{id:"vendor/model",name:"vendor/model",contextTokens:100000,maxOutputTokens:8192}]);
});
