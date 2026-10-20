import { jsx as h } from "../lib/snabbdom/build/index.js";
import { SPINNER_FRAMES, TOOL_SPINNER_FRAMES, UI_ICON_GLYPHS, ICON_GLYPHS, } from "./generated/contracts.js";
// UI-only actions do not change the native protocol's icon vocabulary.
export const BUTTON_ICONS = { ...ICON_GLYPHS, ...UI_ICON_GLYPHS,
    run: "\uf04b", site: "\uf0ac",
    fullscreen: "\uf065", restore: "\uf066", previous: "\uf053", next: "\uf054",
    upload: "\uf093", pin: "\uf08d", copy: "\uf0c5", preview: "\uf06e", folder: "\uf07b", file: "\uf15b",
    refresh: "\uf021", connection: "\uf1e6", history: "\uf1da", code: "\uf121",
    expand: "\uf078", collapse: "\uf054", columns: "\uf0db", list: "\uf03a",
};
/** For the small native-dialog templates; glyphs are static trusted constants. */
export function buttonIconHtml(icon) {
    return `<span class="ui-icon" aria-hidden="true">${BUTTON_ICONS[icon]}</span>`;
}
export function activitySpinner(kind) {
    const frames = kind === "regular" ? SPINNER_FRAMES : TOOL_SPINNER_FRAMES;
    return (h("span", { attrs: {
            class: `activity-spinner activity-spinner-${kind}`,
            "aria-hidden": "true",
        } }, frames.map((frame, index) => (h("span", { key: `${kind}-${String(index)}`, attrs: { class: "activity-spinner-frame" } }, frame)))));
}
export function uiIcon(icon) {
    return (h("span", { attrs: { class: "ui-icon", "aria-hidden": "true" } }, BUTTON_ICONS[icon]));
}
/** Nerd Font file-type glyphs, using only the filename extension. */
export function fileTypeIcon(path) {
    const extension = path.split("/").at(-1)?.split(".").at(-1)?.toLowerCase() ?? "";
    const types = {
        py: ["\ue73c", "Python"], pyw: ["\ue73c", "Python"], ipynb: ["\ue73c", "Notebook"],
        js: ["\ue74e", "JavaScript"], mjs: ["\ue74e", "JavaScript"], jsx: ["\ue74e", "JavaScript"],
        ts: ["\ue628", "TypeScript"], tsx: ["\ue628", "TypeScript"], rs: ["\ue7a8", "Rust"],
        md: ["\ue73e", "Markdown"], markdown: ["\ue73e", "Markdown"],
        json: ["\ue60b", "JSON"], html: ["\uf13b", "HTML"], htm: ["\uf13b", "HTML"], css: ["\uf13c", "CSS"],
        csv: ["\uf1c3", "Spreadsheet"], tsv: ["\uf1c3", "Spreadsheet"], xlsx: ["\uf1c3", "Spreadsheet"],
        png: ["\uf1c5", "Image"], jpg: ["\uf1c5", "Image"], jpeg: ["\uf1c5", "Image"],
        gif: ["\uf1c5", "Image"], webp: ["\uf1c5", "Image"], svg: ["\uf1c5", "SVG"],
        pdf: ["\uf1c1", "PDF"], zip: ["\uf1c6", "Archive"], gz: ["\uf1c6", "Archive"], tar: ["\uf1c6", "Archive"],
        txt: ["\uf15c", "Text"], log: ["\uf15c", "Log"],
        yml: ["\uf013", "Configuration"], yaml: ["\uf013", "Configuration"], toml: ["\uf013", "Configuration"],
    };
    const [glyph, label] = types[extension] ?? [BUTTON_ICONS.file, "File"];
    return h("span", { attrs: { class: "ui-icon file-type-icon", "aria-hidden": "true", title: label } }, glyph);
}
