import { createDialog, errorMessage, field } from "./dialog.js";
import type { BrowserFiles } from "./files.js";

export function createFileEditor(files: BrowserFiles, saved: (path: string) => void): {
  dialog: HTMLDialogElement; open(path: string, content: string, existing?: boolean): void; destroy(): void;
} {
  const dialog = createDialog("Save local file", `<p>${files.storage ? "Files are saved unencrypted with this session and sync with Python." : "Files stay in this tab until it closes or reloads."} Saved files are available to this session’s Python tool.</p>
    <form data-field="file-form"><label>Path<input data-field="file-path" placeholder="reports/analysis.py" required maxlength="4096"></label>
    <label>Contents<textarea data-field="file-content" rows="16" spellcheck="false"></textarea></label>
    <button type="submit" class="primary-button">Save file</button><p data-field="file-status" role="status"></p></form>`);
  let original: string | null = null;
  field(dialog, "file-form").addEventListener("submit", (event) => {
    event.preventDefault();
    const path = field<HTMLInputElement>(dialog, "file-path").value.trim();
    try {
      if (path !== original && files.list().some((file) => file.path === path)) throw new Error("That file exists. Open it in Files to edit it, or choose another path.");
      files.save(path, field<HTMLTextAreaElement>(dialog, "file-content").value);
      dialog.close(); saved(path);
    } catch (error) { field(dialog, "file-status").textContent = errorMessage(error); }
  });
  dialog.addEventListener("close", () => { if (!dialog.open) field<HTMLTextAreaElement>(dialog, "file-content").value = ""; });
  return { dialog, open(path, content, existing = false): void {
    original = existing ? path : null;
    field<HTMLInputElement>(dialog, "file-path").value = path;
    field<HTMLTextAreaElement>(dialog, "file-content").value = content;
    field(dialog, "file-status").textContent = "";
    if (!dialog.open) dialog.showModal();
  }, destroy(): void { dialog.remove(); } };
}
