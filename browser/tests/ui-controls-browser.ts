/** Keyboard navigation, status icons, and preview clipboard behavior in the shared UI. */
export async function testUiControls(evaluate: (source: string) => Promise<unknown>): Promise<void> {
  await evaluate(`(async()=>{
    const {mountLethetic}=await import('./src/embed.js');
    const {createFilePreview}=await import('./src/file-actions.js');
    const root=document.createElement('div');root.style='height:500px;width:900px';document.body.replaceChildren(root);
    const view=mountLethetic(root,{connections:[{name:'UI test',provider:'openai',endpoint:location.origin+'/v1/chat/completions',model:'test'}]});
    const check=(ok,message)=>{if(!ok)throw Error(message)};
    const frame=()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)));
    const key=(target,value)=>target.dispatchEvent(new KeyboardEvent('keydown',{key:value,bubbles:true,cancelable:true}));
    const clipboard=Object.getOwnPropertyDescriptor(navigator,'clipboard'),copied=[];
    Object.defineProperty(navigator,'clipboard',{configurable:true,value:{writeText:async text=>{copied.push(text)}}});
    let preview;
    const originalFetch=window.fetch;
    const wait=async predicate=>{for(let i=0;i<400;i++){if(predicate())return;await new Promise(resolve=>setTimeout(resolve,20));}throw Error('UI condition timed out')};
    try {
      check(!root.querySelector('.connection-dot'),'Misleading connection dot remains');
      check(root.querySelectorAll('.header-capability .ui-icon').length===5,'Header capability icons missing');
      const connectionButton=root.querySelector('[data-browser-action=connections]');
      check(connectionButton.dataset.connectionState==='untested','Untested connection falsely shown connected');
      connectionButton.click();
      await wait(()=>connectionButton.dataset.connectionState==='valid');
      const dialog=document.querySelector('dialog[open]');
      const apiKey=dialog.querySelector('[data-field=key]');
      apiKey.value='different-draft-key';apiKey.dispatchEvent(new Event('input',{bubbles:true}));
      window.fetch=(url,init)=>String(url)===location.origin+'/v1/chat/completions' ?
        Promise.resolve(new Response(JSON.stringify({error:{message:'Invalid test key'}}),{status:401,headers:{'Content-Type':'application/json'}})) : originalFetch(url,init);
      dialog.querySelector('[data-field=test]').click();
      await wait(()=>dialog.querySelector('[data-field=test-state] .is-invalid'));
      check(connectionButton.dataset.connectionState==='valid','Testing an unsaved draft changed the active connection indicator');
      dialog.querySelector('[data-field=use]').click();
      await wait(()=>connectionButton.dataset.connectionState==='invalid');
      window.fetch=originalFetch;
      if(dialog.open)dialog.close();await frame();

      root.querySelector('[aria-label="Open command palette"]').click();await frame();
      const list=root.querySelector('#palette-list');
      check(list.scrollHeight>list.clientHeight,'Palette fixture does not overflow');
      const visible=()=>{const row=list.querySelector('[aria-selected="true"]').getBoundingClientRect(),box=list.getBoundingClientRect();return row.top>=box.top-1 && row.bottom<=box.bottom+1};
      list.focus();key(list,'End');await frame();
      check(list.scrollTop>0 && visible(),'End did not reveal selected command');
      for(let i=0;i<10;i++){key(list,'ArrowUp');check(visible(),'Up arrow left selection outside palette')}
      key(list,'Home');check(visible() && list.scrollTop<5,'Home did not reveal first command');
      for(let i=0;i<list.children.length-1;i++){key(list,'ArrowDown');check(visible(),'Down arrow left selection outside palette')}
      check([...list.querySelectorAll('.choice-icon')].every(icon=>icon.querySelector('.ui-icon')?.textContent),'Palette command has no icon');
      check(new Set([...list.querySelectorAll('.choice-icon')].map(icon=>icon.textContent)).size>=10,'Palette icons are indistinguishable');
      const search=root.querySelector('#palette-search');search.focus();search.value='persona';search.dispatchEvent(new Event('input',{bubbles:true}));
      key(search,'ArrowDown');
      check(document.activeElement===search && list.children[1].getAttribute('aria-selected')==='true','Search arrow navigation lost focus or selection');
      search.value='nothing-matches-this-command';search.dispatchEvent(new Event('input',{bubbles:true}));key(search,'ArrowDown');
      check(list.children.length===0,'Empty palette failed');
      key(search,'Escape');await frame();

      preview=createFilePreview();
      const code='<h1>Hello & goodbye</h1>';
      preview.open(code,'html','first.html');await frame();
      const copy=preview.dialog.querySelector('[data-field=preview-copy]');
      copy.click();await Promise.resolve();
      check(copied.at(-1)===code,'Rendered preview copied markup output instead of original contents');
      preview.dialog.querySelector('[data-field=preview-expand]').click();
      copy.click();await Promise.resolve();check(copied.at(-1)===code,'Expanded preview Copy failed');
      const text=(path,value)=>({path,data:btoa(value),modifiedAt:1});
      // Use the same UTF-8 workspace shape as session files.
      const files=[text('first.txt','first'),text('second.txt','second')];
      preview.openFile(files[0],files,true);
      preview.dialog.querySelector('[data-field=preview-next]').click();copy.click();await Promise.resolve();
      check(copied.at(-1)==='second','Copy retained the previous file after navigation');
      preview.openFile({path:'pixel.png',data:'AA==',modifiedAt:1});
      check(copy.disabled,'Binary preview attempted text clipboard copy');
    } finally {
      window.fetch=originalFetch;preview?.destroy();view.destroy();root.remove();
      if(clipboard)Object.defineProperty(navigator,'clipboard',clipboard);else delete navigator.clipboard;
    }
  })()`);
}
