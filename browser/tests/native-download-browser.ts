type Evaluate = (code: string) => Promise<unknown>;
type Until = (code: string) => Promise<void>;

/** Exercise the actual buttons with the native IPC boundary replaced by a recorder. */
export async function testNativeFileDownloads(evaluate: Evaluate, until: Until): Promise<void> {
  await install(evaluate);
  try {
    await click(evaluate, until, '.files-content-actions button[title^="Download the selected file"]', 1);
    await verify(evaluate, "nativeSaved[0].name==='analysis.py' && new TextDecoder().decode(new Uint8Array(nativeSaved[0].bytes))===\"print('private snippet')\"", "native single-file bytes");
    await click(evaluate, until, '[data-browser-action=download]', 2);
    await verify(evaluate, "nativeSaved[1].name==='snippet.py' && new TextDecoder().decode(new Uint8Array(nativeSaved[1].bytes)).includes('private snippet')", "native snippet download");
    await click(evaluate, until, '[data-file-action=download-all]', 3);
    await verify(evaluate, "nativeSaved[2].name==='session-files.zip' && nativeSaved[2].bytes[0]===80 && nativeSaved[2].bytes[1]===75", "native ZIP download");
    await evaluate("globalThis.nativeSaveResult=false");
    await click(evaluate, until, '.files-content-actions button[title^="Download the selected file"]', 4);
    await until("document.body.textContent.includes('Download cancelled.')");
    await evaluate("globalThis.nativeSaveFailure=true");
    await evaluate("document.querySelector('[data-browser-action=download]').click()");
    await until("document.body.textContent.includes('Download failed: Error: Native disk full')");
  } finally { await uninstall(evaluate); }
}

export async function testNativeSessionDownloads(evaluate: Evaluate, until: Until): Promise<void> {
  await install(evaluate);
  try {
    await click(evaluate, until, '[data-session-export]', 1);
    await verify(evaluate, "nativeSaved[0].name.endsWith('.tar.gz') && nativeSaved[0].bytes[0]===31 && nativeSaved[0].bytes[1]===139", "native debug archive download");
    await until("!document.querySelector('[data-session-export]').disabled");
    await click(evaluate, until, '[data-session-database]', 2);
    await verify(evaluate, "nativeSaved[1].name.endsWith('.db') && new TextDecoder().decode(new Uint8Array(nativeSaved[1].bytes.slice(0,15)))==='SQLite format 3'", "native database download");
    await until("!document.querySelector('[data-session-database]').disabled");
  } finally { await uninstall(evaluate); }
}
async function click(evaluate: Evaluate, until: Until, selector: string, count: number): Promise<void> {
  await evaluate(`document.querySelector(${JSON.stringify(selector)}).click()`);
  await until(`globalThis.nativeSaved.length===${count} && globalThis.nativeSaveIdle`);
}
async function verify(evaluate: Evaluate, code: string, message: string): Promise<void> {
  if (!await evaluate(code)) throw new Error(message);
}
async function install(evaluate: Evaluate): Promise<void> {
  await evaluate(`{
    globalThis.nativeDownloadDescriptors=['__LETHETIC_HOST_TOKEN__','__TAURI__'].map(name=>[name,Object.getOwnPropertyDescriptor(window,name)]);
    globalThis.nativeSaved=[];globalThis.nativeSaveResult=true;globalThis.nativeSaveFailure=false;globalThis.nativeSaveIdle=true;
    globalThis.nativeDownloadAnchor=HTMLAnchorElement.prototype.click;
    HTMLAnchorElement.prototype.click=function(){throw Error('Native download used a browser anchor')};
    Object.defineProperty(window,'__LETHETIC_HOST_TOKEN__',{configurable:true,value:'test-download-main-capability'});
    Object.defineProperty(window,'__TAURI__',{configurable:true,value:{core:{invoke:async(command,args)=>{
      if(args.token!=='test-download-main-capability')throw Error('Missing main-window capability');
      if(command.endsWith('begin_download')){nativeSaveIdle=false;globalThis.nativeTransfer={name:args.name,mime:args.mime,size:args.size,bytes:[]};return 'transfer';}
      if(command.endsWith('write_download')){
        if(nativeSaveFailure)throw Error('Native disk full');
        if(args.offset!==nativeTransfer.bytes.length)throw Error('Wrong chunk offset');
        for(const byte of atob(args.data))nativeTransfer.bytes.push(byte.charCodeAt(0));return;
      }
      if(command.endsWith('finish_download')){
        if(nativeTransfer.size!==nativeTransfer.bytes.length)throw Error('Incomplete native file');
        nativeSaved.push(nativeTransfer);return nativeSaveResult;
      }
      if(command.endsWith('cancel_download'))nativeSaveIdle=true;
    }}}});
    void 0;
  }`);
}
async function uninstall(evaluate: Evaluate): Promise<void> {
  await evaluate(`HTMLAnchorElement.prototype.click=globalThis.nativeDownloadAnchor;
    for(const [name,descriptor] of globalThis.nativeDownloadDescriptors){if(descriptor)Object.defineProperty(window,name,descriptor);else delete window[name];}`);
}
