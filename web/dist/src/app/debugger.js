import { jsx as h } from "../../lib/snabbdom/build/index.js";
import { boundedText } from "../safety.js";
function diagnosticCodeLabel(code) {
    return code.replaceAll("_", " ");
}
function renderDebuggerContent(debuggerState, close, autofocus) {
    return [
        h("header", { attrs: { class: "debugger-header" } },
            h("div", null,
                h("span", { attrs: { class: "eyebrow" } }, "Operational diagnostics"),
                h("strong", null, "Debugger")),
            h("button", { attrs: {
                    type: "button",
                    class: "text-button debugger-close",
                    "aria-label": "Close debugger",
                    title: "Close debugger",
                    ...(autofocus ? { "data-autofocus": "true" } : {}),
                }, on: { click: close } }, "Close")),
        h("p", { attrs: { class: "debugger-summary" } }, boundedText(debuggerState.summary)),
        debuggerState.omitted_before === 0 ? (h("span", { attrs: { class: "debugger-omission-placeholder", "aria-hidden": "true" } })) : (h("p", { attrs: { class: "bounded-notice" } }, `${debuggerState.omitted_before} older diagnostic event(s) were omitted.`)),
        debuggerState.entries.length === 0 ? (h("p", { attrs: { class: "empty-state debugger-empty" } }, "No browser-safe operational events have been recorded.")) : (h("ol", { attrs: { class: "debugger-entries" } }, debuggerState.entries.map((entry, index) => (h("li", { key: `diagnostic-${index}-${entry.code}`, attrs: { class: `diagnostic diagnostic-${entry.severity}` } },
            h("span", { attrs: { class: "diagnostic-heading" } },
                h("strong", null, diagnosticCodeLabel(entry.code)),
                h("span", null, entry.severity)),
            h("p", null, boundedText(entry.message))))))),
    ];
}
export function renderDebugger(context) {
    const snapshot = context.state.snapshot;
    if (snapshot === null ||
        !snapshot.debugger.open ||
        (!context.state.debuggerWide && context.state.debuggerDrawerDismissed)) {
        return null;
    }
    const close = context.actions.toggleDebugger;
    const covered = !context.state.debuggerWide && context.state.activePanel !== null;
    const content = renderDebuggerContent(snapshot.debugger, close, !context.state.debuggerWide && !covered);
    if (context.state.debuggerWide) {
        return (h("aside", { attrs: {
                class: "debugger-pane",
                "aria-label": "Debugger",
            } }, content));
    }
    return (h("div", { attrs: {
            class: `debugger-drawer-layer${covered ? " is-covered" : ""}`,
            ...(covered ? { "aria-hidden": "true", inert: "" } : {}),
        } },
        h("button", { attrs: {
                type: "button",
                class: "debugger-backdrop",
                "aria-label": "Close debugger",
                tabindex: "-1",
            }, on: { click: close } }),
        h("aside", { attrs: {
                class: "debugger-drawer",
                role: "dialog",
                "aria-label": "Debugger",
                ...(covered
                    ? {}
                    : { "aria-modal": "true", "data-focus-trap": "true" }),
            } }, content)));
}
