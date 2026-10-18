import assert from "node:assert/strict";
import test from "node:test";
import { PinnedStats } from "../src/pinned-stats.js";

test("pins survive reload, including an intentionally empty bar", () => {
  let value: string | null = null;
  const storage = { getItem: () => value, setItem: (_key: string, next: string) => { value = next; } };
  const pins = new PinnedStats(storage);
  pins.toggle("python");
  assert.deepEqual(new PinnedStats(storage).ids, ["input", "output", "elapsed", "python"]);
  for (const id of [...pins.ids]) pins.toggle(id);
  assert.deepEqual(new PinnedStats(storage).ids, []);
  value = '["python","unknown","python"]';
  assert.deepEqual(new PinnedStats(storage).ids, ["python"]);
  value = 'broken';
  assert.deepEqual(new PinnedStats(storage).ids, ["input", "output", "elapsed"]);
});

test("storage failures keep pins usable in memory", () => {
  const pins = new PinnedStats({ getItem: () => null, setItem: () => { throw Error("quota"); } });
  pins.toggle("python");
  assert.ok(pins.ids.includes("python"));
  assert.ok(pins.error.includes("unavailable"));
});
