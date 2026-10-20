import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";
import { createRuntime } from "../src/sdk.js";
import { loadConnections, saveConnections, validateConnection } from "../src/connection.js";
import type { Connection, Tool } from "../src/sdk.js";

const wasm = await readFile(new URL("../wasm/lethetic_wasm_bg.wasm", import.meta.url));
const connection: Connection = { name: "Test", provider: "openai",
  endpoint: "https://models.example/v1/chat/completions", model: "test-model", apiKey: "test-secret" };
const answer = (text: string): Response => Response.json({ choices: [{ finish_reason: "stop",
  message: { role: "assistant", content: text } }] });
const call = (name: string, arguments_ = "{}"): Response => Response.json({ choices: [{ finish_reason: "tool_calls",
  message: { role: "assistant", content: null, tool_calls: [
    { id: "one", type: "function", function: { name, arguments: arguments_ } },
  ] } }] });

await test("thinking preference reaches model requests and returned reasoning has its own event", async () => {
  for (const thinking of [undefined, "enabled", "disabled"] as const) {
    const events: import("../src/sdk.js").RuntimeEvent[] = [];
    const runtime = await createRuntime({connection: {...connection, thinking}, wasm, onEvent: event => events.push(event),
      fetch: async (_url, init) => {
        const body = JSON.parse(String(init?.body));
        assert.deepEqual(body.reasoning, thinking === "enabled" ? {enabled: true, exclude: false} : thinking === "disabled" ? {enabled: false} : undefined);
        return Response.json({choices: [{finish_reason: "stop", message: {role: "assistant", content: "Answer",
          reasoning: "Visible reasoning", reasoning_details: [{type: "reasoning.text", text: "Visible reasoning", signature: "opaque"}]}}]});
      }});
    try {
      assert.equal(await runtime.send("Question"), "Answer");
      assert.deepEqual(events.filter(event => event.type === "thinking" || event.type === "assistant"),
        [{type: "thinking", text: "Visible reasoning"}, {type: "assistant", text: "Answer"}]);
      const restored = await createRuntime({connection, wasm});
      try { restored.restore(runtime.history()); assert.deepEqual(restored.history(), runtime.history()); }
      finally { restored.dispose(); }
    } finally { runtime.dispose(); }
  }
});

await test("chat refuses a session URL as its key before fetching or loading a conversation", async () => {
  let requests = 0;
  await assert.rejects(createRuntime({connection: {...connection, apiKey: "https://maxfridbe.github.io/lethetic/#/sessions/example"},
    wasm, fetch: async () => { requests++; return answer("unexpected"); }}), /URL or session link/);
  assert.equal(requests, 0);
});

await test("concurrent first initialization keeps WASM conversations independent", async () => {
  const runtimes = await Promise.all(Array.from({ length: 8 }, () => createRuntime({ connection, wasm,
    fetch: async () => answer("ok") })));
  try {
    await Promise.all(runtimes.map((runtime, index) => runtime.send(`prompt ${index}`)));
    for (let index = 0; index < runtimes.length; index++) {
      assert.deepEqual(runtimes[index]!.history(), [{ role: "user", content: `prompt ${index}` }, { role: "assistant", content: "ok" }]);
    }
  } finally { for (const runtime of runtimes) runtime.dispose(); }
});

await test("chat trims pasted token edges and retains the correct credential across turns", async () => {
  const supplied = {...connection, apiKey: " \ttest-token\r\n"};
  let requests = 0;
  const runtime = await createRuntime({connection: supplied, wasm, fetch: async (_url, init) => {
    assert.equal(new Headers(init?.headers).get("Authorization"), "Bearer test-token");
    requests++; return answer("ok");
  }});
  try {
    supplied.apiKey = "https://maxfridbe.github.io/lethetic/#/sessions/example";
    await runtime.send("hello"); await runtime.send("again");
    assert.equal(requests, 2);
  } finally { runtime.dispose(); }
});

await test("actual WASM SDK exposes provider usage and measured response time", async () => {
  const events: import("../src/sdk.js").RuntimeEvent[] = [];
  const runtime = await createRuntime({ connection, wasm, onEvent: (event) => events.push(event),
    fetch: async () => Response.json({ choices: [{ finish_reason: "stop", message: { role: "assistant", content: "ok" } }],
      usage: { prompt_tokens: 1200, completion_tokens: 64, prompt_cache_hit_tokens: 1024 } }) });
  await runtime.send("hello"); runtime.dispose();
  const response = events.find((event) => event.type === "response"); assert.ok(response);
  assert.equal(response.usage.total, 1264); assert.equal(response.usage.cached, 1024);
  assert.ok(response.durationMs >= 0 && Number.isFinite(response.durationMs));
});

await test("actual WASM sends provider requests and keeps separate conversations", async () => {
  const runtime = await createRuntime({ connection, wasm, fetch: async (url, init) => {
    assert.equal(url, connection.endpoint);
    assert.equal(init?.credentials, "omit");
    assert.equal(init?.redirect, "error");
    assert.equal(new Headers(init?.headers).get("Authorization"), "Bearer test-secret");
    const body = JSON.parse(String(init?.body));
    assert.equal(body.model, "test-model");
    assert.equal(body.messages[1].content, "hello");
    assert.equal(body.stream, false);
    assert.doesNotMatch(String(init?.body), /test-secret/);
    return answer("world");
  } });
  assert.equal(await runtime.send("hello"), "world");
  assert.equal(runtime.history().length, 2);
  const other = await createRuntime({ connection, wasm });
  assert.equal(other.history().length, 0);
  runtime.dispose(); other.dispose();
});

await test("host tool results reach the next request", async () => {
  let requests = 0;
  const tools: Tool[] = [{ name: "lookup", description: "Lookup", parameters: { type: "object" },
    execute: (arguments_) => { assert.equal(arguments_["id"], 7); return "found"; } }];
  const runtime = await createRuntime({ connection, wasm, tools, fetch: async (_url, init) => {
    requests++;
    if (requests === 1) return call("lookup", '{"id":7}');
    const body = JSON.parse(String(init?.body));
    assert.equal(body.messages[3].role, "tool");
    assert.equal(body.messages[3].content, "found");
    return answer("finished");
  } });
  assert.equal(await runtime.send("look up"), "finished");
  assert.equal(requests, 2);
  runtime.dispose();
});

await test("Anthropic uses its own headers and content blocks", async () => {
  const runtime = await createRuntime({ connection: { ...connection, provider: "anthropic" }, wasm,
    fetch: async (_url, init) => {
      const headers = new Headers(init?.headers);
      assert.equal(headers.get("x-api-key"), "test-secret");
      assert.equal(headers.get("Authorization"), null);
      const body = JSON.parse(String(init?.body));
      assert.equal(body.messages[0].role, "user");
      assert.equal(body.max_tokens, 4096);
      return Response.json({ role: "assistant", stop_reason: "end_turn", content: [{ type: "text", text: "hi" }] });
    } });
  assert.equal(await runtime.send("hello"), "hi");
  runtime.dispose();
});

await test("provider errors roll back the failed turn and allow recovery", async () => {
  let fail = true;
  const runtime = await createRuntime({ connection, wasm, fetch: async () =>
    fail ? new Response("private server diagnostics", { status: 401 }) : answer("ok") });
  await assert.rejects(runtime.send("fail"), /HTTP 401/);
  assert.equal(runtime.history().length, 0);
  fail = false;
  assert.equal(await runtime.send("retry"), "ok");
  runtime.dispose();
});

await test("unregistered tools and oversized responses cannot execute", async () => {
  const runtime = await createRuntime({ connection, wasm, fetch: async () => call("shell") });
  await assert.rejects(runtime.send("go"), /unregistered/);
  assert.equal(runtime.history().length, 0);
  runtime.dispose();
  const oversized = await createRuntime({ connection, wasm, fetch: async () =>
    new Response("x".repeat(4 * 1024 * 1024 + 1)) });
  await assert.rejects(oversized.send("go"), /exceeds/);
  oversized.dispose();
});

await test("cancellation stops a pending tool and rejects concurrent sends", async () => {
  let started!: () => void;
  const ready = new Promise<void>((resolve) => { started = resolve; });
  const runtime = await createRuntime({ connection, wasm, tools: [{ name: "wait", description: "wait",
    parameters: { type: "object" }, execute: () => { started(); return new Promise<string>(() => {}); } }],
    fetch: async () => call("wait") });
  const sending = runtime.send("go");
  await ready;
  await assert.rejects(runtime.send("again"), /already running/);
  runtime.cancel();
  await assert.rejects(sending, /stopped/);
  assert.equal(runtime.history().length, 0);
  runtime.dispose();
});

await test("round limits prevent runaway tool loops", async () => {
  const runtime = await createRuntime({ connection, wasm, maxRounds: 2, tools: [{ name: "loop",
    description: "loop", parameters: { type: "object" }, execute: () => "again" }],
    fetch: async () => call("loop") });
  await assert.rejects(runtime.send("go"), /round limit/);
  assert.equal(runtime.history().length, 0);
  runtime.dispose();
});

await test("saved connection metadata never includes keys and ignores malformed entries", async () => {
  let data = "";
  const storage = { setItem: (_key: string, value: string): void => { data = value; }, getItem: () => data };
  saveConnections(storage, [connection]);
  assert.doesNotMatch(data, /secret|apiKey/);
  assert.equal(loadConnections(storage)[0]?.model, "test-model");
  data = '[{"name":"unsafe","endpoint":"javascript:alert(1)","provider":"openai","model":"x"}]';
  assert.deepEqual(loadConnections(storage), []);
  data = '{broken';
  assert.deepEqual(loadConnections(storage), []);
  for (const endpoint of ["https://user:pass@example.com", "https://example.com/#token", "http://example.com", "https://example.com/?key=secret"]) {
    await assert.rejects(async () => validateConnection({ ...connection, endpoint }));
  }
});

await test("pre-aborted requests and timeout do not leave a partial conversation", async () => {
  const controller = new AbortController(); controller.abort();
  const runtime = await createRuntime({ connection, wasm, timeoutMs: 10,
    fetch: async (_url, init) => new Promise<Response>((_resolve, reject) => {
      init?.signal?.addEventListener("abort", () => reject(init.signal?.reason), { once: true });
    }) });
  await assert.rejects(runtime.send("go", controller.signal));
  await assert.rejects(runtime.send("go"), /timed out/);
  assert.equal(runtime.history().length, 0);
  runtime.dispose();
});

await test("restoring actual WASM history continues context without reexecuting tools", async () => {
  let executions = 0;
  const tools: Tool[] = [{ name: "lookup", description: "Lookup", parameters: { type: "object" },
    execute: () => { executions++; return "saved result"; } }];
  let requests = 0;
  const original = await createRuntime({ connection, wasm, tools,
    fetch: async () => ++requests === 1 ? call("lookup") : answer("finished") });
  await original.send("original question");
  const history = original.history(); original.dispose();
  const restored = await createRuntime({ connection, wasm, tools, fetch: async (_url, init) => {
    const messages = JSON.parse(String(init?.body)).messages;
    assert.equal(messages[1].content, "original question");
    assert.equal(messages[3].content, "saved result");
    assert.equal(messages[5].content, "continue");
    return answer("continued");
  } });
  restored.restore(history); assert.equal(executions, 1);
  assert.equal(await restored.send("continue"), "continued");
  assert.equal(executions, 1); restored.dispose();
});
