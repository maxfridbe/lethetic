import assert from "node:assert/strict";
import test from "node:test";
import { TraceCapture, ProviderTraceLog, TRACE_LIMITS, parseProviderTraces } from "../src/provider-trace.js";
const meta = { turnId:"turn", round:1, attempt:1, provider:"openai", model:"test" };
const bytes = (text:string) => new TextEncoder().encode(text).length;
await test("redacts known credentials before capture across every possible chunk boundary", () => {
  const key = 'secret-"key\\value';
  for (const variant of [key, JSON.stringify(key).slice(1,-1), JSON.stringify(JSON.stringify(key).slice(1,-1)).slice(1,-1), encodeURIComponent(key)]) {
    for (let boundary=1; boundary<variant.length; boundary++) {
      const capture=new TraceCapture(meta,JSON.stringify({prompt:key}),[key]);
      capture.chunk("prefix "+variant.slice(0,boundary),bytes("prefix "+variant.slice(0,boundary)));
      assert.ok(!capture.value.responseHead.includes("secret-"));
      capture.chunk(variant.slice(boundary)+" suffix",bytes(variant.slice(boundary)+" suffix"));
      capture.finish("error",`bad ${key}`);
      assert.equal(capture.value.requestBody,'{"prompt":"[REDACTED]"}');
      assert.equal(capture.value.responseHead+capture.value.responseTail,"prefix [REDACTED] suffix");
      assert.equal(capture.value.error,"bad [REDACTED]"); assert.equal(capture.value.redacted,true);
    }
  }
});
await test("bounded UTF-8 head/tail keeps stream order, byte counts and explicit omissions", () => {
  const capture=new TraceCapture(meta,"x".repeat(TRACE_LIMITS.requestBytes+5),[]);
  const first="a".repeat(TRACE_LIMITS.responseBytes/2-1)+"🌍";
  capture.chunk(first,bytes(first));capture.chunk("z",1);
  assert.equal(capture.value.responseHead+capture.value.responseTail,first+"z");
  capture.chunk("b".repeat(TRACE_LIMITS.responseBytes),TRACE_LIMITS.responseBytes);
  capture.finish("error");
  assert.equal(capture.value.requestOmittedBytes,5);
  assert.equal(capture.value.responseOmittedBytes,capture.value.receivedBytes-bytes(capture.value.responseHead+capture.value.responseTail));
  assert.ok(!capture.value.responseHead.includes("�"));
  assert.ok(bytes(capture.value.responseHead+capture.value.responseTail)<=TRACE_LIMITS.responseBytes);
  assert.ok(capture.value.responseTail.endsWith("b"));
});
await test("bounded log survives serialization and evicts oldest attempts explicitly", () => {
  const log=new ProviderTraceLog();
  for(let i=0;i<20;i++) { const capture=new TraceCapture({...meta,attempt:i+1},"{}",[]);capture.finish("complete");log.add(capture.value); }
  assert.equal(log.snapshot().omittedRequests,4); assert.equal(log.snapshot().requests[0]!.attempt,5);
  const restored=new ProviderTraceLog();restored.restore(JSON.parse(JSON.stringify(log.snapshot())));
  assert.deepEqual(restored.snapshot(),log.snapshot());
  const snapshot=log.snapshot(); snapshot.requests[0]!.requestBody="x".repeat(TRACE_LIMITS.requestBytes+1);
  assert.throws(()=>parseProviderTraces(snapshot),/limits/);
  const large=new ProviderTraceLog();
  for(let i=0;i<12;i++) {const c=new TraceCapture(meta,"{}",[]);c.chunk("x".repeat(TRACE_LIMITS.responseBytes),TRACE_LIMITS.responseBytes);c.finish("error");large.add(c.value);}
  assert.ok(large.snapshot().omittedRequests>0);assert.ok(bytes(JSON.stringify(large.snapshot()))<=TRACE_LIMITS.sessionBytes);
});
await test("small embedding stores retain the newest trace with accurate truncation",()=>{
  const log=new ProviderTraceLog(128*1024);
  const trace=new TraceCapture(meta,'x'.repeat(TRACE_LIMITS.requestBytes),[]);
  trace.chunk('y'.repeat(TRACE_LIMITS.responseBytes),TRACE_LIMITS.responseBytes);trace.finish('error','invalid arguments');
  log.add(trace.value);
  const snapshot=log.snapshot(), saved=snapshot.requests[0]!;
  assert.equal(snapshot.requests.length,1);assert.equal(snapshot.omittedRequests,0);
  assert.ok(bytes(JSON.stringify(snapshot))<=128*1024);
  assert.equal(bytes(saved.requestBody)+saved.requestOmittedBytes,TRACE_LIMITS.requestBytes);
  assert.equal(bytes(saved.responseHead+saved.responseTail)+saved.responseOmittedBytes,TRACE_LIMITS.responseBytes);
});
