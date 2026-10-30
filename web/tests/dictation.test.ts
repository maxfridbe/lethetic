import assert from "node:assert/strict";
import test from "node:test";
import { HoldToTalk, type HoldClock } from "../src/dictation/hold.js";
import { desktopDictationAvailable, dictationCommand } from "../src/dictation/native.js";
const flush = async (): Promise<void> => { for(let n=0;n<8;n++)await Promise.resolve(); };
function fixture() {
  let n=0;
  const timers=new Map<number,()=>void>(), calls:string[]=[], texts:string[]=[], errors:unknown[]=[];
  const clock:HoldClock={set:fn=>{timers.set(++n,fn);return n;},clear:id=>{timers.delete(id);}};
  let start=async():Promise<void>=>{},finish=async():Promise<string>=>"dictated text";
  const hold=new HoldToTalk({start:async id=>{calls.push('start:'+id);await start()},finish:async id=>{calls.push('finish:'+id);return finish()},cancel:async id=>{calls.push('cancel:'+id)}},()=>{},text=>texts.push(text),error=>errors.push(error),clock);
  return {hold,calls,texts,errors,tick:()=>{const first=timers.entries().next().value;if(first){timers.delete(first[0]);first[1]();}},start:(fn:()=>Promise<void>)=>{start=fn},finish:(fn:()=>Promise<string>)=>{finish=fn}};
}
await test("short clicks send normally and never open the microphone",async()=>{
  const f=fixture();f.hold.press();f.hold.release();f.tick();await flush();
  assert.equal(f.hold.phase,'idle');assert.equal(f.hold.consumeClick(),false);assert.deepEqual(f.calls,[]);
});
await test("hold records; release transcribes once and suppresses the Send click",async()=>{
  const f=fixture();f.hold.press();f.tick();await flush();assert.equal(f.hold.phase,'recording');
  f.hold.release();f.hold.release();assert.equal(f.hold.consumeClick(),true);await flush();
  assert.equal(f.hold.phase,'idle');assert.deepEqual(f.texts,['dictated text']);
  assert.equal(f.calls.filter(c=>c.startsWith('finish:')).length,1);
});
await test("release during microphone startup waits for readiness before stopping",async()=>{
  const f=fixture();let ready!:()=>void;f.start(()=>new Promise(resolve=>{ready=resolve}));
  f.hold.press();f.tick();f.hold.release();assert.equal(f.hold.phase,'starting');
  ready();await flush();assert.deepEqual(f.texts,['dictated text']);assert.equal(f.hold.consumeClick(),true);
});
await test("cancel during startup revokes a late microphone and never adds text",async()=>{
  const f=fixture();let ready!:()=>void;f.start(()=>new Promise(resolve=>{ready=resolve}));
  f.hold.press();f.tick();f.hold.cancel();ready();await flush();
  assert.deepEqual(f.texts,[]);assert.equal(f.hold.phase,'idle');
  assert.equal(f.calls.filter(c=>c.startsWith('cancel:')).length,2);
});
await test("cancel or session change discards late inference and leaves the draft alone",async()=>{
  const f=fixture();let result!:(s:string)=>void;f.finish(()=>new Promise(resolve=>{result=resolve}));
  f.hold.press();f.tick();await flush();f.hold.release();f.hold.cancel();result('old session text');await flush();
  assert.deepEqual(f.texts,[]);assert.equal(f.hold.phase,'idle');
});
await test("microphone failures release the gesture without accidentally sending",async()=>{
  const f=fixture();f.start(async()=>{throw Error('permission denied')});f.hold.press();f.tick();await flush();
  assert.equal(f.errors.length,1);assert.equal(f.hold.consumeClick(),true);assert.equal(f.hold.phase,'idle');
  f.hold.clearClick();assert.equal(f.hold.consumeClick(),false);
});
await test("the maximum recording deadline transcribes once",async()=>{
  const f=fixture();f.hold.press();f.tick();await flush();f.tick();await flush();
  assert.deepEqual(f.texts,['dictated text']);f.hold.release();await flush();assert.equal(f.texts.length,1);
});
await test("dictation is desktop-only and requires the top-level native capability",async()=>{
  assert.equal(desktopDictationAvailable(),false);
  const old=Object.getOwnPropertyDescriptor(globalThis,'window');
  const host={top:null as unknown,__LETHETIC_DESKTOP__:true,__LETHETIC_HOST_TOKEN__:'private',__TAURI__:{core:{invoke:async(command:string,args:Record<string,unknown>)=>{
    assert.equal(command,'plugin:lethetic-host|start_dictation');assert.equal(args["token"],'private');assert.equal(args["id"],'one');return 'ok';
  }}}};host.top=host;Object.defineProperty(globalThis,'window',{configurable:true,value:host});
  try {
    assert.equal(desktopDictationAvailable(),true);assert.equal(await dictationCommand('start_dictation',{id:'one'}),'ok');
    host.top={};assert.equal(desktopDictationAvailable(),false);await assert.rejects(()=>dictationCommand('start_dictation'),/desktop/);
    host.top=host;host.__LETHETIC_DESKTOP__=false;assert.equal(desktopDictationAvailable(),false);
  } finally {if(old)Object.defineProperty(globalThis,'window',old);else Reflect.deleteProperty(globalThis,'window');}
});
