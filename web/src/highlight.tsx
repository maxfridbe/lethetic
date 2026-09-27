import { jsx as h } from "../lib/snabbdom/build/jsx.js";
import type { VNode } from "../lib/snabbdom/build/vnode.js";

/**
 * Small, dependency-free code highlighter for fenced blocks. Tokens get
 * `code-*` classes coloured from the active theme's CSS variables, matching
 * the categories the terminal highlighter uses (src/markdown.rs).
 */

type TokenKind =
  | "added"
  | "removed"
  | "comment"
  | "keyword"
  | "string"
  | "number"
  | "function"
  | "type"
  | "key";

interface LanguageRules {
  readonly keywords: ReadonlySet<string>;
  readonly lineComments: readonly string[];
  readonly blockComment: readonly [string, string] | null;
  readonly quotes: readonly string[];
  /** Words starting with an upper-case letter are types. */
  readonly capitalizedTypes: boolean;
  /** `name:` / `name =` at line start is a key (YAML, TOML). */
  readonly keyLines: boolean;
  /** Match keywords ignoring case (SQL). */
  readonly caseInsensitive?: boolean;
  /** Upper-case word at line start is an instruction (Dockerfile). */
  readonly lineInstructions?: boolean;
}

const MAX_HIGHLIGHT_BYTES = 64 * 1024;
const MAX_HIGHLIGHT_TOKENS = 12_000;

function words(list: string): ReadonlySet<string> {
  return new Set(list.split(/\s+/u).filter((word) => word.length > 0));
}

const C_LIKE_QUOTES = ["\"", "'", "`"];

const RULES: Readonly<Record<string, LanguageRules>> = {
  sql: {
    keywords: words(
      "select from where and or not insert into values update set delete create table alter drop index view join left right inner outer full on group by order having limit offset as distinct union all case when then else end null is in exists like between primary key foreign references default begin commit rollback returning with integer int text varchar boolean serial timestamp date",
    ),
    lineComments: ["--"],
    blockComment: ["/*", "*/"],
    quotes: ["'", "\""],
    capitalizedTypes: false,
    keyLines: false,
    caseInsensitive: true,
  },
  css: {
    keywords: words("important inherit initial unset none auto"),
    lineComments: [],
    blockComment: ["/*", "*/"],
    quotes: ["\"", "'"],
    capitalizedTypes: false,
    keyLines: true,
  },
  ruby: {
    keywords: words(
      "alias and begin break case class def defined? do else elsif end ensure false for if in module next nil not or redo rescue retry return self super then true undef unless until when while yield require attr_accessor puts",
    ),
    lineComments: ["#"],
    blockComment: null,
    quotes: ["\"", "'"],
    capitalizedTypes: true,
    keyLines: false,
  },
  lua: {
    keywords: words(
      "and break do else elseif end false for function goto if in local nil not or repeat return then true until while require",
    ),
    lineComments: ["--"],
    blockComment: null,
    quotes: ["\"", "'"],
    capitalizedTypes: false,
    keyLines: false,
  },
  php: {
    keywords: words(
      "abstract and array as break case catch class clone const continue declare default do echo else elseif empty extends final finally fn for foreach function global if implements include instanceof interface isset list match namespace new null or print private protected public readonly require return static switch throw trait true false try unset use var while yield",
    ),
    lineComments: ["//", "#"],
    blockComment: ["/*", "*/"],
    quotes: ["\"", "'"],
    capitalizedTypes: true,
    keyLines: false,
  },
  dockerfile: {
    keywords: words(""),
    lineComments: ["#"],
    blockComment: null,
    quotes: ["\"", "'"],
    capitalizedTypes: false,
    keyLines: false,
    lineInstructions: true,
  },
  makefile: {
    keywords: words("ifeq ifneq ifdef ifndef else endif include define endef export"),
    lineComments: ["#"],
    blockComment: null,
    quotes: ["\"", "'"],
    capitalizedTypes: false,
    keyLines: true,
  },
  rust: {
    keywords: words(
      "as async await break const continue crate dyn else enum extern false fn for if impl in let loop match mod move mut pub ref return self Self static struct super trait true type unsafe use where while i8 i16 i32 i64 i128 isize u8 u16 u32 u64 u128 usize f32 f64 bool char str String Vec Option Result Some None Ok Err Box",
    ),
    lineComments: ["//"],
    blockComment: ["/*", "*/"],
    quotes: ["\""],
    capitalizedTypes: true,
    keyLines: false,
  },
  typescript: {
    keywords: words(
      "abstract any as async await boolean break case catch class const constructor continue debugger declare default delete do else enum export extends false finally for from function get if implements import in instanceof interface is keyof let module namespace never new null number object of private protected public readonly return set static string super switch symbol this throw true try type typeof undefined unknown var void while yield",
    ),
    lineComments: ["//"],
    blockComment: ["/*", "*/"],
    quotes: C_LIKE_QUOTES,
    capitalizedTypes: true,
    keyLines: false,
  },
  python: {
    keywords: words(
      "and as assert async await break class continue def del elif else except False finally for from global if import in is lambda None nonlocal not or pass raise return self True try while with yield int str float bool list dict set tuple",
    ),
    lineComments: ["#"],
    blockComment: null,
    quotes: ["\"", "'"],
    capitalizedTypes: true,
    keyLines: false,
  },
  shell: {
    keywords: words(
      "if then else elif fi for while until do done case esac in function return local export readonly set unset echo exit cd source sudo",
    ),
    lineComments: ["#"],
    blockComment: null,
    quotes: ["\"", "'"],
    capitalizedTypes: false,
    keyLines: false,
  },
  c: {
    keywords: words(
      "auto bool break case char class const continue default delete do double else enum extern false float for goto if inline int long namespace new nullptr private protected public register return short signed sizeof static struct switch template this true typedef typename union unsigned using var virtual void volatile while async await base foreach in interface internal is override readonly string object sealed get set func go chan defer map package range select type import fmt",
    ),
    lineComments: ["//"],
    blockComment: ["/*", "*/"],
    quotes: ["\"", "'"],
    capitalizedTypes: true,
    keyLines: false,
  },
  json: {
    keywords: words("true false null"),
    lineComments: [],
    blockComment: null,
    quotes: ["\""],
    capitalizedTypes: false,
    keyLines: false,
  },
  config: {
    keywords: words("true false null yes no on off"),
    lineComments: ["#"],
    blockComment: null,
    quotes: ["\"", "'"],
    capitalizedTypes: false,
    keyLines: true,
  },
};

const ALIASES: Readonly<Record<string, string>> = {
  rs: "rust",
  rust: "rust",
  ts: "typescript",
  tsx: "typescript",
  typescript: "typescript",
  js: "typescript",
  jsx: "typescript",
  javascript: "typescript",
  mjs: "typescript",
  py: "python",
  python: "python",
  sh: "shell",
  bash: "shell",
  zsh: "shell",
  shell: "shell",
  console: "shell",
  c: "c",
  h: "c",
  cpp: "c",
  "c++": "c",
  cc: "c",
  hpp: "c",
  cs: "c",
  csharp: "c",
  "c#": "c",
  java: "c",
  go: "c",
  kotlin: "c",
  swift: "c",
  json: "json",
  jsonc: "json",
  yaml: "config",
  yml: "config",
  toml: "config",
  ini: "config",
  cfg: "config",
  conf: "config",
  env: "config",
  dotenv: "config",
  properties: "config",
  editorconfig: "config",
  gitconfig: "config",
  sql: "sql",
  css: "css",
  scss: "css",
  less: "css",
  rb: "ruby",
  ruby: "ruby",
  lua: "lua",
  php: "php",
  dockerfile: "dockerfile",
  containerfile: "dockerfile",
  docker: "dockerfile",
  makefile: "makefile",
  make: "makefile",
  mk: "makefile",
  scala: "c",
  dart: "c",
  zig: "c",
};

const MARKUP_LANGUAGES = new Set(["html", "htm", "xml", "svg", "xhtml", "vue", "jsx-html"]);
const DIFF_LANGUAGES = new Set(["diff", "patch"]);

/** HTML/XML: comments, tag names, attribute names and quoted values. */
function highlightMarkup(source: string): Array<VNode | string> {
  const children: Array<VNode | string> = [];
  const pattern = /(<!--[\s\S]*?(?:-->|$))|(<\/?)([A-Za-z][\w:.-]*)|([A-Za-z_:][\w:.-]*)(?==)|("[^"]*"|'[^']*')/gu;
  let last = 0;
  for (const match of source.matchAll(pattern)) {
    const index = match.index;
    if (index > last) {
      children.push(source.slice(last, index));
    }
    if (match[1] !== undefined) {
      children.push(<span attrs={{ class: "code-comment" }}>{match[1]}</span>);
    } else if (match[3] !== undefined) {
      children.push(match[2] ?? "");
      children.push(<span attrs={{ class: "code-key" }}>{match[3]}</span>);
    } else if (match[4] !== undefined) {
      children.push(<span attrs={{ class: "code-type" }}>{match[4]}</span>);
    } else if (match[5] !== undefined) {
      children.push(<span attrs={{ class: "code-string" }}>{match[5]}</span>);
    }
    last = index + match[0].length;
  }
  if (last < source.length) {
    children.push(source.slice(last));
  }
  return children;
}

/** Unified diffs: added, removed, hunk and file header lines. */
function highlightDiff(source: string): Array<VNode | string> {
  const children: Array<VNode | string> = [];
  const lines = source.split("\n");
  lines.forEach((line, index) => {
    const suffix = index < lines.length - 1 ? "\n" : "";
    const kind: TokenKind | null =
      line.startsWith("+++") || line.startsWith("---")
        ? "key"
        : line.startsWith("@@")
          ? "type"
          : line.startsWith("+")
            ? "added"
            : line.startsWith("-")
              ? "removed"
              : null;
    if (kind === null) {
      children.push(line + suffix);
    } else {
      children.push(<span attrs={{ class: `code-${kind}` }}>{line}</span>);
      if (suffix.length > 0) {
        children.push(suffix);
      }
    }
  });
  return children;
}

export function highlightLanguage(language: string | null): LanguageRules | null {
  if (language === null) {
    return null;
  }
  const name = ALIASES[language.toLowerCase()];
  return name === undefined ? null : (RULES[name] ?? null);
}

const IDENTIFIER_START = /[A-Za-z_$]/u;
const IDENTIFIER_PART = /[A-Za-z0-9_$]/u;
const DIGIT = /[0-9]/u;

/**
 * Highlight `source` for `language`, or return null when the language is
 * unknown or the source is too large (callers then show plain text).
 */
export function highlightCode(
  source: string,
  language: string | null,
): Array<VNode | string> | null {
  if (source.length > MAX_HIGHLIGHT_BYTES || language === null) {
    return null;
  }
  const lowered = language.toLowerCase();
  if (MARKUP_LANGUAGES.has(lowered)) {
    return highlightMarkup(source);
  }
  if (DIFF_LANGUAGES.has(lowered)) {
    return highlightDiff(source);
  }
  const rules = highlightLanguage(language);
  if (rules === null) {
    return null;
  }
  const children: Array<VNode | string> = [];
  let plain = "";
  let tokens = 0;
  const flush = (): void => {
    if (plain.length > 0) {
      children.push(plain);
      plain = "";
    }
  };
  const push = (kind: TokenKind, text: string): void => {
    flush();
    tokens += 1;
    children.push(<span attrs={{ class: `code-${kind}` }}>{text}</span>);
  };

  let index = 0;
  let lineStart = true;
  while (index < source.length) {
    if (tokens > MAX_HIGHLIGHT_TOKENS) {
      plain += source.slice(index);
      break;
    }
    const rest = source.slice(index);
    const character = source[index] ?? "";

    if (rules.lineInstructions === true && lineStart) {
      const instruction = /^([ \t]*)([A-Z][A-Z0-9_]+)(?=\s|$)/u.exec(rest);
      if (instruction !== null && instruction[1] !== undefined && instruction[2] !== undefined) {
        plain += instruction[1];
        push("keyword", instruction[2]);
        index += instruction[0].length;
        lineStart = false;
        continue;
      }
    }
    if (rules.keyLines && lineStart) {
      const key = /^([ \t-]*)([A-Za-z0-9_.\-"']+)(\s*[:=])/u.exec(rest);
      if (key !== null && key[1] !== undefined && key[2] !== undefined && key[3] !== undefined) {
        plain += key[1];
        push("key", key[2]);
        plain += key[3];
        index += key[0].length;
        lineStart = false;
        continue;
      }
    }

    const lineComment = rules.lineComments.find((marker) => rest.startsWith(marker));
    if (lineComment !== undefined) {
      const end = source.indexOf("\n", index);
      const stop = end < 0 ? source.length : end;
      push("comment", source.slice(index, stop));
      index = stop;
      continue;
    }
    if (rules.blockComment !== null && rest.startsWith(rules.blockComment[0])) {
      const end = source.indexOf(rules.blockComment[1], index + rules.blockComment[0].length);
      const stop = end < 0 ? source.length : end + rules.blockComment[1].length;
      push("comment", source.slice(index, stop));
      index = stop;
      continue;
    }
    if (rules.quotes.includes(character)) {
      let cursor = index + 1;
      while (cursor < source.length) {
        const current = source[cursor];
        if (current === "\\") {
          cursor += 2;
          continue;
        }
        if (current === character || (current === "\n" && character !== "`")) {
          cursor += current === character ? 1 : 0;
          break;
        }
        cursor += 1;
      }
      push("string", source.slice(index, Math.min(cursor, source.length)));
      index = Math.min(cursor, source.length);
      lineStart = false;
      continue;
    }
    if (DIGIT.test(character) && !IDENTIFIER_PART.test(source[index - 1] ?? " ")) {
      const number = /^(?:0x[0-9a-fA-F_]+|[0-9][0-9_]*(?:\.[0-9_]+)?(?:[eE][+-]?[0-9]+)?)[A-Za-z0-9]*/u.exec(rest);
      const text = number?.[0] ?? character;
      push("number", text);
      index += text.length;
      lineStart = false;
      continue;
    }
    if (IDENTIFIER_START.test(character)) {
      let cursor = index + 1;
      while (cursor < source.length && IDENTIFIER_PART.test(source[cursor] ?? "")) {
        cursor += 1;
      }
      const word = source.slice(index, cursor);
      const next = source.slice(cursor).match(/^\s*(!?\()/u);
      if (rules.keywords.has(rules.caseInsensitive === true ? word.toLowerCase() : word)) {
        push("keyword", word);
      } else if (next !== null) {
        push("function", word);
      } else if (rules.capitalizedTypes && /^[A-Z][A-Za-z0-9_]*[a-z]/u.test(word)) {
        push("type", word);
      } else {
        plain += word;
      }
      index = cursor;
      lineStart = false;
      continue;
    }
    plain += character;
    if (character === "\n") {
      lineStart = true;
    } else if (character !== " " && character !== "\t") {
      lineStart = false;
    }
    index += 1;
  }
  flush();
  return children;
}
