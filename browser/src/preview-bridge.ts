// Loaded as a classic script inside an opaque, otherwise inert preview iframe.
// It has no same-origin access; the parent verifies source and per-document token.
const previewBridgeToken = (document.currentScript as HTMLScriptElement | null)?.dataset["previewToken"];
let previewBridgeFull = false;
addEventListener("message", event => {
  if (event.source === parent && event.data?.token === previewBridgeToken) previewBridgeFull = event.data.full === true;
});
const previewBridgeSend = (direction: number): void => parent.postMessage({type:"lethetic-preview", token:previewBridgeToken, direction}, "*");
const previewBridgeSelected = (): boolean => !!getSelection()?.toString();
addEventListener("keydown", event => {
  if (!previewBridgeFull || event.shiftKey || event.ctrlKey || event.metaKey || event.altKey || previewBridgeSelected() ||
    (event.target instanceof Element && event.target.closest("input,textarea,[contenteditable]"))) return;
  if (event.key === "ArrowLeft" || event.key === "ArrowRight") { event.preventDefault(); previewBridgeSend(event.key === "ArrowLeft" ? -1 : 1); }
  if (event.key === "Escape") { event.preventDefault(); previewBridgeSend(0); }
});
let previewBridgeStart: {x:number;y:number;time:number} | null = null;
addEventListener("touchstart", event => {
  const touch = event.touches[0];
  previewBridgeStart = event.touches.length === 1 && touch ? {x:touch.clientX,y:touch.clientY,time:Date.now()} : null;
}, {passive:true});
addEventListener("touchend", event => {
  const start = previewBridgeStart, end = event.changedTouches[0]; previewBridgeStart = null;
  if (!previewBridgeFull || !start || !end || previewBridgeSelected()) return;
  const dx = end.clientX-start.x, dy = end.clientY-start.y;
  if (Date.now()-start.time < 600 && Math.abs(dx)>70 && Math.abs(dx)>Math.abs(dy)*2) previewBridgeSend(dx<0?1:-1);
}, {passive:true});
