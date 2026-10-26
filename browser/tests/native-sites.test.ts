import assert from "node:assert/strict";
import test from "node:test";
import { NativeSites } from "../src/native-sites.js";

await test("native sites forward virtual requests, open only registered routes, and never acquire a CPU wake lock", async()=>{
  const previous=Object.getOwnPropertyDescriptor(globalThis,"window");
  const calls:{command:string,args:Record<string,unknown>}[]=[];
  let delayGrant=false, releaseGrant:((value:unknown)=>void)|undefined;
  let next:((value:unknown)=>void)|undefined, cancel:((error:Error)=>void)|undefined;
  const host={top:null as unknown,__LETHETIC_HOST_TOKEN__:"private-native-token",__TAURI__:{core:{invoke:async(command:string,args:Record<string,unknown>)=>{
    assert.equal(args.token,"private-native-token");calls.push({command,args});
    if(command.endsWith("|forward_site") && delayGrant)return new Promise(resolve=>{releaseGrant=resolve;});
    if(command.endsWith("|forward_site"))return {id:"site-id",port:8080,url:"http://127.0.0.1:8080/?__lethetic_site=capability"};
    if(command.endsWith("|next_site_request"))return new Promise((resolve,reject)=>{next=resolve;cancel=reject;});
    if(command.endsWith("|stop_forwarded_sites"))cancel?.(Error("stopped"));
    return undefined;
  }}}};host.top=host;Object.defineProperty(globalThis,"window",{configurable:true,value:host});
  const errors:string[]=[], site=new NativeSites(error=>errors.push(error));
  try {
    await site.start(8080,false,3000,async request=>{
      assert.equal(request.port,3000);assert.equal(request.path,"/api/test");assert.equal(request.body,"AP8=");
      return {status:201,headers:{"content-type":["application/octet-stream"]},body:"AP8="};
    });
    await site.openBrowser();assert.ok(calls.some(call=>call.command.endsWith("|open_site_browser") && call.args.id==="site-id"));
    next?.({id:"request-id",method:"POST",path:"/api/test",headers:{},body:"AP8="});
    for(let i=0;i<100 && !calls.some(call=>call.command.endsWith("|reply_site_request"));i++)await new Promise(resolve=>setTimeout(resolve,1));
    const reply=calls.find(call=>call.command.endsWith("|reply_site_request"));assert.equal(reply?.args.requestId,"request-id");
    assert.deepEqual(reply?.args.response,{status:201,headers:{"content-type":["application/octet-stream"]},body:"AP8="});
    await site.stop();await new Promise(resolve=>setTimeout(resolve,0));assert.equal(site.route,null);assert.deepEqual(errors,[]);
    const work=calls.filter(call=>call.command.endsWith("|set_working"));assert.ok(work.some(call=>call.args.active===true));assert.ok(work.every(call=>call.args.working===false));assert.equal(work.at(-1)?.args.active,false);
    delayGrant=true;const pending=site.start(0,false,3000,async()=>{throw Error("Cancelled site dispatched a request");});
    for(let i=0;i<100 && !releaseGrant;i++)await new Promise(resolve=>setTimeout(resolve,1));
    assert.ok(releaseGrant);await site.stop();releaseGrant({id:"cancelled-site",port:8081,url:"http://127.0.0.1:8081/"});await pending;
    assert.equal(site.route,null);assert.ok(calls.some(call=>call.command.endsWith("|stop_forwarded_sites") && call.args.id==="cancelled-site"));
  } finally {await site.stop();if(previous)Object.defineProperty(globalThis,"window",previous);else Reflect.deleteProperty(globalThis,"window");}
});
