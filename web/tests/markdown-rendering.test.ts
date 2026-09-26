import assert from "node:assert/strict";
import test from "node:test";
import {
  isMarkdownBlockKind,
  markdownContentThunk,
  renderMarkdownContent,
  renderToolResultContent,
  toolResultContentThunk,
} from "../src/markdown.js";
import {
  attr,
  attrs,
  byClass,
  bySelector,
  domPropValue,
  propValue,
  runInitHook,
  runPrepatchHook,
  textContent,
  visit,
  type VNode,
} from "./support/vnode.js";

function jsonTokenCount(vnode: VNode): number {
  return [
    "json-key",
    "json-string",
    "json-number",
    "json-literal",
    "json-punctuation",
  ].reduce((total, className) => total + byClass(vnode, className).length, 0);
}

function assertLiteral(vnode: VNode, source: string): void {
  assert.equal(byClass(vnode, "markdown-content").length, 0);
  const pre = bySelector(vnode, "pre");
  assert.equal(pre.length, 1);
  const [only] = pre;
  assert.ok(only);
  assert.equal(textContent(only), source);
}

function assertRich(vnode: VNode): void {
  assert.equal(byClass(vnode, "markdown-content").length, 1);
}

function assertSafeTree(vnode: VNode): void {
  const forbiddenSelectors = new Set([
    "audio",
    "embed",
    "form",
    "iframe",
    "img",
    "input",
    "link",
    "meta",
    "object",
    "script",
    "source",
    "style",
    "svg",
    "video",
  ]);
  visit(vnode, (candidate) => {
    if (candidate.sel !== undefined) {
      assert.equal(
        forbiddenSelectors.has(candidate.sel),
        false,
        `unsafe selector ${candidate.sel}`,
      );
    }
    assert.equal(propValue(candidate, "innerHTML"), undefined);
    assert.equal(domPropValue(candidate, "innerHTML"), undefined);
    assert.equal(candidate.data?.on, undefined);
    for (const [name, value] of Object.entries(attrs(candidate))) {
      assert.equal(/^on/iu.test(name), false, `event attribute ${name}`);
      assert.equal(
        ["formaction", "src", "srcdoc", "srcset", "style"].includes(name),
        false,
        `fetching or active attribute ${name}`,
      );
      if (name === "href") {
        assert.equal(candidate.sel, "a");
        assert.ok(typeof value === "string");
        assert.match(value, /^https:\/\//u);
      }
    }
  });
}

test("only assistant text and explicit markdown are renderer candidates", () => {
  for (const kind of ["text", "markdown"]) {
    assert.equal(isMarkdownBlockKind(kind), true, kind);
  }
  for (const kind of [
    "user",
    "thought",
    "tool_call",
    "tool_result",
    "divider",
    "formulating",
    "truncation",
  ]) {
    assert.equal(isMarkdownBlockKind(kind), false, kind);
  }
});

test("text uses parser-based detection while explicit markdown always renders", () => {
  const plain = "plain assistant prose\nwith a second line";
  assertLiteral(renderMarkdownContent("text", plain), plain);
  assertLiteral(
    renderMarkdownContent("text", "<strong>raw HTML only</strong>"),
    "<strong>raw HTML only</strong>",
  );

  const formatted = renderMarkdownContent(
    "text",
    "# Heading\n\nA **strong** statement.",
  );
  assertRich(formatted);
  assert.equal(bySelector(formatted, "h1").length, 1);
  assert.equal(bySelector(formatted, "strong").length, 1);

  const entity = renderMarkdownContent("text", "A &amp; B");
  assertRich(entity);
  assert.equal(textContent(entity), "A & B");

  const explicit = renderMarkdownContent("markdown", "plain paragraph");
  assertRich(explicit);
  assert.equal(bySelector(explicit, "p").length, 1);
});

test("GFM block and inline constructs become fixed semantic VNodes", () => {
  const source = [
    "# Heading",
    "",
    "> quoted **strong** and *emphasized* text",
    "",
    "1. first",
    "2. second",
    "   - nested",
    "",
    "- [x] complete",
    "- [ ] pending",
    "",
    "~~deleted~~ and `inline`  ",
    "hard break",
    "",
    "```rust",
    "fn main() {}",
    "```",
    "",
    "| left | right |",
    "| :--- | ---: |",
    "| one | two |",
    "",
    "---",
    "",
    "\\*escaped asterisk*",
  ].join("\n");
  const vnode = renderMarkdownContent("markdown", source);
  assertRich(vnode);
  for (const selector of [
    "h1",
    "blockquote",
    "ol",
    "ul",
    "li",
    "strong",
    "em",
    "del",
    "br",
    "pre",
    "code",
    "table",
    "thead",
    "tbody",
    "th",
    "td",
    "hr",
  ]) {
    assert.ok(bySelector(vnode, selector).length > 0, selector);
  }
  const renderedText = textContent(vnode);
  assert.equal((renderedText.match(/☑/gu) ?? []).length, 1);
  assert.equal((renderedText.match(/☐/gu) ?? []).length, 1);
  assert.match(renderedText, /rust/u);
  assert.match(renderedText, /\*escaped asterisk\*/u);
  assertSafeTree(vnode);
});

test("asterisk Markdown remains stable across provider thought boundaries", () => {
  const thematicBreak = renderMarkdownContent("markdown", "***");
  assertRich(thematicBreak);
  assert.equal(bySelector(thematicBreak, "hr").length, 1);

  const nested = renderMarkdownContent("markdown", "***text***");
  assertRich(nested);
  assert.equal(bySelector(nested, "strong").length, 1);
  assert.equal(bySelector(nested, "em").length, 1);
  assert.equal(textContent(nested), "text");

  const separated = renderMarkdownContent(
    "markdown",
    "**segment A**\n\n**segment B**",
  );
  assertRich(separated);
  assert.equal(bySelector(separated, "strong").length, 2);
  assert.equal(textContent(separated), "segment Asegment B");

  const json = '{"marker":"***","joined":"****"}';
  const strictJson = renderToolResultContent(json, false);
  assert.equal(byClass(strictJson, "json-highlight").length, 1);
  assert.equal(textContent(strictJson), json);
});

test("only valid fenced JSON receives source-preserving token spans", () => {
  const source = [
    "```JSON",
    '{"key": [true, 1, "<script>"]}',
    "```",
  ].join("\n");
  const rendered = renderMarkdownContent("markdown", source);
  assert.equal(byClass(rendered, "json-highlight").length, 1);
  assert.equal(byClass(rendered, "json-key").length, 1);
  assert.equal(byClass(rendered, "json-literal").length, 1);
  assert.match(textContent(rendered), /\{"key": \[true, 1, "<script>"\]\}/u);
  assertSafeTree(rendered);

  const lossy = renderMarkdownContent("markdown", source, false);
  assert.equal(byClass(lossy, "json-highlight").length, 0);
  assert.match(textContent(lossy), /\{"key": \[true, 1, "<script>"\]\}/u);

  const malformed = renderMarkdownContent(
    "markdown",
    ["```json", '{"key": true,}', "```"].join("\n"),
  );
  assert.equal(byClass(malformed, "json-highlight").length, 0);
  assert.match(textContent(malformed), /\{"key": true,\}/u);

  const otherLanguage = renderMarkdownContent(
    "markdown",
    ["```rust", '{"key": true}', "```"].join("\n"),
  );
  assert.equal(byClass(otherLanguage, "json-highlight").length, 0);
});

test("tool results prefer strict JSON and otherwise detect meaningful Markdown", () => {
  const overview = [
    "# Repository Overview: `[REDACTED]`",
    "",
    "## Directory Structure",
    "```text",
    "project/",
    "└── src/",
    "```",
  ].join("\n");
  const markdown = renderToolResultContent(overview, false);
  assertRich(markdown);
  assert.equal(bySelector(markdown, "h1").length, 1);
  assert.equal(bySelector(markdown, "h2").length, 1);
  assert.equal(bySelector(markdown, "code").length, 2);
  assert.equal(byClass(markdown, "markdown-code-block").length, 1);
  assert.match(textContent(markdown), /project\/\n└── src\//u);
  assertSafeTree(markdown);

  const json = '{"markdown":"# heading","html":"<img src=x>"}';
  const structured = renderToolResultContent(json, false);
  assert.equal(byClass(structured, "json-highlight").length, 1);
  assert.equal(bySelector(structured, "h1").length, 0);
  assert.equal(bySelector(structured, "img").length, 0);
  assert.equal(textContent(structured), json);
  assertSafeTree(structured);

  const plain = "plain tool output without markup";
  assertLiteral(renderToolResultContent(plain, false), plain);
  const malformed = '{"not": markdown,}';
  assertLiteral(renderToolResultContent(malformed, false), malformed);
  assertLiteral(renderToolResultContent(overview, true), overview);
});

test("bounded valid JSON never falls through into Markdown detection", () => {
  const source = `[${Array.from(
    { length: 2_100 },
    () => '"# heading"',
  ).join(",")}]`;
  const rendered = renderToolResultContent(source, false);
  assertLiteral(rendered, source);
  assert.equal(bySelector(rendered, "h1").length, 0);
  assertSafeTree(rendered);
});

test("tool-result thunk keeps a stable root across JSON and Markdown", () => {
  const json = toolResultContentThunk('{"key": true}', false);
  assert.equal(json.sel, "div");
  runInitHook(json);
  assert.equal(byClass(json, "json-highlight").length, 1);

  const markdown = toolResultContentThunk("# Heading", false);
  runPrepatchHook(json, markdown);
  assert.equal(markdown.sel, "div");
  assert.equal(bySelector(markdown, "h1").length, 1);
  assert.equal(byClass(markdown, "json-highlight").length, 0);
});

test("fenced JSON shares one lexical-segment budget per Markdown block", () => {
  const payload = `[${Array.from({ length: 100 }, () => "0").join(",")}]`;
  const source = Array.from(
    { length: 50 },
    () => ["```json", payload, "```"].join("\n"),
  ).join("\n\n");
  const rendered = renderMarkdownContent("markdown", source);
  const code = bySelector(rendered, "code");

  assert.equal(code.length, 50);
  assert.equal(code.map(textContent).join(""), payload.repeat(50));
  assert.ok(jsonTokenCount(rendered) <= 4_096);
  assert.ok(byClass(rendered, "json-highlight").length < code.length);
  assertSafeTree(rendered);
});

test("character references decode in prose and links but not code or escapes", () => {
  const source = [
    "A &amp; B &#x1f9ea; &copy; &constructor; and `&amp;`.",
    "\\&amp; remains literal.",
    "",
    "[query](https://example.com/search?a=1&amp;b=2)",
    "",
    "0. zero",
    "1. one",
  ].join("\n");
  const vnode = renderMarkdownContent("markdown", source);
  assertRich(vnode);
  const renderedText = textContent(vnode);
  assert.match(renderedText, /A & B 🧪 © &constructor;/u);
  assert.equal((renderedText.match(/&amp;/gu) ?? []).length, 2);
  const anchors = bySelector(vnode, "a");
  assert.equal(anchors.length, 1);
  assert.equal(
    attr(anchors[0], "href"),
    "https://example.com/search?a=1&b=2",
  );
  const ordered = bySelector(vnode, "ol");
  assert.equal(ordered.length, 1);
  assert.equal(attr(ordered[0], "start"), 0);
  assertSafeTree(vnode);
});

test("raw HTML and images remain visible text and cannot create active elements", () => {
  const source = [
    "# Safe wrapper",
    "<script>alert(1)</script>",
    '<img src="https://tracker.invalid/pixel" onerror="alert(2)">',
    '<svg><script>alert(3)</script></svg>',
    "![remote image](https://tracker.invalid/image.svg)",
  ].join("\n\n");
  const vnode = renderMarkdownContent("markdown", source);
  assertRich(vnode);
  assert.equal(bySelector(vnode, "script").length, 0);
  assert.equal(bySelector(vnode, "img").length, 0);
  assert.equal(bySelector(vnode, "svg").length, 0);
  assert.match(textContent(vnode), /<script>alert\(1\)<\/script>/u);
  assert.match(textContent(vnode), /<img src=/u);
  assert.match(textContent(vnode), /!\[remote image\]/u);
  assertSafeTree(vnode);
});

test("only canonical credential-free HTTPS links become anchors", () => {
  const source = [
    "[safe](https://example.com/docs?q=one#part)",
    "[javascript](javascript:alert(1))",
    "[entity scheme](javascript&colon;alert(1))",
    "[data](data:text/html,boom)",
    "[file](file:///etc/passwd)",
    "[relative](/auth/session)",
    "[protocol relative](//example.com/path)",
    "[credentials](https://user:pass@example.com/path)",
    "[backslash](https:\\\\example.com/path)",
    "[encoded control](https://example.com/%0apath)",
    "[entity whitespace](https://example.com/&Tab;path)",
    "![never fetched](https://example.com/image.png)",
  ].join("\n\n");
  const vnode = renderMarkdownContent("markdown", source);
  const anchors = bySelector(vnode, "a");
  assert.equal(anchors.length, 1);
  const [anchor] = anchors;
  assert.ok(anchor);
  assert.deepEqual(attrs(anchor), {
    class: "markdown-link",
    href: "https://example.com/docs?q=one#part",
    referrerpolicy: "no-referrer",
    rel: "noopener noreferrer nofollow",
    target: "_blank",
  });
  assert.equal(bySelector(vnode, "img").length, 0);
  for (const label of [
    "javascript",
    "entity scheme",
    "data",
    "file",
    "relative",
    "protocol relative",
    "credentials",
    "backslash",
    "encoded control",
    "entity whitespace",
    "never fetched",
  ]) {
    assert.match(textContent(vnode), new RegExp(`\\[${label}`, "u"));
  }
  assertSafeTree(vnode);
});

test("malformed, truncated, and Unicode input never throws", () => {
  for (const source of [
    "```typescript\nconst value = 'unterminated fence';",
    "**unterminated emphasis",
    "[broken link](https://example.com",
    "| incomplete | table |\n| --- |",
    "emoji 🧪 and combining é with ~~formatting~~",
    "… [truncated]",
  ]) {
    const vnode = renderMarkdownContent("markdown", source);
    assert.equal(vnode.sel, "div");
    assert.equal(textContent(vnode).length > 0, true);
    assertSafeTree(vnode);
  }
});

test("source, node, and nesting budgets fail closed to complete literal text", () => {
  const oversized = `# heading\n${"x".repeat(64 * 1024)}`;
  assertLiteral(renderMarkdownContent("markdown", oversized), oversized);

  const tooManyNodes = Array.from(
    { length: 4_100 },
    (_, index) => `- item ${index}`,
  ).join("\n");
  assertLiteral(renderMarkdownContent("markdown", tooManyNodes), tooManyNodes);

  const tooDeep = `${"> ".repeat(40)}deep`;
  assertLiteral(renderMarkdownContent("markdown", tooDeep), tooDeep);
});

test("Snabbdom thunk reuses unchanged content and replaces a streamed tail", () => {
  const first = markdownContentThunk("text", "# first");
  assert.equal(first.sel, "div");
  runInitHook(first);

  const same = markdownContentThunk("text", "# first");
  runPrepatchHook(first, same);
  assert.equal(same.children, first.children);

  const jsonSource = ["```json", '{"key": true}', "```"].join("\n");
  const completeJson = markdownContentThunk("markdown", jsonSource, true);
  runInitHook(completeJson);
  assert.equal(byClass(completeJson, "json-highlight").length, 1);
  const lossyJson = markdownContentThunk("markdown", jsonSource, false);
  runPrepatchHook(completeJson, lossyJson);
  assert.notEqual(lossyJson.children, completeJson.children);
  assert.equal(byClass(lossyJson, "json-highlight").length, 0);

  const changed = markdownContentThunk("text", "# first\n\nnew tail");
  runPrepatchHook(same, changed);
  assert.notEqual(changed.children, same.children);
  assert.match(textContent(changed), /new tail/u);
});
