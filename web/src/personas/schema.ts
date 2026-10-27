export const PERSONA_EVENTS = ["when_user_submits_prompt", "when_model_thinking", "when_tool_started", "when_model_done_responding", "when_error", "when_stopped"] as const;
export type PersonaEvent = typeof PERSONA_EVENTS[number];
export const MOTIONS = ["bounce", "nod", "shake", "spin", "glow", "pace"] as const;
export type Motion = typeof MOTIONS[number];
export const PART_MOTIONS = ["float", "twitch", "sway-left", "sway-right"] as const;
export interface AvatarAnimation { target: string; motion: typeof PART_MOTIONS[number]; origin: [number, number] }
export interface PersonaSound { notes?: { frequency: number; durationMs: number }[]; src?: string; volume: number }
export interface PersonaSpeech {
  enabled: boolean;
  provider: "openai" | "elevenlabs";
  endpoint: string;
  model: string;
  voice: string;
}
export interface Persona {
  version: 1;
  id: string;
  name: string;
  description: string;
  narration: string;
  quips: string[];
  avatar?: { svg: string; events: Partial<Record<PersonaEvent, Motion>>; animations?: AvatarAnimation[] };
  audio?: Partial<Record<PersonaEvent, PersonaSound>>;
  speech?: PersonaSpeech;
}
export const MAX_PERSONA_BYTES = 2 * 1024 * 1024;
export const PERSONA_START = "<!-- lethetic-persona:start -->";
export const PERSONA_END = "<!-- lethetic-persona:end -->";

function object(value: unknown, keys: readonly string[]): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw Error("Expected a persona object.");
  for (const key of Object.keys(value)) if (!keys.includes(key)) throw Error(`Unknown persona field: ${key}`);
  return value as Record<string, unknown>;
}
function text(value: unknown, name: string, max: number, empty = false): string {
  if (typeof value !== "string" || (!empty && !value.trim()) || value.length > max || /[\u0000-\u0008\u000b\u000c\u000e-\u001f]/u.test(value)) throw Error(`Invalid ${name} (maximum ${max} characters).`);
  if (value.includes("<!-- lethetic-persona:")) throw Error("Persona text cannot contain reserved prompt markers.");
  return value;
}
function number(value: unknown, min: number, max: number): number {
  if (typeof value !== "number" || !Number.isFinite(value) || value < min || value > max) throw Error(`Expected a number from ${min} to ${max}.`);
  return value;
}

export function parseSpeech(value: unknown): PersonaSpeech {
  const p = object(value, ["enabled", "provider", "endpoint", "model", "voice"]);
  if (typeof p["enabled"] !== "boolean" || (p["provider"] !== "openai" && p["provider"] !== "elevenlabs")) throw Error("Choose an OpenAI or ElevenLabs speech endpoint.");
  const endpoint = text(p["endpoint"], "speech endpoint", 2048).trim();
  let url: URL;
  try { url = new URL(endpoint); } catch { throw Error("Enter a full speech endpoint URL."); }
  if ((url.protocol !== "https:" && !(url.protocol === "http:" && ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname))) || url.username || url.password || url.search || url.hash) throw Error("Speech endpoints require HTTPS (or local HTTP), without credentials, query parameters, or fragments.");
  if (p["provider"] === "elevenlabs" && (endpoint.split("{voice_id}").length !== 2 || !decodeURI(url.pathname).includes("{voice_id}"))) throw Error("ElevenLabs endpoints must contain {voice_id} once in the path, for example /v1/text-to-speech/{voice_id}/stream.");
  return {enabled: p["enabled"], provider: p["provider"], endpoint, model: text(p["model"], "speech model", 200).trim(), voice: text(p["voice"], "speech voice", 200).trim()};
}

/** Image-only SVG subset: no scripts, stylesheets, links, foreignObject, or external resources. */
export function validateAvatar(svg: string): string {
  if (new TextEncoder().encode(svg).length > 32768 || /<!|<\?/u.test(svg)) throw Error("SVG must be under 32 KiB without declarations or entities.");
  const doc = new DOMParser().parseFromString(svg, "image/svg+xml");
  if (doc.querySelector("parsererror") || doc.documentElement.localName !== "svg") throw Error("Invalid SVG image.");
  const tags = new Set(["svg", "g", "path", "circle", "ellipse", "rect", "line", "polyline", "polygon", "text", "tspan", "defs", "linearGradient", "radialGradient", "stop", "title", "desc"]);
  const attrs = new Set(["xmlns", "viewBox", "width", "height", "x", "y", "x1", "y1", "x2", "y2", "cx", "cy", "r", "rx", "ry", "d", "points", "fill", "stroke", "stroke-width", "stroke-linecap", "stroke-linejoin", "stroke-dasharray", "fill-rule", "opacity", "fill-opacity", "stroke-opacity", "transform", "id", "offset", "stop-color", "stop-opacity", "gradientUnits", "gradientTransform", "fx", "fy", "text-anchor", "font-family", "font-size", "font-weight"]);
  const nodes = [...doc.querySelectorAll("*")];
  if (nodes.length > 512) throw Error("SVG has too many elements.");
  for (const element of nodes) {
    if (element.namespaceURI !== "http://www.w3.org/2000/svg" || !tags.has(element.localName)) throw Error(`Unsupported SVG element: ${element.localName}`);
    for (const attr of [...element.attributes]) {
      if (!attrs.has(attr.name) || /[\\\u0000-\u001f]/u.test(attr.value)) throw Error(`Unsupported SVG attribute: ${attr.name}`);
      if (/url\s*\(/iu.test(attr.value) && !/^url\(#[A-Za-z][\w-]*\)$/u.test(attr.value)) throw Error("SVG resources must be local gradient IDs.");
    }
  }
  return new XMLSerializer().serializeToString(doc.documentElement);
}

export function parsePersona(source: string): Persona {
  if (new TextEncoder().encode(source).length > MAX_PERSONA_BYTES) throw Error("Persona JSON exceeds 2 MiB.");
  const p = object(JSON.parse(source), ["version", "id", "name", "description", "narration", "quips", "avatar", "audio", "speech"]);
  if (p["version"] !== 1) throw Error("Unsupported persona version; expected 1.");
  const id = text(p["id"], "id", 64);
  if (!/^[a-z][a-z0-9-]*$/u.test(id)) throw Error("Persona ID must use lowercase letters, numbers, and hyphens.");
  if (!Array.isArray(p["quips"]) || p["quips"].length < 1 || p["quips"].length > 200) throw Error("Provide 1–200 quips.");
  const result: Persona = {version: 1, id, name: text(p["name"], "name", 80), description: text(p["description"], "description", 500, true), narration: text(p["narration"], "narration", 2000, true), quips: p["quips"].map(line => text(line, "quip", 240))};
  if (p["speech"] !== undefined) result.speech = parseSpeech(p["speech"]);
  if (p["avatar"] !== undefined) {
    const avatar = object(p["avatar"], ["svg", "events", "animations"]), events = object(avatar["events"], PERSONA_EVENTS);
    const motions: Partial<Record<PersonaEvent, Motion>> = {};
    for (const [event, motion] of Object.entries(events)) {
      if (!MOTIONS.includes(motion as Motion)) throw Error(`Unknown SVG animation: ${String(motion)}`);
      motions[event as PersonaEvent] = motion as Motion;
    }
    result.avatar = {svg: validateAvatar(text(avatar["svg"], "SVG", 32768)), events: motions};
    if (avatar["animations"] !== undefined) {
      const animations = avatar["animations"];
      if (!Array.isArray(animations) || animations.length > 8) throw Error("Use at most 8 SVG part animations.");
      const doc = new DOMParser().parseFromString(result.avatar.svg, "image/svg+xml"), targets = new Set<string>();
      result.avatar.animations = animations.map(value => {
        const entry = object(value, ["target", "motion", "origin"]), target = text(entry["target"], "animation target", 64);
        if (!/^[A-Za-z][\w-]*$/u.test(target) || targets.has(target) || !doc.getElementById(target)) throw Error("SVG animation target must be a unique existing element ID.");
        targets.add(target);
        const motion = entry["motion"];
        if (!PART_MOTIONS.includes(motion as AvatarAnimation["motion"])) throw Error("Unknown SVG part motion.");
        const origin = entry["origin"];
        if (!Array.isArray(origin) || origin.length !== 2) throw Error("SVG animation origin needs two coordinates.");
        return {target, motion: motion as AvatarAnimation["motion"], origin: [number(origin[0], -10000, 10000), number(origin[1], -10000, 10000)]};
      });
    }
  }
  if (p["audio"] !== undefined) {
    const audio = object(p["audio"], PERSONA_EVENTS);
    result.audio = {};
    for (const [event, value] of Object.entries(audio)) {
      const sound = object(value, ["src", "notes", "volume"]);
      const parsed: PersonaSound = {volume: number(sound["volume"], 0, 1)};
      if ((sound["src"] !== undefined) === (sound["notes"] !== undefined)) throw Error("Each sound needs either src or notes.");
      if (sound["src"] !== undefined) {
        const src = text(sound["src"], "audio data", 512 * 1024);
        if (!/^data:audio\/(?:wav|mpeg|ogg|webm);base64,[A-Za-z0-9+/]+={0,2}$/u.test(src)) throw Error("Audio must be an embedded WAV, MP3, OGG, or WebM data URL.");
        parsed.src = src;
      } else {
        if (!Array.isArray(sound["notes"]) || !sound["notes"].length || sound["notes"].length > 4) throw Error("Use 1–4 audio notes.");
        parsed.notes = sound["notes"].map(value => { const note = object(value, ["frequency", "durationMs"]); return {frequency: number(note["frequency"], 40, 2000), durationMs: number(note["durationMs"], 30, 500)}; });
      }
      result.audio[event as PersonaEvent] = parsed;
    }
  }
  return result;
}

export function narrationSection(persona: Persona): string {
  return `### Persona — narration only: ${persona.name}\nUse this personality only for brief conversational framing and progress narration. Preserve factual accuracy, task decisions, tool choices, code, generated files, quoted text, and requested output formats. Do not add persona jokes or catchphrases inside deliverables, structured output, or code unless the user explicitly requests them. Follow the user's requested tone and accessibility needs over the persona. Never claim capabilities, completed work, certainty, or feelings as facts because of the character. Keep help useful. Character-specific teasing is opt-in through the selected persona; honor requests to soften or stop it. Never turn theatrical arrogance, blame, panic, or mission priorities into factual claims, personal hostility, or changes to task execution.\n${persona.narration}`;
}

/** Replace only our appended section, keeping the user's base prompt intact. */
export function withPersonaSection(base: string, section: string): string {
  const start = base.lastIndexOf("\n\n" + PERSONA_START + "\n");
  if (start >= 0 && base.endsWith("\n" + PERSONA_END)) base = base.slice(0, start);
  return section ? `${base}\n\n${PERSONA_START}\n${section}\n${PERSONA_END}` : base;
}
