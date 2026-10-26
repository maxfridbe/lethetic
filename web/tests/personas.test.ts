import assert from "node:assert/strict";
import test from "node:test";
import { PersonaStore } from "../src/personas/store.js";
import { BUILTIN_PERSONAS } from "../src/personas/builtins.js";
import { narrationSection, parsePersona, withPersonaSection } from "../src/personas/schema.js";
import { PersonaEvents } from "../src/personas/events.js";
import { ThinkingLines } from "../src/app/thinking-lines.js";
import type { WebAppSnapshot } from "../src/generated/contracts.js";

const custom = {version: 1, id: "test-persona", name: "Tester", description: "test", narration: "Be dry and concise.", quips: ["Thinking quietly…"]};
test("persona import is bounded and atomic; selection and toggles survive reload", () => {
  const data = new Map<string, string>();
  const storage = {getItem: (key: string) => data.get(key) ?? null, setItem: (key: string, value: string) => { data.set(key, value); }};
  const store = new PersonaStore(storage);
  assert.equal(store.current.id, "sassy"); assert.equal(store.features.audio, false);
  store.importFile(JSON.stringify(custom));
  store.features.quips = false; store.save();
  assert.equal(new PersonaStore(storage).current.id, "test-persona"); assert.equal(new PersonaStore(storage).quips.length, 0);
  assert.throws(() => store.importFile(JSON.stringify({...custom, audio: {when_model_done_responding: {src: "https://tracker.invalid/sound.mp3", volume: 1}}})), /embedded/);
  assert.equal(store.current.id, "test-persona");
  assert.throws(() => parsePersona(JSON.stringify({...custom, quips: []})), /quips/);
  assert.throws(() => parsePersona(JSON.stringify({...custom, narration: "x".repeat(2001)})), /narration/);
  assert.throws(() => parsePersona(JSON.stringify({...custom, unexpected: "value"})), /Unknown/);
  store.features.enabled = false; assert.equal(store.section, ""); assert.equal(store.quips.length, 0);
  store.remove(); assert.equal(store.current.id, "sassy");
});
test("persona instructions replace only the managed system section", () => {
  const base = "Respect my exact JSON format.  ";
  const first = withPersonaSection(base, narrationSection(BUILTIN_PERSONAS[1]!));
  const next = withPersonaSection(first, narrationSection(BUILTIN_PERSONAS[3]!));
  assert.ok(next.includes("HAL9000")); assert.ok(!next.includes("Skippy"));
  assert.equal(next.split("<!-- lethetic-persona:start -->").length, 2);
  assert.equal(withPersonaSection(next, ""), base);
  assert.match(next, /code, generated files/);
});
test("switching and disabling quips cancel the old rotation", () => {
  const timers = new Map<number, () => void>(); let id = 0;
  const lines = new ThinkingLines(() => {}, {setTimeout: callback => {timers.set(++id, callback); return id;}, clearTimeout: key => {timers.delete(key);}});
  assert.equal(lines.current("turn", false, ["one"]), "one"); assert.equal(timers.size, 1);
  assert.equal(lines.current("turn", false, ["two"]), "two"); assert.equal(timers.size, 1);
  assert.equal(lines.current("turn", false, []), null); assert.equal(timers.size, 0);
});
test("events distinguish completion from cancellation, failure, hydration, and session switch", () => {
  const events = new PersonaEvents();
  const state = (kind: string, reason = "Ready", session = "one") => ({session: {session_id: session}, activity: {kind, fully_idle: kind === "idle"}, status: {stop_reason: reason}, blocks: {blocks: []}} as unknown as WebAppSnapshot);
  assert.equal(events.observe(state("idle"), true), null);
  assert.equal(events.observe(state("processing"), true), "when_model_thinking");
  assert.equal(events.observe(state("processing"), true), null);
  assert.equal(events.observe(state("executing_tool"), true), "when_tool_started");
  assert.equal(events.observe(state("idle"), true), "when_model_done_responding");
  events.observe(state("processing"), true);
  assert.equal(events.observe(state("idle", "Turn stopped."), true), "when_stopped");
  events.observe(state("processing"), true);
  assert.equal(events.observe(state("idle", "Request failed"), true), "when_error");
  events.observe(state("processing"), true);
  assert.equal(events.observe(state("idle", "Ready", "another"), true), null);
  events.observe(state("processing"), true); events.observe(null, false);
  assert.equal(events.observe(state("idle"), true), null);
});
