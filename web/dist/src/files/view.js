import { jsx as h } from "../../lib/snabbdom/build/index.js";
import { exclusionLabel } from "./protocol.js";
function breadcrumbs(directory, live, actions) {
    const parts = directory === "" ? [] : directory.split("/");
    return h("nav", { attrs: { class: "files-breadcrumbs", "aria-label": "File directory" } },
        h("button", { attrs: { type: "button", title: "Fixed process launch directory" }, props: { disabled: !live }, on: { click: () => actions.navigate("") } }, "Launch root"),
        parts.map((part, index) => h("span", { key: String(index) },
            h("span", { attrs: { "aria-hidden": "true" } }, " / "),
            h("button", { attrs: { type: "button" }, props: { disabled: !live }, on: { click: () => actions.navigate(parts.slice(0, index + 1).join("/")) } }, part))));
}
export function renderFilesPane(state, live, actions) {
    const path = state.selectedPath ?? state.directory;
    const exclusions = state.listing === null ? null : exclusionLabel(state.listing.exclusions);
    return h("section", { key: "files-pane", attrs: { class: `files-pane${state.open ? " is-open" : ""}`, "aria-label": "Read-only launch-directory files" } },
        h("header", { attrs: { class: "files-toolbar" } },
            h("button", { attrs: { type: "button", id: "files-toggle", "aria-expanded": state.open ? "true" : "false", "aria-controls": "files-body" }, props: { disabled: !live && !state.open }, on: { click: actions.toggle } }, state.open ? "▾ Files" : "▸ Files"),
            h("span", { attrs: { class: "files-scope" } }, "Read-only \u00B7 fixed launch root"),
            !state.open ? null : h("div", { attrs: { class: "files-actions" } },
                h("button", { attrs: { type: "button" }, props: { disabled: !live || state.listingBusy || state.previewBusy }, on: { click: actions.refresh } }, "Refresh"),
                h("button", { attrs: { type: "button", title: "Download the selected file (up to 32 MiB)" }, props: { disabled: !live || state.selectedPath === null || state.downloadBusy }, on: { click: () => actions.download(false) } }, "Download File"),
                h("button", { attrs: { type: "button", title: "Download the current folder as ZIP; exclusions and fixed limits apply" }, props: { disabled: !live || state.downloadBusy || state.listingBusy }, on: { click: () => actions.download(true) } }, "Download Folder"),
                h("button", { attrs: { type: "button", title: "Copy a launch-root-relative path; does not change the prompt" }, props: { disabled: path === "" }, on: { click: actions.copyPath } }, "Copy Path"))),
        !state.open ? null : h("div", { attrs: { id: "files-body", class: "files-body" } },
            h("div", { attrs: { class: "files-browser" } },
                breadcrumbs(state.directory, live, actions),
                h("div", { attrs: { class: "files-list", "aria-label": "Directory entries", "aria-busy": state.listingBusy ? "true" : "false" } },
                    state.listingBusy ? h("p", { attrs: { role: "status" } }, "Loading directory\u2026") : null,
                    state.listingError === null ? null : h("p", { attrs: { class: "files-notice", role: "status" } }, state.listingError),
                    state.listing?.entries.map((entry) => h("button", { key: entry.path, attrs: { type: "button", class: `files-entry${state.selectedPath === entry.path ? " is-selected" : ""}`,
                            title: entry.path, "aria-pressed": state.selectedPath === entry.path ? "true" : "false" }, props: { disabled: !live }, on: { click: () => entry.kind === "directory" ? actions.navigate(entry.path) : actions.select(entry.path) } },
                        h("span", { attrs: { "aria-hidden": "true" } }, entry.kind === "directory" ? "▸" : "·"),
                        h("span", null, entry.name),
                        h("small", null, entry.kind === "directory" ? "Folder" : `${entry.size ?? "0"} B`))),
                    state.listing?.entries.length === 0 ? h("p", null, "No eligible files in this folder.") : null),
                state.listing?.truncated === true ? h("p", { attrs: { class: "files-notice" } }, "Showing the first 1,000 eligible entries. Narrow the folder selection.") : null,
                exclusions === null ? null : h("p", { attrs: { class: "files-notice" } }, exclusions)),
            h("div", { attrs: { class: "files-preview" } },
                h("div", { attrs: { class: "files-path", title: "Path relative to the fixed launch directory" } },
                    h("span", null, state.selectedPath ?? "Select a file to view"),
                    state.preview === null ? null : h("small", null,
                        state.preview.size,
                        " bytes \u00B7 read-only")),
                h("div", { attrs: { class: "files-editor-area" } },
                    h("div", { key: "monaco-host", attrs: { id: "files-monaco", class: "files-monaco", "aria-label": "Read-only Monaco editor" } }),
                    state.preview !== null ? null : h("p", { attrs: { class: "files-preview-message", role: "status" } }, state.previewBusy ? "Loading file…" : state.previewError ?? "UTF-8 previews up to 2 MiB. File contents stay outside chat until you explicitly include them in a prompt."))),
            !state.downloadBusy ? null : h("div", { attrs: { class: "files-download-status", role: "status" } }, "Preparing download\u2026")));
}
