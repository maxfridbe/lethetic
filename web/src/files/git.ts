import type { GitChangedFile, GitChangeKind, GitDiffResponse, GitStatusResponse } from "../generated/contracts.js";
import { isFiniteInteger, isRecord } from "../safety.js";
import { FILE_VIEW_BYTES, validFilePath } from "./protocol.js";

const KINDS: readonly GitChangeKind[] = ["added", "modified", "deleted", "untracked"];
const encoder = new TextEncoder();

function exact(value: unknown, keys: readonly string[]): value is Record<string, unknown> {
  return isRecord(value) && Object.keys(value).length === keys.length &&
    keys.every((key) => Object.hasOwn(value, key));
}

function lineCount(value: unknown): value is number | null {
  return value === null || (isFiniteInteger(value) && value <= 0xffffffff);
}

function isChangedFile(value: unknown): value is GitChangedFile {
  return exact(value, ["path", "kind", "added", "removed"]) &&
    validFilePath(value["path"], false) &&
    typeof value["kind"] === "string" && (KINDS as readonly string[]).includes(value["kind"]) &&
    lineCount(value["added"]) && lineCount(value["removed"]);
}

export function parseGitStatus(value: unknown): GitStatusResponse | null {
  if (!exact(value, ["repository", "branch", "files", "truncated", "protected"]) ||
      typeof value["repository"] !== "boolean" || typeof value["truncated"] !== "boolean" ||
      !(value["branch"] === null || (typeof value["branch"] === "string" && value["branch"].length <= 256)) ||
      !isFiniteInteger(value["protected"]) || value["protected"] > 0xffffffff ||
      !Array.isArray(value["files"]) || value["files"].length > 1000 ||
      !value["files"].every(isChangedFile)) return null;
  const files: GitChangedFile[] = value["files"];
  if (new Set(files.map((file) => file.path)).size !== files.length) return null;
  return { repository: value["repository"], branch: value["branch"], files,
    truncated: value["truncated"], protected: value["protected"] };
}

export function parseGitDiff(value: unknown, path: string): GitDiffResponse | null {
  if (!exact(value, ["path", "original", "modified"]) || value["path"] !== path ||
      !validFilePath(path, false) || typeof value["original"] !== "string" ||
      typeof value["modified"] !== "string" ||
      encoder.encode(value["original"]).length > FILE_VIEW_BYTES ||
      encoder.encode(value["modified"]).length > FILE_VIEW_BYTES) return null;
  return { path, original: value["original"], modified: value["modified"] };
}

/** One row of the change tree: a folder with summed counts, or a changed file. */
export interface ChangeRow {
  readonly path: string;
  readonly name: string;
  readonly depth: number;
  readonly folder: boolean;
  readonly kind: GitChangeKind | null;
  /** Summed line counts; `null` when every file below is binary. */
  readonly added: number | null;
  readonly removed: number | null;
  /** Changed files at or below this row. */
  readonly files: number;
}

interface Folder {
  readonly folders: Map<string, Folder>;
  readonly files: GitChangedFile[];
}

function sum(left: number | null, right: number | null): number | null {
  return left === null ? right : right === null ? left : left + right;
}

/**
 * Flattens changed files into a depth-first tree. Folders come before files,
 * each group is sorted by name, and collapsed folders hide their contents.
 */
export function changeRows(files: readonly GitChangedFile[], collapsed: ReadonlySet<string>): ChangeRow[] {
  const root: Folder = { folders: new Map(), files: [] };
  for (const file of files) {
    const parts = file.path.split("/");
    let folder = root;
    for (const part of parts.slice(0, -1)) {
      let next = folder.folders.get(part);
      if (next === undefined) {
        next = { folders: new Map(), files: [] };
        folder.folders.set(part, next);
      }
      folder = next;
    }
    folder.files.push(file);
  }
  const rows: ChangeRow[] = [];
  const visit = (folder: Folder, prefix: string, depth: number): { added: number | null; removed: number | null; files: number } => {
    let added: number | null = null;
    let removed: number | null = null;
    let count = 0;
    for (const name of [...folder.folders.keys()].sort()) {
      const path = prefix === "" ? name : `${prefix}/${name}`;
      const index = rows.length;
      rows.push({ path, name, depth, folder: true, kind: null, added: null, removed: null, files: 0 });
      const hidden = collapsed.has(path);
      const mark = rows.length;
      const totals = visit(folder.folders.get(name) as Folder, path, depth + 1);
      if (hidden) rows.length = mark;
      rows[index] = { ...rows[index] as ChangeRow, ...totals };
      added = sum(added, totals.added);
      removed = sum(removed, totals.removed);
      count += totals.files;
    }
    for (const file of [...folder.files].sort((a, b) => a.path.localeCompare(b.path))) {
      rows.push({ path: file.path, name: file.path.split("/").at(-1) ?? file.path, depth,
        folder: false, kind: file.kind, added: file.added, removed: file.removed, files: 1 });
      added = sum(added, file.added);
      removed = sum(removed, file.removed);
      count += 1;
    }
    return { added, removed, files: count };
  };
  visit(root, "", 0);
  return rows;
}

export const CHANGE_LETTERS: Record<GitChangeKind, string> = {
  added: "A", modified: "M", deleted: "D", untracked: "U",
};
