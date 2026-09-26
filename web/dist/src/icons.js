import { jsx as h } from "../lib/snabbdom/build/index.js";
import { SPINNER_FRAMES, TOOL_SPINNER_FRAMES, UI_ICON_GLYPHS, } from "./generated/contracts.js";
export function activitySpinner(kind) {
    const frames = kind === "regular" ? SPINNER_FRAMES : TOOL_SPINNER_FRAMES;
    return (h("span", { attrs: {
            class: `activity-spinner activity-spinner-${kind}`,
            "aria-hidden": "true",
        } }, frames.map((frame, index) => (h("span", { key: `${kind}-${String(index)}`, attrs: { class: "activity-spinner-frame" } }, frame)))));
}
export function uiIcon(icon) {
    return (h("span", { attrs: { class: "ui-icon", "aria-hidden": "true" } }, UI_ICON_GLYPHS[icon]));
}
