import { fileSize } from "./size.js";
import { jsx as h } from "../../lib/snabbdom/build/index.js";
import type { VNode } from "../../lib/snabbdom/build/index.js";
import type { FileListEntry } from "../generated/contracts.js";
import type { DiffLayout, FileDock, FilePaneState, FilePaneTab } from "./state.js";
import { uiIcon, fileTypeIcon } from "../icons.js";
import { exclusionLabel } from "./protocol.js";
import { CHANGE_LETTERS, changeRows, type ChangeRow } from "./git.js";

export interface FilePaneActions {
  readonly expandPreview: () => void;
  readonly nextPreview: (direction: number) => void;
  readonly toggle: () => void;
  readonly setDock: (dock: FileDock) => void;
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

export interface FilePaneExtensions {
  viewBytes?: number;
  previewContent?: (state: FilePaneState) => VNode | null;
  expandRendered?: (state: FilePaneState) => boolean;
  nativeText?: () => boolean;
  count?: () => number;
  entries?: (entries: readonly FileListEntry[]) => FileListEntry[];
  entryTitle?: (path: string) => string;
  scope?: string;
  rootLabel?: string;
  local?: boolean;
  actions?: (state: FilePaneState) => VNode;
  contentActions?: (state: FilePaneState) => VNode;
}

/** Pointer capture keeps resizing reliable even when the pointer leaves the divider. */
function splitter(outer = false): VNode {
  const context = (target: HTMLElement) => {
    const pane = target.closest<HTMLElement>(".files-pane")!;
    const left = pane.dataset["dock"] === "left" && matchMedia("(min-width: 900px)").matches;
    const host = outer ? pane.parentElement! : target.parentElement!;
    const vertical = outer ? !left : left;
    const pixels = outer && vertical;
    return { pane, host, vertical, pixels, property: outer ? (vertical ? "--files-height" : "--files-width") : vertical ? "--files-list-height" : "--files-list-width" };
  };
  return <div attrs={{ class: `files-splitter${outer ? " files-outer-splitter" : ""}`, role: "separator", tabindex: "0",
    "aria-label": outer ? "Resize Files pane" : "Resize file list and preview", title: "Drag to resize; arrow keys adjust the split" }}
    on={{ pointerdown: (event: PointerEvent) => {
      event.preventDefault(); const target = event.currentTarget as HTMLElement;
      const {host, vertical, pixels, property} = context(target); const bounds = host.getBoundingClientRect();
      target.setPointerCapture(event.pointerId);
      const move = (event: PointerEvent) => {
        const ratio = vertical ? (event.clientY-bounds.top)/bounds.height : (event.clientX-bounds.left)/bounds.width;
        host.style.setProperty(property, pixels ? `${Math.max(160, Math.min(innerHeight * .55, 650, event.clientY - bounds.top))}px` : `${Math.max(15, Math.min(80, ratio*100))}%`);
      };
      const stop = () => { target.removeEventListener("pointermove", move); target.removeEventListener("lostpointercapture", stop); };
      target.addEventListener("pointermove", move); target.addEventListener("lostpointercapture", stop);
    }, keydown: (event: KeyboardEvent) => {
      if (!["ArrowLeft","ArrowRight","ArrowUp","ArrowDown"].includes(event.key)) return;
      const {pane, host, property, pixels, vertical} = context(event.currentTarget as HTMLElement);
      if (!(vertical ? ["ArrowUp", "ArrowDown"] : ["ArrowLeft", "ArrowRight"]).includes(event.key)) return;
      event.preventDefault();
      const direction = ["ArrowRight", "ArrowDown"].includes(event.key) ? 1 : -1;
      const current = pixels ? pane.getBoundingClientRect().height : Number.parseFloat(host.style.getPropertyValue(property)) || 34;
      host.style.setProperty(property, pixels ? `${Math.max(160, Math.min(innerHeight * .55, 650, current + direction * 20))}px`
        : `${Math.max(15,Math.min(80,current + direction * 3))}%`);
    } }}></div>;
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
      {row.folder ? uiIcon(collapsed ? "collapse" : "expand") : fileTypeIcon(row.path)}
      {row.folder ? null : CHANGE_LETTERS[row.kind ?? "modified"]}
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
      on={{ click: () => actions.setDiffLayout(layout) }}>{uiIcon(layout === "inline" ? "list" : "columns")}{label}</button>;
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
    {splitter()}
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

function breadcrumbs(directory: string, live: boolean, actions: FilePaneActions, rootLabel = "Launch root"): VNode {
  const parts = directory === "" ? [] : directory.split("/");
  return <nav attrs={{ class: "files-breadcrumbs", "aria-label": "File directory" }}>
    <button attrs={{ type: "button", title: "Fixed process launch directory" }} props={{ disabled: !live }}
      on={{ click: () => actions.navigate("") }}>{uiIcon("folder")}{rootLabel}</button>
    {parts.map((part, index) => <span key={String(index)}>
      <span attrs={{ "aria-hidden": "true" }}> / </span>
      <button attrs={{ type: "button" }} props={{ disabled: !live }}
        on={{ click: () => actions.navigate(parts.slice(0, index + 1).join("/")) }}>{uiIcon("folder")}{part}</button>
    </span>)}
  </nav>;
}

export function renderFilesPane(state: FilePaneState, live: boolean, actions: FilePaneActions, extensions: FilePaneExtensions = {}): VNode {
  const inlinePreview = extensions.previewContent?.(state) ?? null;
  const path = state.selectedPath ?? state.directory;
  const selectedSize = state.preview?.size ?? state.listing?.entries.find(entry => entry.path === state.selectedPath)?.size ?? null;
  const entries = extensions.entries?.(state.listing?.entries ?? []) ?? state.listing?.entries ?? [];
  const count = extensions.count?.();
  const rootLabel = (extensions.rootLabel ?? "Launch root") + (count === undefined ? "" : ` (${count})`);
  const exclusions = state.listing === null ? null : exclusionLabel(state.listing.exclusions);
  return <section key="files-pane" attrs={{ class: `files-pane${state.open ? " is-open" : ""}${state.fullPreview ? " is-full-preview" : ""}`, "data-dock": state.dock, "aria-label": extensions.scope ?? "Read-only launch-directory files" }}>
    {state.open ? splitter(true) : null}
    <header attrs={{ class: "files-toolbar" }}>
      <button attrs={{ type: "button", id: "files-toggle", "aria-expanded": state.open ? "true" : "false", "aria-controls": "files-body" }}
        props={{ disabled: !live && !state.open }} on={{ click: actions.toggle }}>
        {uiIcon(state.open ? "expand" : "collapse")}Files{count === undefined ? "" : ` (${count})`}
      </button>
      {!state.open ? <span attrs={{ class: "files-scope" }}>{extensions.scope ?? "Read-only · fixed launch root"}</span>
        : <span attrs={{ class: "files-tabs", role: "tablist", "aria-label": "Files view" }}>
          {(extensions.local ? ["files"] as const : ["files", "changes"] as const).map((tab) => <button key={tab}
            attrs={{ type: "button", role: "tab", class: state.tab === tab ? "is-active" : "",
              "aria-selected": state.tab === tab ? "true" : "false" }}
            props={{ disabled: !live }} on={{ click: () => actions.showTab(tab) }}>
            {uiIcon(tab === "files" ? "folder" : "code")}{tab === "files" ? "Browse" : "Changes"}</button>)}
        </span>}
      <span attrs={{ class: "files-dock-controls" }}>
        <button attrs={{ type: "button", "data-files-dock": state.dock === "top" ? "left" : "top",
          "aria-label": state.dock === "top" ? "Dock Files left (currently top)" : "Dock Files top (currently left)",
          title: state.dock === "top" ? "Dock Files left. Narrow screens use the top layout." : "Dock Files above the conversation" }}
          on={{ click: () => actions.setDock(state.dock === "top" ? "left" : "top") }}>{uiIcon(state.dock === "top" ? "columns" : "list")}</button>
      </span>
      {!state.open || state.tab !== "changes" ? null : <div attrs={{ class: "files-actions" }}>
        <button attrs={{ type: "button" }} props={{ disabled: !live || state.changesBusy }}
          on={{ click: actions.refreshChanges }}>{uiIcon("refresh")}Refresh</button>
      </div>}
      {!state.open || state.tab !== "files" ? null : <div attrs={{ class: "files-actions" }}>
        <button attrs={{ type: "button" }} props={{ disabled: !live || state.listingBusy || state.previewBusy }}
          on={{ click: actions.refresh }}>{uiIcon("refresh")}Refresh</button>
        {extensions.local ? null : <button attrs={{ type: "button", title: "Download the current folder as ZIP; exclusions and fixed limits apply" }}
          props={{ disabled: !live || state.downloadBusy || state.listingBusy }}
          on={{ click: () => actions.download(true) }}>{uiIcon("download")}Download Folder</button>}
        <button attrs={{ type: "button", title: "Copy a launch-root-relative path; does not change the prompt" }}
          props={{ disabled: path === "" }} on={{ click: actions.copyPath }}>{uiIcon("copy")}Copy Path</button>
        {extensions.actions?.(state) ?? null}
      </div>}
    </header>
    {state.open && state.tab === "changes" ? renderChangesBody(state, live, actions) : null}
    {!state.open || state.tab !== "files" ? null : <div attrs={{ id: "files-body", class: "files-body" }}>
      <div attrs={{ class: "files-browser" }}>
        {breadcrumbs(state.directory, live, actions, rootLabel)}
        <div attrs={{ class: "files-list", "aria-label": "Directory entries", "aria-busy": state.listingBusy ? "true" : "false" }}>
          {state.listingBusy && state.listing === null ? <p attrs={{ role: "status" }}>Loading directory…</p> : null}
          {state.listingError === null ? null : <p attrs={{ class: "files-notice", role: "status" }}>{state.listingError}</p>}
          {entries.map((entry) => <button key={entry.path}
            attrs={{ type: "button", class: `files-entry${state.selectedPath === entry.path ? " is-selected" : ""}`,
              title: entry.path, "aria-pressed": state.selectedPath === entry.path ? "true" : "false" }}
            props={{ disabled: !live }}
            on={{ click: () => entry.kind === "directory" ? actions.navigate(entry.path) : actions.select(entry.path) }}>
            <span attrs={{ "aria-hidden": "true" }}>{entry.kind === "directory" ? uiIcon("folder") : fileTypeIcon(entry.name)}</span>
            <span>{entry.name}</span>
            <small attrs={{ title: entry.kind === "directory" ? "Folder" : `${entry.size ?? "Unknown"} bytes${extensions.entryTitle ? " · " + extensions.entryTitle(entry.path) : ""}` }}>{entry.kind === "directory" ? "Folder" : fileSize(entry.size)}</small>
          </button>)}
          {state.listing?.entries.length === 0 ? <p>No eligible files in this folder.</p> : null}
        </div>
        {state.listing?.truncated === true ? <p attrs={{ class: "files-notice" }}>Showing the first 1,000 eligible entries. Narrow the folder selection.</p> : null}
        {exclusions === null ? null : <p attrs={{ class: "files-notice" }}>{exclusions}</p>}
      </div>
      {splitter()}
      <div attrs={{ class: "files-preview has-content-actions", ...(state.fullPreview ? { role: "dialog", "aria-modal": "true", "aria-label": state.selectedPath ?? "File preview" } : {}) }}>
        <div attrs={{ class: "files-path", title: "Path relative to the fixed launch directory" }}>
          <span>{state.selectedPath ?? "Select a file to view"}</span>
          {selectedSize === null ? null : <small attrs={{ title: `${selectedSize} bytes` }}>{fileSize(selectedSize)}{extensions.local ? "" : " · read-only"}</small>}
        </div>
        <div attrs={{ class: "files-content-actions files-actions", tabindex: "0", "aria-label": "Preview controls" }}
          on={{ keydown: (event: KeyboardEvent) => {
            if (!state.fullPreview || event.shiftKey || event.ctrlKey || event.metaKey) return;
            if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); actions.expandPreview(); }
            if (event.key === "ArrowLeft" || event.key === "ArrowRight") { event.preventDefault(); actions.nextPreview(event.key === "ArrowLeft" ? -1 : 1); }
          } }}>
        <button attrs={{ type: "button", "data-preview-expand": "", title: state.fullPreview ? "Restore preview" : "Expand preview to fill the window", "aria-pressed": String(state.fullPreview) }}
          on={{ click: () => { if (!extensions.expandRendered?.(state)) actions.expandPreview(); } }}>{uiIcon(state.fullPreview ? "restore" : "fullscreen")}{state.fullPreview ? "Restore" : "Expand"}</button>
        {!state.fullPreview ? null : <span attrs={{ class: "preview-navigation", tabindex: "0", title: "Swipe here or use arrow keys to change files" }}
          on={{ touchstart: (event: TouchEvent) => { const touch = event.touches[0]; if (touch) (event.currentTarget as HTMLElement).dataset["swipeX"] = String(touch.clientX); },
            touchend: (event: TouchEvent) => { const x = Number((event.currentTarget as HTMLElement).dataset["swipeX"]); const touch = event.changedTouches[0]; if (touch && Math.abs(touch.clientX - x) > 60) actions.nextPreview(touch.clientX < x ? 1 : -1); } }}>
          <button attrs={{ type: "button", title: "Previous file" }} on={{ click: () => actions.nextPreview(-1) }}>{uiIcon("previous")}Previous</button>
          <button attrs={{ type: "button", title: "Next file" }} on={{ click: () => actions.nextPreview(1) }}>{uiIcon("next")}Next</button>
        </span>}

        <button attrs={{ type: "button", title: "Download the selected file (up to 32 MiB)" }}
          props={{ disabled: !live || state.selectedPath === null || state.downloadBusy }}
          on={{ click: () => actions.download(false) }}>{uiIcon("download")}Download File</button>
          {extensions.contentActions?.(state) ?? null}
        </div>
        <div attrs={{ class: "files-editor-area" }}>
          {inlinePreview ?? (extensions.nativeText?.() ? <textarea key="native-preview" attrs={{ class: "files-native-text", readonly: true, spellcheck: "false", "aria-label": `File contents: ${state.selectedPath ?? ""}` }}
            props={{ value: state.preview?.content ?? "" }}></textarea>
            : <div key="monaco-host" attrs={{ id: "files-monaco", class: "files-monaco", "aria-label": "Read-only Monaco editor" }}></div>)}
          {state.preview !== null || inlinePreview !== null ? null : <p attrs={{ class: "files-preview-message", role: "status" }}>
            {state.previewBusy ? "Loading file…" : state.previewError ?? `UTF-8 previews up to ${(extensions.viewBytes ?? 2 * 1024 * 1024) / 1024 / 1024} MiB. File contents stay outside chat until you explicitly include them in a prompt.`}
          </p>}
        </div>
      </div>
      {!state.downloadBusy ? null : <div attrs={{ class: "files-download-status", role: "status" }}>Preparing download…</div>}
    </div>}
  </section>;
}
