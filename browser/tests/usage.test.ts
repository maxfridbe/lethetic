import assert from "node:assert/strict";
import test from "node:test";
import { readUsage } from "../src/usage.js";
import { BrowserMetrics } from "../src/metrics.js";

await test("DeepSeek and OpenAI usage preserve cache and reasoning counts without double counting", () => {
  const deepseek = readUsage("openai", { usage: { prompt_tokens: 1200, completion_tokens: 64,
    total_tokens: 1264, prompt_cache_hit_tokens: 1024, prompt_cache_miss_tokens: 176,
    completion_tokens_details: { reasoning_tokens: 16 } } });
  assert.deepEqual(deepseek, { input: 1200, output: 64, total: 1264, cached: 1024, cacheWrite: null, reasoning: 16 });
  assert.equal(readUsage("openai", { usage: { prompt_tokens: 10, completion_tokens: 0,
    prompt_tokens_details: { cached_tokens: 4 } } }).total, 10);
  assert.equal(readUsage("openai", { usage: { prompt_cache_hit_tokens: 4, prompt_cache_miss_tokens: 6,
    completion_tokens: 2 } }).input, 10);
});

await test("Anthropic input includes cache reads and writes once", () => {
  assert.deepEqual(readUsage("anthropic", { usage: { input_tokens: 10, output_tokens: 20,
    cache_read_input_tokens: 100, cache_creation_input_tokens: 50 } }),
  { input: 160, output: 20, total: 180, cached: 100, cacheWrite: 50, reasoning: null });
});

await test("absent, partial and invalid usage stays unknown; actual zeros stay zero", () => {
  for (const value of [null, {}, { usage: {} }, { usage: { prompt_tokens: -1, completion_tokens: "5", total_tokens: 1.5 } }]) {
    assert.equal(readUsage("openai", value).total, null);
  }
  assert.equal(readUsage("openai", { usage: { prompt_tokens: Number.MAX_SAFE_INTEGER, completion_tokens: 1 } }).total, null);
  assert.equal(readUsage("openai", { usage: { prompt_tokens: 0, completion_tokens: 0 } }).total, 0);
  assert.equal(readUsage("openai", { usage: { prompt_tokens: 12 } }).output, null);
});

await test("turn and session metrics aggregate tool rounds, mark missing usage and reset", () => {
  let now = 0; const metrics = new BrowserMetrics(() => now);
  const usage = readUsage("openai", { usage: { prompt_tokens: 100, completion_tokens: 20 } });
  metrics.begin(); metrics.event({ type: "request", round: 1 });
  metrics.event({ type: "response", usage, durationMs: 1000 });
  now = 1000; const call = { id: "tool", name: "python", arguments: {} };
  metrics.event({ type: "tool_start", call }); now = 1300;
  metrics.event({ type: "tool_end", call, output: "42" });
  metrics.event({ type: "request", round: 2 });
  metrics.event({ type: "response", usage, durationMs: 700 }); now = 2000;
  metrics.finish(); assert.equal(metrics.elapsed, 2000); assert.equal(metrics.toolMs, 300);
  assert.equal(metrics.modelMs, 1700); assert.equal(metrics.turn.sums.total, 240);
  metrics.begin(); metrics.event({ type: "request", round: 1 });
  metrics.event({ type: "response", usage: readUsage("openai", {}), durationMs: 20 });
  assert.equal(metrics.turn.label("total"), "Not reported");
  assert.equal(metrics.session.label("total"), "240 (partial)");
  metrics.finish(); metrics.reset(); assert.equal(metrics.session.requests, 0);
  assert.equal(metrics.elapsed, 0); assert.equal(metrics.turn.sums.total, null);
});

await test("context tracks the last request, not cumulative usage, and survives refresh", () => {
  const metrics = new BrowserMetrics(); metrics.begin();
  metrics.event({type:"request",round:1,contextTokens:250});
  assert.deepEqual(metrics.context,{tokens:250,source:"local_estimate"});
  metrics.event({type:"response",usage:readUsage("openai",{usage:{prompt_tokens:300,completion_tokens:10}}),durationMs:10});
  metrics.event({type:"request",round:2,contextTokens:350});
  metrics.event({type:"response",usage:readUsage("openai",{usage:{prompt_tokens:400,completion_tokens:20}}),durationMs:10});
  assert.equal(metrics.turn.sums.input,700); assert.deepEqual(metrics.context,{tokens:400,source:"server_usage"});
  const restored=new BrowserMetrics(); restored.restore(metrics.snapshot()); assert.deepEqual(restored.context,metrics.context);
  metrics.event({type:"request",round:3,contextTokens:450});
  metrics.event({type:"response",usage:readUsage("openai",{}),durationMs:10});
  assert.deepEqual(metrics.context,{tokens:450,source:"local_estimate"});
  metrics.reset(); assert.equal(metrics.context.tokens,0);
});
