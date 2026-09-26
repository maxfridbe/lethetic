import type {
  FileExclusionCounts,
  FileListEntry,
  FilesErrorCode,
  FilesListResponse,
  FilesReadResponse,
} from "../generated/contracts.js";
import { isFiniteInteger, isRecord } from "../safety.js";

export const FILE_VIEW_BYTES = 2 * 1024 * 1024;
export const FILE_DOWNLOAD_BYTES = 32 * 1024 * 1024;
export const FILE_ARCHIVE_BYTES = 64 * 1024 * 1024;
export const FILE_JSON_BYTES = 16 * 1024 * 1024;
export type FileEndpoint = "/api/files/list" | "/api/files/read" |
  "/api/files/download" | "/api/files/archive";

const encoder = new TextEncoder();

function exact(value: unknown, keys: readonly string[]): value is Record<string, unknown> {
  return isRecord(value) && Object.keys(value).length === keys.length &&
    keys.every((key) => Object.hasOwn(value, key));
}

export function validFilePath(value: unknown, allowRoot = true): value is string {
  if (typeof value !== "string" || encoder.encode(value).length > 4096) return false;
  if (value === "") return allowRoot;
  if (/[\\\p{Cc}]/u.test(value)) return false;
  const parts = value.split("/");
  return parts.length <= 64 && parts.every((part) =>
    part !== "" && part !== "." && part !== ".." && !/^[a-z]:/iu.test(part));
}

function byteSize(value: unknown): value is string {
  return typeof value === "string" && /^(0|[1-9][0-9]{0,19})$/u.test(value) &&
    BigInt(value) <= 18446744073709551615n;
}

export function isExclusions(value: unknown): value is FileExclusionCounts {
  return exact(value, ["protected", "unsupported", "unreadable"]) &&
    Object.values(value).every((count) => isFiniteInteger(count) && count <= 0xffffffff);
}

function isEntry(value: unknown, parent: string): value is FileListEntry {
  if (!exact(value, ["name", "path", "kind", "size"]) ||
      typeof value["name"] !== "string" || value["name"].includes("/") ||
      !validFilePath(value["name"], false) || !validFilePath(value["path"], false)) return false;
  if (value["path"] !== (parent === "" ? value["name"] : `${parent}/${value["name"]}`)) return false;
  return (value["kind"] === "directory" && value["size"] === null) ||
    (value["kind"] === "file" && byteSize(value["size"]));
}

export function parseFileList(value: unknown, path: string): FilesListResponse | null {
  if (!exact(value, ["path", "entries", "truncated", "exclusions"]) ||
      value["path"] !== path || !validFilePath(path) ||
      typeof value["truncated"] !== "boolean" || !isExclusions(value["exclusions"]) ||
      !Array.isArray(value["entries"]) || value["entries"].length > 1000 ||
      !value["entries"].every((entry) => isEntry(entry, path))) return null;
  const entries: FileListEntry[] = value["entries"];
  if (new Set(entries.map((entry) => entry.path)).size !== entries.length) return null;
  return { path, entries, truncated: value["truncated"], exclusions: value["exclusions"] };
}

export function parseFileRead(value: unknown, path: string): FilesReadResponse | null {
  if (!exact(value, ["path", "content", "size"]) || value["path"] !== path ||
      !validFilePath(path, false) || typeof value["content"] !== "string" ||
      !byteSize(value["size"])) return null;
  const bytes = encoder.encode(value["content"]).length;
  if (bytes > FILE_VIEW_BYTES || value["size"] !== String(bytes)) return null;
  return { path, content: value["content"], size: value["size"] };
}

const ERROR_LABELS: Record<FilesErrorCode, string> = {
  bad_request: "The file request was invalid.",
  not_found: "The selected item is no longer available. Refresh the folder.",
  protected: "This item is excluded by the file disclosure policy.",
  unsupported: "Only ordinary files and directories within the launch root are supported.",
  preview_unavailable: "Preview unavailable: select Download File for eligible binary or larger files.",
  too_large: "This operation exceeds the file service size or entry limit.",
  sensitive_content: "Content was refused because it matched the credential disclosure policy.",
  changed: "The selected item changed while being read. Refresh and try again.",
  busy: "The file service is busy. Try again after the current operation finishes.",
  deadline: "The file operation timed out.",
  cancelled: "The file operation was cancelled.",
  unavailable: "The file service is unavailable.",
};

export class FileServiceError extends Error {
  constructor(readonly code: FilesErrorCode) {
    super(ERROR_LABELS[code]);
    this.name = "FileServiceError";
  }
}

export function parseFileError(value: unknown): FileServiceError {
  if (exact(value, ["error"]) && exact(value["error"], ["code", "message", "retryable"])) {
    const code = value["error"]["code"];
    if (typeof code === "string" && Object.hasOwn(ERROR_LABELS, code) &&
        typeof value["error"]["message"] === "string" &&
        typeof value["error"]["retryable"] === "boolean") {
      return new FileServiceError(code as FilesErrorCode);
    }
  }
  return new FileServiceError("unavailable");
}

export function fileFailure(error: unknown): string {
  return error instanceof FileServiceError ? error.message : ERROR_LABELS.unavailable;
}

export function exclusionLabel(counts: FileExclusionCounts): string | null {
  const total = counts.protected + counts.unsupported + counts.unreadable;
  return total === 0 ? null : `Excluded: ${counts.protected} protected, ${counts.unsupported} unsupported, ${counts.unreadable} unreadable.`;
}
