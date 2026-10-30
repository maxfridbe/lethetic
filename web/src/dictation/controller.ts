import { buttonIconHtml } from "../icons.js";
import { HoldToTalk } from "./hold.js";
import { nativeDictationAvailable, androidDictation, dictationCommand } from "./native.js";
import type { LocalPaletteCommand } from "../app/state.js";

const KEY = "lethetic.dictation.v1";
type Model = "tiny" | "base";
export class DictationController {
  readonly available = nativeDictationAvailable();
  readonly hold: HoldToTalk;
  enabled = true;
  model: Model = androidDictation() ? "tiny" : "base";
  language = "";
  ready = false;
  preparing = false;
  message = "Hold Send to set up local dictation.";
  level = 0;
  #session: string | null = null;
  #allowed = false;
  #dialog: HTMLDialogElement | null = null;
  #meter: number | null = null;
  #disposed = false;
  #storage?: Storage;
  #pointer: number | null = null;
  readonly #cancel = (): void => { if (this.hold.phase !== "idle") this.hold.cancel(); };
  readonly #visibility = (): void => { if (document.hidden) this.#cancel(); };
  readonly #key = (event: KeyboardEvent): void => { if (event.key === "Escape" && this.hold.active) { event.preventDefault(); this.hold.cancel(); } };
  constructor(private readonly changed: () => void, private readonly receive: (text: string) => void) {
    this.hold = new HoldToTalk({
      start: async id => {
        if (!this.ready) { this.show(); throw Error("Download and enable a local Whisper model first."); }
        this.message = "Hold Send to dictate locally. Release to add text; click to send.";
        await dictationCommand("start_dictation", {id});
      },
      finish: id => dictationCommand<string>("finish_dictation", {id, language: this.language || null}),
      cancel: id => dictationCommand<void>("cancel_dictation", {id}),
    }, () => this.#update(), text => {
      this.message = text.trim() ? "Dictation added. Review it, then Send." : "No speech detected. Hold Send to try again.";
      if (text.trim()) this.receive(text.trim());
    }, error => { this.message = error instanceof Error ? error.message : String(error); this.#update(); });
    if (!this.available) return;
    try {
      this.#storage = localStorage;
      const saved = JSON.parse(this.#storage.getItem(KEY) ?? "{}");
      this.enabled = saved.enabled !== false;
      if (saved.model === "tiny" || saved.model === "base") this.model = saved.model;
      this.language = typeof saved.language === "string" && /^[a-z]{2,3}$/.test(saved.language) ? saved.language : "";
    } catch { /* Preferences are optional. */ }
    globalThis.addEventListener("blur", this.#cancel);
    document.addEventListener("visibilitychange", this.#visibility);
    globalThis.addEventListener("keydown", this.#key, true);
    // Warm an already downloaded model without making any network request.
    queueMicrotask(() => { void dictationCommand("cancel_dictation", {id: null}).then(() => {
      if (!this.#disposed && this.enabled) void this.prepare(false);
    }).catch(error => { this.message = String(error); this.#update(); }); });
  }
  get active(): boolean { return this.hold.active; }
  get usable(): boolean { return this.available && this.enabled && this.#allowed; }
  get open(): boolean { return this.#dialog?.open ?? false; }
  get label(): string {
    return this.hold.phase === "starting" ? "Opening mic…" : this.hold.phase === "recording" ? "Listening…" : this.hold.phase === "transcribing" ? "Transcribing…" : "Send";
  }
  get status(): string {
    return this.hold.phase === "recording" ? "Release to transcribe · Esc cancels · audio stays on this device" :
      this.hold.phase === "starting" ? "Opening the default microphone…" : this.hold.phase === "transcribing" ? "Transcribing locally…" : this.message;
  }
  observe(session: string | null, allowed: boolean): void {
    const cancel = session !== this.#session || !allowed;
    this.#session = session; this.#allowed = allowed;
    if (cancel && this.hold.phase !== "idle") this.hold.cancel();
  }
  down(event: PointerEvent): void {
    if (!this.usable || event.button !== 0 || !event.isPrimary) return;
    this.#pointer = event.pointerId;
    (event.currentTarget as HTMLElement).setPointerCapture?.(event.pointerId);
    this.hold.press();
  }
  up(event: PointerEvent): void {
    if (this.#pointer !== event.pointerId) return;
    this.#pointer = null; this.hold.release();
  }
  pointerCancel(event: PointerEvent): void {
    if (this.#pointer !== event.pointerId) return;
    this.#pointer = null; this.hold.cancel();
  }
  keyDown(event: KeyboardEvent): void {
    if (event.key === "Tab") this.pause();
    if (event.key === "Enter") this.hold.clearClick();
    if (event.key === " " && !event.repeat && this.usable) this.hold.press();
  }
  keyUp(event: KeyboardEvent): void { if (event.key === " ") this.hold.release(); }
  click(): boolean { return this.hold.consumeClick(); }
  pause(): void { this.#cancel(); }
  dispose(): void {
    this.#disposed = true; this.hold.cancel(); this.#dialog?.remove();
    if (this.#meter !== null) globalThis.clearInterval(this.#meter);
    globalThis.removeEventListener("blur", this.#cancel); globalThis.removeEventListener("keydown", this.#key, true);
    document.removeEventListener("visibilitychange", this.#visibility);
  }
  commands(): LocalPaletteCommand[] {
    return this.available ? [{id: "local:dictation", label: "Speech to text · hold Send", description: "Local Whisper microphone dictation, model download and language", icon: "command", behavior: "open_panel", accelerator: null, enabled: true, disabled_reason: null, run: () => this.show()}] : [];
  }
  #update(): void {
    if (this.#disposed) return;
    if (this.hold.phase === "idle" || this.hold.phase === "transcribing") this.#pointer = null;
    if (this.hold.phase === "recording" && this.#meter === null) {
      this.#meter = globalThis.setInterval(() => { void dictationCommand<{level: number | null}>("dictation_status").then(state => {
        if (this.#disposed || this.hold.phase !== "recording" || state.level === null) return;
        this.level = Math.max(0, Math.min(1, state.level)); this.changed();
      }).catch(() => {}); }, 100);
    } else if (this.hold.phase !== "recording" && this.#meter !== null) {
      globalThis.clearInterval(this.#meter); this.#meter = null; this.level = 0;
    }
    this.#refreshDialog(); this.changed();
  }
  async prepare(download: boolean): Promise<void> {
    if (this.preparing) return;
    this.preparing = true; this.ready = false;
    this.message = download ? "Downloading/verifying the model, then loading Whisper…" : "Loading cached Whisper model…";
    this.#update();
    try {
      await dictationCommand("prepare_dictation", {model: this.model, download});
      this.ready = true; this.message = "Hold Send to dictate locally. Release to add text; click to send.";
    } catch (error) { this.message = error instanceof Error ? error.message : String(error); }
    finally { this.preparing = false; this.#update(); }
  }
  show(): void {
    if (!this.available || this.#disposed) return;
    this.pause();
    if (!this.#dialog) {
      const dialog = document.createElement("dialog"); dialog.className = "dictation-dialog";
      dialog.setAttribute("aria-label", "Speech to text");
      dialog.innerHTML = `<h2>${buttonIconHtml("microphone")} Speech to text</h2><p>Hold Send (or hold Space while Send is focused), speak, then release. Text is added to your draft for review. Esc or leaving the window cancels. Maximum recording: two minutes.</p>
        <label><input type="checkbox" data-field="enabled"> Enable hold-to-talk</label>
        <label>Local Whisper model<select data-field="model"><option value="tiny">Tiny · ~75 MiB · faster</option><option value="base">Base · ~142 MiB · more accurate</option></select></label>
        <label>Language code (blank = automatic)<input data-field="language" placeholder="en" maxlength="3" spellcheck="false"></label>
        <p>Uses the default system microphone. Audio is transcribed on this device, never uploaded or saved. The model downloads once from Hugging Face; later use is offline. No sttme daemon is required.</p>
        ${androidDictation() ? '<p>Android: setup requests Microphone permission. Tiny is recommended for phones. Return here after granting permission, then hold Send. Switching apps or locking the screen cancels recording.</p>' : ''}
        <p role="status" data-field="status"></p><div><button type="button" data-field="prepare">${buttonIconHtml("download")} Download / load model</button><button type="button" data-field="close">${buttonIconHtml("close")} Close</button></div>`;
      document.body.append(dialog); this.#dialog = dialog;
      this.#field<HTMLInputElement>("enabled").checked = this.enabled;
      this.#field<HTMLSelectElement>("model").value = this.model;
      this.#field<HTMLInputElement>("language").value = this.language;
      this.#field("prepare").onclick = () => {
        const language = this.#field<HTMLInputElement>("language").value.trim().toLowerCase();
        if (language && !/^[a-z]{2,3}$/.test(language)) { this.message = "Use a two- or three-letter language code, or leave it blank."; this.#update(); return; }
        this.language = language; this.model = this.#field<HTMLSelectElement>("model").value === "tiny" ? "tiny" : "base";
        this.enabled = this.#field<HTMLInputElement>("enabled").checked; this.#save();
        if (this.enabled) void this.prepare(true);
      };
      this.#field<HTMLInputElement>("enabled").onchange = () => { this.enabled = this.#field<HTMLInputElement>("enabled").checked; this.#save(); this.#update(); };
      this.#field("close").onclick = () => dialog.close();
    }
    this.#refreshDialog(); if (!this.#dialog.open) this.#dialog.showModal();
  }
  #field<T extends HTMLElement>(field: string): T { return this.#dialog!.querySelector<T>(`[data-field="${field}"]`)!; }
  #save(): void {
    try { this.#storage?.setItem(KEY, JSON.stringify({enabled: this.enabled, model: this.model, language: this.language})); }
    catch { this.message = "Preferences could not be saved. Dictation settings remain active for this window."; }
  }
  #refreshDialog(): void {
    if (!this.#dialog) return;
    this.#field("status").textContent = this.status;
    for (const name of ["prepare", "model", "language"]) (this.#field(name) as HTMLButtonElement).disabled = this.preparing;
  }
}
