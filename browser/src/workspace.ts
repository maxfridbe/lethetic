import { validFilePath } from "../../web/src/files/protocol.js";

export const WORKSPACE_ROOT = "/home/pyodide";
export const FILE_BYTES = 1024 * 1024;
export const WORKSPACE_BYTES = 2 * 1024 * 1024;
export interface WorkspaceFile { path: string; data: string; modifiedAt?: number }
export interface Workspace { version: 1; directories: string[]; files: WorkspaceFile[] }
export const emptyWorkspace = (): Workspace => ({ version: 1, directories: [], files: [] });
export function encodeBytes(bytes: Uint8Array): string {
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 8192) binary += String.fromCharCode(...bytes.subarray(offset, offset + 8192));
  return btoa(binary);
}
export function decodeBytes(data: string): Uint8Array<ArrayBuffer> { return Uint8Array.from(atob(data), (char) => char.charCodeAt(0)); }
export function textFile(file: WorkspaceFile): string | null {
  try {
    const text = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(decodeBytes(file.data));
    return text.includes("\0") ? null : text;
  } catch { return null; }
}

/** Canonical, bounded snapshots shared by persistence, the worker and the Files UI. */
export function parseWorkspace(value: unknown): Workspace {
  if (!value || typeof value !== "object") throw new Error("Invalid Python workspace.");
  const item = value as Workspace;
  if (item.version !== 1 || !Array.isArray(item.files) || item.files.length > 100 ||
      !Array.isArray(item.directories) || item.directories.length > 256) throw new Error("Workspace exceeds 100 files / 256 folders.");
  const directories = new Set<string>(); const paths = new Set<string>(); let total = 0;
  const parents = (path: string): void => {
    const parts = path.split("/"); parts.pop();
    while (parts.length) { directories.add(parts.join("/")); parts.pop(); }
  };
  for (const dir of item.directories) {
    if (!validFilePath(dir, false) || directories.has(dir)) throw new Error("Invalid or duplicate workspace folder.");
    directories.add(dir);
  }
  for (const dir of item.directories) parents(dir);
  const files = item.files.map((file) => {
    if (!file || !validFilePath(file.path, false) || paths.has(file.path) || typeof file.data !== "string" ||
        file.data.length > Math.ceil(FILE_BYTES / 3) * 4 || /[^A-Za-z0-9+/=]/u.test(file.data)) throw new Error("Invalid workspace file or file exceeds 1 MiB.");
    const bytes = decodeBytes(file.data);
    if (bytes.length > FILE_BYTES || encodeBytes(bytes) !== file.data) throw new Error("Invalid workspace encoding or file exceeds 1 MiB.");
    total += bytes.length; paths.add(file.path); parents(file.path);
    if (file.modifiedAt !== undefined && (!Number.isSafeInteger(file.modifiedAt) || file.modifiedAt < 0 || file.modifiedAt > 8_640_000_000_000_000)) throw new Error("Invalid file date.");
    return { path: file.path, data: file.data, ...(file.modifiedAt === undefined ? {} : { modifiedAt: file.modifiedAt }) };
  });
  if (directories.size > 256 || total > WORKSPACE_BYTES) throw new Error("Workspace exceeds 256 folders / 2 MiB.");
  if ([...paths].some((path) => directories.has(path))) throw new Error("A workspace file conflicts with a folder.");
  return { version: 1, directories: [...directories].sort(), files: files.sort((a, b) => a.path.localeCompare(b.path)) };
}

/** Preserve dates across worker restores; only changed/new bytes get a new date. */
export function dateWorkspace(value: Workspace, previous: Workspace, now = Date.now()): Workspace {
  const next = parseWorkspace(value);
  const before = new Map(previous.files.map(file => [file.path, file]));
  next.files = next.files.map(file => {
    const old = before.get(file.path);
    const modifiedAt = old?.data === file.data ? old.modifiedAt : now;
    return { path: file.path, data: file.data, ...(modifiedAt === undefined ? {} : { modifiedAt }) };
  });
  return next;
}
