import { buttonIconHtml } from "../icons.js";
import { h } from "../../lib/snabbdom/build/index.js";
import type { VNode } from "../../lib/snabbdom/build/index.js";
import type { WebAppSnapshot } from "../generated/contracts.js";
import { downloadBlob } from "../download.js";
import { PersonaStore } from "./store.js";
import type { PersonaFeatures } from "./store.js";
import { PersonaEvents } from "./events.js";
import { PersonaEffects } from "./effects.js";
import { MAX_PERSONA_BYTES } from "./schema.js";
import type { Motion, PersonaEvent } from "./schema.js";

function preferences(): Storage | undefined { try { return globalThis.localStorage; } catch { return undefined; } }
export class PersonaController {
  readonly store: PersonaStore;
  readonly effects = new PersonaEffects(message => { if (this.#dialog?.open) this.#field("status").textContent = message; });
  readonly events = new PersonaEvents();
  readonly previewEffects = new PersonaEffects();
  #dialog: HTMLDialogElement | null = null;
  #pending: Motion | null = null;
  #repeat = false;
  #session: string | null = null;
  #disposed = false;
  #generation = 0;
  #unlock = Promise.resolve();
  #avatarSource = "";
  #avatarUrl = "";
  constructor(readonly changed: () => void, storage = preferences()) { this.store = new PersonaStore(storage); }
  get open(): boolean { return this.#dialog?.open ?? false; }
  get section(): string { return this.store.section; }
  get lines(): readonly string[] { return this.store.quips; }
  primeAudio(): void { if (this.store.features.enabled && this.store.features.audio) this.#unlock = this.effects.unlock(); }
  emit(event: PersonaEvent): void {
    if (this.#disposed || !this.store.features.enabled) return;
    const persona = this.store.current, generation = this.#generation;
    this.effects.stopAnimation();
    if (this.store.features.avatar) this.#pending = persona.avatar?.events[event] ?? null;
    this.#repeat = event === "when_model_thinking" && this.#pending === "pace";
    const sound = this.store.features.audio ? persona.audio?.[event] : undefined;
    if (sound) void this.#unlock.then(() => { if (!this.#disposed && generation === this.#generation && this.store.features.enabled && this.store.features.audio && this.store.current === persona) this.effects.play(sound); });
  }
  observe(snapshot: WebAppSnapshot | null, live: boolean): void {
    const session = snapshot?.session.session_id ?? null;
    if (!live || session !== this.#session || snapshot?.activity.kind !== "processing") {
      if (this.#repeat) { this.effects.stopAnimation(); this.#pending = null; this.#repeat = false; }
    }
    this.#session = session;
    const event = this.events.observe(snapshot, live);
    if (event) this.emit(event);
    // Switching/enabling the companion mid-response should show its current state.
    if (live && snapshot?.activity.kind === "processing" &&
      this.store.features.enabled && this.store.features.avatar &&
      this.store.current.avatar?.events.when_model_thinking === "pace" && !this.#repeat) {
      this.#pending = "pace"; this.#repeat = true;
    }
  }
  afterRender(): void {
    if (this.#pending) this.effects.animate(document.getElementById("persona-avatar"), this.#pending, matchMedia("(prefers-reduced-motion: reduce)").matches, this.#repeat);
    this.#pending = null;
  }
  avatar(): VNode | null {
    const p = this.store.current;
    if (!this.store.features.enabled || !this.store.features.avatar || !p.avatar) return null;
    if (this.#avatarSource !== p.avatar.svg) { this.#avatarSource = p.avatar.svg; this.#avatarUrl = "data:image/svg+xml;charset=utf-8," + encodeURIComponent(p.avatar.svg); }
    return h("div.persona-companion", {attrs: {"aria-hidden": "true", "data-persona": p.id}}, [h("img#persona-avatar", {attrs: {src: this.#avatarUrl, alt: "", draggable: "false"}})]);
  }
  pause(): void { this.#generation++; this.effects.stop(); this.#pending = null; this.#repeat = false; this.previewEffects.stop(); this.events.reset(); }
  dispose(): void { this.#disposed = true; this.pause(); this.effects.dispose(); this.previewEffects.dispose(); this.#dialog?.remove(); }
  importDialog(): void { this.show(); this.#field<HTMLInputElement>("file").click(); }
  exportCurrent(): void { this.show(); this.#field("export").click(); }
  show(): void {
    if (!this.#dialog) this.#dialog = this.#createDialog();
    this.#refresh(); if (!this.#dialog.open) this.#dialog.showModal();
  }
  #apply(): void { this.#generation++; this.effects.stop(); this.previewEffects.stop(); this.#pending = null; this.#repeat = false; this.store.save(); this.#refresh(); this.changed(); }
  #field<T extends HTMLElement>(name: string): T { return this.#dialog!.querySelector<T>(`[data-persona-field="${name}"]`)!; }
  #refresh(): void {
    if (!this.#dialog) return;
    const select = this.#field<HTMLSelectElement>("select"); select.replaceChildren();
    for (const p of this.store.list) { const option = document.createElement("option"); option.value = p.id; option.textContent = p.name; select.append(option); }
    select.value = this.store.selected;
    for (const key of Object.keys(this.store.features) as (keyof PersonaFeatures)[]) {
      const input = this.#field<HTMLInputElement>(key); input.checked = this.store.features[key]; input.disabled = key !== "enabled" && !this.store.features.enabled;
    }
    const p = this.store.current;
    this.#field("description").textContent = p.description;
    const preview = this.#field<HTMLImageElement>("preview-image");
    preview.hidden = !p.avatar;
    if (p.avatar) preview.src = "data:image/svg+xml;charset=utf-8," + encodeURIComponent(p.avatar.svg);
    else preview.removeAttribute("src");
    this.#field("prompt").textContent = this.section || "Persona narration is disabled. Your base prompt is preserved.";
    this.#field("quips-list").textContent = p.quips.join("\n");
    this.#field<HTMLButtonElement>("remove").disabled = !this.store.custom.some(c => c.id === p.id);
    this.#field<HTMLButtonElement>("test").disabled = !this.store.features.enabled || !this.store.features.audio;
    this.#field("status").textContent = this.store.error || this.effects.error;
  }
  #createDialog(): HTMLDialogElement {
    const dialog = document.createElement("dialog"); dialog.className = "persona-dialog"; dialog.setAttribute("aria-labelledby", "persona-title");
    dialog.innerHTML = `<div class="persona-dialog-heading"><h2 id="persona-title">Personas</h2><button type="button" data-persona-field="close" aria-label="Close personas">${buttonIconHtml("close")} Close</button></div>
      <label><input type="checkbox" data-persona-field="enabled"> Enable persona features</label>
      <label>Persona<select data-persona-field="select"></select></label><img class="persona-dialog-preview" data-persona-field="preview-image" alt="Persona artwork preview"><p data-persona-field="description"></p>
      <div class="persona-features"><label><input type="checkbox" data-persona-field="narration"> Narration in system prompt</label><label><input type="checkbox" data-persona-field="quips"> Thinking quips</label><label><input type="checkbox" data-persona-field="avatar"> Animated SVG companion</label><label><input type="checkbox" data-persona-field="audio"> Event sounds</label></div>
      <p>Changes apply to the next message. Narration keeps code, files, data, and requested formats unchanged. Display and sound settings apply immediately. Sounds require a user gesture; completion sounds can play in the background.</p>
      <label>Preview event<select data-persona-field="preview-event"><option value="when_model_done_responding">Model done responding</option><option value="when_user_submits_prompt">User submits prompt</option><option value="when_model_thinking">Model thinking</option><option value="when_tool_started">Tool started</option><option value="when_error">Error</option><option value="when_stopped">Stopped</option></select></label>
      <div class="persona-dialog-actions"><button type="button" data-persona-field="test">${buttonIconHtml("run")} Test sound</button><button type="button" data-persona-field="preview">${buttonIconHtml("preview")} Animate preview</button><button type="button" data-persona-field="import">${buttonIconHtml("upload")} Import JSON</button><button type="button" data-persona-field="export">${buttonIconHtml("download")} Export JSON</button><button type="button" data-persona-field="remove">${buttonIconHtml("refresh")} Reset / delete custom</button></div>
      <input type="file" accept=".json,application/json" data-persona-field="file" hidden>
      <details><summary>Persona section for the next system prompt</summary><pre data-persona-field="prompt"></pre></details>
      <details><summary>Thinking quips</summary><pre data-persona-field="quips-list"></pre></details>
      <p role="status" data-persona-field="status"></p>`;
    document.body.append(dialog); this.#dialog = dialog;
    this.#field("close").onclick = () => { this.previewEffects.stop(); this.effects.stopSound(); dialog.close(); };
    dialog.addEventListener("close", () => this.previewEffects.stop());
    this.#field<HTMLSelectElement>("select").onchange = event => { this.store.select((event.target as HTMLSelectElement).value); this.#apply(); };
    for (const key of Object.keys(this.store.features) as (keyof PersonaFeatures)[]) this.#field<HTMLInputElement>(key).onchange = event => {
      this.store.features[key] = (event.target as HTMLInputElement).checked; this.#apply(); if (key === "audio") this.primeAudio();
    };
    const previewEvent = (): PersonaEvent => this.#field<HTMLSelectElement>("preview-event").value as PersonaEvent;
    this.#field("test").onclick = () => {
      if (!this.store.current.audio?.[previewEvent()]) { this.#field("status").textContent = "No sound mapped to this event."; return; }
      this.primeAudio(); this.emit(previewEvent()); this.afterRender(); void this.#unlock.then(() => this.#refresh());
    };
    this.#field("preview").onclick = () => {
      const motion = this.store.current.avatar?.events[previewEvent()];
      if (motion) this.previewEffects.animate(this.#field("preview-image"), motion, matchMedia("(prefers-reduced-motion: reduce)").matches);
      else this.#field("status").textContent = "No animation mapped to this event.";
    };
    this.#field("remove").onclick = () => { this.store.remove(); this.#apply(); };
    this.#field("import").onclick = () => this.#field<HTMLInputElement>("file").click();
    this.#field<HTMLInputElement>("file").onchange = () => {
      const input = this.#field<HTMLInputElement>("file"), file = input.files?.[0]; input.value = "";
      if (!file) return;
      void (async () => { try {
        if (file.size > MAX_PERSONA_BYTES) throw Error("Persona JSON exceeds 2 MiB.");
        const source = await file.text(); if (this.#disposed) return;
        this.store.importFile(source); this.#apply(); this.#field("status").textContent = this.store.error || `Imported ${this.store.current.name}.`;
      } catch (error) { if (!this.#disposed) this.#field("status").textContent = String(error); } })();
    };
    this.#field("export").onclick = () => { const p = this.store.current; void downloadBlob(new Blob([JSON.stringify(p, null, 2) + "\n"], {type: "application/json"}), `${p.id}.persona.json`).then(saved => { if (!this.#disposed) this.#field("status").textContent = saved ? "Persona exported." : "Export cancelled."; }).catch(error => { if (!this.#disposed) this.#field("status").textContent = String(error); }); };
    return dialog;
  }
}
