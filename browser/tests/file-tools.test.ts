import "../../web/tests/support/files-globals.js";
import assert from "node:assert/strict";
import test from "node:test";
import { dateWorkspace, emptyWorkspace, encodeBytes, parseWorkspace } from "../src/workspace.js";
import { sortSessionFiles } from "../src/file-tools.js";
import { workspaceZip } from "../src/file-zip.js";

await test("session dates survive unchanged worker snapshots and change only with bytes", () => {
  const original = parseWorkspace({version:1,directories:[],files:[{path:"old",data:""}]});
  const first = dateWorkspace({...original,files:[...original.files,{path:"new",data:"YQ=="}]}, original, 1000);
  assert.equal(first.files.find(file=>file.path==="old")?.modifiedAt, undefined);
  assert.equal(first.files.find(file=>file.path==="new")?.modifiedAt, 1000);
  const worker = {...first, files:first.files.map(({path,data})=>({path,data}))};
  assert.deepEqual(dateWorkspace(worker, first, 2000), first);
  worker.files.find(file=>file.path==="old")!.data="Yg==";
  const edited = dateWorkspace(worker, first, 3000);
  assert.equal(edited.files.find(file=>file.path==="old")?.modifiedAt, 3000);
  assert.deepEqual(parseWorkspace(JSON.parse(JSON.stringify(edited))), edited);
  assert.deepEqual(sortSessionFiles(edited.files,"date",true).map(file=>file.path),["old","new"]);
  assert.deepEqual(sortSessionFiles([{path:"big",data:"YWJj"},{path:"small",data:"YQ=="}],"size",true).map(file=>file.path),["big","small"]);
});

await test("ZIP preserves nested Unicode names, binary bytes, empty directories and CRCs", () => {
  const workspace = parseWorkspace({version:1,directories:["empty"],files:[
    {path:"nested/名字.bin",data:encodeBytes(new Uint8Array([0,255,1]))},
    {path:"crc.txt",data:encodeBytes(new TextEncoder().encode("123456789"))},
  ]});
  const zip = workspaceZip(workspace), view = new DataView(zip.buffer);
  const extracted = new Map<string,Uint8Array>(); let offset=0;
  while(view.getUint32(offset,true)===0x04034b50) {
    assert.equal(view.getUint16(offset+6,true),0x800);
    assert.equal(view.getUint16(offset+8,true),0);
    const size=view.getUint32(offset+18,true), length=view.getUint16(offset+26,true);
    const name=new TextDecoder().decode(zip.slice(offset+30,offset+30+length));
    if(name==="crc.txt")assert.equal(view.getUint32(offset+14,true),0xcbf43926);
    extracted.set(name,zip.slice(offset+30+length,offset+30+length+size)); offset+=30+length+size;
  }
  assert.deepEqual(extracted.get("nested/名字.bin"),new Uint8Array([0,255,1]));
  assert.equal(extracted.get("empty/")?.length,0);
  assert.equal(view.getUint32(offset,true),0x02014b50);
  assert.equal(view.getUint32(zip.length-22,true),0x06054b50);
  assert.equal(view.getUint16(zip.length-12,true),4);
  assert.equal(view.getUint32(zip.length-6,true),offset);
  assert.equal(workspaceZip(emptyWorkspace()).length,22);
});
