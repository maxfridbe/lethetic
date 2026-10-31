import test from "node:test";
import assert from "node:assert/strict";
import { CredentialStorage } from "../src/native-credentials.js";
const connection = "lethetic.connection-keys.v1", speech = "lethetic.speech-keys.v1";
test("credential writes serialize and restore in a new application instance", async () => {
  const disk: Record<string, string> = {};
  const write = async (key: string, value: string | null): Promise<void> => { await Promise.resolve(); if (value === null) delete disk[key]; else disk[key] = value; };
  const store = new CredentialStorage({}, write);
  store.setItem(connection, "old"); store.setItem(connection, "new"); store.setItem(speech, "voice-key");
  await store.flush();
  const reopened = new CredentialStorage(disk, write);
  assert.equal(reopened.getItem(connection), "new"); assert.equal(reopened.getItem(speech), "voice-key");
  reopened.clear(); await reopened.flush(); assert.deepEqual(disk, {});
  assert.throws(() => store.setItem("unrelated", "secret"), /Invalid/);
});
test("a successful unrelated write does not hide a failed credential save", async () => {
  let fail = true;
  const store = new CredentialStorage({}, async key => { if (key === connection && fail) throw Error("disk full"); });
  store.setItem(connection, "key"); store.setItem(speech, "voice");
  await assert.rejects(store.flush(), /could not be saved/);
  fail = false; store.setItem(connection, "key"); await store.flush();
});
