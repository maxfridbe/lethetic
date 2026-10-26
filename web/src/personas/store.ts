import { BUILTIN_PERSONAS } from "./builtins.js";
import { narrationSection, parsePersona } from "./schema.js";
import type { Persona } from "./schema.js";
export interface PersonaFeatures { enabled: boolean; narration: boolean; quips: boolean; avatar: boolean; audio: boolean }
const NO_QUIPS: readonly string[] = [];
const KEY = "lethetic.personas.v1";
export class PersonaStore {
  selected = "sassy";
  features: PersonaFeatures = {enabled: true, narration: true, quips: true, avatar: true, audio: false};
  custom: Persona[] = [];
  error = "";
  constructor(readonly storage?: Pick<Storage, "getItem" | "setItem">) {
    try {
      const source = storage?.getItem(KEY);
      if (!source) return;
      if (new TextEncoder().encode(source).length > 4 * 1024 * 1024) throw Error("Saved persona library is too large.");
      const data = JSON.parse(source) as {version?: unknown; selected?: unknown; features?: Record<string, unknown>; custom?: unknown[]};
      if (data.version !== 1 || !Array.isArray(data.custom) || data.custom.length > 20) throw Error("Invalid persona library.");
      const custom = data.custom.map(value => parsePersona(JSON.stringify(value)));
      if (new Set(custom.map(p => p.id)).size !== custom.length) throw Error("Duplicate persona IDs.");
      this.custom = custom;
      if (typeof data.selected === "string" && this.list.some(p => p.id === data.selected)) this.selected = data.selected;
      for (const key of Object.keys(this.features) as (keyof PersonaFeatures)[]) if (typeof data.features?.[key] === "boolean") this.features[key] = data.features[key];
    } catch { this.error = "Saved personas could not be loaded. Built-in personas are available."; }
  }
  get list(): readonly Persona[] { return [...BUILTIN_PERSONAS.map(p => this.custom.find(c => c.id === p.id) ?? p), ...this.custom.filter(c => !BUILTIN_PERSONAS.some(p => p.id === c.id))]; }
  get current(): Persona { return this.list.find(p => p.id === this.selected) ?? BUILTIN_PERSONAS[0]!; }
  get section(): string { return this.features.enabled && this.features.narration ? narrationSection(this.current) : ""; }
  get quips(): readonly string[] { return this.features.enabled && this.features.quips ? this.current.quips : NO_QUIPS; }
  select(id: string): void { if (!this.list.some(p => p.id === id)) throw Error("Unknown persona."); this.selected = id; this.save(); }
  importFile(source: string): void {
    const persona = parsePersona(source);
    const next = [...this.custom.filter(p => p.id !== persona.id), persona];
    if (next.length > 20 || new TextEncoder().encode(JSON.stringify(next)).length > 4 * 1024 * 1024) throw Error("Persona library limit: 20 custom personas / 4 MiB. Remove one first.");
    this.custom = next; this.selected = persona.id; this.save();
  }
  remove(): void { this.custom = this.custom.filter(p => p.id !== this.selected); if (!this.list.some(p => p.id === this.selected)) this.selected = "sassy"; this.save(); }
  save(): void {
    try { this.storage?.setItem(KEY, JSON.stringify({version: 1, selected: this.selected, features: this.features, custom: this.custom})); this.error = ""; }
    catch { this.error = "Personas changed for this tab; storage is unavailable or full. Export to keep a copy."; }
  }
}
