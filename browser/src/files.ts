import type { FileEndpoint } from "../../web/src/files/protocol.js";
import { validFilePath } from "../../web/src/files/protocol.js";
import type { FileListEntry } from "../../web/src/generated/contracts.js";

const KEY = "lethetic.files.v1";
export interface LocalFile { path: string; content: string }
export class BrowserFiles {
  #memory: LocalFile[] = [];
  constructor(readonly storage?: Pick<Storage, "getItem" | "setItem">) {}
  list(): LocalFile[] {
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
    const files = this.list().filter((file) => file.path !== path);
    if (files.some((file) => file.path.startsWith(path + "/") || path.startsWith(file.path + "/"))) throw new Error("A file already uses part of this directory path.");
    files.push({ path, content });
    this.#write(files);
  }
  remove(path: string): void { this.#write(this.list().filter((file) => file.path !== path)); }
  #write(files: LocalFile[]): void {
    const raw = JSON.stringify({ version: 1, files });
    if (files.length > 100 || new TextEncoder().encode(raw).byteLength > 4 * 1024 * 1024) throw new Error("Local files exceed the 100 file / 4 MiB limit.");
    this.storage?.setItem(KEY, raw); this.#memory = files;
  }
  request(endpoint: FileEndpoint, path: string, signal: AbortSignal): Promise<Response> {
    signal.throwIfAborted();
    try {
      if (!validFilePath(path, endpoint === "/api/files/list")) throw new Error("Invalid path");
      const files = this.list();
      if (endpoint === "/api/files/list") {
        const entries = new Map<string, FileListEntry>();
        const prefix = path ? path + "/" : "";
        for (const file of files) {
          if (!file.path.startsWith(prefix)) continue;
          const remainder = file.path.slice(prefix.length); const name = remainder.split("/")[0]!;
          const directory = remainder.includes("/");
          entries.set(name, { name, path: prefix + name, kind: directory ? "directory" : "file",
            size: directory ? null : String(new TextEncoder().encode(file.content).byteLength) });
        }
        return Promise.resolve(Response.json({ path, entries: [...entries.values()].sort((a, b) => a.name.localeCompare(b.name)),
          truncated: false, exclusions: { protected: 0, unsupported: 0, unreadable: 0 } }));
      }
      const file = files.find((file) => file.path === path);
      if (!file) throw new Error("File not found");
      if (endpoint === "/api/files/read") return Promise.resolve(Response.json({ path, content: file.content, size: String(new TextEncoder().encode(file.content).byteLength) }));
      if (endpoint === "/api/files/download") {
        const filename = (path.split("/").at(-1) ?? "file.txt").replace(/[^a-zA-Z0-9._-]/gu, "_").slice(0, 90);
        return Promise.resolve(new Response(file.content, { headers: { "Content-Type": "text/plain; charset=utf-8",
          "Content-Disposition": `attachment; filename="${/^[a-zA-Z0-9_-]/u.test(filename) ? filename : "file" + filename}"` } }));
      }
      throw new Error("Unsupported endpoint");
    } catch { return Promise.resolve(Response.json({ error: { code: "unavailable", message: "Local file unavailable", retryable: false } }, { status: 400 })); }
  }
}
