import assert from "node:assert/strict";
import test from "node:test";
import { createDotnetTool } from "../src/dotnet.js";
import type { DotnetResult, DotnetRuntime } from "../src/dotnet.js";
import { emptyWorkspace } from "../src/workspace.js";
import type { Workspace } from "../src/workspace.js";

class Runtime implements DotnetRuntime {
  files = emptyWorkspace(); imports = 0; terminated = false;
  result: DotnetResult = { success: true, stdout: "42", stderr: "", value: null, diagnostics: [], error: null };
  importWorkspace(workspace: Workspace): Promise<void> { this.imports++; this.files = structuredClone(workspace); return Promise.resolve(); }
  exportWorkspace(): Promise<unknown> { return Promise.resolve(this.files); }
  terminate(): void { this.terminated = true; }
  runCSharpAsync(_code: string, _options: { signal: AbortSignal }): Promise<DotnetResult> { return Promise.resolve(this.result); }
}
const textFile = (path: string, content: string) => ({path, data:btoa(content)});
const signal = (): AbortSignal => new AbortController().signal;

await test("startup initializes the worker and binds saved files without running C#", async () => {
  const runtime = new Runtime();
  runtime.runCSharpAsync = async () => { throw new Error("Startup must not run saved programs"); };
  const tool = createDotnetTool({workspace: {...emptyWorkspace(), files: [textFile("Program.cs", "saved source")]}, loadRuntime: async () => runtime});
  await tool.initialize(signal());
  assert.equal(runtime.imports, 1);
  assert.equal(runtime.files.files[0]?.path, "Program.cs");
  assert.equal(runtime.terminated, false);
  tool.reset();
});

await test("iframe console forwarding never starts a worker and stops on reset", async () => {
  const entries: string[] = [];
  const runtime = Object.assign(new Runtime(), { recordConsole: (entry: { message: string }) => entries.push(entry.message) });
  const tool = createDotnetTool({loadRuntime: async () => runtime});
  const entry = {level: "log" as const, message: "iframe output", source: "iframe :8080"};
  tool.recordConsole(entry); assert.deepEqual(entries, []);
  await tool.execute({code: "42"}, signal());
  tool.recordConsole(entry); assert.deepEqual(entries, ["iframe output"]);
  tool.reset(); tool.recordConsole(entry); assert.equal(entries.length, 1);
});

await test("stream headers keep the tool busy until completion and checkpoint cancelled response effects", async () => {
  let finish!: () => void, cancel!: (error: Error) => void;
  const runtime = Object.assign(new Runtime(), {requestSiteStream: async () => ({status:200,headers:{},
    body:new ReadableStream<Uint8Array>(), completed:new Promise<void>((resolve, reject) => {finish=resolve;cancel=reject;}),
    cancel: () => {cancel(new Error("reader cancelled"));},
  })});
  const tool = createDotnetTool({siteAccess:true,loadRuntime:async()=>runtime});
  await tool.execute({code:"start"},signal());
  const first=await tool.requestSiteStream({port:8080,method:"GET",path:"/"},signal());
  await assert.rejects(Promise.resolve(tool.execute({code:"42"},signal())), /already running/);
  runtime.files.files.push(textFile("stream.txt","done")); finish(); await first.completed;
  assert.equal(tool.snapshotWorkspace().files[0]?.path,"stream.txt");
  const second=await tool.requestSiteStream({port:8080,method:"GET",path:"/"},signal());
  runtime.files.files.push(textFile("cancel.txt","kept")); second.cancel();
  await assert.rejects(second.completed,/reader cancelled/);
  assert.equal(runtime.terminated,false);
  assert.ok(tool.snapshotWorkspace().files.some(file => file.path === "cancel.txt"));
  assert.equal(await tool.execute({code:"42"},signal()),"42"); tool.reset();
});

await test(".NET tool loads lazily without credentials and reuses its workspace/runtime", async () => {
  const runtime = new Runtime(); let loads = 0;
  const tool = createDotnetTool({ workspace: { ...emptyWorkspace(), files: [textFile("input.txt", "uploaded")] },
    loadRuntime: options => { loads++; assert.deepEqual(Object.keys(options).sort(), ["networkAccess", "nugetAccess", "signal", "siteAccess"]); assert.equal(options.networkAccess, false); assert.equal(options.nugetAccess, false); return Promise.resolve(runtime); } });
  assert.equal(loads, 0);
  assert.equal(await tool.execute({code:"Console.Write(42);"}, signal()), "42");
  assert.equal(runtime.files.files[0]?.path, "input.txt");
  await tool.execute({code:"6 * 7"}, signal()); assert.equal(loads, 1); assert.equal(runtime.imports, 1);
  tool.setWorkspace(emptyWorkspace()); await tool.execute({code:"1"}, signal()); assert.equal(runtime.imports, 2);
  tool.reset(); assert.equal(runtime.terminated, true);
});

await test("failed C# submissions still checkpoint created files and return compiler diagnostics", async () => {
  const runtime = new Runtime(); let saved: Workspace | undefined;
  runtime.runCSharpAsync = async () => {
    runtime.files.files.push(textFile("output.txt", "kept"));
    return { ...runtime.result, success: false, diagnostics: [{id:"CS0103",message:"Missing name",line:1,column:2}], error:{type:"Exception",message:"boom"} };
  };
  const tool = createDotnetTool({ loadRuntime: async () => runtime, onWorkspace: snapshot => { saved = snapshot; } });
  const output = await tool.execute({code:"throw new Exception();"}, signal());
  assert.match(output, /CS0103/); assert.match(output, /boom/); assert.equal(saved?.files[0]?.path, "output.txt");
  tool.reset(); assert.equal(tool.snapshotWorkspace().files[0]?.path, "output.txt");
});

await test("stop and timeout terminate .NET without overwriting the last completed files", async () => {
  const runtimes: Runtime[] = [];
  const tool = createDotnetTool({ timeoutMs: 30, workspace: { ...emptyWorkspace(), files: [textFile("kept.txt", "safe")] }, loadRuntime: async options => {
    assert.equal(options.networkAccess, true);
    const runtime = new Runtime(); runtimes.push(runtime);
    runtime.runCSharpAsync = (_code, {signal}) => new Promise((_resolve, reject) => signal.addEventListener("abort", () => reject(signal.reason), {once:true}));
    return runtime;
  }, networkAccess: true });
  const controller = new AbortController();
  const first = tool.execute({code:"while(true) {}"}, controller.signal);
  await new Promise(resolve => setTimeout(resolve, 0)); controller.abort();
  await assert.rejects(Promise.resolve(first)); assert.equal(runtimes[0]?.terminated, true);
  await assert.rejects(Promise.resolve(tool.execute({code:"while(true) {}"}, signal())), /timed out/);
  assert.equal(runtimes[1]?.terminated, true); assert.equal(tool.snapshotWorkspace().files[0]?.path, "kept.txt");
});

await test("invalid checkpoints preserve saved files and import failure cannot execute code", async () => {
  const runtime = new Runtime();
  runtime.exportWorkspace = async () => ({version:1,directories:[],files:[{path:"../escape",data:"AA=="}]});
  const tool = createDotnetTool({loadRuntime: async () => runtime});
  assert.match(await tool.execute({code:"1"}, signal()), /Workspace not saved/);
  assert.deepEqual(tool.snapshotWorkspace(), emptyWorkspace());
  runtime.importWorkspace = async () => { throw new Error("bad import"); };
  runtime.runCSharpAsync = async () => { throw new Error("must not execute after failed restore"); };
  await assert.rejects(Promise.resolve(tool.execute({code:"1"}, signal())), /bad import/);
  assert.equal(runtime.terminated, true);
});

await test("only explicit relay credentials reach the .NET loader", async () => {
  const relay = {endpoint:"https://relay.example/relay", token:"relay-token-only-not-model-token"};
  const tool = createDotnetTool({networkAccess:true,relay,loadRuntime: async options => {
    assert.deepEqual(Object.keys(options).sort(),["networkAccess","nugetAccess","relay","signal","siteAccess"]);
    assert.deepEqual(options.relay,relay); return new Runtime();
  }});
  assert.equal(await tool.execute({code:"42"},signal()),"42"); tool.reset();
});

await test("interactive requests enforce permission, serialize work, and checkpoint request-created files", async () => {
  const runtime = new Runtime(); let saved: Workspace | undefined;
  const worker: DotnetRuntime = Object.assign(runtime, {
    listSites: async () => [{port:8080,webRoot:"site"}],
    requestSite: async () => {
      runtime.files.files.push(textFile("request.txt", "written by C#"));
      return {status:201,headers:{},body:btoa("saved")};
    },
  });
  const tool = createDotnetTool({siteAccess:true,loadRuntime: async options => {
    assert.equal(options.siteAccess,true); assert.equal(options.networkAccess,false); return worker;
  }, onWorkspace: value => {saved=value;}});
  assert.deepEqual(tool.sites(), []);
  await tool.execute({code:"start site"},signal());
  assert.equal(tool.sites()[0]?.port,8080);
  const response=await tool.requestSite({port:8080,method:"POST",path:"/save"},signal());
  assert.equal(response.status,201); assert.equal(saved?.files[0]?.path,"request.txt");
  tool.reset(); assert.deepEqual(tool.sites(),[]);
  await assert.rejects(tool.requestSite({port:8080,method:"GET",path:"/"},signal()), /first/);
  const denied=createDotnetTool({loadRuntime:async()=>{throw Error("must not load");}});
  await assert.rejects(denied.requestSite({port:8080,method:"GET",path:"/"},signal()), /disabled/);
});
