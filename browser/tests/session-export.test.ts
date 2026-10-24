import assert from "node:assert/strict";
import test from "node:test";
import { spawn } from "node:child_process";
import { TraceCapture, ProviderTraceLog } from "../src/provider-trace.js";
import { sessionDebugBundle } from "../src/session-export.js";
import { encodeBytes, parseWorkspace } from "../src/workspace.js";

await test("debug bundles are standard gzip/tar with Unicode paths, binary files, and redacted keys", async () => {
  const path = "reports/" + "long-name-".repeat(20) + "résumé.txt";
  const capture=new TraceCapture({turnId:"turn",round:1,attempt:1,provider:"openai",model:"test"},"test-secret",["test-secret"]);
  capture.chunk('data: {invalid tool arguments test-secret',40);capture.finish("error","Invalid arguments");
  const log=new ProviderTraceLog();log.add(capture.value);
  const bundle = await sessionDebugBundle({id:crypto.randomUUID(),name:"Debug",label:"debug",updatedAt:new Date().toISOString(),
    providerTrace:log.snapshot(),blocks:[],messages:[{role:"user",content:"key: test-secret"}],inputs:[],system:"help",python:false,mode:"dotnet-wasm",networkAccess:false,dotnetRelayEndpoint:"https://relay.example/relay",theme:"dracula",
    connection:{name:"Example",provider:"openai",endpoint:"https://example.com/v1/chat/completions",model:"test"},
    workspace:parseWorkspace({version:1,directories:["empty"],files:[{path,data:encodeBytes(new TextEncoder().encode("test-secret héllo"))},{path:"image.bin",data:"AP8B"}]})},
    {status:"Failed test-secret",apiKey:"test-secret",relay:{token:"relay-secret",note:"relay-secret"}},["test-secret","relay-secret"]);
  const bytes = new Uint8Array(await bundle.arrayBuffer()); assert.equal(bundle.type,"application/gzip");
  const encoded = btoa(String.fromCharCode(...bytes));
  await new Promise<void>((resolve,reject) => {
    const child = spawn("python3",["-c",`
import base64,io,json,sys,tarfile
with tarfile.open(fileobj=io.BytesIO(base64.b64decode(sys.argv[1])),mode='r:gz') as archive:
    assert archive.getmember('workspace/empty/').isdir()
    assert archive.extractfile('workspace/image.bin').read() == bytes([0,255,1])
    assert archive.extractfile('workspace/'+sys.argv[2]).read().decode() == '[REDACTED] héllo'
    metadata=json.load(archive.extractfile('session.json'))
    traces=[json.loads(line) for line in archive.extractfile('provider-traces.jsonl').read().decode().splitlines() if line]
    assert traces[0]['outcome']=='error' and 'invalid tool arguments [REDACTED]' in traces[0]['responseHead']
    assert 'providerTrace' not in metadata
    assert metadata['messages'][0]['content']=='key: [REDACTED]'
    assert 'apiKey' not in metadata['connection']
    diagnostics=json.load(archive.extractfile('diagnostics.json'))
    assert diagnostics['diagnostics']=={'status':'Failed [REDACTED]', 'relay':{'note':'[REDACTED]'}}
    assert metadata['dotnetRelayEndpoint']=='https://relay.example/relay'
    assert diagnostics['workspace']['root']=='/home/dotnetdide'
    assert metadata['mode']=='dotnet-wasm' and metadata['networkAccess'] is False
`,encoded,path],{stdio:["ignore","ignore","pipe"]});
    let errors=""; child.stderr?.setEncoding("utf8"); child.stderr?.on("data",text=>{errors+=text;});
    child.on("exit",code=>{if(code===0)resolve();else reject(Error(errors));});
  });
});
