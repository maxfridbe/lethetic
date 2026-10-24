import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";
import { createRuntime } from "../src/sdk.js";
import type { RuntimeEvent } from "../src/sdk.js";
const wasm = await readFile(new URL("../wasm/lethetic_wasm_bg.wasm", import.meta.url));
const connection = {name:"Stream",provider:"openai" as const,endpoint:"https://example.com/v1/chat/completions",model:"test"};
const frame = (value: unknown) => `data: ${typeof value === "string" ? value : JSON.stringify(value)}\r\n\r\n`;
const chunk = (delta: unknown, finish_reason: string | null = null) => ({choices:[{index:0,delta,finish_reason}]});
const headers = {"Content-Type":"text/event-stream; charset=utf-8"};
const encoder = new TextEncoder();
const sse = (source: string) => new Response(new ReadableStream<Uint8Array>({start(controller) {
  // Split every UTF-8 sequence and CRLF delimiter across reads.
  for (const byte of encoder.encode(source)) controller.enqueue(new Uint8Array([byte])); controller.close();
}}), {headers});
const wait = async (check: () => boolean) => { for(let i=0;i<200;i++){if(check())return;await new Promise(resolve=>setTimeout(resolve,2));}throw Error("Live delta not received"); };

await test("CLI WASM parser surfaces thinking and text before EOF and preserves trailing usage", async () => {
  const events: RuntimeEvent[]=[]; let controller!: ReadableStreamDefaultController<Uint8Array>;
  const runtime = await createRuntime({connection,wasm,providerRetries:0,onEvent:event=>events.push(event),fetch:async()=>new Response(
    new ReadableStream<Uint8Array>({start(value){controller=value;}}),{headers})});
  try {
    const sending=runtime.send("hi"); await wait(()=>!!controller);
    controller.enqueue(encoder.encode(frame(chunk({reasoning:"Consider 🧠"}))));
    await wait(()=>events.some(event=>event.type==="thinking_delta"));
    controller.enqueue(encoder.encode(frame(chunk({content:"Hello 🌍"}))));
    await wait(()=>events.some(event=>event.type==="assistant_delta"));
    assert.equal(runtime.busy,true);
    controller.enqueue(encoder.encode(frame(chunk({},"stop"))+frame({choices:[],usage:{prompt_tokens:9,completion_tokens:7,completion_tokens_details:{reasoning_tokens:4}}})+frame("[DONE]")));
    assert.equal(await sending,"Hello 🌍");
    const usage=events.find(event=>event.type==="response");assert.ok(usage);assert.equal(usage.usage.reasoning,4);
    assert.equal(events.filter(event=>event.type==="assistant").length,1);
  } finally {runtime.dispose();}
});

await test("fragmented UTF-8, comments, tool arguments and signed reasoning survive a streamed tool round", async () => {
  let requests=0, executions=0;
  const runtime=await createRuntime({connection:{...connection,thinking:"enabled"},wasm,providerRetries:0,
    tools:[{name:"lookup",description:"lookup",parameters:{type:"object"},execute:args=>{assert.equal(args["value"],"é");executions++;return "42";}}],
    fetch:async(_url,init)=>{
      const body=JSON.parse(String(init?.body));assert.equal(body.stream,true);requests++;
      if(requests===1)return sse(": keepalive\r\n\r\n"+frame(chunk({reasoning_details:[{type:"reasoning.text",index:0,text:"Look ",signature:"sig"}]}))+
        frame(chunk({reasoning_details:[{type:"reasoning.text",index:0,text:"up",signature:"ned"}]}))+
        frame(chunk({tool_calls:[{index:0,id:"one",type:"function",function:{name:"lookup",arguments:'{"value":'}}]}))+
        frame(chunk({tool_calls:[{index:0,function:{arguments:'"é"}'}}]},"tool_calls"))+frame("[DONE]"));
      assert.equal(body.messages[2].reasoning_details[0].text,"Look up");
      assert.equal(body.messages[2].reasoning_details[0].signature,"signed");
      return sse(frame(chunk({content:"42"},"stop"))+frame("[DONE]"));
    }});
  try {assert.equal(await runtime.send("lookup"),"42");assert.equal(executions,1);assert.equal(requests,2);}finally{runtime.dispose();}
});

await test("incomplete tool streams never authorize tools or commit history", async () => {
  let executions=0;
  const runtime=await createRuntime({connection,wasm,providerRetries:0,tools:[{name:"lookup",description:"lookup",parameters:{type:"object"},execute:()=>{executions++;return "bad";}}],
    fetch:async()=>sse(frame(chunk({tool_calls:[{index:0,id:"one",type:"function",function:{name:"lookup",arguments:"{}"}}]},"tool_calls"))+
      frame({choices:[],usage:{prompt_tokens:1,completion_tokens:1}}))});
  try {await assert.rejects(runtime.send("go"),/terminal marker/);assert.equal(executions,0);assert.deepEqual(runtime.history(),[]);}finally{runtime.dispose();}
});

await test("Anthropic uses the CLI parser for thinking signatures, text and usage", async () => {
  const events:RuntimeEvent[]=[];
  const runtime=await createRuntime({connection:{...connection,provider:"anthropic"},wasm,providerRetries:0,onEvent:event=>events.push(event),fetch:async()=>sse([
    {type:"message_start",message:{role:"assistant",content:[],usage:{input_tokens:10,output_tokens:1}}},
    {type:"content_block_start",index:0,content_block:{type:"thinking",thinking:"",signature:""}},
    {type:"content_block_delta",index:0,delta:{type:"thinking_delta",thinking:"Plan"}},
    {type:"content_block_delta",index:0,delta:{type:"signature_delta",signature:"signed"}},
    {type:"content_block_stop",index:0},
    {type:"content_block_start",index:1,content_block:{type:"text",text:""}},
    {type:"content_block_delta",index:1,delta:{type:"text_delta",text:"Answer"}},
    {type:"content_block_stop",index:1},
    {type:"message_delta",delta:{stop_reason:"end_turn"},usage:{output_tokens:5}},
    {type:"message_stop"},
  ].map(frame).join(""))});
  try {assert.equal(await runtime.send("hi"),"Answer");assert.ok(events.some(event=>event.type==="thinking_delta" && event.text==="Plan"));
    assert.ok(JSON.stringify(runtime.history()).includes('"signature":"signed"'));
    const usage=events.find(event=>event.type==="response");assert.ok(usage);assert.equal(usage.usage.input,10);assert.equal(usage.usage.output,5);
  }finally{runtime.dispose();}
});

await test("rate-limit retries resend only the current model request, never completed tools", async()=>{
  let requests=0,executions=0;
  const runtime=await createRuntime({connection,wasm,tools:[{name:"lookup",description:"lookup",parameters:{type:"object"},execute:()=>{executions++;return "42";}}],fetch:async()=>{
    requests++;
    if(requests===1)return sse(frame(chunk({tool_calls:[{index:0,id:"one",type:"function",function:{name:"lookup",arguments:"{}"}}]},"tool_calls"))+frame("[DONE]"));
    if(requests===2)return new Response("busy",{status:429,headers:{"Retry-After":"0"}});
    return sse(frame(chunk({content:"done"},"stop"))+frame("[DONE]"));
  }});
  try {assert.equal(await runtime.send("go"),"done");assert.equal(executions,1);assert.equal(requests,3);}finally{runtime.dispose();}
});

await test("Stop cancels an unlimited live stream and discards partial history", async()=>{
  const events:RuntimeEvent[]=[];let cancelled=false;
  const runtime=await createRuntime({connection,wasm,timeoutMs:0,onEvent:event=>events.push(event),fetch:async()=>new Response(new ReadableStream({
    start(controller){controller.enqueue(encoder.encode(frame(chunk({content:"partial"}))));},cancel(){cancelled=true;},
  }),{headers})});
  try {const sending=runtime.send("go");await wait(()=>events.some(event=>event.type==="assistant_delta"));runtime.cancel();
    await assert.rejects(sending,/stopped/);assert.equal(cancelled,true);assert.deepEqual(runtime.history(),[]);
    assert.ok(events.some(event=>event.type==="response_reset"));
  }finally{runtime.dispose();}
});

await test("malformed streamed tool arguments are captured before parsing and survive rollback", async()=>{
  const traces: import('../src/provider-trace.js').ProviderTrace[]=[];let executions=0;
  const payload=frame(chunk({tool_calls:[{index:0,id:"bad",type:"function",function:{name:"lookup",arguments:'{"code":broken trace-secret'}}]},"tool_calls"))+frame("[DONE]");
  const runtime=await createRuntime({connection:{...connection,apiKey:"trace-secret"},wasm,providerRetries:0,onTrace:trace=>traces.push(trace),
    tools:[{name:"lookup",description:"lookup",parameters:{type:"object"},execute:()=>{executions++;return "unused";}}],fetch:async()=>sse(payload)});
  try {await assert.rejects(runtime.send("inspect trace-secret"),/invalid.*arguments/i);
    assert.equal(executions,0);assert.deepEqual(runtime.history(),[]);assert.equal(traces.length,1);
    const trace=traces[0]!;assert.equal(trace.outcome,"error");assert.equal(trace.httpStatus,200);
    assert.match(trace.responseHead+trace.responseTail,/broken \[REDACTED\]/);
    assert.ok(!JSON.stringify(traces).includes("trace-secret"));assert.equal(runtime.activeTrace,null);
  }finally{runtime.dispose();}
});
await test("nonstream validation failure and successful retry attempts retain distinct traces", async()=>{
  const traces: import('../src/provider-trace.js').ProviderTrace[]=[];let requests=0;
  const runtime=await createRuntime({connection,wasm,stream:false,onTrace:trace=>traces.push(trace),fetch:async()=>{
    if(++requests===1)return new Response("busy",{status:429,headers:{"Retry-After":"0"}});
    return Response.json({choices:[{message:{role:"assistant",content:null,tool_calls:[{id:"bad",type:"function",function:{name:"lookup",arguments:"broken-json"}}]},finish_reason:"tool_calls"}]});
  }});
  try {await assert.rejects(runtime.send("go"));assert.equal(traces.length,2);
    assert.deepEqual(traces.map(t=>[t.attempt,t.outcome,t.httpStatus]),[[1,"error",429],[2,"error",200]]);
    assert.equal(traces[0]!.responseHead,"busy");assert.equal(traces[0]!.responseComplete,true);
    assert.equal(traces[0]!.turnId,traces[1]!.turnId);assert.notEqual(traces[0]!.id,traces[1]!.id);
    assert.equal(traces[1]!.responseComplete,true);assert.match(traces[1]!.responseHead,/broken-json/);
    assert.deepEqual(runtime.history(),[]);
  }finally{runtime.dispose();}
});
await test("active exports and cancellations retain partial provider output",async()=>{
  const traces: import('../src/provider-trace.js').ProviderTrace[]=[];
  const runtime=await createRuntime({connection,wasm,timeoutMs:0,onTrace:trace=>traces.push(trace),fetch:async()=>new Response(new ReadableStream({
    start(controller){controller.enqueue(encoder.encode(frame(chunk({content:"partial diagnostic"}))));}
  }),{headers})});
  try {const sending=runtime.send("go");await wait(()=>!!runtime.activeTrace?.receivedBytes);
    assert.match(runtime.activeTrace!.responseHead,/partial diagnostic/);runtime.cancel();await assert.rejects(sending,/stopped/);
    assert.equal(traces[0]!.outcome,"cancelled");assert.equal(traces[0]!.responseComplete,false);assert.deepEqual(runtime.history(),[]);
  }finally{runtime.dispose();}
});
await test("oversized HTTP error bodies have bounded head/tail and explicit incomplete capture",async()=>{
  const traces: import('../src/provider-trace.js').ProviderTrace[]=[];
  const runtime=await createRuntime({connection,wasm,providerRetries:0,onTrace:trace=>traces.push(trace),fetch:async()=>new Response('x'.repeat(96*1024),{status:400})});
  try {await assert.rejects(runtime.send('go'),/HTTP 400/);
    const trace=traces[0]!;assert.equal(trace.receivedBytes,96*1024);assert.equal(trace.responseComplete,false);
    assert.equal(trace.responseOmittedBytes,32*1024);assert.equal(trace.responseHead.length+trace.responseTail.length,64*1024);
  }finally{runtime.dispose();}
});
await test("a stalled HTTP error body cannot prevent the provider retry",async()=>{
  const traces: import('../src/provider-trace.js').ProviderTrace[]=[];let calls=0,cancelled=false;
  const runtime=await createRuntime({connection,wasm,timeoutMs:0,onTrace:trace=>traces.push(trace),fetch:async()=>{
    if(++calls===1)return new Response(new ReadableStream({start(c){c.enqueue(encoder.encode('busy'));},cancel(){cancelled=true;}}),{status:429,headers:{'Retry-After':'0'}});
    return Response.json({choices:[{finish_reason:'stop',message:{role:'assistant',content:'recovered'}}]});
  }});
  try {assert.equal(await runtime.send('go'),'recovered');assert.equal(cancelled,true);
    assert.equal(traces[0]!.responseHead,'busy');assert.equal(traces[0]!.responseComplete,false);
    assert.equal(traces[1]!.outcome,'complete');
  }finally{runtime.dispose();}
});
