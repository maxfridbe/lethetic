import { jsx as h } from "../lib/snabbdom/build/jsx.js";
const MAX_HIGHLIGHT_BYTES = 64 * 1024;
const MAX_HIGHLIGHT_TOKENS = 12_000;
function words(list) {
    return new Set(list.split(/\s+/u).filter((word) => word.length > 0));
}
const C_LIKE_QUOTES = ["\"", "'", "`"];
const RULES = {
    rust: {
        keywords: words("as async await break const continue crate dyn else enum extern false fn for if impl in let loop match mod move mut pub ref return self Self static struct super trait true type unsafe use where while i8 i16 i32 i64 i128 isize u8 u16 u32 u64 u128 usize f32 f64 bool char str String Vec Option Result Some None Ok Err Box"),
        lineComments: ["//"],
        blockComment: ["/*", "*/"],
        quotes: ["\""],
        capitalizedTypes: true,
        keyLines: false,
    },
    typescript: {
        keywords: words("abstract any as async await boolean break case catch class const constructor continue debugger declare default delete do else enum export extends false finally for from function get if implements import in instanceof interface is keyof let module namespace never new null number object of private protected public readonly return set static string super switch symbol this throw true try type typeof undefined unknown var void while yield"),
        lineComments: ["//"],
        blockComment: ["/*", "*/"],
        quotes: C_LIKE_QUOTES,
        capitalizedTypes: true,
        keyLines: false,
    },
    python: {
        keywords: words("and as assert async await break class continue def del elif else except False finally for from global if import in is lambda None nonlocal not or pass raise return self True try while with yield int str float bool list dict set tuple"),
        lineComments: ["#"],
        blockComment: null,
        quotes: ["\"", "'"],
        capitalizedTypes: true,
        keyLines: false,
    },
    shell: {
        keywords: words("if then else elif fi for while until do done case esac in function return local export readonly set unset echo exit cd source sudo"),
        lineComments: ["#"],
        blockComment: null,
        quotes: ["\"", "'"],
        capitalizedTypes: false,
        keyLines: false,
    },
    c: {
        keywords: words("auto bool break case char class const continue default delete do double else enum extern false float for goto if inline int long namespace new nullptr private protected public register return short signed sizeof static struct switch template this true typedef typename union unsigned using var virtual void volatile while async await base foreach in interface internal is override readonly string object sealed get set func go chan defer map package range select type import fmt"),
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
const ALIASES = {
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
};
export function highlightLanguage(language) {
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
export function highlightCode(source, language) {
    const rules = highlightLanguage(language);
    if (rules === null || source.length > MAX_HIGHLIGHT_BYTES) {
        return null;
    }
    const children = [];
    let plain = "";
    let tokens = 0;
    const flush = () => {
        if (plain.length > 0) {
            children.push(plain);
            plain = "";
        }
    };
    const push = (kind, text) => {
        flush();
        tokens += 1;
        children.push(h("span", { attrs: { class: `code-${kind}` } }, text));
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
            if (rules.keywords.has(word)) {
                push("keyword", word);
            }
            else if (next !== null) {
                push("function", word);
            }
            else if (rules.capitalizedTypes && /^[A-Z][A-Za-z0-9_]*[a-z]/u.test(word)) {
                push("type", word);
            }
            else {
                plain += word;
            }
            index = cursor;
            lineStart = false;
            continue;
        }
        plain += character;
        if (character === "\n") {
            lineStart = true;
        }
        else if (character !== " " && character !== "\t") {
            lineStart = false;
        }
        index += 1;
    }
    flush();
    return children;
}
