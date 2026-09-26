import { jsx as h } from "../../lib/snabbdom/build/index.js";
import type { VNode } from "../../lib/snabbdom/build/index.js";
import type { FilePaneState } from "./state.js";
import { exclusionLabel } from "./protocol.js";

export interface FilePaneActions {
  readonly toggle: () => void;
  readonly navigate: (path: string) => void;
  readonly select: (path: string) => void;
  readonly refresh: () => void;
  readonly download: (archive: boolean) => void;
  readonly copyPath: () => void;
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
      <span attrs={{ class: "files-scope" }}>Read-only · fixed launch root</span>
      {!state.open ? null : <div attrs={{ class: "files-actions" }}>
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
    {!state.open ? null : <div attrs={{ id: "files-body", class: "files-body" }}>
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
