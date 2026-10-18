import type { Connection } from "./connection.js";
import { validateConnection } from "./connection.js";
import type { RenderBlockView } from "../../web/src/generated/contracts.js";
import { initialState } from "./local-state.js";
import { parseServerMessage } from "../../web/src/protocol.js";
import { WFE_PROTOCOL_VERSION } from "../../web/src/generated/contracts.js";
import { parseMetrics } from "./metrics.js";
import type { MetricsSnapshot } from "./metrics.js";

import { parseWorkspace } from "./workspace.js";
import type { Workspace } from "./workspace.js";

export interface SavedSession {
  id: string; name: string | null; label: string; updatedAt: string;
  blocks: RenderBlockView[]; messages: unknown[]; inputs: string[];
  system: string; connection: Omit<Connection, "apiKey"> | null; python: boolean; theme: string;
  metrics?: MetricsSnapshot;
  workspace?: Workspace;
}
const KEY = "lethetic.sessions.v1";
export const SESSION_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/u;
type StorageAccess = Pick<Storage, "getItem" | "setItem">;

export class SessionStore {
  #memory: string | null = null;
  constructor(readonly storage?: StorageAccess) {}
  list(): SavedSession[] {
    const raw = this.storage ? this.storage.getItem(KEY) : this.#memory;
    if (raw === null) return [];
    if (raw.length > 4_500_000) throw new Error("Saved sessions exceed the storage limit.");
    const value = JSON.parse(raw) as { version?: unknown; sessions?: unknown };
    if (value.version !== 1 || !Array.isArray(value.sessions) || value.sessions.length > 50) throw new Error("Unsupported or damaged saved sessions.");
    const sessions = value.sessions.map(parseSession);
    if (new Set(sessions.map((session) => session.id)).size !== sessions.length) throw new Error("Duplicate saved session IDs.");
    return sessions.sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
  }
  save(session: SavedSession): void {
    const sessions = this.list().filter((item) => item.id !== session.id);
    sessions.unshift(parseSession(session)); this.#write(sessions);
  }
  remove(id: string): void { this.#write(this.list().filter((session) => session.id !== id)); }
  clear(): void { this.#write([]); }
  #write(sessions: SavedSession[]): void {
    if (sessions.length > 50) throw new Error("Session limit reached (50). Delete an older session to save this one.");
    const raw = JSON.stringify({ version: 1, sessions });
    if (new TextEncoder().encode(raw).byteLength > 4 * 1024 * 1024) throw new Error("Saved sessions exceed 4 MiB. Delete an older session to free space.");
    this.storage?.setItem(KEY, raw); this.#memory = raw;
  }
}

function parseSession(value: unknown): SavedSession {
  if (!value || typeof value !== "object") throw new Error("Invalid saved session.");
  const item = value as SavedSession;
  if (typeof item.id !== "string" || !SESSION_ID.test(item.id) ||
      (item.name !== null && (typeof item.name !== "string" || item.name.length > 255)) ||
      typeof item.label !== "string" || item.label.length > 200 ||
      typeof item.updatedAt !== "string" || !Number.isFinite(Date.parse(item.updatedAt)) ||
      !Array.isArray(item.messages) || item.messages.length > 512 || !Array.isArray(item.blocks) ||
      !Array.isArray(item.inputs) || !item.inputs.every((input) => typeof input === "string") ||
      typeof item.system !== "string" || typeof item.python !== "boolean" || typeof item.theme !== "string") {
    throw new Error("Invalid saved session.");
  }
  const state = initialState(); state.blocks = { blocks: item.blocks, omitted_before: 0, truncated: false };
  if (!parseServerMessage(JSON.stringify({ type: "state_snapshot", snapshot: {
    protocol_version: WFE_PROTOCOL_VERSION, revision: 1, sequence: 1, state } })).ok) throw new Error("Invalid saved transcript.");
  let connection: SavedSession["connection"] = null;
  if (item.connection !== null) {
    const checked = validateConnection(item.connection);
    connection = { name: checked.name, provider: checked.provider, endpoint: checked.endpoint, model: checked.model };
  }
  return { id: item.id, name: item.name, label: item.label, updatedAt: item.updatedAt, blocks: item.blocks,
    messages: item.messages, inputs: item.inputs, system: item.system, python: item.python, theme: item.theme, connection,
    ...(item.workspace !== undefined ? { workspace: parseWorkspace(item.workspace) } : {}),
    ...(item.metrics ? { metrics: parseMetrics(item.metrics) } : {}) };
}

/** Hash paths work on GitHub Pages without a server-side SPA rewrite. */
export function sessionRoute(hash: string): string {
  if (!hash || hash === "#/latest") return "latest";
  if (hash === "#/new") return "new";
  const id = hash.match(/^#\/sessions\/([^/]+)$/u)?.[1];
  if (id && SESSION_ID.test(id)) return id;
  throw new Error("Unknown session URL. Use #/latest, #/new, or #/sessions/<id>.");
}
