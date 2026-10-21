import { WORKSPACE_ROOT, FILE_BYTES, WORKSPACE_BYTES, decodeBytes, encodeBytes, parseWorkspace } from "./workspace.js";
import type { Workspace } from "./workspace.js";

/** The subset of Emscripten FS exposed by Pyodide; lstat never follows links. */
export interface PythonFS {
  lstat(path: string): { mode: number; size: number };
  isDir(mode: number): boolean;
  isFile(mode: number): boolean;
  readdir(path: string): string[];
  readFile(path: string): Uint8Array;
  writeFile(path: string, bytes: Uint8Array): void;
  mkdirTree(path: string): void;
  unlink(path: string): void;
  rmdir(path: string): void;
  chdir(path: string): void;
}

export function captureWorkspace(fs: PythonFS): Workspace {
  if (!fs.isDir(fs.lstat("/home").mode)) throw new Error("The workspace parent must be a real directory.");
  const snapshot: Workspace = { version: 1, directories: [], files: [] };
  let bytes = 0;
  const visit = (relative: string, depth: number): void => {
    if (depth > 64) throw new Error("Workspace folders are too deeply nested.");
    const path = relative ? `${WORKSPACE_ROOT}/${relative}` : WORKSPACE_ROOT;
    const stat = fs.lstat(path);
    if (fs.isDir(stat.mode)) {
      if (relative) snapshot.directories.push(relative);
      if (snapshot.directories.length > 256) throw new Error("Workspace exceeds 256 folders.");
      for (const name of fs.readdir(path)) if (name !== "." && name !== "..") visit(relative ? `${relative}/${name}` : name, depth + 1);
    } else if (relative && fs.isFile(stat.mode)) {
      if (stat.size > FILE_BYTES) throw new Error(`${relative} exceeds 16 MiB; workspace was not synced.`);
      bytes += stat.size;
      if (bytes > WORKSPACE_BYTES || snapshot.files.length >= 100) throw new Error("Workspace exceeds 100 files / 64 MiB; workspace was not synced.");
      snapshot.files.push({ path: relative, data: encodeBytes(fs.readFile(path)) });
    } else throw new Error(`Cannot sync symbolic links or special files: ${relative || WORKSPACE_ROOT}`);
  };
  visit("", 0); return parseWorkspace(snapshot);
}

/** Replace only the session workspace; never traverse symlinks or touch runtime packages. */
export function restoreWorkspace(fs: PythonFS, value: Workspace): void {
  const snapshot = parseWorkspace(value);
  if (!fs.isDir(fs.lstat("/home").mode)) throw new Error("The workspace parent must be a real directory.");
  // Keep the cwd outside the directory while replacing its contents.
  fs.chdir("/");
  const remove = (path: string, depth: number): void => {
    if (depth > 65) throw new Error("Workspace folders are too deeply nested to replace.");
    if (fs.isDir(fs.lstat(path).mode)) {
      for (const name of fs.readdir(path)) if (name !== "." && name !== "..") remove(`${path}/${name}`, depth + 1);
      fs.rmdir(path);
    } else fs.unlink(path);
  };
  let exists = true;
  try { fs.lstat(WORKSPACE_ROOT); } catch { exists = false; }
  if (exists) remove(WORKSPACE_ROOT, 0);
  fs.mkdirTree(WORKSPACE_ROOT);
  for (const dir of snapshot.directories) fs.mkdirTree(`${WORKSPACE_ROOT}/${dir}`);
  for (const file of snapshot.files) fs.writeFile(`${WORKSPACE_ROOT}/${file.path}`, decodeBytes(file.data));
  fs.chdir(WORKSPACE_ROOT);
}
