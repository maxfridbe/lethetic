import assert from "node:assert/strict";
import test from "node:test";
import { installPythonNetworkPolicy, pythonPackageDownloads } from "../src/python-network.js";

function fixture() {
  const calls: Request[] = [];
  const scope = { fetch: async (input: RequestInfo | URL, init?: RequestInit) => {
    calls.push(new Request(input, init)); return new Response("ok");
  }, XMLHttpRequest: class {}, WebSocket: class {}, EventSource: class {}, WebTransport: class {} };
  return { scope: scope as unknown as typeof globalThis, calls };
}

await test("Python networking defaults deny while exact runtime packages remain available", async () => {
  const {scope, calls} = fixture();
  const url = "https://cdn.example/pyodide/numpy.whl";
  const downloads = pythonPackageDownloads({packages:{numpy:{file_name:"numpy.whl"}}}, "https://cdn.example/pyodide/");
  installPythonNetworkPolicy({networkAccess:false, downloads}, scope);
  downloads.add("https://example.org/api"); // Mutating configuration must not grant new access.
  await scope.fetch(url, {headers:{Authorization:"secret"},credentials:"include",referrer:"https://cdn.example/private"});
  assert.equal(calls[0]?.credentials,"omit"); assert.equal(calls[0]?.redirect,"error");
  assert.equal(calls[0]?.referrer,""); assert.equal(calls[0]?.referrerPolicy,"no-referrer");
  assert.equal(calls[0]?.headers.has("authorization"),false);
  for (const target of ["https://example.org/api",url+"?upload=secret",url.replace("numpy","other")])
    await assert.rejects(scope.fetch(target), /Network access is disabled/);
  await assert.rejects(scope.fetch(url,{method:"POST",body:"data"}),/disabled/);
  for (const name of ["XMLHttpRequest","WebSocket","EventSource","WebTransport"]) {
    const constructor = (scope as unknown as Record<string, typeof XMLHttpRequest>)[name]!;
    assert.throws(()=>new constructor(),/disabled/);
  }
  assert.throws(()=>Object.defineProperty(scope,"fetch",{value:()=>{}}),TypeError);
  assert.equal(calls.length,1);
});

await test("Python direct network preserves methods, bodies and abort signals", async () => {
  const {scope,calls}=fixture();
  installPythonNetworkPolicy({networkAccess:true,downloads:new Set()},scope);
  const controller=new AbortController();
  await scope.fetch("https://example.org/api",{method:"PATCH",body:"payload",signal:controller.signal});
  assert.equal(calls[0]?.method,"PATCH"); assert.equal(await calls[0]?.text(),"payload");
  controller.abort(); assert.equal(calls[0]?.signal.aborted,true);
  assert.ok(new scope.XMLHttpRequest());
});

await test("Python relay never falls back; socket and XHR paths cannot bypass it", async () => {
  const {scope,calls}=fixture(); let relayed=0;
  installPythonNetworkPolicy({networkAccess:true,downloads:new Set(["https://cdn.example/numpy.whl"]),
    relay:{endpoint:"https://relay.example/relay",token:"a-secret-test-token-123456"},
    relayFetch:async (_fetch,relay,input,init)=>{
      relayed++;assert.equal(relay.endpoint,"https://relay.example/relay");
      assert.equal(new Request(input,init).method,"DELETE");throw Error("relay unavailable");
    }},scope);
  await assert.rejects(scope.fetch("https://example.org/api",{method:"DELETE"}),/relay unavailable/);
  assert.equal(relayed,1);assert.equal(calls.length,0);
  assert.throws(()=>new scope.XMLHttpRequest(),/pyfetch/);
  assert.throws(()=>new scope.WebSocket("wss://example.org"),/configured route/);
  await scope.fetch("https://cdn.example/numpy.whl");
  assert.equal(calls.length,1);assert.equal(relayed,1);
});

await test("Python package exemptions reject remote URLs and directory traversal", () => {
  assert.equal(pythonPackageDownloads({packages:{tests:{file_name:"numpy-tests.tar"}}},"https://cdn.example/").size,1);
  for(const file_name of ["https://elsewhere/a.whl","../a.whl","a.whl?x=1","a.js","a.whl#x"])
    assert.throws(()=>pythonPackageDownloads({packages:{bad:{file_name}}},"https://cdn.example/"),/filename/);
});
