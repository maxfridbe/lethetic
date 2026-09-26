import { jsx as h } from "../lib/snabbdom/build/jsx.js";
import { thunk } from "../lib/snabbdom/build/thunk.js";
import type { VNode } from "../lib/snabbdom/build/vnode.js";

const MAX_JSON_SOURCE_BYTES = 64 * 1024;
const MAX_JSON_DEPTH = 64;
export const MAX_JSON_SEGMENTS = 4_096;
const UTF8_ENCODER = new TextEncoder();

type JsonClass =
  | "json-key"
  | "json-literal"
  | "json-number"
  | "json-punctuation"
  | "json-string";

export type JsonPreformattedKind = "approval" | "block";

export type JsonInspection =
  | { readonly status: "bounded" }
  | { readonly status: "invalid" }
  | { readonly status: "valid"; readonly segments: number };

export type JsonTokenization =
  | { readonly status: "bounded" }
  | { readonly status: "invalid" }
  | {
      readonly status: "valid";
      readonly segments: number;
      readonly children: Array<VNode | string>;
    };

class JsonSyntaxError extends Error {}
class JsonBoundError extends Error {}

class JsonScanner {
  readonly #source: string;
  readonly #segmentLimit: number;
  readonly #children: Array<VNode | string> | null;
  #index = 0;
  #segments = 0;

  constructor(source: string, segmentLimit: number, emitChildren: boolean) {
    this.#source = source;
    this.#segmentLimit = segmentLimit;
    this.#children = emitChildren ? [] : null;
  }

  scan(): JsonInspection | JsonTokenization {
    try {
      this.#whitespace();
      this.#value(0);
      this.#whitespace();
      if (this.#index !== this.#source.length) {
        throw new JsonSyntaxError("JSON has trailing content");
      }
      return this.#children === null
        ? { status: "valid", segments: this.#segments }
        : {
            status: "valid",
            segments: this.#segments,
            children: this.#children,
          };
    } catch (error) {
      if (error instanceof JsonBoundError) {
        return { status: "bounded" };
      }
      if (error instanceof JsonSyntaxError) {
        return { status: "invalid" };
      }
      throw error;
    }
  }

  #claim(): void {
    this.#segments += 1;
    if (this.#segments > this.#segmentLimit) {
      throw new JsonBoundError("JSON has too many lexical segments");
    }
  }

  #text(start: number, end: number): void {
    if (end <= start) {
      return;
    }
    this.#claim();
    this.#children?.push(this.#source.slice(start, end));
  }

  #span(start: number, end: number, className: JsonClass): void {
    this.#claim();
    if (this.#children !== null) {
      this.#children.push(
        <span attrs={{ class: className }}>{this.#source.slice(start, end)}</span>,
      );
    }
  }

  #whitespace(): void {
    const start = this.#index;
    while (this.#index < this.#source.length) {
      const code = this.#source.charCodeAt(this.#index);
      if (code !== 0x20 && code !== 0x09 && code !== 0x0a && code !== 0x0d) {
        break;
      }
      this.#index += 1;
    }
    this.#text(start, this.#index);
  }

  #value(depth: number): void {
    if (this.#index >= this.#source.length) {
      throw new JsonSyntaxError("JSON value is missing");
    }
    const code = this.#source.charCodeAt(this.#index);
    switch (code) {
      case 0x22:
        this.#string("json-string");
        return;
      case 0x5b:
        this.#array(depth + 1);
        return;
      case 0x7b:
        this.#object(depth + 1);
        return;
      case 0x74:
        this.#literal("true");
        return;
      case 0x66:
        this.#literal("false");
        return;
      case 0x6e:
        this.#literal("null");
        return;
      default:
        if (code === 0x2d || isDigit(code)) {
          this.#number();
          return;
        }
        throw new JsonSyntaxError("JSON value is invalid");
    }
  }

  #array(depth: number): void {
    this.#checkDepth(depth);
    this.#punctuation(0x5b);
    this.#whitespace();
    if (this.#peek(0x5d)) {
      this.#punctuation(0x5d);
      return;
    }
    while (true) {
      this.#value(depth);
      this.#whitespace();
      if (this.#peek(0x5d)) {
        this.#punctuation(0x5d);
        return;
      }
      this.#punctuation(0x2c);
      this.#whitespace();
    }
  }

  #object(depth: number): void {
    this.#checkDepth(depth);
    this.#punctuation(0x7b);
    this.#whitespace();
    if (this.#peek(0x7d)) {
      this.#punctuation(0x7d);
      return;
    }
    while (true) {
      if (!this.#peek(0x22)) {
        throw new JsonSyntaxError("JSON object key is not a string");
      }
      this.#string("json-key");
      this.#whitespace();
      this.#punctuation(0x3a);
      this.#whitespace();
      this.#value(depth);
      this.#whitespace();
      if (this.#peek(0x7d)) {
        this.#punctuation(0x7d);
        return;
      }
      this.#punctuation(0x2c);
      this.#whitespace();
    }
  }

  #string(className: "json-key" | "json-string"): void {
    const start = this.#index;
    this.#index += 1;
    while (this.#index < this.#source.length) {
      const code = this.#source.charCodeAt(this.#index);
      if (code === 0x22) {
        this.#index += 1;
        this.#span(start, this.#index, className);
        return;
      }
      if (code < 0x20) {
        throw new JsonSyntaxError("JSON string contains a control character");
      }
      if (code !== 0x5c) {
        this.#index += 1;
        continue;
      }
      this.#index += 1;
      if (this.#index >= this.#source.length) {
        throw new JsonSyntaxError("JSON string escape is incomplete");
      }
      const escaped = this.#source.charCodeAt(this.#index);
      if (escaped === 0x75) {
        for (let offset = 1; offset <= 4; offset += 1) {
          if (!isHexDigit(this.#source.charCodeAt(this.#index + offset))) {
            throw new JsonSyntaxError("JSON Unicode escape is invalid");
          }
        }
        this.#index += 5;
        continue;
      }
      if (
        escaped !== 0x22 &&
        escaped !== 0x5c &&
        escaped !== 0x2f &&
        escaped !== 0x62 &&
        escaped !== 0x66 &&
        escaped !== 0x6e &&
        escaped !== 0x72 &&
        escaped !== 0x74
      ) {
        throw new JsonSyntaxError("JSON string escape is invalid");
      }
      this.#index += 1;
    }
    throw new JsonSyntaxError("JSON string is unterminated");
  }

  #number(): void {
    const start = this.#index;
    if (this.#peek(0x2d)) {
      this.#index += 1;
    }
    if (this.#peek(0x30)) {
      this.#index += 1;
    } else {
      const first = this.#source.charCodeAt(this.#index);
      if (first < 0x31 || first > 0x39) {
        throw new JsonSyntaxError("JSON number has an invalid integer part");
      }
      this.#index += 1;
      while (isDigit(this.#source.charCodeAt(this.#index))) {
        this.#index += 1;
      }
    }
    if (this.#peek(0x2e)) {
      this.#index += 1;
      if (!isDigit(this.#source.charCodeAt(this.#index))) {
        throw new JsonSyntaxError("JSON number has an invalid fraction");
      }
      while (isDigit(this.#source.charCodeAt(this.#index))) {
        this.#index += 1;
      }
    }
    if (this.#peek(0x65) || this.#peek(0x45)) {
      this.#index += 1;
      if (this.#peek(0x2b) || this.#peek(0x2d)) {
        this.#index += 1;
      }
      if (!isDigit(this.#source.charCodeAt(this.#index))) {
        throw new JsonSyntaxError("JSON number has an invalid exponent");
      }
      while (isDigit(this.#source.charCodeAt(this.#index))) {
        this.#index += 1;
      }
    }
    this.#span(start, this.#index, "json-number");
  }

  #literal(expected: "false" | "null" | "true"): void {
    if (!this.#source.startsWith(expected, this.#index)) {
      throw new JsonSyntaxError("JSON literal is invalid");
    }
    const start = this.#index;
    this.#index += expected.length;
    this.#span(start, this.#index, "json-literal");
  }

  #punctuation(expected: number): void {
    if (!this.#peek(expected)) {
      throw new JsonSyntaxError("JSON punctuation is invalid");
    }
    const start = this.#index;
    this.#index += 1;
    this.#span(start, this.#index, "json-punctuation");
  }

  #peek(expected: number): boolean {
    return this.#source.charCodeAt(this.#index) === expected;
  }

  #checkDepth(depth: number): void {
    if (depth > MAX_JSON_DEPTH) {
      throw new JsonBoundError("JSON nesting is too deep");
    }
  }
}

function isDigit(code: number): boolean {
  return code >= 0x30 && code <= 0x39;
}

function isHexDigit(code: number): boolean {
  return (
    isDigit(code) ||
    (code >= 0x41 && code <= 0x46) ||
    (code >= 0x61 && code <= 0x66)
  );
}

function invalidSegmentLimit(maxSegments: number): boolean {
  return (
    !Number.isSafeInteger(maxSegments) ||
    maxSegments < 0 ||
    maxSegments > MAX_JSON_SEGMENTS
  );
}

function oversizedJsonSource(source: string): boolean {
  return UTF8_ENCODER.encode(source).byteLength > MAX_JSON_SOURCE_BYTES;
}

export function inspectJson(source: string): JsonInspection {
  if (oversizedJsonSource(source)) {
    return { status: "bounded" };
  }
  const result = new JsonScanner(source, MAX_JSON_SEGMENTS, false).scan();
  return result.status === "valid"
    ? { status: "valid", segments: result.segments }
    : result;
}

export function tokenizeJson(
  source: string,
  maxSegments = MAX_JSON_SEGMENTS,
): JsonTokenization {
  if (invalidSegmentLimit(maxSegments) || oversizedJsonSource(source)) {
    return { status: "bounded" };
  }
  const result = new JsonScanner(source, maxSegments, true).scan();
  if (result.status !== "valid") {
    return result;
  }
  if (!("children" in result)) {
    throw new Error("JSON token scanner did not emit children");
  }
  return result;
}

export function jsonTokenChildren(
  source: string,
  maxSegments = MAX_JSON_SEGMENTS,
): Array<VNode | string> | null {
  const result = tokenizeJson(source, maxSegments);
  return result.status === "valid" ? result.children : null;
}

export function renderJsonPreformatted(
  kind: JsonPreformattedKind,
  source: string,
  highlight: boolean,
  maxSegments = MAX_JSON_SEGMENTS,
): VNode {
  const children = highlight ? jsonTokenChildren(source, maxSegments) : null;
  const highlighted = children !== null;
  if (kind === "approval") {
    return (
      <pre
        attrs={{
          class: highlighted
            ? "approval-preview json-highlight"
            : "approval-preview",
          tabindex: "0",
        }}
      >
        {children ?? source}
      </pre>
    );
  }
  return (
    <pre
      attrs={{
        class: highlighted ? "block-content json-highlight" : "block-content",
      }}
    >
      {children ?? source}
    </pre>
  );
}

export function jsonPreformattedThunk(
  kind: JsonPreformattedKind,
  source: string,
  highlight: boolean,
  maxSegments = MAX_JSON_SEGMENTS,
): VNode {
  return thunk("pre", renderJsonPreformatted, [
    kind,
    source,
    highlight,
    maxSegments,
  ]);
}
