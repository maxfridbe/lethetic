import { h } from "../../lib/snabbdom/build/index.js";
import type { VNode } from "../../lib/snabbdom/build/index.js";
import { buttonIconHtml, uiIcon } from "../icons.js";
import type { RenderBlockView } from "../generated/contracts.js";
import type { PersonaStore } from "./store.js";
import { parseSpeech } from "./schema.js";
import type { PersonaSpeech } from "./schema.js";
import { SPEECH_DEFAULTS, SpeechPlayer } from "./speech.js";

export class PersonaSpeechControls {
  readonly player: SpeechPlayer;
  #keys = new Map<string, string>();
  #root: HTMLElement | null = null;
  constructor(readonly store: PersonaStore, readonly changed: () => void) {
    this.player = new SpeechPlayer(() => { this.#playbackStatus(); this.changed(); });
  }
  #identity(config: PersonaSpeech): string { return JSON.stringify([this.store.current.id, config.provider, config.endpoint]); }
  #key(config: PersonaSpeech): string { return this.#keys.get(this.#identity(config)) ?? ""; }
  stop(): void { this.player.stop(false); this.player.error = ""; this.player.errorTarget = null; this.#playbackStatus(); }
  forgetCurrent(): void {
    const prefix = JSON.stringify([this.store.current.id]).slice(0, -1) + ",";
    for (const key of this.#keys.keys()) if (key.startsWith(prefix)) this.#keys.delete(key);
    this.stop();
  }
  dispose(): void { this.stop(); this.#keys.clear(); this.#root = null; }
  action(block: RenderBlockView, id: string): VNode | null {
    const config = this.store.current.speech;
    if (!this.store.features.enabled || !config?.enabled || !["markdown", "text"].includes(block.kind) || !block.content.trim() || block.success === false) return null;
    const active = this.player.active === id;
    return h("span.persona-speech-action", [
      h("button", {attrs: {type: "button", "data-persona-speech": id, "aria-label": active ? "Stop speech" : "Play response", title: active ? "Stop speech playback" : "Read this response with the persona’s AI-generated voice"}, on: {click: () => {
        if (active) this.player.stop(); else void this.player.play(id, block.content, config, this.#key(config));
      }}}, [uiIcon(active ? "stop" : "run"), active ? "Stop" : "Play"]),
      ...(this.player.errorTarget === id && this.player.error ? [h("span", {attrs: {role: "status"}}, this.player.error)] : []),
    ]);
  }
  mount(root: HTMLElement): void {
    this.#root = root;
    root.innerHTML = `<details class="persona-speech-settings"><summary>Speech playback · OpenAI / ElevenLabs</summary>
      <label><input type="checkbox" data-speech="enabled"> Enable response Play buttons for this persona</label>
      <label>API style<select data-speech="provider"><option value="openai">OpenAI-compatible</option><option value="elevenlabs">ElevenLabs-compatible</option></select></label>
      <label>Speech endpoint URL<input type="url" data-speech="endpoint" spellcheck="false"></label>
      <p>OpenAI: full /v1/audio/speech URL. ElevenLabs: /v1/text-to-speech/{voice_id}/stream. Endpoint must support raw PCM streaming and browser CORS.</p>
      <label>Model<input data-speech="model" spellcheck="false"></label>
      <label>Voice name / ID<input data-speech="voice" spellcheck="false"></label>
      <label>API key<input type="password" data-speech="key" autocomplete="off" spellcheck="false"></label>
      <p data-speech="key-status"></p>
      <p>Keys stay in memory in this tab until reload; they are excluded from persona exports and sessions. Play sends that response to the configured endpoint and may incur provider charges. Voices are AI-generated; event sounds are separate.</p>
      <div class="persona-dialog-actions"><button type="button" data-speech="save">${buttonIconHtml("save")} Save voice</button><button type="button" data-speech="test">${buttonIconHtml("run")} Test voice</button><button type="button" data-speech="clear">${buttonIconHtml("close")} Clear key</button></div>
      <p role="status" data-speech="status"></p>
      </details>`;
    this.#field<HTMLSelectElement>("provider").onchange = () => {
      const provider = this.#field<HTMLSelectElement>("provider").value as PersonaSpeech["provider"];
      const enabled = this.#field<HTMLInputElement>("enabled").checked;
      this.#populate({...SPEECH_DEFAULTS[provider], enabled});
    };
    this.#field<HTMLInputElement>("endpoint").oninput = () => this.#keyStatus();
    this.#field<HTMLInputElement>("key").oninput = () => this.#keyStatus();
    this.#field("save").onclick = () => { this.#save(); };
    this.#field("test").onclick = () => {
      if (this.player.active) { this.player.stop(); return; }
      if (!this.#save()) return;
      const config = this.store.current.speech!;
      void this.player.play("voice-preview", `Hello. This is the voice for ${this.store.current.name}.`, config, this.#key(config));
    };
    this.#field("clear").onclick = () => {
      const config = this.store.current.speech;
      if (config) this.#keys.delete(this.#identity(config));
      this.#field<HTMLInputElement>("key").value = ""; this.refresh(); this.player.stop();
    };
    this.refresh();
  }
  #field<T extends HTMLElement>(name: string): T { return this.#root!.querySelector<T>(`[data-speech="${name}"]`)!; }
  #populate(config: PersonaSpeech): void {
    for (const key of ["provider", "endpoint", "model", "voice"] as const) this.#field<HTMLInputElement | HTMLSelectElement>(key).value = config[key];
    this.#field<HTMLInputElement>("enabled").checked = config.enabled;
    this.#field<HTMLInputElement>("key").value = "";
    this.#keyStatus();
  }
  #keyStatus(): void {
    const config = {...SPEECH_DEFAULTS.openai, provider: this.#field<HTMLSelectElement>("provider").value as PersonaSpeech["provider"], endpoint: this.#field<HTMLInputElement>("endpoint").value.trim()};
    this.#field("key-status").textContent = this.#field<HTMLInputElement>("key").value ? "New key entered. Save voice to use it for this endpoint." : this.#key(config) ? "API key present for this persona and endpoint. Leave blank to keep it." : "No API key stored for this endpoint. Keyless local endpoints are supported.";
  }
  refresh(): void {
    if (!this.#root) return;
    this.#populate(this.store.current.speech ?? SPEECH_DEFAULTS.openai);
    this.#field<HTMLButtonElement>("test").disabled = !this.store.features.enabled;
    this.#playbackStatus();
  }
  #playbackStatus(): void {
    if (!this.#root) return;
    this.#field("test").innerHTML = this.player.active ? `${buttonIconHtml("stop")} Stop` : `${buttonIconHtml("run")} Test voice`;
    this.#field("status").textContent = this.player.error || (this.player.active ? "Streaming speech…" : "");
  }
  #save(): boolean {
    try {
      const value = (name: string): string => this.#field<HTMLInputElement>(name).value.trim();
      const config = parseSpeech({enabled: this.#field<HTMLInputElement>("enabled").checked, provider: value("provider"), endpoint: value("endpoint"), model: value("model"), voice: value("voice")});
      const key = value("key");
      if (/[\r\n]/u.test(key)) throw Error("API keys cannot contain line breaks.");
      this.store.importFile(JSON.stringify({...this.store.current, speech: config}));
      if (key) this.#keys.set(this.#identity(config), key);
      this.stop(); this.refresh(); this.changed();
      const reset = this.#root?.closest("dialog")?.querySelector<HTMLButtonElement>('[data-persona-field="remove"]');
      if (reset) reset.disabled = false;
      this.#field("status").textContent = this.store.error || "Voice saved. Use Play beside an assistant response.";
      return true;
    } catch (error) { this.#field("status").textContent = error instanceof Error ? error.message : "Could not save voice settings."; return false; }
  }
}
