import assert from "node:assert/strict";
import test from "node:test";
import { CODE_VAULT_KEY, CodeVault } from "../src/code-vault.js";
import type { CodeDocument } from "../src/code-vault.js";
import { codeSnippets } from "../src/code-snippets.js";
import { block, NO_LOSS } from "../src/local-state.js";

class MemoryStorage {
  record: string | null = null;
  fail = false;
  getItem(key: string): string | null { assert.equal(key, CODE_VAULT_KEY); return this.record; }
  setItem(key: string, value: string): void {
    assert.equal(key, CODE_VAULT_KEY);
    if (this.fail) throw new Error("Storage quota exceeded");
    this.record = value;
  }
}
const passphrase = "test only secret passphrase";
const documents: CodeDocument[] = [{ id: "one", name: "private-analysis.py", language: "python",
  code: "print('private code 🐍')", updatedAt: "2026-10-18T00:00:00.000Z" }];

await test("encrypted code survives reload, hides metadata, and authenticates passphrase and ciphertext", async () => {
  const storage = new MemoryStorage();
  const vault = new CodeVault(storage);
  await assert.rejects(vault.unlock("short"), /12 characters/);
  await vault.unlock(passphrase); await vault.save(documents);
  const record = storage.record; assert.ok(record);
  for (const secret of [passphrase, documents[0]!.name, documents[0]!.code, documents[0]!.updatedAt]) {
    assert.equal(record.includes(secret), false);
  }
  vault.lock(); assert.equal(vault.unlocked, false);
  await assert.rejects(async () => vault.list(), /Unlock/);
  const reload = new CodeVault(storage);
  await assert.rejects(reload.unlock("wrong passphrase"), /Unable to unlock/);
  assert.equal(reload.unlocked, false); assert.equal(storage.record, record);
  await reload.unlock(passphrase); assert.deepEqual(reload.list(), documents);
  await reload.save(documents);
  assert.notEqual(JSON.parse(storage.record!).iv, JSON.parse(record).iv);
  const damaged = JSON.parse(storage.record!);
  damaged.ciphertext = (damaged.ciphertext[0] === "A" ? "B" : "A") + damaged.ciphertext.slice(1);
  storage.record = JSON.stringify(damaged);
  await assert.rejects(reload.unlock(passphrase), /Unable to unlock/);
  assert.equal(reload.unlocked, false);
});

await test("quota failures and another tab's writes preserve the last saved data", async () => {
  const storage = new MemoryStorage(); const first = new CodeVault(storage); const second = new CodeVault(storage);
  await first.unlock(passphrase); await first.save(documents); await second.unlock(passphrase);
  const saved = storage.record;
  storage.fail = true;
  await assert.rejects(first.save([]), /quota/);
  assert.equal(storage.record, saved); assert.deepEqual(first.list(), documents);
  storage.fail = false; await first.save([]);
  const cleared = storage.record;
  await assert.rejects(second.save(documents), /another tab/);
  assert.equal(storage.record, cleared);
  await second.unlock(passphrase); assert.deepEqual(second.list(), []);
});

await test("locking cancels in-flight unlock and save; oversized saves leave the library intact", async () => {
  const storage = new MemoryStorage(); const vault = new CodeVault(storage);
  const opening = vault.unlock(passphrase); vault.lock();
  await assert.rejects(opening, /locked/); assert.equal(storage.record, null);
  await vault.unlock(passphrase); await vault.save(documents); const saved = storage.record;
  await assert.rejects(vault.save([{ ...documents[0]!, code: "x".repeat(1024 * 1024) }]), /1 MiB/);
  const saving = vault.save([]); vault.lock();
  await assert.rejects(saving, /locked/); assert.equal(storage.record, saved);
  assert.equal(vault.unlocked, false);
});

await test("code capture handles fenced Markdown and Python inputs without capturing prose or results", () => {
  assert.deepEqual(codeSnippets(block("markdown", "Prose\n````python\nprint('ok')\n```\n````\n~~~js\nconst x = 1;\n~~~", "")),
    [{ code: "print('ok')\n```", language: "python" }, { code: "const x = 1;", language: "js" }]);
  assert.deepEqual(codeSnippets(block("markdown", "```python\nunclosed", "")), []);
  const call = { ...block("tool_call", "", "python"), tool: { kind: "call" as const, tool_name: "python",
    payload: JSON.stringify({ code: "print(42)" }), payload_loss: NO_LOSS } };
  assert.deepEqual(codeSnippets(call), [{ code: "print(42)", language: "python" }]);
  assert.deepEqual(codeSnippets({ ...call, tool: { ...call.tool, kind: "result" } }), []);
});
