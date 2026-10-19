import { h } from "../../web/lib/snabbdom/build/index.js";
import type { VNode } from "../../web/lib/snabbdom/build/index.js";
import { uiIcon } from "../../web/src/icons.js";
import type { BrowserFiles } from "./files.js";
import { FILE_BYTES, WORKSPACE_BYTES } from "./workspace.js";

export async function readUploads(files: readonly File[]): Promise<{ name: string; bytes: Uint8Array }[]> {
  if (files.length > 100) throw new Error("Choose up to 100 files.");
  let total = 0;
  for (const file of files) {
    if (file.size > FILE_BYTES) throw new Error(`${file.name} exceeds the 1 MiB per-file limit.`);
    total += file.size;
  }
  if (total > WORKSPACE_BYTES) throw new Error("The selected files exceed the 2 MiB workspace limit.");
  const result = [];
  for (const file of files) result.push({ name: file.name, bytes: new Uint8Array(await file.arrayBuffer()) });
  return result;
}

type Session = { id: string; busy: boolean };
export function createFileUpload(files: BrowserFiles, session: () => Session | null,
  updated: (message: string) => void): { render(directory: string): VNode; destroy(): void } {
  let target: { id: string; directory: string } | null = null;
  let uploading = false, destroyed = false;
  const receive = async (event: Event): Promise<void> => {
    const input = event.currentTarget as HTMLInputElement;
    const selected = [...(input.files ?? [])], destination = target;
    input.value = ""; target = null;
    if (!selected.length || uploading || destroyed || !destination) return;
    const current = (): boolean => !destroyed && session()?.id === destination.id && !session()?.busy;
    uploading = true; updated("Reading selected files…");
    try {
      if (!current()) throw new Error("Upload canceled: the session changed or a turn started. Choose the files again when idle.");
      const incoming = await readUploads(selected);
      if (!current()) throw new Error("Upload canceled: the session changed or a turn started. Choose the files again when idle.");
      const paths = files.importFiles(destination.directory, incoming);
      updated(`Uploaded ${paths.length} file${paths.length === 1 ? "" : "s"} to ${destination.directory || "session root"}. Available to Python under /home/pyodide. Existing names are kept; duplicates get numbered filenames.`);
    } catch (error) { if (!destroyed) updated(error instanceof Error ? error.message : String(error)); }
    finally { uploading = false; if (!destroyed) updated(""); }
  };
  return {
    render(directory): VNode {
      return h("span.browser-upload", [
        h("button", { attrs: { type: "button", "data-file-action": "upload",
          title: "Upload into this folder for Python: 1 MiB per file, 2 MiB total per session. Existing files are kept." },
          props: { disabled: uploading || !session() || session()?.busy }, on: { click: (event: Event) => {
            const current = session(); if (!current || current.busy || uploading || destroyed) return;
            target = { id: current.id, directory };
            (event.currentTarget as HTMLElement).parentElement?.querySelector<HTMLInputElement>("input[type=file]")?.click();
          } } }, [uiIcon("upload"), uploading ? "Uploading…" : "Upload"]),
        h("input", { attrs: { type: "file", hidden: true, multiple: true, "data-file-upload": "", "aria-label": "Upload session files" },
          on: { change: (event: Event) => { void receive(event); } } }),
      ]);
    },
    destroy(): void { destroyed = true; target = null; },
  };
}
