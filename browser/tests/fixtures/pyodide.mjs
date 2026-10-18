// Offline worker dependency fixture; real Pyodide is exercised with --pyodide.
export async function loadPyodide() {
  let stdout;
  const nodes = new Map([['/', {mode: 0x4000}], ['/home', {mode: 0x4000}], ['/home/pyodide', {mode: 0x4000}]]);
  const FS = {
    lstat(path) { const node=nodes.get(path); if(!node) throw Error('ENOENT'); return {mode:node.mode,size:node.data?.length ?? 0}; },
    isDir(mode) { return mode===0x4000; }, isFile(mode) { return mode===0x8000; },
    readdir(path) { const prefix=path==='/'?'/':path+'/'; return ['.','..',...[...nodes.keys()].filter(p=>p.startsWith(prefix)&&p!==path&&!p.slice(prefix.length).includes('/')).map(p=>p.slice(prefix.length))]; },
    readFile(path) { if(!nodes.has(path))throw Error('ENOENT');return nodes.get(path).data.slice(); },
    writeFile(path,data) { nodes.set(path,{mode:0x8000,data:data.slice()}); },
    mkdirTree(path) { let current=''; for(const part of path.split('/').filter(Boolean)){current+='/'+part;nodes.set(current,{mode:0x4000});} },
    unlink(path) { nodes.delete(path); }, rmdir(path) { nodes.delete(path); }, chdir() {},
  };
  return { FS, setStdout(o){stdout=o.batched;},setStderr(){},setStdin(){},
    async loadPackagesFromImports(){},async runPythonAsync(code){
      if(code.includes('while True')){while(true){}}
      if(code.includes('# create-session-files')) {
        FS.mkdirTree('/home/pyodide/outputs/empty');
        FS.writeFile('/home/pyodide/outputs/data.csv',new TextEncoder().encode('value\n42\n'));
        FS.writeFile('/home/pyodide/outputs/chart.png',Uint8Array.from(atob('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII='),c=>c.charCodeAt(0)));
        stdout('files created');
      } else if(code.includes('# read-session-file')) stdout('read: '+new TextDecoder().decode(FS.readFile('/home/pyodide/outputs/data.csv')));
      else if(code.includes('# restore-session-file')) stdout('restored: '+new TextDecoder().decode(FS.readFile('/home/pyodide/outputs/data.csv')));
      else if(code.includes('# delete-session-file')) { FS.unlink('/home/pyodide/outputs/data.csv');stdout('file deleted'); }
      else if(code.includes('# list-session-files')) stdout('workspace empty: '+String(FS.readdir('/home/pyodide').length===2));
      else if(code.includes('raise'))throw new Error('fixture Python error');
      else stdout('42');
      return undefined;
    }};
}
