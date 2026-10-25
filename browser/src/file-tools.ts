import { downloadBlob } from "../../web/src/download.js";
import { h } from "../../web/lib/snabbdom/build/index.js";
import { uiIcon } from "../../web/src/icons.js";
import type { FileListEntry } from "../../web/src/generated/contracts.js";
import type { BrowserFiles } from "./files.js";
import { type WorkspaceFile } from "./workspace.js";
import { workspaceZip } from "./file-zip.js";

export type FileSort = "name" | "date" | "size";
export function sortSessionFiles(files: readonly WorkspaceFile[], by: FileSort, descending = false): WorkspaceFile[] {
  const size = (data: string): number => data.length * 3 / 4 - (data.endsWith("==") ? 2 : data.endsWith("=") ? 1 : 0);
  return [...files].sort((a, b) => {
    const difference = by === "date" ? (a.modifiedAt ?? -1) - (b.modifiedAt ?? -1)
      : by === "size" ? size(a.data) - size(b.data) : a.path.localeCompare(b.path);
    return (descending ? -difference : difference) || a.path.localeCompare(b.path);
  });
}
export function createFileTools(files: BrowserFiles, changed: () => void) {
  let by: FileSort = "name", descending = false, error = "";
  const sortFiles = () => sortSessionFiles(files.snapshotWorkspace().files, by, descending);
  return {
    sortFiles,
    title(path: string): string { const file = files.get(path); return `${path}\nModified: ${file?.modifiedAt === undefined ? "Unknown" : new Date(file.modifiedAt).toLocaleString()}`; },
    sort(entries: readonly FileListEntry[]): FileListEntry[] {
      const order = new Map(sortFiles().map((file, index) => [file.path, index]));
      return [...entries].sort((a, b) => a.kind !== b.kind ? (a.kind === "directory" ? -1 : 1)
        : a.kind === "directory" ? a.name.localeCompare(b.name) : (order.get(a.path) ?? 0) - (order.get(b.path) ?? 0));
    },
    render() { return h("span.browser-file-actions", [
      h("label.files-sort", ["Sort ", h("select", { attrs: { "aria-label": "Sort session files" }, props: { value: by }, on: { change: (event: Event) => {
        by = (event.target as HTMLSelectElement).value as FileSort; descending = by !== "name"; changed();
      } } }, [h("option", { attrs: { value: "name" } }, "Name"), h("option", { attrs: { value: "date" } }, "Date"), h("option", { attrs: { value: "size" } }, "Size")])]),
      h("button", { attrs: { type: "button", title: "Reverse sort order", "aria-label": descending ? "Descending; reverse sort order" : "Ascending; reverse sort order" }, on: { click: () => { descending = !descending; changed(); } } }, [uiIcon(descending ? "expand" : "collapse"), descending ? "Descending" : "Ascending"]),
      h("button", { attrs: { type: "button", "data-file-action": "download-all", title: "Download every session file and folder as ZIP" }, on: { click: async () => {
        try {
          const saved = await downloadBlob(new Blob([workspaceZip(files.snapshotWorkspace())], { type: "application/zip" }), "session-files.zip");
          error = saved ? "Download ready." : "Download cancelled.";
        } catch (cause) { error = String(cause); } changed();
      } } }, [uiIcon("download"), "Download all (.zip)"]),
      h("span", { attrs: { role: "status" } }, error),
    ]); },
  };
}
