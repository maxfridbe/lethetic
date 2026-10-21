import { validFilePath } from "../../web/src/files/protocol.js";

export const WORKSPACE_ROOT = "/home/pyodide";
export const FILE_BYTES = 16 * 1024 * 1024;
export const WORKSPACE_BYTES = 64 * 1024 * 1024;
export interface WorkspaceFile { path: string; data: string; modifiedAt?: number }
export interface Workspace { version: 1; directories: string[]; files: WorkspaceFile[] }
export const emptyWorkspace = (): Workspace => ({ version: 1, directories: [], files: [] });
export function encodeBytes(bytes: Uint8Array): string {
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 8192) binary += String.fromCharCode(...bytes.subarray(offset, offset + 8192));
  return btoa(binary);
}
export function decodeBytes(data: string): Uint8Array<ArrayBuffer> {
  const binary = atob(data), bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}
/** Validate canonical base64 without allocating another full file to check its size. */
export function encodedByteLength(data: string): number {
  if (data.length % 4 || /[^A-Za-z0-9+/=]/u.test(data)) throw new Error("Invalid workspace encoding.");
  const padding = data.endsWith("==") ? 2 : data.endsWith("=") ? 1 : 0;
  const body = data.length - padding;
  if (data.indexOf("=") !== (padding ? body : -1)) throw new Error("Invalid workspace encoding.");
  const last = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/".indexOf(data[body - 1] ?? "A");
  if ((padding === 2 && (last & 15)) || (padding === 1 && (last & 3))) throw new Error("Invalid workspace encoding.");
  return data.length / 4 * 3 - padding;
}
export function textFile(file: WorkspaceFile): string | null {
  try {
    const text = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(decodeBytes(file.data));
    return text.includes("\0") ? null : text;
  } catch { return null; }
}

/** Canonical, bounded snapshots shared by persistence, the worker and the Files UI. */
export function parseWorkspace(value: unknown): Workspace {
  if (!value || typeof value !== "object") throw new Error("Invalid session workspace.");
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
        file.data.length > Math.ceil(FILE_BYTES / 3) * 4 || /[^A-Za-z0-9+/=]/u.test(file.data)) throw new Error("Invalid workspace file or file exceeds 16 MiB.");
    const bytes = encodedByteLength(file.data);
    if (bytes > FILE_BYTES) throw new Error("Invalid workspace encoding or file exceeds 16 MiB.");
    total += bytes; paths.add(file.path); parents(file.path);
    if (file.modifiedAt !== undefined && (!Number.isSafeInteger(file.modifiedAt) || file.modifiedAt < 0 || file.modifiedAt > 8_640_000_000_000_000)) throw new Error("Invalid file date.");
    return { path: file.path, data: file.data, ...(file.modifiedAt === undefined ? {} : { modifiedAt: file.modifiedAt }) };
  });
  if (directories.size > 256 || total > WORKSPACE_BYTES) throw new Error("Workspace exceeds 256 folders / 64 MiB.");
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

/** File-level invalidation keeps unrelated previews, focus and playback intact. */
export function changedWorkspacePaths(previous: Workspace, next: Workspace): string[] {
  const before = new Map(previous.files.map(file => [file.path, file.data]));
  const changed = new Set<string>();
  for (const file of next.files) {
    if (before.get(file.path) !== file.data) changed.add(file.path);
    before.delete(file.path);
  }
  for (const path of before.keys()) changed.add(path);
  const oldDirs = new Set(previous.directories), newDirs = new Set(next.directories);
  for (const path of oldDirs) if (!newDirs.has(path)) changed.add(path);
  for (const path of newDirs) if (!oldDirs.has(path)) changed.add(path);
  return [...changed];
}
