import assert from "node:assert/strict";
import test from "node:test";
import { emptyWorkspace, encodeBytes, decodeBytes, parseWorkspace, textFile, FILE_BYTES } from "../src/workspace.js";
import { captureWorkspace, restoreWorkspace } from "../src/python-filesystem.js";
import type { PythonFS } from "../src/python-filesystem.js";
import { BrowserFiles } from "../src/files.js";

await test("workspace preserves binary bytes, UTF-8 BOM and empty nested directories", async () => {
  const bytes = new Uint8Array([0, 255, 128, 10]);
  let workspace = parseWorkspace({ version: 1, directories: ['reports/empty'], files: [
    { path: 'reports/data.bin', data: encodeBytes(bytes) },
    { path: 'bom.txt', data: encodeBytes(new Uint8Array([239,187,191,65])) },
  ] });
  assert.deepEqual(workspace.directories, ['reports','reports/empty']);
  assert.equal(textFile(workspace.files.find(f=>f.path==='bom.txt')!), '\ufeffA');
  const files = new BrowserFiles(); files.bindWorkspace(()=>structuredClone(workspace), value=>{workspace=parseWorkspace(value);});
  const signal = new AbortController().signal;
  const listing = await (await files.request('/api/files/list','reports',signal)).json();
  assert.deepEqual(listing.entries.map((entry: {name:string})=>entry.name), ['data.bin','empty']);
  const download = await files.request('/api/files/download','reports/data.bin',signal);
  assert.deepEqual(new Uint8Array(await download.arrayBuffer()), bytes);
  assert.equal((await files.request('/api/files/read','reports/data.bin',signal)).status,415);
  files.save('reports/new.csv','value\n42');
  assert.equal(textFile(workspace.files.find(file=>file.path==='reports/new.csv')!), 'value\n42');
  files.remove('reports/data.bin'); assert.ok(!workspace.files.some(file=>file.path.endsWith('.bin')));
});

await test("workspace rejects traversal, path collisions, malformed data and quotas before mutation", () => {
  for (const path of ['../escape', '/absolute', 'a//b', 'C:/drive', 'a/./b']) {
    assert.throws(()=>parseWorkspace({version:1,directories:[],files:[{path,data:''}]}));
  }
  for (const data of ['bad!', 'YQ', 'YR==', '====']) assert.throws(()=>parseWorkspace({version:1,directories:[],files:[{path:'file',data}]}));
  assert.throws(()=>parseWorkspace({version:1,directories:['a/b'],files:[{path:'a',data:''}]}));
  assert.throws(()=>parseWorkspace({version:1,directories:[],files:[{path:'a',data:''},{path:'a',data:''}]}));
  const data = encodeBytes(new Uint8Array(FILE_BYTES));
  assert.equal(decodeBytes(parseWorkspace({version:1,directories:[],files:[{path:'a',data}]}).files[0]!.data).length, FILE_BYTES);
  assert.throws(()=>parseWorkspace({version:1,directories:[],files:['a','b','c'].map(path=>({path,data}))}), /2 MiB/);
});

class MemoryFS implements PythonFS {
  nodes = new Map<string, { mode: number; data?: Uint8Array }>([['/', {mode:0x4000}],['/home',{mode:0x4000}]]);
  lstat(path: string) { const node=this.nodes.get(path); if(!node)throw Error('ENOENT'); return {mode:node.mode,size:node.data?.length ?? 0}; }
  isDir(mode: number) { return mode===0x4000; }
  isFile(mode: number) { return mode===0x8000; }
  readdir(path: string) { const prefix=path+'/'; return ['.','..',...[...this.nodes.keys()].filter(p=>p.startsWith(prefix)&&!p.slice(prefix.length).includes('/')).map(p=>p.slice(prefix.length))]; }
  readFile(path: string) { return this.nodes.get(path)!.data!.slice(); }
  writeFile(path: string,data: Uint8Array) { this.nodes.set(path,{mode:0x8000,data:data.slice()}); }
  mkdirTree(path: string) { let current=''; for(const part of path.split('/').filter(Boolean)){current+='/'+part;this.nodes.set(current,{mode:0x4000});} }
  unlink(path: string) { this.nodes.delete(path); }
  rmdir(path: string) { this.nodes.delete(path); }
  chdir(_path: string) {}
}

await test("filesystem snapshots restore deletions and never traverse symlinks or include runtime files", () => {
  const fs = new MemoryFS(); fs.mkdirTree('/lib'); fs.writeFile('/lib/runtime',new Uint8Array([99]));
  const workspace = parseWorkspace({version:1,directories:['empty'],files:[{path:'nested/file',data:'AP8='}]});
  restoreWorkspace(fs,workspace); assert.deepEqual(captureWorkspace(fs),workspace);
  fs.nodes.set('/home/pyodide/link',{mode:0xa000}); assert.throws(()=>captureWorkspace(fs),/symbolic/);
  restoreWorkspace(fs,emptyWorkspace()); assert.deepEqual(captureWorkspace(fs),emptyWorkspace());
  assert.deepEqual(fs.readFile('/lib/runtime'),new Uint8Array([99]));
});
