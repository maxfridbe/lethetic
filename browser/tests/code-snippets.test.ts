import assert from "node:assert/strict";
import test from "node:test";
import { codeSnippets } from "../src/code-snippets.js";
import { block, NO_LOSS } from "../src/local-state.js";

await test("code capture handles fenced Markdown and Python inputs without capturing prose or results", () => {
  assert.deepEqual(codeSnippets(block("markdown", "Prose\n````python\nprint('ok')\n```\n````\n~~~js\nconst x = 1;\n~~~", "")),
    [{ code: "print('ok')\n```", language: "python" }, { code: "const x = 1;", language: "js" }]);
  assert.deepEqual(codeSnippets(block("markdown", "```python\nunclosed", "")), []);
  const call = { ...block("tool_call", "", "python"), tool: { kind: "call" as const, tool_name: "python",
    payload: JSON.stringify({ code: "print(42)" }), payload_loss: NO_LOSS } };
  assert.deepEqual(codeSnippets(call), [{ code: "print(42)", language: "python" }]);
  assert.deepEqual(codeSnippets({ ...call, tool: { ...call.tool, kind: "result" } }), []);
});
