/** Fixed-size SVGs must fit both the narrow Files pane and the full preview viewport. */
export async function testSvgPreview(
  evaluate: (source: string) => Promise<unknown>,
  inspect: (source: string, full?: boolean, selector?: string) => Promise<unknown>,
): Promise<void> {
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="800" height="900" viewBox="0 0 800 900">
    <style>@keyframes pulse {from{opacity:.5}to{opacity:1}} .animated{animation:pulse 1s infinite alternate}</style>
    <defs><linearGradient id="sky"><stop stop-color="red"/><stop offset="1" stop-color="blue"/></linearGradient>
    <filter id="glow"><feGaussianBlur stdDeviation="2"/></filter></defs>
    <rect width="800" height="900" fill="url(#sky)"/>
    <circle class="animated" cx="400" cy="450" r="100" filter="url(#glow)"/>
    <text x="400" y="880">Bottom of the artwork</text>
    <script>parent.svgPreviewEscaped=true</script><rect onload="parent.svgPreviewEscaped=true"/>
    </svg>`;
  const geometry = `(() => {
    const svg=document.querySelector('svg'), matrix=svg?.getScreenCTM();
    if(!matrix)return {visible:false};
    const first=new DOMPoint(0,0).matrixTransform(matrix), last=new DOMPoint(800,900).matrixTransform(matrix);
    return {visible:first.x>=0 && first.y>=0 && last.x<=innerWidth+1 && last.y<=innerHeight+1,
      width:svg.getAttribute('width'),height:svg.getAttribute('height'),
      animated:document.querySelector('.animated').getAnimations().length>0,
      gradient:!!document.querySelector('linearGradient stop'),filter:!!document.querySelector('feGaussianBlur'),
      unsafe:!!document.querySelector('script:not([data-preview-token]),[onload]'),
      bounds:[first.x,first.y,last.x,last.y,innerWidth,innerHeight]};
  })()`;
  const check = async (full: boolean, selector?: string): Promise<void> => {
    const result = await inspect(geometry, full, selector) as {visible:boolean;width:string;height:string;animated:boolean;gradient:boolean;filter:boolean;unsafe:boolean};
    if (!result.visible || result.width !== "800" || result.height !== "900" || !result.animated || !result.gradient || !result.filter || result.unsafe) {
      throw Error("SVG preview clipped artwork, lost styling, or retained active content: " + JSON.stringify(result));
    }
  };
  try {
    await evaluate(`(async()=>{
      const {workspacePreview,createFilePreview}=await import('./src/file-actions.js');
      const {encodeBytes}=await import('./src/workspace.js');
      globalThis.svgPreviewFile={path:'animated.svg',data:encodeBytes(new TextEncoder().encode(${JSON.stringify(svg)}))};
      globalThis.svgPreviewDialog=createFilePreview();
      const frame=document.createElement('iframe');frame.id='svg-preview-regression';frame.sandbox='';
      frame.style='position:fixed;left:0;top:0;width:400px;height:340px';
      const loaded=new Promise(resolve=>frame.onload=resolve);
      frame.srcdoc=workspacePreview(svgPreviewFile);document.body.append(frame);await loaded;
    })()`);
    for (const [width, height] of [[400, 340], [180, 140], [800, 320]]) {
      await evaluate(`(async()=>{const frame=document.getElementById('svg-preview-regression');frame.style.width='${width}px';frame.style.height='${height}px';await new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)));})()`);
      await check(false, "#svg-preview-regression");
    }
    await evaluate("document.getElementById('svg-preview-regression').remove();svgPreviewDialog.openFile(svgPreviewFile)");
    await check(false);
    await evaluate("svgPreviewDialog.dialog.querySelector('[data-field=preview-expand]').click()");
    await check(true);
    if (await evaluate("!!globalThis.svgPreviewEscaped")) throw Error("SVG executed user script");
  } finally {
    await evaluate("document.getElementById('svg-preview-regression')?.remove();globalThis.svgPreviewDialog?.destroy();delete globalThis.svgPreviewDialog;delete globalThis.svgPreviewFile;");
  }
}
