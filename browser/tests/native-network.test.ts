import assert from "node:assert/strict";
import test from "node:test";
import { NativeNetwork, nativeNetworkDiagnostics } from "../src/native-network.js";

await test("native runtime networking is opt-in, automatic, reusable and revoked on reset", async () => {
  const prior = Object.getOwnPropertyDescriptor(globalThis,"window");
  const calls: {command:string;args:Record<string,unknown>}[]=[];
  const route={endpoint:"http://127.0.0.1:12345/relay",token:"ephemeral-network-capability"};
  const host={top:null as unknown,__LETHETIC_HOST_TOKEN__:"main-window-only",__TAURI__:{core:{invoke:async(command:string,args:Record<string,unknown>)=>{
    calls.push({command,args});return route;
  }}}};host.top=host;
  Object.defineProperty(globalThis,"window",{configurable:true,value:host});
  try {
    const network=new NativeNetwork("session-one"), signal=new AbortController().signal;
    const saved={endpoint:"https://old-relay.example/relay",token:"saved-browser-relay-token"};
    assert.equal(await network.route(false,saved,signal),null);assert.equal(calls.length,0);
    assert.deepEqual(await network.route(true,saved,signal),route);
    assert.deepEqual(await network.route(true,saved,signal),route);assert.equal(calls.length,1);
    assert.equal(calls[0]?.args.sessionId,"session-one");
    network.reset();await Promise.resolve();await Promise.resolve();
    assert.equal(calls[1]?.command,"plugin:lethetic-host|revoke_network");
    assert.equal(calls[1]?.args.grant,route.token);
    assert.ok(calls.every(call=>call.args.token==="main-window-only"));
    host.top={};
    assert.deepEqual(await network.route(true,saved,signal),saved);
    assert.equal(calls.length,2);
  } finally {if(prior)Object.defineProperty(globalThis,"window",prior);else Reflect.deleteProperty(globalThis,"window");}
});

await test("a grant arriving after runtime reset is revoked instead of being used", async () => {
  const prior=Object.getOwnPropertyDescriptor(globalThis,"window");
  let resolve!: (route:unknown)=>void;
  const revoked:unknown[]=[];
  const host={top:null as unknown,__LETHETIC_HOST_TOKEN__:"main",__TAURI__:{core:{invoke:(command:string,args:Record<string,unknown>)=>{
    if(command.endsWith("grant_network"))return new Promise(done=>{resolve=done;});
    revoked.push(args.grant);return Promise.resolve();
  }}}};host.top=host;Object.defineProperty(globalThis,"window",{configurable:true,value:host});
  try {
    const network=new NativeNetwork();
    const pending=network.route(true,null,new AbortController().signal);
    await Promise.resolve();network.reset();
    resolve({endpoint:"http://127.0.0.1:1234/relay",token:"late-grant"});
    await assert.rejects(pending,/reset/);
    await Promise.resolve();assert.deepEqual(revoked,["late-grant"]);
  } finally {if(prior)Object.defineProperty(globalThis,"window",prior);else Reflect.deleteProperty(globalThis,"window");}
});

await test("Tor-only grants are explicit, cannot use browser HTTP, and do not fall back after native failure", async () => {
  const prior=Object.getOwnPropertyDescriptor(globalThis,"window");
  const calls:{command:string;args:Record<string,unknown>}[]=[];
  let fail=false;
  const host={top:null as unknown,__LETHETIC_HOST_TOKEN__:"main",__TAURI__:{core:{invoke:async(command:string,args:Record<string,unknown>)=>{
    calls.push({command,args});
    if(command.endsWith("grant_network")) {if(fail)throw Error("Tor unavailable");return {endpoint:"http://127.0.0.1:1234/relay",token:"tor-cap"};}
    return undefined;
  }}}};host.top=host;Object.defineProperty(globalThis,"window",{configurable:true,value:host});
  const network=new NativeNetwork(), signal=new AbortController().signal;
  try {
    await network.route(true,null,signal,"tor");assert.equal(calls[0]?.args.mode,"tor");
    network.reset();await Promise.resolve();fail=true;
    await assert.rejects(()=>network.route(true,null,signal,"tor"),/Tor unavailable/);
    assert.ok(calls.filter(call=>call.command.endsWith("grant_network")).every(call=>call.args.mode==="tor"));
    host.top={};
    await assert.rejects(()=>network.route(true,{endpoint:"https://fallback.example/"},signal,"tor"),/direct networking is blocked/);
  } finally {network.reset();await Promise.resolve();if(prior)Object.defineProperty(globalThis,"window",prior);else Reflect.deleteProperty(globalThis,"window");}
});

await test("network exports query only the requested session and report missing host support", async () => {
  assert.deepEqual(await nativeNetworkDiagnostics("old-session"), {available:false,reason:"Native relay diagnostics require the executable app."});
  const prior=Object.getOwnPropertyDescriptor(globalThis,"window");
  let fail=false;
  const host={top:null as unknown,__LETHETIC_HOST_TOKEN__:"main",__TAURI__:{core:{invoke:async(command:string,args:Record<string,unknown>)=>{
    assert.equal(command,"plugin:lethetic-host|network_diagnostics");
    assert.equal(args.sessionId,"old-session"); assert.equal(args.token,"main");
    if(fail)throw Error("unsupported command");
    return {requests:[{hostname:"example.org",relayStatus:502,error:"Tor connection failed"}],omittedRequests:0};
  }}}};host.top=host;Object.defineProperty(globalThis,"window",{configurable:true,value:host});
  try {
    const result=await nativeNetworkDiagnostics("old-session") as {available:boolean;capture:{requests:{relayStatus:number}[]}};
    assert.equal(result.available,true);assert.equal(result.capture.requests[0]?.relayStatus,502);
    fail=true;assert.equal((await nativeNetworkDiagnostics("old-session") as {available:boolean}).available,false);
  } finally {if(prior)Object.defineProperty(globalThis,"window",prior);else Reflect.deleteProperty(globalThis,"window");}
});
