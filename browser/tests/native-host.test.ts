import assert from "node:assert/strict";
import test from "node:test";
import { NativeDirectory, nativeAvailable, nativeWorking } from "../src/native-host.js";
await test("native grant is explicit, queued, revocable and unavailable in frames", async () => {
  const prior = Object.getOwnPropertyDescriptor(globalThis, "window");
  const calls: { command: string; args: Record<string, unknown> }[] = [];
  const host = { top: null as unknown, __LETHETIC_HOST_TOKEN__: "test-capability", __TAURI__: { core: { invoke: async (command: string, args: Record<string, unknown>) => {
    calls.push({command,args});
    return command.endsWith("choose_directory") ? {name:"chosen",workspace:{version:1,files:[],directories:[]}} : undefined;
  } } } };
  host.top = host; Object.defineProperty(globalThis,"window",{configurable:true,value:host});
  try {
    assert.equal(nativeAvailable(),true);
    const directory = new NativeDirectory();
    await directory.sync({version:1,files:[],directories:[]}); assert.equal(calls.length,0);
    const workspace = await directory.choose(); assert.equal(directory.name,"chosen");
    await directory.sync(workspace!); await directory.revoke();
    await directory.sync(workspace!); assert.equal(calls.length,3);
    await Promise.all([nativeWorking(true),nativeWorking(false)]);
    assert.deepEqual(calls.slice(-2).map(call=>call.args.working),[true,false]);
    assert.ok(calls.every(call=>call.args.token==="test-capability"));
    host.top = {}; assert.equal(nativeAvailable(),false);
  } finally { if(prior)Object.defineProperty(globalThis,"window",prior);else Reflect.deleteProperty(globalThis,"window"); }
});
