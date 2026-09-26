import "./support/window-global.js";
import assert from "node:assert/strict";
import test from "node:test";
import {
  inspectJson,
  jsonPreformattedThunk,
  jsonTokenChildren,
  renderJsonPreformatted,
  tokenizeJson,
} from "../src/json.js";
import {
  all,
  attr,
  attrs,
  byClass,
  domPropValue,
  propValue,
  runInitHook,
  runPrepatchHook,
  textContent,
  visit,
  type VNode,
} from "./support/vnode.js";

function assertSafeTree(vnode: VNode): void {
  visit(vnode, (candidate) => {
    if (candidate.sel !== undefined) {
      assert.equal(["pre", "span"].includes(candidate.sel), true, candidate.sel);
    }
    assert.equal(propValue(candidate, "innerHTML"), undefined);
    assert.equal(domPropValue(candidate, "innerHTML"), undefined);
    assert.equal(candidate.data?.on, undefined);
    for (const name of Object.keys(attrs(candidate))) {
      assert.equal(/^on/iu.test(name), false, `event attribute ${name}`);
      assert.equal(
        ["formaction", "href", "src", "srcdoc", "srcset", "style"].includes(name),
        false,
        `active attribute ${name}`,
      );
    }
  });
}

function assertLiteral(source: string): void {
  const rendered = renderJsonPreformatted("block", source, true);
  assert.equal(textContent(rendered), source);
  assert.equal(byClass(rendered, "json-highlight").length, 0);
  assert.equal(byClass(rendered, "json-key").length, 0);
  assertSafeTree(rendered);
}

test("JSON highlighting preserves every source character and token role", () => {
  const source = ` {\n  "same": "first",\n  "same": "\\u0041\\n<&script>",\n  "unicode": "雪☃",\n  "number": -12.50e+3,\n  "items": [true, false, null, 0]\n} \n`;
  const rendered = renderJsonPreformatted("block", source, true);

  assert.equal(textContent(rendered), source);
  assert.equal(byClass(rendered, "json-highlight").length, 1);
  assert.equal(byClass(rendered, "json-key").length, 5);
  assert.equal(byClass(rendered, "json-string").length, 3);
  assert.equal(byClass(rendered, "json-number").length, 2);
  assert.equal(byClass(rendered, "json-literal").length, 3);
  assert.ok(byClass(rendered, "json-punctuation").length > 0);
  assertSafeTree(rendered);
});

test("hostile JSON strings stay inert text nodes", () => {
  const source = JSON.stringify({
    html: "</script><img src=x onerror=alert(1)>",
    attribute: "javascript:alert(1)",
  });
  const rendered = renderJsonPreformatted("approval", source, true);

  assert.equal(textContent(rendered), source);
  assert.equal(rendered.sel, "pre");
  assert.equal(attr(rendered, "tabindex"), "0");
  assert.equal(all(rendered, (node) => node.sel === "img").length, 0);
  assert.equal(all(rendered, (node) => node.sel === "script").length, 0);
  assertSafeTree(rendered);
});

test("strict malformed JSON falls back to complete literal source", () => {
  for (const source of [
    "",
    "{key: 1}",
    '{"key": 01}',
    '{"key": 1.}',
    '{"key": true,}',
    '{"key": "\\x20"}',
    '["unterminated]',
    "true false",
    "[1,,2]",
    "[NaN]",
  ]) {
    assert.equal(jsonTokenChildren(source), null, source);
    assertLiteral(source);
  }
});

test("inspection distinguishes syntax failure from bounded valid JSON", () => {
  const source = ' {"markdown": "# heading", "value": [1, true]} ';
  const inspection = inspectJson(source);
  assert.ok(inspection.status === "valid");
  const tokenization = tokenizeJson(source);
  assert.ok(tokenization.status === "valid");
  assert.equal(tokenization.segments, inspection.segments);
  assert.equal(tokenization.children.length, inspection.segments);
  assert.equal(
    tokenizeJson(source, inspection.segments - 1).status,
    "bounded",
  );

  assert.equal(inspectJson('{"key": true,}').status, "invalid");
  const depth65 = `${"[".repeat(65)}0${"]".repeat(65)}`;
  assert.equal(inspectJson(depth65).status, "bounded");
  assert.equal(inspectJson(JSON.stringify("雪".repeat(30_000))).status, "bounded");
});

test("source, depth, and segment budgets fail closed", () => {
  const depth64 = `${"[".repeat(64)}0${"]".repeat(64)}`;
  const depth65 = `${"[".repeat(65)}0${"]".repeat(65)}`;
  assert.notEqual(jsonTokenChildren(depth64), null);
  assertLiteral(depth65);

  assert.notEqual(jsonTokenChildren("[0]", 3), null);
  assert.equal(jsonTokenChildren("[0]", 2), null);
  assert.equal(jsonTokenChildren("[0]", 4_097), null);

  const manySegments = `[${Array.from({ length: 2_100 }, () => "0").join(",")}]`;
  assertLiteral(manySegments);

  const oversized = JSON.stringify("雪".repeat(30_000));
  assert.ok(new TextEncoder().encode(oversized).byteLength > 64 * 1024);
  assertLiteral(oversized);
});

test("highlight selection can be disabled without changing source", () => {
  const source = '{"key": true}';
  const rendered = renderJsonPreformatted("approval", source, false);
  assert.equal(textContent(rendered), source);
  assert.equal(byClass(rendered, "json-highlight").length, 0);
  assert.equal(byClass(rendered, "json-key").length, 0);
});

test("JSON thunk reuses unchanged content and replaces changed source", () => {
  const first = jsonPreformattedThunk("block", '{"first": 1}', true);
  assert.equal(first.sel, "pre");
  runInitHook(first);

  const same = jsonPreformattedThunk("block", '{"first": 1}', true);
  runPrepatchHook(first, same);
  assert.equal(same.children, first.children);

  const changed = jsonPreformattedThunk("block", '{"second": 2}', true);
  runPrepatchHook(same, changed);
  assert.notEqual(changed.children, same.children);
  assert.equal(textContent(changed), '{"second": 2}');
});
