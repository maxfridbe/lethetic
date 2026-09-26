import { jsx as h } from "../lib/snabbdom/build/jsx.js";
import { thunk } from "../lib/snabbdom/build/thunk.js";
import { Lexer, } from "../lib/marked/marked.esm.js";
import { highlightCode } from "./highlight.js";
import { MAX_JSON_SEGMENTS, inspectJson, jsonTokenChildren, } from "./json.js";
const MAX_MARKDOWN_SOURCE_BYTES = 64 * 1024;
const MAX_MARKDOWN_NODES = 4_096;
const MAX_MARKDOWN_DEPTH = 32;
const MAX_LINK_LENGTH = 2_048;
const UTF8_ENCODER = new TextEncoder();
const MARKED_OPTIONS = Object.freeze({
    async: false,
    breaks: false,
    gfm: true,
    pedantic: false,
    silent: true,
});
const NAMED_CHARACTER_REFERENCES = Object.freeze({
    NewLine: "\n",
    Tab: "\t",
    amp: "&",
    apos: "'",
    asymp: "≈",
    bull: "•",
    cent: "¢",
    colon: ":",
    copy: "©",
    deg: "°",
    divide: "÷",
    darr: "↓",
    emsp: " ",
    ensp: " ",
    euro: "€",
    ge: "≥",
    gt: ">",
    harr: "↔",
    hellip: "…",
    laquo: "«",
    larr: "←",
    ldquo: "“",
    le: "≤",
    lsquo: "‘",
    lt: "<",
    mdash: "—",
    micro: "µ",
    middot: "·",
    nbsp: " ",
    ndash: "–",
    ne: "≠",
    para: "¶",
    plusmn: "±",
    pound: "£",
    quot: '"',
    raquo: "»",
    rarr: "→",
    rdquo: "”",
    reg: "®",
    rsquo: "’",
    sect: "§",
    thinsp: " ",
    times: "×",
    trade: "™",
    uarr: "↑",
    yen: "¥",
    zwj: "‍",
    zwnj: "‌",
});
const CHARACTER_REFERENCE = /&(?:#([0-9]{1,7})|#[xX]([0-9A-Fa-f]{1,6})|([A-Za-z][A-Za-z0-9]{1,31}));/gu;
/** Assistant text, explicit markdown, and thinking (where reasoning models put
 * most formatted output) render as markdown. */
export function isMarkdownBlockKind(value) {
    return value === "markdown" || value === "text" || value === "thought";
}
class MarkdownComplexityError extends Error {
}
class MarkdownNormalizer {
    #nodes = 0;
    #significant = false;
    normalize(tokens) {
        return {
            blocks: this.#blocks(tokens, 0, false),
            significant: this.#significant,
        };
    }
    #claim(depth) {
        if (depth > MAX_MARKDOWN_DEPTH) {
            throw new MarkdownComplexityError("Markdown nesting is too deep");
        }
        this.#nodes += 1;
        if (this.#nodes > MAX_MARKDOWN_NODES) {
            throw new MarkdownComplexityError("Markdown contains too many nodes");
        }
    }
    #markSignificant() {
        this.#significant = true;
    }
    #blocks(tokens, depth, suppressCheckbox) {
        if (depth > MAX_MARKDOWN_DEPTH) {
            throw new MarkdownComplexityError("Markdown nesting is too deep");
        }
        const blocks = [];
        for (const token of tokens) {
            switch (token.type) {
                case "space":
                case "def":
                    break;
                case "heading": {
                    const heading = token;
                    this.#markSignificant();
                    this.#claim(depth);
                    blocks.push({
                        kind: "heading",
                        depth: headingDepth(heading.depth),
                        children: this.#inlines(heading.tokens, depth + 1, suppressCheckbox),
                    });
                    break;
                }
                case "paragraph": {
                    const paragraph = token;
                    this.#claim(depth);
                    blocks.push({
                        kind: "paragraph",
                        children: this.#inlines(paragraph.tokens, depth + 1, suppressCheckbox),
                    });
                    break;
                }
                case "text": {
                    const text = token;
                    this.#claim(depth);
                    blocks.push({
                        kind: "paragraph",
                        children: text.tokens === undefined
                            ? this.#textOnly(text.text, depth + 1)
                            : this.#inlines(text.tokens, depth + 1, suppressCheckbox),
                    });
                    break;
                }
                case "blockquote": {
                    const quote = token;
                    this.#markSignificant();
                    this.#claim(depth);
                    blocks.push({
                        kind: "blockquote",
                        children: this.#blocks(quote.tokens, depth + 1, suppressCheckbox),
                    });
                    break;
                }
                case "list": {
                    const list = token;
                    this.#markSignificant();
                    this.#claim(depth);
                    const items = list.items.map((item) => {
                        this.#claim(depth + 1);
                        return {
                            checked: item.task ? item.checked === true : null,
                            children: this.#blocks(item.tokens, depth + 2, item.task),
                        };
                    });
                    blocks.push({
                        kind: "list",
                        ordered: list.ordered,
                        start: typeof list.start === "number" &&
                            Number.isSafeInteger(list.start) &&
                            list.start >= 0
                            ? list.start
                            : 1,
                        items,
                    });
                    break;
                }
                case "code": {
                    const code = token;
                    this.#markSignificant();
                    this.#claim(depth);
                    blocks.push({
                        kind: "code",
                        language: codeLanguage(code.lang),
                        text: code.text,
                    });
                    break;
                }
                case "table": {
                    const table = token;
                    this.#markSignificant();
                    this.#claim(depth);
                    blocks.push({
                        kind: "table",
                        header: table.header.map((cell) => this.#tableCell(cell, depth + 1)),
                        rows: table.rows.map((row) => row.map((cell) => this.#tableCell(cell, depth + 1))),
                    });
                    break;
                }
                case "hr":
                    this.#markSignificant();
                    this.#claim(depth);
                    blocks.push({ kind: "rule" });
                    break;
                case "html":
                    this.#claim(depth);
                    blocks.push({ kind: "literal", text: token.raw });
                    break;
                case "strong":
                case "em":
                case "del":
                case "codespan":
                case "br":
                case "escape":
                case "link":
                case "image":
                    this.#claim(depth);
                    blocks.push({
                        kind: "paragraph",
                        children: this.#inlines([token], depth + 1, suppressCheckbox),
                    });
                    break;
                case "checkbox":
                    if (!suppressCheckbox) {
                        this.#claim(depth);
                        blocks.push({
                            kind: "paragraph",
                            children: this.#inlines([token], depth + 1, false),
                        });
                    }
                    break;
                default:
                    this.#claim(depth);
                    blocks.push({ kind: "literal", text: token.raw });
                    break;
            }
        }
        return blocks;
    }
    #tableCell(cell, depth) {
        this.#claim(depth);
        return {
            alignment: tableAlignment(cell.align),
            children: this.#inlines(cell.tokens, depth + 1, false),
        };
    }
    #textOnly(text, depth) {
        const nodes = [];
        this.#pushMarkdownText(nodes, text, depth);
        return nodes;
    }
    #inlines(tokens, depth, suppressCheckbox) {
        if (depth > MAX_MARKDOWN_DEPTH) {
            throw new MarkdownComplexityError("Markdown nesting is too deep");
        }
        const nodes = [];
        for (const token of tokens) {
            switch (token.type) {
                case "text": {
                    const text = token;
                    if (text.tokens === undefined) {
                        this.#pushMarkdownText(nodes, text.text, depth);
                    }
                    else {
                        this.#pushAll(nodes, this.#inlines(text.tokens, depth + 1, suppressCheckbox));
                    }
                    break;
                }
                case "escape":
                    this.#markSignificant();
                    this.#pushText(nodes, token.text, depth);
                    break;
                case "strong": {
                    const strong = token;
                    this.#markSignificant();
                    this.#claim(depth);
                    nodes.push({
                        kind: "strong",
                        children: this.#inlines(strong.tokens, depth + 1, suppressCheckbox),
                    });
                    break;
                }
                case "em": {
                    const emphasis = token;
                    this.#markSignificant();
                    this.#claim(depth);
                    nodes.push({
                        kind: "emphasis",
                        children: this.#inlines(emphasis.tokens, depth + 1, suppressCheckbox),
                    });
                    break;
                }
                case "del": {
                    const deletion = token;
                    this.#markSignificant();
                    this.#claim(depth);
                    nodes.push({
                        kind: "delete",
                        children: this.#inlines(deletion.tokens, depth + 1, suppressCheckbox),
                    });
                    break;
                }
                case "codespan":
                    this.#markSignificant();
                    this.#claim(depth);
                    nodes.push({
                        kind: "code",
                        text: token.text,
                    });
                    break;
                case "br":
                    this.#markSignificant();
                    this.#claim(depth);
                    nodes.push({ kind: "break" });
                    break;
                case "link": {
                    const link = token;
                    this.#markSignificant();
                    const href = safeHttpsUrl(decodeCharacterReferences(link.href));
                    this.#claim(depth);
                    if (href === null) {
                        nodes.push({
                            kind: "literal",
                            literalKind: "blocked-link",
                            text: link.raw,
                        });
                    }
                    else {
                        nodes.push({
                            kind: "link",
                            href,
                            children: this.#inlines(link.tokens, depth + 1, suppressCheckbox),
                        });
                    }
                    break;
                }
                case "image":
                    this.#markSignificant();
                    this.#claim(depth);
                    nodes.push({
                        kind: "literal",
                        literalKind: "image",
                        text: token.raw,
                    });
                    break;
                case "html":
                    this.#claim(depth);
                    nodes.push({
                        kind: "literal",
                        literalKind: "html",
                        text: token.raw,
                    });
                    break;
                case "checkbox":
                    if (suppressCheckbox) {
                        break;
                    }
                    this.#markSignificant();
                    this.#pushText(nodes, token.checked ? "☑ " : "☐ ", depth);
                    break;
                default:
                    this.#claim(depth);
                    nodes.push({
                        kind: "literal",
                        literalKind: "unknown",
                        text: token.raw,
                    });
                    break;
            }
        }
        return nodes;
    }
    #pushAll(target, source) {
        for (const node of source) {
            if (node.kind === "text") {
                const previous = target.at(-1);
                if (previous?.kind === "text") {
                    previous.text += node.text;
                    continue;
                }
            }
            target.push(node);
        }
    }
    #pushMarkdownText(target, text, depth) {
        const decoded = decodeCharacterReferences(text);
        if (decoded !== text) {
            this.#markSignificant();
        }
        this.#pushText(target, decoded, depth);
    }
    #pushText(target, text, depth) {
        if (text.length === 0) {
            return;
        }
        const previous = target.at(-1);
        if (previous?.kind === "text") {
            previous.text += text;
            return;
        }
        this.#claim(depth);
        target.push({ kind: "text", text });
    }
}
function headingDepth(value) {
    switch (value) {
        case 1:
        case 2:
        case 3:
        case 4:
        case 5:
        case 6:
            return value;
        default:
            return 6;
    }
}
function tableAlignment(value) {
    switch (value) {
        case "center":
        case "left":
        case "right":
            return value;
        case null:
            return "unset";
    }
}
function remapC1Control(value) {
    switch (value) {
        case 0x80:
            return 0x20ac;
        case 0x82:
            return 0x201a;
        case 0x83:
            return 0x0192;
        case 0x84:
            return 0x201e;
        case 0x85:
            return 0x2026;
        case 0x86:
            return 0x2020;
        case 0x87:
            return 0x2021;
        case 0x88:
            return 0x02c6;
        case 0x89:
            return 0x2030;
        case 0x8a:
            return 0x0160;
        case 0x8b:
            return 0x2039;
        case 0x8c:
            return 0x0152;
        case 0x8e:
            return 0x017d;
        case 0x91:
            return 0x2018;
        case 0x92:
            return 0x2019;
        case 0x93:
            return 0x201c;
        case 0x94:
            return 0x201d;
        case 0x95:
            return 0x2022;
        case 0x96:
            return 0x2013;
        case 0x97:
            return 0x2014;
        case 0x98:
            return 0x02dc;
        case 0x99:
            return 0x2122;
        case 0x9a:
            return 0x0161;
        case 0x9b:
            return 0x203a;
        case 0x9c:
            return 0x0153;
        case 0x9e:
            return 0x017e;
        case 0x9f:
            return 0x0178;
        default:
            return value;
    }
}
function decodeNumericCharacterReference(value) {
    const remapped = remapC1Control(value);
    if (remapped === 0 ||
        remapped > 0x10ffff ||
        (remapped >= 0xd800 && remapped <= 0xdfff)) {
        return "�";
    }
    return String.fromCodePoint(remapped);
}
function decodeCharacterReferences(value) {
    return value.replace(CHARACTER_REFERENCE, (reference, decimal, hexadecimal, name) => {
        if (name !== undefined) {
            return Object.hasOwn(NAMED_CHARACTER_REFERENCES, name)
                ? NAMED_CHARACTER_REFERENCES[name]
                : reference;
        }
        const digits = decimal ?? hexadecimal;
        if (digits === undefined) {
            return reference;
        }
        const codePoint = Number.parseInt(digits, decimal === undefined ? 16 : 10);
        return decodeNumericCharacterReference(codePoint);
    });
}
function codeLanguage(value) {
    const language = value?.trim().split(/\s+/u, 1)[0] ?? "";
    return language.length > 0 &&
        language.length <= 40 &&
        /^[A-Za-z0-9_+.#-]+$/u.test(language)
        ? language
        : null;
}
function hasUnsafeUrlCharacter(value) {
    for (const character of value) {
        const point = character.codePointAt(0);
        if (point === undefined ||
            point <= 0x20 ||
            point === 0x5c ||
            point === 0x7f ||
            character.trim().length === 0) {
            return true;
        }
    }
    return false;
}
function safeHttpsUrl(value) {
    if (value.length === 0 ||
        value.length > MAX_LINK_LENGTH ||
        !value.startsWith("https://") ||
        hasUnsafeUrlCharacter(value) ||
        /%(?:0[0-9a-f]|1[0-9a-f]|20|7f)/iu.test(value)) {
        return null;
    }
    try {
        const parsed = new URL(value);
        if (parsed.protocol !== "https:" ||
            parsed.hostname.length === 0 ||
            parsed.username.length > 0 ||
            parsed.password.length > 0 ||
            parsed.href.length > MAX_LINK_LENGTH) {
            return null;
        }
        return parsed.href;
    }
    catch {
        return null;
    }
}
function renderInlines(nodes) {
    return nodes.map((node) => {
        switch (node.kind) {
            case "text":
                return node.text;
            case "break":
                return h("br", null);
            case "code":
                return h("code", { attrs: { class: "markdown-inline-code" } }, node.text);
            case "strong":
                return h("strong", null, renderInlines(node.children));
            case "emphasis":
                return h("em", null, renderInlines(node.children));
            case "delete":
                return h("del", null, renderInlines(node.children));
            case "link":
                return (h("a", { attrs: {
                        class: "markdown-link",
                        href: node.href,
                        referrerpolicy: "no-referrer",
                        rel: "noopener noreferrer nofollow",
                        target: "_blank",
                    } }, renderInlines(node.children)));
            case "literal":
                return (h("span", { attrs: { class: `markdown-literal markdown-literal-${node.literalKind}` } }, node.text));
        }
    });
}
function renderHeading(node) {
    const children = renderInlines(node.children);
    switch (node.depth) {
        case 1:
            return h("h1", null, children);
        case 2:
            return h("h2", null, children);
        case 3:
            return h("h3", null, children);
        case 4:
            return h("h4", null, children);
        case 5:
            return h("h5", null, children);
        case 6:
            return h("h6", null, children);
    }
}
function alignmentClass(alignment) {
    switch (alignment) {
        case "center":
            return "markdown-align-center";
        case "left":
            return "markdown-align-left";
        case "right":
            return "markdown-align-right";
        case "unset":
            return "markdown-align-unset";
    }
}
function renderListItem(item, budget) {
    if (item.checked === null) {
        return h("li", null, renderBlocks(item.children, budget));
    }
    return (h("li", { attrs: { class: "markdown-task-item" } },
        h("span", { attrs: { class: "markdown-task-marker" } }, item.checked ? "☑" : "☐"),
        h("div", { attrs: { class: "markdown-task-body" } }, renderBlocks(item.children, budget))));
}
function renderBlocks(nodes, budget) {
    return nodes.map((node) => {
        switch (node.kind) {
            case "paragraph":
                return h("p", null, renderInlines(node.children));
            case "heading":
                return renderHeading(node);
            case "blockquote":
                return h("blockquote", null, renderBlocks(node.children, budget));
            case "list": {
                const items = node.items.map((item) => renderListItem(item, budget));
                if (!node.ordered) {
                    return h("ul", null, items);
                }
                return node.start === 1 ? (h("ol", null, items)) : (h("ol", { attrs: { start: node.start } }, items));
            }
            case "code": {
                const jsonChildren = budget.highlightJson && node.language?.toLowerCase() === "json"
                    ? jsonTokenChildren(node.text, budget.jsonSegmentsRemaining)
                    : null;
                if (jsonChildren !== null) {
                    budget.jsonSegmentsRemaining -= jsonChildren.length;
                }
                const codeChildren = jsonChildren === null ? highlightCode(node.text, node.language) : null;
                return (h("div", { attrs: { class: "markdown-code-block" } },
                    node.language === null ? null : (h("div", { attrs: { class: "markdown-code-language" } }, node.language)),
                    h("pre", null,
                        h("code", { attrs: jsonChildren !== null
                                ? { class: "json-highlight" }
                                : codeChildren !== null
                                    ? { class: "code-highlight" }
                                    : {} }, jsonChildren ?? codeChildren ?? node.text))));
            }
            case "table":
                return (h("div", { attrs: { class: "markdown-table-scroll" } },
                    h("table", null,
                        h("thead", null,
                            h("tr", null, node.header.map((cell) => (h("th", { attrs: { class: alignmentClass(cell.alignment) } }, renderInlines(cell.children)))))),
                        h("tbody", null, node.rows.map((row) => (h("tr", null, row.map((cell) => (h("td", { attrs: { class: alignmentClass(cell.alignment) } }, renderInlines(cell.children)))))))))));
            case "rule":
                return h("hr", null);
            case "literal":
                return (h("pre", { attrs: { class: "markdown-literal markdown-literal-block" } }, node.text));
        }
    });
}
function literalContent(source) {
    return (h("div", { attrs: { class: "markdown-render-host" } },
        h("pre", { attrs: { class: "block-content" } }, source)));
}
export function renderMarkdownContent(blockKind, source, highlightJson = true, maxJsonSegments = MAX_JSON_SEGMENTS) {
    if (UTF8_ENCODER.encode(source).byteLength > MAX_MARKDOWN_SOURCE_BYTES) {
        return literalContent(source);
    }
    try {
        const tokens = Lexer.lex(source, { ...MARKED_OPTIONS });
        const normalized = new MarkdownNormalizer().normalize(tokens);
        if (blockKind === "text" && !normalized.significant) {
            return literalContent(source);
        }
        return (h("div", { attrs: { class: "markdown-render-host" } },
            h("div", { attrs: { class: "block-content markdown-content" } }, renderBlocks(normalized.blocks, {
                highlightJson,
                jsonSegmentsRemaining: maxJsonSegments,
            }))));
    }
    catch {
        return literalContent(source);
    }
}
export function renderToolResultContent(source, payloadTruncated, maxJsonSegments = MAX_JSON_SEGMENTS) {
    if (payloadTruncated) {
        return literalContent(source);
    }
    const inspection = inspectJson(source);
    if (inspection.status === "bounded") {
        return literalContent(source);
    }
    if (inspection.status === "valid") {
        if (inspection.segments > maxJsonSegments) {
            return literalContent(source);
        }
        const children = jsonTokenChildren(source, maxJsonSegments);
        if (children === null) {
            return literalContent(source);
        }
        return (h("div", { attrs: { class: "markdown-render-host" } },
            h("pre", { attrs: { class: "block-content json-highlight" } }, children)));
    }
    return renderMarkdownContent("text", source, true, maxJsonSegments);
}
export function toolResultContentThunk(source, payloadTruncated, maxJsonSegments = MAX_JSON_SEGMENTS) {
    return thunk("div", renderToolResultContent, [
        source,
        payloadTruncated,
        maxJsonSegments,
    ]);
}
export function markdownContentThunk(blockKind, source, highlightJson = true, maxJsonSegments = MAX_JSON_SEGMENTS) {
    return thunk("div", renderMarkdownContent, [blockKind, source, highlightJson, maxJsonSegments]);
}
