import { jsx as h } from "../lib/snabbdom/build/index.js";
import { SPINNER_FRAMES, TOOL_SPINNER_FRAMES, UI_ICON_GLYPHS, ICON_GLYPHS, } from "./generated/contracts.js";
// UI-only actions do not change the native protocol's icon vocabulary.
export const BUTTON_ICONS = { ...ICON_GLYPHS, ...UI_ICON_GLYPHS,
    pin: "\uf08d", copy: "\uf0c5", preview: "\uf06e", folder: "\uf07b", file: "\uf15b",
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
