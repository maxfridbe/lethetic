import type { FileEndpoint } from "../../web/src/files/protocol.js";
import { validFilePath } from "../../web/src/files/protocol.js";
import { decodeBytes, encodeBytes, emptyWorkspace, parseWorkspace, textFile, FILE_BYTES } from "./workspace.js";
import type { Workspace, WorkspaceFile } from "./workspace.js";
import type { FileListEntry } from "../../web/src/generated/contracts.js";

const KEY = "lethetic.files.v1";
export interface LocalFile { path: string; content: string | null }
export class BrowserFiles {
  #memory: LocalFile[] = [];
  #binding: { read: () => Workspace; write: (workspace: Workspace) => void } | null = null;
  bindWorkspace(read: () => Workspace, write: (workspace: Workspace) => void): void { this.#binding = { read, write }; }
  snapshotWorkspace(): Workspace {
    if (this.#binding) return this.#binding.read();
    return parseWorkspace({ ...emptyWorkspace(), files: this.list().map((file) => ({ path: file.path, data: encodeBytes(new TextEncoder().encode(file.content!)) })) });
  }
  get(path: string): WorkspaceFile | undefined { return this.snapshotWorkspace().files.find((file) => file.path === path); }
  constructor(readonly storage?: Pick<Storage, "getItem" | "setItem">) {}
  list(): LocalFile[] {
    if (this.#binding) return this.#binding.read().files.map((file) => ({ path: file.path, content: textFile(file) }));
    const raw = this.storage?.getItem(KEY);
    if (raw === undefined) return structuredClone(this.#memory);
    if (raw === null) return [];
    const record = JSON.parse(raw) as { version?: number; files?: LocalFile[] };
    if (raw.length > 4_500_000 || record.version !== 1 || !Array.isArray(record.files) || record.files.length > 100 ||
        record.files.some((file) => !file || !validFilePath(file.path, false) || typeof file.content !== "string" || new TextEncoder().encode(file.content).byteLength > 1_048_576)) throw new Error("Saved files are damaged or too large.");
    const paths = record.files.map((file) => file.path).sort();
    if (paths.some((path, index) => paths.some((other, otherIndex) => otherIndex !== index && (path === other || path.startsWith(other + "/"))))) throw new Error("Saved file paths conflict.");
    return record.files;
  }
  save(path: string, content: string): void {
    if (!validFilePath(path, false)) throw new Error("Use a relative path such as reports/analysis.py.");
    if (new TextEncoder().encode(content).byteLength > 1_048_576) throw new Error("Files are limited to 1 MiB each.");
    if (this.#binding) {
      const workspace = this.#binding.read();
      workspace.files = workspace.files.filter((file) => file.path !== path);
      workspace.files.push({ path, data: encodeBytes(new TextEncoder().encode(content)) });
      this.#binding.write(parseWorkspace(workspace)); return;
    }
    const files = this.list().filter((file) => file.path !== path);
    if (files.some((file) => file.path.startsWith(path + "/") || path.startsWith(file.path + "/"))) throw new Error("A file already uses part of this directory path.");
    files.push({ path, content });
    this.#write(files);
  }
  /** Commit a whole upload batch at once, preserving binary bytes and existing files. */
  importFiles(directory: string, incoming: readonly { name: string; bytes: Uint8Array }[]): string[] {
    if (!this.#binding) throw new Error("Upload requires an active session workspace.");
    if (!validFilePath(directory, true)) throw new Error("Invalid upload folder.");
    if (incoming.length > 100) throw new Error("Choose up to 100 files.");
    const workspace = structuredClone(this.#binding.read());
    const occupied = new Set([...workspace.files.map(file => file.path), ...workspace.directories]);
    const paths: string[] = [];
    for (const file of incoming) {
      if (file.bytes.length > FILE_BYTES) throw new Error(`${file.name} exceeds the 1 MiB per-file limit.`);
      if (file.name.includes("/") || !validFilePath(file.name, false)) throw new Error(`Invalid filename: ${file.name}`);
      const dot = file.name.lastIndexOf(".");
      const stem = dot > 0 ? file.name.slice(0, dot) : file.name;
      const extension = dot > 0 ? file.name.slice(dot) : "";
      const prefix = directory ? directory + "/" : "";
      let path = prefix + file.name, suffix = 2;
      while (occupied.has(path)) path = `${prefix}${stem} (${suffix++})${extension}`;
      occupied.add(path); paths.push(path);
      workspace.files.push({ path, data: encodeBytes(file.bytes) });
    }
    if (paths.length) this.#binding.write(parseWorkspace(workspace));
    return paths;
  }

  remove(path: string): void {
    if (this.#binding) {
      const workspace = this.#binding.read(); workspace.files = workspace.files.filter((file) => file.path !== path);
      this.#binding.write(workspace);
    } else this.#write(this.list().filter((file) => file.path !== path));
  }
  #write(files: LocalFile[]): void {
    const raw = JSON.stringify({ version: 1, files });
    if (files.length > 100 || new TextEncoder().encode(raw).byteLength > 4 * 1024 * 1024) throw new Error("Local files exceed the 100 file / 4 MiB limit.");
    this.storage?.setItem(KEY, raw); this.#memory = files;
  }
  request(endpoint: FileEndpoint, path: string, signal: AbortSignal): Promise<Response> {
    signal.throwIfAborted();
    try {
      if (!validFilePath(path, endpoint === "/api/files/list")) throw new Error("Invalid path");
      const workspace = this.snapshotWorkspace();
      const files = workspace.files;
      if (endpoint === "/api/files/list") {
        const entries = new Map<string, FileListEntry>();
        const prefix = path ? path + "/" : "";
        for (const file of [...files, ...workspace.directories.map((path) => ({ path: path + "/", data: "" }))]) {
          if (!file.path.startsWith(prefix)) continue;
          const remainder = file.path.slice(prefix.length); if (!remainder) continue; const name = remainder.split("/")[0]!;
          const directory = remainder.includes("/");
          entries.set(name, { name, path: prefix + name, kind: directory ? "directory" : "file",
            size: directory ? null : String(decodeBytes(file.data).byteLength) });
        }
        return Promise.resolve(Response.json({ path, entries: [...entries.values()].sort((a, b) => a.name.localeCompare(b.name)),
          truncated: false, exclusions: { protected: 0, unsupported: 0, unreadable: 0 } }));
      }
      const file = files.find((file) => file.path === path);
      if (!file) throw new Error("File not found");
      if (endpoint === "/api/files/read") {
        const content = textFile(file);
        if (content === null) return Promise.resolve(Response.json({ error: { code: "unsupported", message: "Binary file: use Preview for images or Download File.", retryable: false } }, { status: 415 }));
        return Promise.resolve(Response.json({ path, content, size: String(decodeBytes(file.data).byteLength) }));
      }
      if (endpoint === "/api/files/download") {
        const filename = (path.split("/").at(-1) ?? "file.txt").replace(/[^a-zA-Z0-9._-]/gu, "_").slice(0, 90);
        return Promise.resolve(new Response(decodeBytes(file.data), { headers: { "Content-Type": "application/octet-stream",
          "Content-Disposition": `attachment; filename="${/^[a-zA-Z0-9_-]/u.test(filename) ? filename : "file" + filename}"` } }));
      }
      throw new Error("Unsupported endpoint");
    } catch { return Promise.resolve(Response.json({ error: { code: "unavailable", message: "Local file unavailable", retryable: false } }, { status: 400 })); }
  }
}
