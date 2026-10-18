import { CodeVault } from "./code-vault.js";
import type { CodeDocument } from "./code-vault.js";
import type { CodeSnippet } from "./code-snippets.js";
import { createDialog, errorMessage, field } from "./dialog.js";

export function createVaultDialog(storage?: Storage): {
  dialog: HTMLDialogElement; open(snippet?: CodeSnippet): void; destroy(): void;
} {
  const dialog = createDialog("Encrypted code library", `
    <p>Save code on this device, encrypted with your passphrase. The passphrase is never stored. Keep it safe: there is no recovery if it is lost.</p>
    <form data-field="unlock-form" class="browser-unlock">
      <label>Library passphrase<input data-field="passphrase" type="password" autocomplete="off" required maxlength="1024" placeholder="At least 12 characters for a new library"></label>
      <button class="primary-button" type="submit">Unlock / create library</button>
    </form>
    <div class="panel-toolbar"><button data-field="lock" type="button">Lock library</button><span data-field="status" role="status">Locked. Code is decrypted only while this dialog is open.</span></div>
    <div data-field="editor" class="browser-code-editor" hidden>
      <label>Saved code<select data-field="files"><option value="">New code file</option></select></label>
      <form data-field="save-form">
        <div class="browser-code-metadata"><label>Filename<input data-field="filename" maxlength="200" required placeholder="analysis.py"></label><label>Language<input data-field="language" maxlength="64" value="python"></label></div>
        <label>Code<textarea data-field="code" rows="16" spellcheck="false" autocomplete="off"></textarea></label>
        <div class="panel-toolbar"><button class="primary-button" type="submit">Save encrypted</button><button data-field="delete" type="button">Delete selected file</button><button data-field="download" type="button">Download plaintext</button></div>
      </form>
    </div>`);
  const vault = storage ? new CodeVault(storage) : null;
  let selected = "";
  let pending: CodeSnippet | undefined;
  let busy = false;
  let disposed = false;
  const passphrase = field<HTMLInputElement>(dialog, "passphrase");
  const name = field<HTMLInputElement>(dialog, "filename");
  const language = field<HTMLInputElement>(dialog, "language");
  const code = field<HTMLTextAreaElement>(dialog, "code");
  const files = field<HTMLSelectElement>(dialog, "files");
  const status = (text: string): void => { if (!disposed) field(dialog, "status").textContent = text; };
  const controls = (): void => {
    const unlocked = vault?.unlocked ?? false;
    field(dialog, "editor").hidden = !unlocked;
    field(dialog, "unlock-form").hidden = unlocked;
    for (const control of dialog.querySelectorAll<HTMLInputElement | HTMLButtonElement | HTMLTextAreaElement | HTMLSelectElement>("input, button, textarea, select")) {
      control.disabled = busy && !control.hasAttribute("data-close") && control.dataset["field"] !== "lock";
    }
    field<HTMLButtonElement>(dialog, "delete").disabled = busy || !selected;
  };
  const refresh = (): void => {
    files.replaceChildren(new Option("New code file", ""));
    for (const document of vault?.list() ?? []) files.add(new Option(document.name, document.id));
    files.value = selected;
    controls();
  };
  const load = (document?: CodeDocument | CodeSnippet): void => {
    selected = document && "id" in document ? document.id : "";
    name.value = document && "name" in document ? document.name : "";
    language.value = document?.language ?? "python";
    code.value = document?.code ?? "";
  };
  const lock = (): void => {
    vault?.lock(); passphrase.value = ""; pending = undefined;
    load(); files.replaceChildren(); status("Locked. Enter your passphrase to reopen saved code."); controls();
  };
  const perform = (operation: () => Promise<void>): void => {
    if (busy) return;
    busy = true; controls();
    void operation().catch((error: unknown) => status(errorMessage(error))).finally(() => {
      busy = false; if (!disposed) controls();
    });
  };
  field(dialog, "unlock-form").addEventListener("submit", (event) => {
    event.preventDefault();
    const secret = passphrase.value; passphrase.value = "";
    perform(async () => {
      if (!vault) throw new Error("Browser storage is unavailable. Allow localStorage to save encrypted code.");
      status("Unlocking…"); await vault.unlock(secret);
      load(pending); pending = undefined; refresh(); status("Unlocked. Save encrypts code and filenames together.");
    });
  });
  field(dialog, "save-form").addEventListener("submit", (event) => {
    event.preventDefault();
    perform(async () => {
      if (!vault) throw new Error("Storage is unavailable.");
      const document: CodeDocument = { id: selected || crypto.randomUUID(), name: name.value.trim(),
        language: language.value.trim(), code: code.value, updatedAt: new Date().toISOString() };
      const documents = vault.list().filter((item) => item.id !== document.id);
      documents.push(document);
      await vault.save(documents); selected = document.id; refresh(); status("Saved encrypted on this device.");
    });
  });
  files.addEventListener("change", () => {
    load(vault?.list().find((document) => document.id === files.value)); controls();
  });
  field(dialog, "delete").addEventListener("click", () => perform(async () => {
    if (!vault || !selected) return;
    await vault.save(vault.list().filter((document) => document.id !== selected));
    load(); refresh(); status("Code file deleted.");
  }));
  field(dialog, "download").addEventListener("click", () => {
    const url = URL.createObjectURL(new Blob([code.value], { type: "text/plain;charset=utf-8" }));
    const link = document.createElement("a"); link.href = url; link.download = name.value || "code.txt";
    link.click(); setTimeout(() => URL.revokeObjectURL(url), 1000);
  });
  field(dialog, "lock").addEventListener("click", lock);
  dialog.addEventListener("close", lock);
  globalThis.addEventListener("pagehide", lock);
  controls();
  return { dialog, open(snippet): void {
    pending = snippet;
    if (vault?.unlocked && snippet) { load(snippet); refresh(); pending = undefined; }
    if (!dialog.open) dialog.showModal();
    if (!vault?.unlocked) passphrase.focus();
  }, destroy(): void { disposed = true; vault?.lock(); lock(); globalThis.removeEventListener("pagehide", lock); dialog.remove(); } };
}
