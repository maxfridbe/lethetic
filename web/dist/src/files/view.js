import { jsx as h } from "../../lib/snabbdom/build/index.js";
import { exclusionLabel } from "./protocol.js";
import { CHANGE_LETTERS, changeRows } from "./git.js";
/** `+added −removed` counts, or `binary` when git reported no line numbers. */
function lineCounts(added, removed) {
    if (added === null && removed === null) {
        return h("span", { attrs: { class: "changes-counts is-binary", title: "Binary or unreadable content" } }, "binary");
    }
    return h("span", { attrs: { class: "changes-counts", title: `${added ?? 0} lines added, ${removed ?? 0} lines removed` } },
        h("span", { attrs: { class: "changes-added" } },
            "+",
            String(added ?? 0)),
        h("span", { attrs: { class: "changes-removed" } },
            "\u2212",
            String(removed ?? 0)));
}
function changeRow(row, state, live, actions) {
    const selected = !row.folder && state.selectedChange === row.path;
    const collapsed = row.folder && state.collapsed.has(row.path);
    return h("button", { key: `${row.folder ? "d" : "f"}:${row.path}`, attrs: { type: "button", class: `changes-entry${row.folder ? " is-folder" : ` is-${row.kind ?? "modified"}`}${selected ? " is-selected" : ""}`,
            title: row.folder ? `${row.path} · ${row.files} changed file${row.files === 1 ? "" : "s"}` : `${row.path} · ${row.kind ?? ""}`,
            "aria-pressed": selected ? "true" : "false",
            ...(row.folder ? { "aria-expanded": collapsed ? "false" : "true" } : {}) }, style: { paddingLeft: `${0.5 + row.depth * 0.9}rem` }, props: { disabled: !live && !row.folder }, on: { click: () => row.folder ? actions.toggleFolder(row.path) : actions.selectChange(row.path) } },
        h("span", { attrs: { class: "changes-marker", "aria-hidden": "true" } }, row.folder ? (collapsed ? "▸" : "▾") : CHANGE_LETTERS[row.kind ?? "modified"]),
        h("span", { attrs: { class: "changes-name" } }, row.folder ? `${row.name}/` : row.name),
        lineCounts(row.added, row.removed));
}
function renderChangesBody(state, live, actions) {
    const changes = state.changes;
    const rows = changes === null ? [] : changeRows(changes.files, state.collapsed);
    const totals = changes === null ? null : changes.files.reduce((sum, file) => ({
        added: file.added === null ? sum.added : (sum.added ?? 0) + file.added,
        removed: file.removed === null ? sum.removed : (sum.removed ?? 0) + file.removed,
    }), { added: null, removed: null });
    const layoutButton = (layout, label, title) => h("button", { attrs: { type: "button", title, "aria-pressed": state.diffLayout === layout ? "true" : "false",
            class: state.diffLayout === layout ? "is-active" : "" }, on: { click: () => actions.setDiffLayout(layout) } }, label);
    return h("div", { attrs: { id: "files-body", class: "files-body changes-body" } },
        h("div", { attrs: { class: "files-browser" } },
            h("div", { attrs: { class: "changes-summary" } },
                h("span", null, changes === null ? "Git changes" : !changes.repository ? "Not a git repository"
                    : `${changes.branch ?? "detached"} · ${changes.files.length} file${changes.files.length === 1 ? "" : "s"}`),
                totals === null || changes === null || changes.files.length === 0 ? null : lineCounts(totals.added, totals.removed)),
            h("div", { attrs: { class: "files-list", "aria-label": "Changed files", "aria-busy": state.changesBusy ? "true" : "false" } },
                state.changesBusy ? h("p", { attrs: { role: "status" } }, "Reading git status\u2026") : null,
                state.changesError === null ? null : h("p", { attrs: { class: "files-notice", role: "status" } }, state.changesError),
                rows.map((row) => changeRow(row, state, live, actions)),
                changes?.repository === true && changes.files.length === 0 ? h("p", null, "No changes against HEAD.") : null),
            changes?.truncated === true ? h("p", { attrs: { class: "files-notice" } }, "Showing the first 1,000 changed files.") : null,
            changes === null || changes.protected === 0 ? null
                : h("p", { attrs: { class: "files-notice" } }, `${changes.protected} changed path${changes.protected === 1 ? " is" : "s are"} hidden by the disclosure policy.`)),
        h("div", { attrs: { class: "files-preview" } },
            h("div", { attrs: { class: "files-path changes-path" } },
                h("span", null, state.selectedChange ?? "Select a changed file"),
                h("span", { attrs: { class: "changes-layout", role: "group", "aria-label": "Diff layout" } },
                    layoutButton("side-by-side", "Side by side", "HEAD on the left, working tree on the right"),
                    layoutButton("inline", "Inline", "Removed lines stacked above added lines"))),
            h("div", { attrs: { class: "files-editor-area" } },
                h("div", { key: "monaco-diff-host", attrs: { id: "files-diff-monaco", class: "files-monaco", "aria-label": "Read-only Monaco diff" } }),
                state.diff !== null ? null : h("p", { attrs: { class: "files-preview-message", role: "status" } }, state.diffBusy ? "Loading diff…" : state.diffError ?? "Compares HEAD with the working tree. Protected paths are never shown."))));
}
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
            !state.open ? h("span", { attrs: { class: "files-scope" } }, "Read-only \u00B7 fixed launch root")
                : h("span", { attrs: { class: "files-tabs", role: "tablist", "aria-label": "Files view" } }, ["files", "changes"].map((tab) => h("button", { key: tab, attrs: { type: "button", role: "tab", class: state.tab === tab ? "is-active" : "",
                        "aria-selected": state.tab === tab ? "true" : "false" }, props: { disabled: !live }, on: { click: () => actions.showTab(tab) } }, tab === "files" ? "Browse" : "Changes"))),
            !state.open || state.tab !== "changes" ? null : h("div", { attrs: { class: "files-actions" } },
                h("button", { attrs: { type: "button" }, props: { disabled: !live || state.changesBusy }, on: { click: actions.refreshChanges } }, "Refresh")),
            !state.open || state.tab !== "files" ? null : h("div", { attrs: { class: "files-actions" } },
                h("button", { attrs: { type: "button" }, props: { disabled: !live || state.listingBusy || state.previewBusy }, on: { click: actions.refresh } }, "Refresh"),
                h("button", { attrs: { type: "button", title: "Download the selected file (up to 32 MiB)" }, props: { disabled: !live || state.selectedPath === null || state.downloadBusy }, on: { click: () => actions.download(false) } }, "Download File"),
                h("button", { attrs: { type: "button", title: "Download the current folder as ZIP; exclusions and fixed limits apply" }, props: { disabled: !live || state.downloadBusy || state.listingBusy }, on: { click: () => actions.download(true) } }, "Download Folder"),
                h("button", { attrs: { type: "button", title: "Copy a launch-root-relative path; does not change the prompt" }, props: { disabled: path === "" }, on: { click: actions.copyPath } }, "Copy Path"))),
        state.open && state.tab === "changes" ? renderChangesBody(state, live, actions) : null,
        !state.open || state.tab !== "files" ? null : h("div", { attrs: { id: "files-body", class: "files-body" } },
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
