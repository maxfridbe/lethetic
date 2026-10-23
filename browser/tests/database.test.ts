import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";
import { contentDatabaseFactory } from "../src/sdk.js";

const Database = await contentDatabaseFactory(await readFile(new URL("../wasm/lethetic_wasm_bg.wasm", import.meta.url)));
const encode = (text: string): Uint8Array => new TextEncoder().encode(text);
await test("real Turso WASM commits, exports SQLite bytes, reopens and isolates instances", () => {
  const db = new Database(new Uint8Array()), separate = new Database(new Uint8Array());
  try {
    db.put("session", "transcript", encode("first\n"));
    db.append("session", "transcript", encode("second\n"));
    db.put("files", "image.png", new Uint8Array([0, 255, 1, 2]));
    assert.equal(separate.read("session", "transcript"), undefined);
    const image = db.export();
    assert.equal(new TextDecoder().decode(image.subarray(0, 16)), "SQLite format 3\0");
    const restored = new Database(image);
    try {
      assert.deepEqual(restored.read("session", "transcript"), encode("first\nsecond\n"));
      assert.deepEqual(restored.read("files", "image.png"), new Uint8Array([0, 255, 1, 2]));
      restored.put("session", "transcript", encode("replacement"));
      restored.remove_scope("files");
      assert.deepEqual(JSON.parse(restored.names("files")), []);
      assert.deepEqual(restored.read("session", "transcript"), encode("replacement"));
      assert.deepEqual(db.read("session", "transcript"), encode("first\nsecond\n"));
    } finally { restored.free(); }
  } finally { separate.free(); db.free(); }
});
await test("real Turso WASM rejects invalid database images without damaging another instance", () => {
  const db = new Database(new Uint8Array());
  try {
    assert.throws(() => new Database(encode("not sqlite")));
    for (const version of [0, 999]) {
      const foreign = db.export();new DataView(foreign.buffer).setUint32(60, version);
      assert.throws(() => new Database(foreign));
    }
    db.put("session", "survived", encode("yes"));
    assert.deepEqual(db.read("session", "survived"), encode("yes"));
  } finally { db.free(); }
});

await test("real WASM stores queryable relational rows and reloads their SQL values", () => {
  const db = new Database(new Uint8Array());
  try {
    const session = {id: "test", messages: [{role: "user", content: "hello"},
      {role: "assistant", content: "", tool_calls: [{id: "call-1", function: {name: "python", arguments: {code: "print(2)"}}}]}],
      blocks: [{kind: "user", content: "hello"}], inputs: ["hello"]};
    db.put("browser/sessions", "test", encode(JSON.stringify(session)));
    const result = JSON.parse(db.query("SELECT m.role,t.function_name FROM messages m JOIN tool_calls t ON t.message_id=m.id", "[]"));
    assert.deepEqual(result.columns, ["role", "function_name"]);
    assert.deepEqual(result.rows, [["assistant", "python"]]);
    db.query("UPDATE messages SET content=? WHERE role='user'", JSON.stringify(["edited"]));
    const restored = new Database(db.export());
    try {
      assert.equal(JSON.parse(new TextDecoder().decode(restored.read("browser/sessions", "test"))).messages[0].content, "edited");
      assert.deepEqual(JSON.parse(restored.query("PRAGMA foreign_key_check", "[]")).rows, []);
      assert.deepEqual(JSON.parse(restored.query("SELECT name FROM sqlite_schema WHERE name='content_records'", "[]")).rows, []);
    } finally { restored.free(); }
  } finally { db.free(); }
});
