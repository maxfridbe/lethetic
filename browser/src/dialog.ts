import { buttonIconHtml } from "../../web/src/icons.js";
/** Native focus trapping and Escape handling, styled with the existing remote UI. */
export function createDialog(title: string, content: string): HTMLDialogElement {
  const dialog = document.createElement("dialog");
  dialog.className = "overlay-panel browser-dialog";
  const headingId = `browser-dialog-${crypto.randomUUID()}`;
  dialog.setAttribute("aria-labelledby", headingId);
  dialog.innerHTML = `<header class="overlay-header"><h2></h2><button type="button" data-close>${buttonIconHtml("close")}Close</button></header><div class="overlay-body">${content}</div>`;
  const heading = dialog.querySelector("h2");
  if (heading) { heading.id = headingId; heading.textContent = title; }
  dialog.querySelector("[data-close]")?.addEventListener("click", () => dialog.close());
  document.body.append(dialog);
  return dialog;
}

export function field<T extends HTMLElement>(root: HTMLElement, name: string): T {
  const result = root.querySelector<T>(`[data-field="${name}"]`);
  if (!result) throw new Error(`Missing field: ${name}`);
  return result;
}

export function errorMessage(error: unknown): string { return error instanceof Error ? error.message : String(error); }
