export async function testVirtualChat(evaluate: (source: string) => Promise<unknown>): Promise<void> {
  await evaluate(`(async()=>{
    const {SpaApplication}=await import('./web/src/app.js');
    const {LocalTransport}=await import('./src/local-transport.js');
    const root=document.createElement('div');root.style='height:800px;width:900px';document.body.replaceChildren(root);
    const mount=document.createElement('div');root.append(mount);
    const app=new SpaApplication(mount),transport=new LocalTransport(app,{connections:[]});
    const check=(value,message)=>{if(!value)throw Error(message)};
    const wait=async fn=>{for(let n=0;n<100;n++){await new Promise(r=>setTimeout(r,25));if(fn())return;}throw Error('Virtual chat timed out '+JSON.stringify({top:document.querySelector('#chat-scroll')?.scrollTop,height:document.querySelector('#chat-scroll')?.scrollHeight,rows:Array.from(document.querySelectorAll('[data-chat-anchor]')).map(row=>row.dataset.chatAnchor)}))};
    const block=i=>({kind:'text',content:('Row '+i+' long content '.repeat(5)+'\\n').repeat(i%7===0?40:2),title:'Row '+i,
      content_loss:{filtered:false,redacted:false,truncation:null},title_loss:{filtered:false,redacted:false,truncation:null},tool:null,success:null,usage:null,estimated_cost:null,duration_label:null});
    try {
      app.attachTransport(transport);transport.start();
      transport.state.blocks.blocks=Array.from({length:1000},(_,i)=>block(i));transport.publish();
      const chat=document.querySelector('#chat-scroll');
      await wait(()=>chat.scrollHeight-chat.scrollTop-chat.clientHeight<3 && document.querySelector('[data-chat-anchor="block-999"]'));
      check(chat.querySelectorAll('[data-chat-index]').length<=60,'Long history bloated the DOM');
      chat.dispatchEvent(new PointerEvent('pointerenter',{pointerType:'mouse'}));
      chat.dispatchEvent(new WheelEvent('wheel',{deltaY:-100}));chat.scrollTop=0;chat.dispatchEvent(new Event('scroll'));
      await wait(()=>!!chat.querySelector('[data-chat-anchor="block-0"]'));
      check(chat.querySelectorAll('[data-chat-index]').length<=60,'Older history accumulated DOM rows');
      chat.dispatchEvent(new WheelEvent('wheel',{deltaY:100}));chat.scrollTop=chat.scrollHeight/2;chat.dispatchEvent(new Event('scroll'));
      await new Promise(r=>setTimeout(r,250));
      const visible=()=>Array.from(chat.querySelectorAll('[data-chat-anchor]')).find(row=>row.getBoundingClientRect().bottom>chat.getBoundingClientRect().top && row.getBoundingClientRect().top<chat.getBoundingClientRect().bottom);
      const anchor=visible();check(anchor,'Middle of history is blank');
      // A native scroll can land after a status render but before its queued RAF.
      // Preserve every input delta instead of snapping back to the previous frame.
      for(let step=0;step<8;step++) {
        const row=visible(),key=row.dataset.chatAnchor,before=row.getBoundingClientRect().top-chat.getBoundingClientRect().top;
        transport.publish();
        chat.scrollTop-=12;chat.dispatchEvent(new Event('scroll'));
        await new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)));
        const retained=chat.querySelector('[data-chat-anchor="'+key+'"]');
        check(retained && Math.abs(retained.getBoundingClientRect().top-chat.getBoundingClientRect().top-before-12)<3,'Status rendering swallowed an upward scroll delta at step '+step+' '+JSON.stringify({key,before,after:retained?.getBoundingClientRect().top-chat.getBoundingClientRect().top,top:chat.scrollTop,rows:[...chat.querySelectorAll('[data-chat-anchor]')].map(row=>row.dataset.chatAnchor)}));
      }
      const current=visible();
      const key=current.dataset.chatAnchor,offset=current.getBoundingClientRect().top-chat.getBoundingClientRect().top;
      transport.state.blocks.blocks.push(block(1000));transport.publish();
      await new Promise(r=>setTimeout(r,250));
      const retained=chat.querySelector('[data-chat-anchor="'+key+'"]');
      check(retained && Math.abs(retained.getBoundingClientRect().top-chat.getBoundingClientRect().top-offset)<3,'New output moved the reader');
      root.style.width='450px';await new Promise(r=>setTimeout(r,250));
      check(!!visible() && chat.querySelectorAll('[data-chat-index]').length<=60,'Resize broke virtualization');
      chat.dispatchEvent(new WheelEvent('wheel',{deltaY:1000}));chat.scrollTop=chat.scrollHeight;chat.dispatchEvent(new Event('scroll'));
      await wait(()=>!!chat.querySelector('[data-chat-anchor="block-1000"]') && chat.scrollHeight-chat.scrollTop-chat.clientHeight<3);
    } finally {app.dispose();transport.dispose();root.remove();}
  })()`);
}
