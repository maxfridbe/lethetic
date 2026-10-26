import assert from "node:assert/strict";
import test from "node:test";
import { LocalTransport } from "../src/local-transport.js";
import { SessionStore } from "../src/sessions.js";
import { injectedInstructions } from "../src/prompts.js";

await test("Tor permission survives session reload, preserves work, disables direct NuGet, and switches explicitly",()=>{
  const prior=Object.getOwnPropertyDescriptor(globalThis,"window");
  const host={top:null as unknown,__LETHETIC_HOST_TOKEN__:"test"};host.top=host;
  Object.defineProperty(globalThis,"window",{configurable:true,value:host});
  const records=new Map<string,string>(),storage={getItem:(key:string)=>records.get(key)??null,setItem:(key:string,value:string)=>{records.set(key,value);}} as Storage;
  const events={socketOpened(){},authenticationEpochChanged(){},transportStatus(){},protocolFault(){},serverMessage(){},commandResponse(){}};
  const transport=new LocalTransport(events,{connections:[],storage,mode:"dotnet-wasm",nugetAccess:true,systemPrompt:"Keep my instructions"});
  try {
    transport.start();const id=transport.state.session.session_id;
    transport.setNetworkMode("tor");
    assert.equal(transport.networkAccess,true);assert.equal(transport.networkMode,"tor");assert.equal(transport.nugetAccess,false);
    assert.throws(()=>transport.setNugetAccess(true),/disabled in Tor-only/);
    const saved=new SessionStore(storage).list().find(session=>session.id===id)!;
    assert.equal(saved.networkMode,"tor");assert.equal(saved.system,"Keep my instructions");
    transport.openSession(id);assert.equal(transport.networkMode,"tor");assert.equal(transport.state.session.session_id,id);
    host.top={};transport.openSession(id);assert.equal(transport.networkMode,"tor");
    assert.throws(()=>transport.setNetworkAccess(true),/requires the native app/);
    transport.setDotnetRelay(null);assert.equal(transport.networkMode,"direct");host.top=host;
    transport.setNetworkMode("off");assert.equal(transport.networkAccess,false);
    transport.setNetworkMode("direct");assert.equal(transport.networkMode,"direct");assert.equal(transport.networkAccess,true);
    assert.match(injectedInstructions("dotnet-wasm",true,false,false,true,"tor"),/No direct fallback/);
  } finally {transport.dispose();if(prior)Object.defineProperty(globalThis,"window",prior);else Reflect.deleteProperty(globalThis,"window");}
});
