import assert from "node:assert/strict";
import test from "node:test";
import { ThinkingLines } from "../src/app/thinking-lines.js";

await test("thinking copy rotates only during processing and cancels timers at idle/unmount",()=>{
  const timers=new Map<number,()=>void>();let sequence=0,changes=0;
  const scheduler={setTimeout(callback:()=>void,delay:number):number {assert.ok(delay>=10_000);timers.set(++sequence,callback);return sequence;},clearTimeout(timer:number):void {timers.delete(timer);}};
  const lines=new ThinkingLines(()=>changes++,scheduler);
  assert.equal(lines.current(null,false),null);assert.equal(timers.size,0);
  const first=lines.current('turn-1',false);assert.ok(first);
  const tick=timers.get(sequence)!;timers.delete(sequence);tick();assert.equal(changes,1);
  assert.notEqual(lines.current('turn-1',false),first);assert.equal(timers.size,1);
  assert.equal(lines.current(null,false),null);assert.equal(timers.size,0);
  assert.equal(lines.current('turn-2',true),first);assert.equal(timers.size,0);
  lines.current('turn-2',false);assert.equal(timers.size,1);lines.dispose();assert.equal(timers.size,0);
});
