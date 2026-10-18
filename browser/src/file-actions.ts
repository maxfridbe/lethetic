import { parse } from "../../web/lib/marked/marked.esm.js";
import { createDialog, field } from "./dialog.js";
import type { CodeSnippet } from "./code-snippets.js";

export function snippetName(snippet: CodeSnippet): string {
  const extensions: Record<string, string> = { python: "py", javascript: "js", typescript: "ts", markdown: "md", text: "txt" };
  const extension = extensions[snippet.language] ?? (/^[a-z0-9]{1,8}$/u.test(snippet.language) ? snippet.language : "txt");
  return `snippet.${extension}`;
}
export function downloadCode(code: string, name: string): void {
  const url = URL.createObjectURL(new Blob([code], { type: "text/plain;charset=utf-8" }));
  const link = document.createElement("a"); link.href = url;
  link.download = name.replace(/[\\/\u0000-\u001f]/gu, "_") || "code.txt";
  link.click(); setTimeout(() => URL.revokeObjectURL(url), 1000);
}
export async function copyCode(code: string): Promise<void> {
  if (!navigator.clipboard?.writeText) throw new Error("Clipboard is unavailable. Download the file or select and copy its text.");
  await navigator.clipboard.writeText(code);
}

/** Static HTML/SVG/Markdown only, inside an opaque sandbox with no scripts or network. */
export function previewDocument(code: string, language: string, name: string): string {
  if (code.length > 1_048_576) throw new Error("Preview is limited to 1 MiB of text.");
  const extension = name.split(".").at(-1)?.toLowerCase() ?? "";
  const kind = language.toLowerCase();
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
      if (name.startsWith("on") || ["srcdoc", "nonce", "action", "formaction", "srcset", "ping", "autofocus"].includes(name) ||
          (name.endsWith("href") && !attribute.value.startsWith("#")) ||
          (name === "src" && !/^data:image\//iu.test(attribute.value))) element.removeAttribute(attribute.name);
    }
  }
  return `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; img-src data:; form-action 'none'; base-uri 'none'"><style>body{font:16px system-ui;color:#222;background:white;padding:20px;overflow-wrap:anywhere}pre{white-space:pre-wrap}img,svg{max-width:100%}table{border-collapse:collapse}td,th{border:1px solid #aaa;padding:6px}</style></head><body>${template.innerHTML}</body></html>`;
}

export function createFilePreview(): { dialog: HTMLDialogElement; open(code: string, language: string, name: string): void; clear(): void; destroy(): void } {
  const dialog = createDialog("File preview", `<p>Static preview. Scripts, forms, navigation and network requests are disabled.</p><iframe data-field="preview-frame" title="File preview" sandbox="" referrerpolicy="no-referrer"></iframe>`);
  dialog.classList.add("browser-preview");
  const frame = field<HTMLIFrameElement>(dialog, "preview-frame");
  const clear = (): void => { frame.removeAttribute("srcdoc"); if (dialog.open) dialog.close(); };
  // The close event is queued; a rapid reopen must keep its new preview.
  dialog.addEventListener("close", () => { if (!dialog.open) frame.removeAttribute("srcdoc"); });
  return { dialog, open(code, language, name): void {
    frame.srcdoc = previewDocument(code, language, name);
    if (!dialog.open) dialog.showModal();
  }, clear, destroy(): void { clear(); dialog.remove(); } };
}
