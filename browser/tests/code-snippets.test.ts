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

await test("Python presentation extracts exact multiline code and refuses incomplete or output payloads", async () => {
  const { pythonToolCode } = await import("../../web/src/app/helpers.js");
  const loss = {filtered:false,redacted:false,truncation:null} as const;
  const code = "for value in [1, 2]:\n\tprint(value)\n# <script> is ordinary code text";
  const tool = {kind:"call" as const,tool_name:"python",payload:JSON.stringify({code}),payload_loss:loss};
  assert.equal(pythonToolCode({tool}),code);
  assert.equal(pythonToolCode({tool:{...tool,kind:"result"}}),null);
  assert.equal(pythonToolCode({tool:{...tool,payload:'{"code":'}}),null);
  assert.equal(pythonToolCode({tool:{...tool,payload_loss:{...loss,truncation:"size_limit"}}}),null);
});
