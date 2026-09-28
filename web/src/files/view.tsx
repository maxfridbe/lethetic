import { jsx as h } from "../../lib/snabbdom/build/index.js";
import type { VNode } from "../../lib/snabbdom/build/index.js";
import type { DiffLayout, FilePaneState, FilePaneTab } from "./state.js";
import { exclusionLabel } from "./protocol.js";
import { CHANGE_LETTERS, changeRows, type ChangeRow } from "./git.js";

export interface FilePaneActions {
  readonly toggle: () => void;
  readonly navigate: (path: string) => void;
  readonly select: (path: string) => void;
  readonly refresh: () => void;
  readonly download: (archive: boolean) => void;
  readonly copyPath: () => void;
  readonly showTab: (tab: FilePaneTab) => void;
  readonly refreshChanges: () => void;
  readonly selectChange: (path: string) => void;
  readonly toggleFolder: (path: string) => void;
  readonly setDiffLayout: (layout: DiffLayout) => void;
}

/** `+added −removed` counts, or `binary` when git reported no line numbers. */
function lineCounts(added: number | null, removed: number | null): VNode {
  if (added === null && removed === null) {
    return <span attrs={{ class: "changes-counts is-binary", title: "Binary or unreadable content" }}>binary</span>;
  }
  return <span attrs={{ class: "changes-counts", title: `${added ?? 0} lines added, ${removed ?? 0} lines removed` }}>
    <span attrs={{ class: "changes-added" }}>+{String(added ?? 0)}</span>
    <span attrs={{ class: "changes-removed" }}>−{String(removed ?? 0)}</span>
  </span>;
}

function changeRow(row: ChangeRow, state: FilePaneState, live: boolean, actions: FilePaneActions): VNode {
  const selected = !row.folder && state.selectedChange === row.path;
  const collapsed = row.folder && state.collapsed.has(row.path);
  return <button key={`${row.folder ? "d" : "f"}:${row.path}`}
    attrs={{ type: "button", class: `changes-entry${row.folder ? " is-folder" : ` is-${row.kind ?? "modified"}`}${selected ? " is-selected" : ""}`,
      title: row.folder ? `${row.path} · ${row.files} changed file${row.files === 1 ? "" : "s"}` : `${row.path} · ${row.kind ?? ""}`,
      "aria-pressed": selected ? "true" : "false",
      ...(row.folder ? { "aria-expanded": collapsed ? "false" : "true" } : {}) }}
    style={{ paddingLeft: `${0.5 + row.depth * 0.9}rem` }}
    props={{ disabled: !live && !row.folder }}
    on={{ click: () => row.folder ? actions.toggleFolder(row.path) : actions.selectChange(row.path) }}>
    <span attrs={{ class: "changes-marker", "aria-hidden": "true" }}>
      {row.folder ? (collapsed ? "▸" : "▾") : CHANGE_LETTERS[row.kind ?? "modified"]}
    </span>
    <span attrs={{ class: "changes-name" }}>{row.folder ? `${row.name}/` : row.name}</span>
    {lineCounts(row.added, row.removed)}
  </button>;
}

function renderChangesBody(state: FilePaneState, live: boolean, actions: FilePaneActions): VNode {
  const changes = state.changes;
  const rows = changes === null ? [] : changeRows(changes.files, state.collapsed);
  const totals = changes === null ? null : changes.files.reduce<{ added: number | null; removed: number | null }>(
    (sum, file) => ({
      added: file.added === null ? sum.added : (sum.added ?? 0) + file.added,
      removed: file.removed === null ? sum.removed : (sum.removed ?? 0) + file.removed,
    }), { added: null, removed: null });
  const layoutButton = (layout: DiffLayout, label: string, title: string): VNode =>
    <button attrs={{ type: "button", title, "aria-pressed": state.diffLayout === layout ? "true" : "false",
      class: state.diffLayout === layout ? "is-active" : "" }}
      on={{ click: () => actions.setDiffLayout(layout) }}>{label}</button>;
  return <div attrs={{ id: "files-body", class: "files-body changes-body" }}>
    <div attrs={{ class: "files-browser" }}>
      <div attrs={{ class: "changes-summary" }}>
        <span>{changes === null ? "Git changes" : !changes.repository ? "Not a git repository"
          : `${changes.branch ?? "detached"} · ${changes.files.length} file${changes.files.length === 1 ? "" : "s"}`}</span>
        {totals === null || changes === null || changes.files.length === 0 ? null : lineCounts(totals.added, totals.removed)}
      </div>
      <div attrs={{ class: "files-list", "aria-label": "Changed files", "aria-busy": state.changesBusy ? "true" : "false" }}>
        {state.changesBusy ? <p attrs={{ role: "status" }}>Reading git status…</p> : null}
        {state.changesError === null ? null : <p attrs={{ class: "files-notice", role: "status" }}>{state.changesError}</p>}
        {rows.map((row) => changeRow(row, state, live, actions))}
        {changes?.repository === true && changes.files.length === 0 ? <p>No changes against HEAD.</p> : null}
      </div>
      {changes?.truncated === true ? <p attrs={{ class: "files-notice" }}>Showing the first 1,000 changed files.</p> : null}
      {changes === null || changes.protected === 0 ? null
        : <p attrs={{ class: "files-notice" }}>{`${changes.protected} changed path${changes.protected === 1 ? " is" : "s are"} hidden by the disclosure policy.`}</p>}
    </div>
    <div attrs={{ class: "files-preview" }}>
      <div attrs={{ class: "files-path changes-path" }}>
        <span>{state.selectedChange ?? "Select a changed file"}</span>
        <span attrs={{ class: "changes-layout", role: "group", "aria-label": "Diff layout" }}>
          {layoutButton("side-by-side", "Side by side", "HEAD on the left, working tree on the right")}
          {layoutButton("inline", "Inline", "Removed lines stacked above added lines")}
        </span>
      </div>
      <div attrs={{ class: "files-editor-area" }}>
        <div key="monaco-diff-host" attrs={{ id: "files-diff-monaco", class: "files-monaco", "aria-label": "Read-only Monaco diff" }}></div>
        {state.diff !== null ? null : <p attrs={{ class: "files-preview-message", role: "status" }}>
          {state.diffBusy ? "Loading diff…" : state.diffError ?? "Compares HEAD with the working tree. Protected paths are never shown."}
        </p>}
      </div>
    </div>
  </div>;
}

function breadcrumbs(directory: string, live: boolean, actions: FilePaneActions): VNode {
  const parts = directory === "" ? [] : directory.split("/");
  return <nav attrs={{ class: "files-breadcrumbs", "aria-label": "File directory" }}>
    <button attrs={{ type: "button", title: "Fixed process launch directory" }} props={{ disabled: !live }}
      on={{ click: () => actions.navigate("") }}>Launch root</button>
    {parts.map((part, index) => <span key={String(index)}>
      <span attrs={{ "aria-hidden": "true" }}> / </span>
      <button attrs={{ type: "button" }} props={{ disabled: !live }}
        on={{ click: () => actions.navigate(parts.slice(0, index + 1).join("/")) }}>{part}</button>
    </span>)}
  </nav>;
}

export function renderFilesPane(state: FilePaneState, live: boolean, actions: FilePaneActions): VNode {
  const path = state.selectedPath ?? state.directory;
  const exclusions = state.listing === null ? null : exclusionLabel(state.listing.exclusions);
  return <section key="files-pane" attrs={{ class: `files-pane${state.open ? " is-open" : ""}`, "aria-label": "Read-only launch-directory files" }}>
    <header attrs={{ class: "files-toolbar" }}>
      <button attrs={{ type: "button", id: "files-toggle", "aria-expanded": state.open ? "true" : "false", "aria-controls": "files-body" }}
        props={{ disabled: !live && !state.open }} on={{ click: actions.toggle }}>
        {state.open ? "▾ Files" : "▸ Files"}
      </button>
      {!state.open ? <span attrs={{ class: "files-scope" }}>Read-only · fixed launch root</span>
        : <span attrs={{ class: "files-tabs", role: "tablist", "aria-label": "Files view" }}>
          {(["files", "changes"] as const).map((tab) => <button key={tab}
            attrs={{ type: "button", role: "tab", class: state.tab === tab ? "is-active" : "",
              "aria-selected": state.tab === tab ? "true" : "false" }}
            props={{ disabled: !live }} on={{ click: () => actions.showTab(tab) }}>
            {tab === "files" ? "Browse" : "Changes"}</button>)}
        </span>}
      {!state.open || state.tab !== "changes" ? null : <div attrs={{ class: "files-actions" }}>
        <button attrs={{ type: "button" }} props={{ disabled: !live || state.changesBusy }}
          on={{ click: actions.refreshChanges }}>Refresh</button>
      </div>}
      {!state.open || state.tab !== "files" ? null : <div attrs={{ class: "files-actions" }}>
        <button attrs={{ type: "button" }} props={{ disabled: !live || state.listingBusy || state.previewBusy }}
          on={{ click: actions.refresh }}>Refresh</button>
        <button attrs={{ type: "button", title: "Download the selected file (up to 32 MiB)" }}
          props={{ disabled: !live || state.selectedPath === null || state.downloadBusy }}
          on={{ click: () => actions.download(false) }}>Download File</button>
        <button attrs={{ type: "button", title: "Download the current folder as ZIP; exclusions and fixed limits apply" }}
          props={{ disabled: !live || state.downloadBusy || state.listingBusy }}
          on={{ click: () => actions.download(true) }}>Download Folder</button>
        <button attrs={{ type: "button", title: "Copy a launch-root-relative path; does not change the prompt" }}
          props={{ disabled: path === "" }} on={{ click: actions.copyPath }}>Copy Path</button>
      </div>}
    </header>
    {state.open && state.tab === "changes" ? renderChangesBody(state, live, actions) : null}
    {!state.open || state.tab !== "files" ? null : <div attrs={{ id: "files-body", class: "files-body" }}>
      <div attrs={{ class: "files-browser" }}>
        {breadcrumbs(state.directory, live, actions)}
        <div attrs={{ class: "files-list", "aria-label": "Directory entries", "aria-busy": state.listingBusy ? "true" : "false" }}>
          {state.listingBusy ? <p attrs={{ role: "status" }}>Loading directory…</p> : null}
          {state.listingError === null ? null : <p attrs={{ class: "files-notice", role: "status" }}>{state.listingError}</p>}
          {state.listing?.entries.map((entry) => <button key={entry.path}
            attrs={{ type: "button", class: `files-entry${state.selectedPath === entry.path ? " is-selected" : ""}`,
              title: entry.path, "aria-pressed": state.selectedPath === entry.path ? "true" : "false" }}
            props={{ disabled: !live }}
            on={{ click: () => entry.kind === "directory" ? actions.navigate(entry.path) : actions.select(entry.path) }}>
            <span attrs={{ "aria-hidden": "true" }}>{entry.kind === "directory" ? "▸" : "·"}</span>
            <span>{entry.name}</span>
            <small>{entry.kind === "directory" ? "Folder" : `${entry.size ?? "0"} B`}</small>
          </button>)}
          {state.listing?.entries.length === 0 ? <p>No eligible files in this folder.</p> : null}
        </div>
        {state.listing?.truncated === true ? <p attrs={{ class: "files-notice" }}>Showing the first 1,000 eligible entries. Narrow the folder selection.</p> : null}
        {exclusions === null ? null : <p attrs={{ class: "files-notice" }}>{exclusions}</p>}
      </div>
      <div attrs={{ class: "files-preview" }}>
        <div attrs={{ class: "files-path", title: "Path relative to the fixed launch directory" }}>
          <span>{state.selectedPath ?? "Select a file to view"}</span>
          {state.preview === null ? null : <small>{state.preview.size} bytes · read-only</small>}
        </div>
        <div attrs={{ class: "files-editor-area" }}>
          <div key="monaco-host" attrs={{ id: "files-monaco", class: "files-monaco", "aria-label": "Read-only Monaco editor" }}></div>
          {state.preview !== null ? null : <p attrs={{ class: "files-preview-message", role: "status" }}>
            {state.previewBusy ? "Loading file…" : state.previewError ?? "UTF-8 previews up to 2 MiB. File contents stay outside chat until you explicitly include them in a prompt."}
          </p>}
        </div>
      </div>
      {!state.downloadBusy ? null : <div attrs={{ class: "files-download-status", role: "status" }}>Preparing download…</div>}
    </div>}
  </section>;
}
