import assert from "node:assert/strict";
import test from "node:test";
import { modelAuthHeaders, normalizeApiKey, validateConnection, saveConnections, loadConnections,
  saveConnectionKeys, restoreConnectionKeys } from "../src/connection.js";
import { fetchModels } from "../src/models.js";

const connection = {name: "Example", provider: "openai" as const, endpoint: "https://models.example/v1/chat/completions", model: "test"};
const sessionURL = "https://maxfridbe.github.io/lethetic/#/sessions/00000000-0000-4000-8000-000000000001";

await test("URLs, session links and copied authorization headers cannot become model credentials", async () => {
  for (const apiKey of [sessionURL, " \n" + sessionURL + "\n", "HTTPS://example.com/key", "#/sessions/example",
    "/sessions/example", "Bearer secret-test", "Authorization: Bearer secret-test", "token\ninjection", "token with spaces"]) {
    for (const provider of ["openai", "anthropic"] as const) {
      assert.throws(() => validateConnection({...connection, provider, apiKey}), /API key/);
      assert.throws(() => modelAuthHeaders({provider, apiKey}), /API key/);
      await assert.rejects(fetchModels({...connection, provider, apiKey}, new AbortController().signal,
        async () => { throw new Error("Invalid key reached fetch"); }), /API key/);
    }
  }
  try { normalizeApiKey(sessionURL); } catch (error) {
    assert.doesNotMatch(String(error), /00000000|github\.io/);
  }
});

await test("chat and discovery share exact bare token handling for both providers", async () => {
  const token = "opaque-test._~+/=-123";
  assert.equal(normalizeApiKey(" \t" + token + "\r\n"), token);
  for (const apiKey of [undefined, "", " \t"]) assert.equal(normalizeApiKey(apiKey), undefined);
  assert.deepEqual(modelAuthHeaders({...connection, apiKey: token}), {Authorization: `Bearer ${token}`});
  assert.deepEqual(modelAuthHeaders({...connection, apiKey: ""}), {});
  for (const provider of ["openai", "anthropic"] as const) {
    const saved = validateConnection({...connection, provider, apiKey: " " + token + "\n"});
    const headers = modelAuthHeaders(saved);
    assert.equal(headers[provider === "openai" ? "Authorization" : "x-api-key"], provider === "openai" ? `Bearer ${token}` : token);
    await fetchModels({...saved, endpoint: provider === "openai" ? saved.endpoint : "https://models.example/v1/messages"},
      new AbortController().signal, async (_url, init) => {
        for (const [name, value] of Object.entries(headers)) assert.equal(new Headers(init?.headers).get(name), value);
        return Response.json({data: []});
      });
  }
});

await test("bad tab credentials remain editable on reload but are rejected before use", () => {
  const memory = new Map<string, string>();
  const storage = {getItem: (key: string) => memory.get(key) ?? null, setItem: (key: string, value: string) => { memory.set(key, value); }};
  saveConnections(storage, [connection]);
  // Simulate a credential stored by an older version, without discarding the connection/session.
  saveConnectionKeys(storage, [{...connection, apiKey: sessionURL}]);
  const restored = restoreConnectionKeys(storage, loadConnections(storage));
  assert.equal(restored.length, 1);
  assert.equal(restored[0]?.apiKey, sessionURL);
  assert.throws(() => validateConnection(restored[0]!), /URL or session link/);
  saveConnectionKeys(storage, [{...connection, apiKey: "corrected-token"}]);
  assert.equal(validateConnection(restoreConnectionKeys(storage, loadConnections(storage))[0]!).apiKey, "corrected-token");
});
