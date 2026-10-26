import { downloadBlob } from "../../web/src/download.js";
import { MonacoFileEditor } from "../../web/src/files/editor.js";
import type { FilesReadResponse } from "../../web/src/generated/contracts.js";
import { buttonIconHtml } from "../../web/src/icons.js";
import { parse } from "../../web/lib/marked/marked.esm.js";
import { createDialog, field, errorMessage } from "./dialog.js";
import { textFile } from "./workspace.js";
import type { WorkspaceFile } from "./workspace.js";
import type { CodeSnippet } from "./code-snippets.js";

export function snippetName(snippet: CodeSnippet): string {
  const extensions: Record<string, string> = { csharp: "cs", "c#": "cs", python: "py", javascript: "js", typescript: "ts", markdown: "md", text: "txt" };
  const extension = extensions[snippet.language] ?? (/^[a-z0-9]{1,8}$/u.test(snippet.language) ? snippet.language : "txt");
  return `snippet.${extension}`;
}
export function downloadCode(code: string, name: string): Promise<boolean> {
  return downloadBlob(new Blob([code], { type: "text/plain;charset=utf-8" }), name);
}
export async function copyCode(code: string): Promise<void> {
  if (!navigator.clipboard?.writeText) throw new Error("Clipboard is unavailable. Download the file or select and copy its text.");
  await navigator.clipboard.writeText(code);
}

/** Static HTML/SVG/Markdown and local media, inside an opaque sandbox with no scripts or network. */
export function previewDocument(code: string, language: string, name: string): string {
  if (code.length > 24 * 1024 * 1024) throw new Error("Preview is limited to 24 MiB of encoded content.");
  const extension = name.split(".").at(-1)?.toLowerCase() ?? "";
  const kind = language.toLowerCase();
  const standaloneSvg = [kind, extension].includes("svg") && ![kind, extension].some(value => ["md", "markdown"].includes(value));
  let html: string;
  if ([kind, extension].some((value) => ["md", "markdown"].includes(value))) html = parse(code, { async: false });
  else if ([kind, extension].some((value) => ["html", "htm", "svg"].includes(value))) html = code;
  else {
    let text = code;
    if (kind === "json" || extension === "json") { try { text = JSON.stringify(JSON.parse(code), null, 2); } catch { /* Show the original text. */ } }
    html = `<pre>${text.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;")}</pre>`;
  }
  const template = document.createElement("template"); template.innerHTML = html;
  for (const element of template.content.querySelectorAll("script,meta,base,link,iframe,object,embed,form,template,foreignObject,animate,animateMotion,animateTransform,set")) element.remove();
  for (const element of template.content.querySelectorAll("*")) {
    for (const attribute of [...element.attributes]) {
      const name = attribute.name.toLowerCase();
      if (name.startsWith("on") || ["srcdoc", "nonce", "action", "formaction", "srcset", "ping", "autofocus", "autoplay"].includes(name) ||
          (name.endsWith("href") && !attribute.value.startsWith("#")) ||
          (name === "src" && !/^data:image\//iu.test(attribute.value) && !(["AUDIO", "VIDEO", "SOURCE"].includes(element.tagName.toUpperCase()) && /^data:(audio|video)\//iu.test(attribute.value)))) element.removeAttribute(attribute.name);
    }
  }
  // Sandboxed documents cannot inherit the host's theme variables. Resolve the
  // colors in the host and use low-specificity defaults so authored styles win.
  const theme = getComputedStyle(document.documentElement);
  return `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; img-src data:; media-src data:; form-action 'none'; base-uri 'none'"><style>:where(body){color:${theme.color};background:${theme.backgroundColor}}body{font:16px system-ui;padding:20px;overflow-wrap:anywhere}pre{white-space:pre-wrap}img,svg,audio,video{max-width:100%}audio,video{display:block;margin:auto}video{max-height:85vh}.preview-svg{margin:0;padding:12px}.preview-svg>svg{display:block;width:100%;height:auto;max-height:calc(100vh - 24px);margin:auto}table{border-collapse:collapse}td,th{border:1px solid #aaa;padding:6px}</style></head><body${standaloneSvg ? ' class="preview-svg"' : ""}>${template.innerHTML}</body></html>`;
}

function imageType(path: string): string | undefined {
  const types: Record<string, string> = { png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif", webp: "image/webp", avif: "image/avif", bmp: "image/bmp", ico: "image/x-icon" };
  return types[path.split(".").at(-1)?.toLowerCase() ?? ""];
}
function mediaType(path: string): { tag: "audio" | "video"; mime: string } | undefined {
  const extension = path.split(".").at(-1)?.toLowerCase() ?? "";
  const audio: Record<string,string> = { mp3:"audio/mpeg", wav:"audio/wav", wave:"audio/wav", ogg:"audio/ogg", oga:"audio/ogg", opus:"audio/ogg; codecs=opus", m4a:"audio/mp4", aac:"audio/aac", flac:"audio/flac" };
  const video: Record<string,string> = { mp4:"video/mp4", m4v:"video/mp4", webm:"video/webm", ogv:"video/ogg", mov:"video/quicktime" };
  if (audio[extension]) return {tag:"audio",mime:audio[extension]};
  if (video[extension]) return {tag:"video",mime:video[extension]};
  return undefined;
}
function playableMedia(path: string): ReturnType<typeof mediaType> {
  const media = mediaType(path);
  return media && document.createElement(media.tag).canPlayType(media.mime) ? media : undefined;
}
export function canPreviewFile(file: WorkspaceFile | undefined): boolean {
  return !!file && (imageType(file.path) !== undefined || playableMedia(file.path) !== undefined || textFile(file) !== null);
}
export function canRenderFile(file: WorkspaceFile | undefined): boolean {
  return !!file && (imageType(file.path) !== undefined || playableMedia(file.path) !== undefined || (/\.(svg|html?|md|markdown)$/iu.test(file.path) && textFile(file) !== null));
}
export function workspacePreview(file: WorkspaceFile): string {
  const media = playableMedia(file.path);
  if (media) return previewDocument(`<${media.tag} controls preload="metadata" playsinline src="data:${media.mime};base64,${file.data}"></${media.tag}>`, "html", "preview.html");
  const mime = imageType(file.path);
  if (mime) return previewDocument(`<img alt="Session file" src="data:${mime};base64,${file.data}">`, "html", "preview.html");
  const content = textFile(file);
  if (content === null) throw new Error("This binary file can be downloaded, but has no browser preview.");
  return previewDocument(content, "", file.path);
}

export function createFilePreview(options: { plainText?: () => boolean; onPlainText?: (value: boolean) => void } = {}): { dialog: HTMLDialogElement; open(code: string, language: string, name: string): void; openFile(file: WorkspaceFile, files?: readonly WorkspaceFile[], expanded?: boolean, source?: boolean): void; clear(): void; destroy(): void } {
  const dialog = createDialog("File preview", `<div class="preview-toolbar" tabindex="0" aria-label="Preview navigation">
    <button type="button" data-field="preview-expand" aria-pressed="false">${buttonIconHtml("fullscreen")}Expand</button>
    <button type="button" data-field="preview-previous">${buttonIconHtml("previous")}Previous</button>
    <span data-field="preview-position" aria-live="polite"></span>
    <button type="button" data-field="preview-next">${buttonIconHtml("next")}Next</button>
    <button type="button" data-field="preview-source">${buttonIconHtml("code")}Source</button>
    <button type="button" data-field="preview-text-mode">${buttonIconHtml("file")}Plain text</button>
    <button type="button" data-field="preview-copy">${buttonIconHtml("copy")}Copy</button>
    <span data-field="preview-copy-status" role="status"></span>
    <small>Use ← → or swipe on the toolbar to change files. Select text in the viewer to copy it.</small>
    </div><div class="preview-content">
    <iframe data-field="preview-frame" title="File preview" sandbox="allow-scripts" referrerpolicy="no-referrer"></iframe>
    <div class="preview-monaco" data-field="preview-monaco" aria-label="Read-only Monaco preview" hidden></div>
    <textarea class="preview-plain-text" data-field="preview-plain-text" readonly spellcheck="false" aria-label="File contents" hidden></textarea>
    </div>`);
  dialog.classList.add("browser-preview");
  const frame = field<HTMLIFrameElement>(dialog, "preview-frame");
  const host = field(dialog, "preview-monaco"), text = field<HTMLTextAreaElement>(dialog, "preview-plain-text");
  const sourceButton = field<HTMLButtonElement>(dialog, "preview-source"), textButton = field<HTMLButtonElement>(dialog, "preview-text-mode");
  let files: readonly WorkspaceFile[] = [], index = 0, token = "";
  const copyButton = field<HTMLButtonElement>(dialog, "preview-copy");
  let current: FilesReadResponse | null = null, rendered = false, source = false, sourcePreference = false, plain = false;
  const isPlain = (): boolean => options.plainText?.() ?? plain;
  const setPlain = (value: boolean): void => { plain = value; options.onPlainText?.(value); };
  const editor = new MonacoFileEditor(() => { setPlain(true); showMode(); }, () => {});
  const showMode = (): void => {
    const code = source && current !== null, native = code && isPlain();
    frame.hidden = source; host.hidden = !code || native; text.hidden = !native;
    sourceButton.hidden = !rendered || current === null;
    sourceButton.innerHTML = buttonIconHtml(source ? "preview" : "code") + (source ? "Rendered" : "Source");
    sourceButton.setAttribute("aria-pressed", String(source));
    textButton.hidden = !code;
    textButton.innerHTML = buttonIconHtml(isPlain() ? "code" : "file") + (isPlain() ? "Monaco" : "Plain text");
    textButton.setAttribute("aria-pressed", String(isPlain()));
    if (text.value !== (current?.content ?? "")) text.value = current?.content ?? "";
    text.setAttribute("aria-label", `File contents: ${current?.path ?? ""}`);
    editor.sync(code && !native ? host : null, current);
  };
  sourceButton.addEventListener("click", () => { source = !source; sourcePreference = source; showMode(); });
  textButton.addEventListener("click", () => { setPlain(!isPlain()); showMode(); });
  copyButton.addEventListener("click", () => {
    const selected = current;
    if (!selected) return;
    void copyCode(selected.content).then(() => {
      if (current === selected) field(dialog, "preview-copy-status").textContent = "Copied.";
    }).catch(error => {
      if (current === selected) field(dialog, "preview-copy-status").textContent = errorMessage(error);
    });
  });
  const expand = field<HTMLButtonElement>(dialog, "preview-expand");
  const previous = field<HTMLButtonElement>(dialog, "preview-previous");
  const next = field<HTMLButtonElement>(dialog, "preview-next");
  const display = (html: string | null, name: string, content: string | null, preferSource = false): void => {
    copyButton.disabled = content === null;
    field(dialog, "preview-copy-status").textContent = "";
    copyButton.title = content === null ? "Binary file: use Download" : "Copy file contents";
    current = content === null ? null : { path: name, content, size: String(new TextEncoder().encode(content).byteLength) };
    sourcePreference = preferSource; rendered = html !== null; source = !rendered || (preferSource && current !== null);
    dialog.querySelector("h2")!.textContent = name;
    frame.title = `Preview: ${name}`;
    previous.disabled = next.disabled = files.length < 2;
    field(dialog, "preview-position").textContent = files.length ? `${index + 1} / ${files.length}` : "";
    // The opaque iframe permits only this nonce-scoped navigation bridge. User
    // scripts/handlers were removed and cannot execute, access the host or fetch.
    token = crypto.randomUUID();
    const nonce = crypto.randomUUID();
    const bridge = new URL("./preview-bridge.js", import.meta.url).href.replaceAll("&", "&amp;").replaceAll('"', "&quot;");
    delete frame.dataset["navigationReady"];
    if (html === null) frame.removeAttribute("srcdoc");
    else frame.srcdoc = html.replace("script-src 'none'", `script-src 'nonce-${nonce}'`).replace("</body>", `<script nonce="${nonce}" src="${bridge}" data-preview-token="${token}"></script></body>`);
    if (!dialog.open) dialog.showModal();
    showMode();
  };
  const syncFull = (): void => frame.contentWindow?.postMessage({token,full:dialog.classList.contains("is-full-preview")}, "*");
  frame.addEventListener("load", syncFull);
  const step = (direction: number): void => {
    if (!files.length) return;
    index = (index + direction + files.length) % files.length;
    const file = files[index]!; display(canRenderFile(file) ? workspacePreview(file) : null, file.path, textFile(file), sourcePreference);
  };
  const toggle = (): void => {
    const full = dialog.classList.toggle("is-full-preview");
    expand.innerHTML = buttonIconHtml(full ? "restore" : "fullscreen") + (full ? "Restore" : "Expand");
    expand.setAttribute("aria-pressed", String(full));
    syncFull();
  };
  expand.addEventListener("click", toggle);
  previous.addEventListener("click", () => step(-1)); next.addEventListener("click", () => step(1));
  const message = (event: MessageEvent): void => {
    if (!dialog.open || event.source !== frame.contentWindow || event.data?.token !== token) return;
    if (event.data.type === "lethetic-preview-ready") { syncFull(); return; }
    if (event.data.type === "lethetic-preview-state") { frame.dataset["navigationReady"] = String(event.data.full); return; }
    if (!dialog.classList.contains("is-full-preview") || event.data.type !== "lethetic-preview") return;
    if (event.data.direction === 0) toggle();
    else if (event.data.direction === -1 || event.data.direction === 1) step(event.data.direction);
  };
  globalThis.addEventListener("message", message);
  dialog.addEventListener("keydown", event => {
    if (event.defaultPrevented || (event.target instanceof Element && event.target.closest(".preview-content")) || !dialog.classList.contains("is-full-preview") || event.shiftKey || event.ctrlKey || event.metaKey || event.altKey) return;
    if (event.key === "ArrowLeft" || event.key === "ArrowRight") { event.preventDefault(); step(event.key === "ArrowLeft" ? -1 : 1); }
  });
  let swipe: {x: number; y: number; time: number} | null = null;
  dialog.addEventListener("touchstart", event => { const t = event.touches[0]; swipe = !(event.target instanceof Element && event.target.closest(".preview-content")) && t ? {x:t.clientX,y:t.clientY,time:Date.now()} : null; }, {passive:true});
  dialog.addEventListener("touchend", event => {
    const t = event.changedTouches[0];
    if (swipe && t && dialog.classList.contains("is-full-preview") && Date.now()-swipe.time<600 &&
      Math.abs(t.clientX-swipe.x)>70 && Math.abs(t.clientX-swipe.x)>Math.abs(t.clientY-swipe.y)*2) step(t.clientX<swipe.x?-1:1);
    swipe=null;
  }, {passive:true});
  const release = (): void => { token = ""; files = []; current = null; text.value = ""; editor.sync(null, null); delete host.dataset["editorReady"]; frame.removeAttribute("srcdoc"); };
  const clear = (): void => { release(); if (dialog.open) dialog.close(); };
  dialog.addEventListener("close", () => { if (!dialog.open) release(); });
  return { dialog, open(code, language, name): void {
      files = [];
      const render = [language.toLowerCase(), name.split(".").at(-1)?.toLowerCase()].some(kind => ["html", "htm", "svg", "md", "markdown"].includes(kind ?? ""));
      display(render ? previewDocument(code, language, name) : null, name, code);
    },
    openFile(file, available = [file], expanded = false, preferSource = false): void {
      if (expanded && !dialog.classList.contains("is-full-preview")) toggle();
      files = available.filter(canPreviewFile); index = files.findIndex(item => item.path === file.path);
      if (index < 0) { files = [file]; index = 0; }
      display(canRenderFile(file) ? workspacePreview(file) : null, file.path, textFile(file), preferSource);
    }, clear, destroy(): void { clear(); globalThis.removeEventListener("message", message); dialog.remove(); } };
}
