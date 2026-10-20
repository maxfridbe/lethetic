import assert from "node:assert/strict";
import test from "node:test";
import { emptyWorkspace, parseWorkspace } from "../src/workspace.js";
import type { Workspace } from "../src/workspace.js";
import { createPythonTool } from "../src/python.js";

class FakeWorker extends EventTarget {
  requests: { id: number; code: string; indexURL: string; workspace: Workspace }[] = [];
  terminated = false;
  handshakes = 0;
  postMessage(data: { id: number; code: string; indexURL: string; workspace: Workspace; type?: string }): void {
    if (data.type === "hello") {
      this.handshakes++;
      this.dispatchEvent(new MessageEvent("message", { data: { id: data.id, type: "ready", protocol: 1, capabilities: ["sites"] } }));
    } else this.requests.push(data);
  }
  terminate(): void { this.terminated = true; }
  answer(output: string, workspace = emptyWorkspace(), extra: Record<string, unknown> = {}): void {
    this.dispatchEvent(new MessageEvent("message", { data: { id: this.requests.at(-1)?.id, protocol: 1, output, workspace, ...extra } }));
  }
}

await test("Python worker is lazy, persistent, and receives no model credentials", async () => {
  const workers: FakeWorker[] = [];
  const python = createPythonTool({ workerFactory: () => {
    const worker = new FakeWorker(); workers.push(worker); return worker as unknown as Worker;
  } });
  assert.match(python.description, /Create CSV, HTML, SVG or PNG/);
  assert.match(python.description, /\/home\/pyodide/);
  assert.equal(workers.length, 0);
  const running = python.execute({ code: "x = 40" }, new AbortController().signal);
  const worker = workers[0]; assert.ok(worker);
  assert.deepEqual(Object.keys(worker.requests[0] ?? {}).sort(), ["code", "id", "indexURL", "protocol", "siteAccess", "workspace"]);
  worker.answer("ok");
  assert.equal(await running, "ok");
  const again = python.execute({ code: "x + 2" }, new AbortController().signal);
  worker.answer("42");
  assert.equal(await again, "42");
  assert.equal(workers.length, 1);
  assert.equal(worker.handshakes, 1);
  python.reset();
  assert.equal(worker.terminated, true);
});

await test("cancelling Python terminates execution and a fresh worker is created", async () => {
  const workers: FakeWorker[] = [];
  const python = createPythonTool({ workerFactory: () => {
    const worker = new FakeWorker(); workers.push(worker); return worker as unknown as Worker;
  } });
  const controller = new AbortController();
  const running = python.execute({ code: "while True: pass" }, controller.signal);
  controller.abort();
  await assert.rejects(Promise.resolve(running), /stopped/);
  assert.equal(workers[0]?.terminated, true);
  const next = python.execute({ code: "1+1" }, new AbortController().signal);
  assert.equal(workers.length, 2);
  workers[1]?.answer("2");
  assert.equal(await next, "2");
  python.reset();
});

await test("timeouts, worker failures, and concurrent execution are handled", async () => {
  const worker = new FakeWorker();
  const python = createPythonTool({ workerFactory: () => worker as unknown as Worker, timeoutMs: 10 });
  const running = python.execute({ code: "slow" }, new AbortController().signal);
  await assert.rejects(Promise.resolve(python.execute({ code: "again" }, new AbortController().signal)), /already running/);
  await assert.rejects(Promise.resolve(running), /timed out/);
  assert.equal(worker.terminated, true);
  const another = python.execute({ code: "go" }, new AbortController().signal);
  worker.dispatchEvent(new Event("error"));
  await assert.rejects(Promise.resolve(another), /could not load/);
  python.reset();
});

await test("completed files survive cancellation and reset; explicit edits reach the next worker", async () => {
  const workers: FakeWorker[] = []; const checkpoints: Workspace[] = [];
  const files = parseWorkspace({version:1,directories:['empty'],files:[{path:'result.bin',data:'AP8='}]});
  const python = createPythonTool({ onWorkspace: workspace => checkpoints.push(workspace), workerFactory: () => {
    const worker = new FakeWorker(); workers.push(worker); return worker as unknown as Worker;
  } });
  const first = python.execute({code:'create files'},new AbortController().signal);
  workers[0]!.answer('created',files); await first;
  assert.deepEqual(python.snapshotWorkspace(),files); assert.deepEqual(checkpoints,[files]);
  const controller = new AbortController(); const interrupted = python.execute({code:'write partial; loop'},controller.signal);
  controller.abort(); await assert.rejects(Promise.resolve(interrupted),/stopped/);
  assert.deepEqual(python.snapshotWorkspace(),files);
  const resumed = python.execute({code:'read saved'},new AbortController().signal);
  assert.deepEqual(workers[1]!.requests[0]!.workspace,files);
  workers[1]!.answer('saved files',files); await resumed; python.reset();
  python.setWorkspace(emptyWorkspace());
  const fresh = python.execute({code:'list empty'},new AbortController().signal);
  assert.deepEqual(workers[2]!.requests[0]!.workspace,emptyWorkspace());
  workers[2]!.answer('empty'); await fresh; python.reset();
});

await test("failed persistence is reported in tool output and retains a recoverable workspace", async () => {
  const worker = new FakeWorker();
  const python = createPythonTool({ workerFactory: ()=>worker as unknown as Worker, onWorkspace:()=>{throw Error('Storage quota exceeded');} });
  const files = parseWorkspace({version:1,directories:[],files:[{path:'keep',data:'YQ=='}]});
  const running = python.execute({code:'create'},new AbortController().signal);
  worker.answer('created',files);
  assert.match(String(await running),/Workspace not saved:.*quota/);
  assert.deepEqual(python.snapshotWorkspace(),files); python.reset();
});

await test("stale workers never receive user code or overwrite saved files", async () => {
  const worker = new FakeWorker();
  const requests: unknown[] = [];
  worker.postMessage = data => {
    requests.push(data);
    worker.dispatchEvent(new MessageEvent("message", { data: { id: data.id, output: "Old worker error" } }));
  };
  const workspace = parseWorkspace({version:1,directories:[],files:[{path:"saved",data:"YQ=="}]});
  const python = createPythonTool({workspace, workerFactory:()=>worker as unknown as Worker});
  await assert.rejects(Promise.resolve(python.execute({code:"do not replay"},new AbortController().signal)), /out of date.*Reload/);
  assert.deepEqual(requests,[{type:"hello",id:1,protocol:1}]);
  assert.deepEqual(python.snapshotWorkspace(),workspace);
  assert.equal(worker.terminated,true);
});

await test("Python sites require permission and a running worker", async () => {
  assert.throws(()=>createPythonTool({siteAccess:"false" as unknown as boolean}),/boolean/);
  const request = {port:8080,method:"GET",path:"/"};
  await assert.rejects(createPythonTool().requestSite(request,new AbortController().signal),/disabled/);
  await assert.rejects(createPythonTool({siteAccess:true}).requestSite(request,new AbortController().signal),/Run.*Python/);
});

await test("Python site requests import edits, checkpoint writes, and serialize with execution", async () => {
  const worker = new FakeWorker(), sites = [{port:8080,webRoot:"site"}];
  const tool = createPythonTool({siteAccess:true,workerFactory:()=>worker as unknown as Worker});
  const run = tool.execute({code:"start site"},new AbortController().signal);
  worker.answer("ready",emptyWorkspace(),{sites});await run;
  assert.deepEqual(tool.sites(),sites);
  const edited = parseWorkspace({version:1,directories:[],files:[{path:"index.html",data:"aGk="}]});
  tool.setWorkspace(edited);
  const request = {port:8080,method:"POST",path:"/note",body:"YQ=="};
  const pending = tool.requestSite(request,new AbortController().signal);
  assert.deepEqual(worker.requests.at(-1)?.workspace,edited);
  assert.equal((worker.requests.at(-1) as unknown as {siteAccess:boolean}).siteAccess,true);
  assert.deepEqual((worker.requests.at(-1) as unknown as {siteRequest:unknown}).siteRequest,request);
  await assert.rejects(tool.execute({code:"concurrent"},new AbortController().signal),/already running/);
  const saved = parseWorkspace({version:1,directories:[],files:[...edited.files,{path:"note.txt",data:"YQ=="}]});
  const response = {status:201,headers:{"Content-Type":["text/plain"]},body:"c2F2ZWQ="};
  worker.answer("",saved,{sites,response});
  assert.deepEqual(await pending,response);
  assert.deepEqual(tool.snapshotWorkspace(),saved);
  const controller = new AbortController();
  const stopped = tool.requestSite(request,controller.signal);controller.abort();
  await assert.rejects(stopped,/stopped/);
  assert.deepEqual(tool.sites(),[]);
  assert.deepEqual(tool.snapshotWorkspace(),saved);
});

await test("Python site permission detects workers without site support before sending code", async () => {
  const worker = new FakeWorker();
  worker.postMessage = data => worker.dispatchEvent(new MessageEvent("message",{
    data:{id:data.id,protocol:1,type:"ready"},
  }));
  const tool=createPythonTool({siteAccess:true,workerFactory:()=>worker as unknown as Worker});
  await assert.rejects(tool.execute({code:"listen"},new AbortController().signal),/out of date/);
  assert.equal(worker.terminated,true);
  assert.equal(worker.requests.length,0);
});
