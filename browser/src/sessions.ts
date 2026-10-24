import { parseProviderTraces } from "./provider-trace.js";
import type { ProviderTraceSnapshot } from "./provider-trace.js";
import { parseTurnLimits, parseLoopMode } from "./turn-limits.js";
import type { TurnLimits } from "./turn-limits.js";
import { relayEndpoint } from "./network-route.js";
import type { Connection } from "./connection.js";
import { validateConnection, connectionMetadata } from "./connection.js";
import type { RenderBlockView } from "../../web/src/generated/contracts.js";
import { initialState } from "./local-state.js";
import { parseServerMessage } from "../../web/src/protocol.js";
import { WFE_PROTOCOL_VERSION } from "../../web/src/generated/contracts.js";
import { parseMetrics } from "./metrics.js";
import type { MetricsSnapshot } from "./metrics.js";

import { parseWorkspace } from "./workspace.js";
import type { Workspace } from "./workspace.js";

export type AgentMode = "general" | "python" | "dotnet-wasm";

export interface SavedSession {
  turnLimits?: TurnLimits;
  loopDetection?: string;
  id: string; name: string | null; label: string; updatedAt: string;
  blocks: RenderBlockView[]; messages: unknown[]; inputs: string[];
  system: string; connection: Omit<Connection, "apiKey"> | null; python: boolean; theme: string;
  mode?: AgentMode; networkAccess?: boolean; nugetAccess?: boolean; siteAccess?: boolean; dotnetRelayEndpoint?: string | null;
  metrics?: MetricsSnapshot;
  workspace?: Workspace;
  providerTrace?: ProviderTraceSnapshot;
}
export const SESSION_BYTES = 256 * 1024 * 1024;
export const SESSION_KEY = "lethetic.sessions.v1";
export const SESSION_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/u;
export type StorageAccess = Pick<Storage, "getItem" | "setItem"> & {
  /** Larger backing stores can opt out of the legacy localStorage quota. */
  sessionByteLimit?: number;
};

export class SessionStore {
  #memory: string | null = null;
  constructor(readonly storage?: StorageAccess) {}
  get byteLimit(): number { return this.storage ? this.storage.sessionByteLimit ?? 4 * 1024 * 1024 : SESSION_BYTES; }
  list(): SavedSession[] {
    const raw = this.storage ? this.storage.getItem(SESSION_KEY) : this.#memory;
    if (raw === null) return [];
    if (raw.length > this.byteLimit) throw new Error("Saved sessions exceed the storage limit.");
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
    if (new TextEncoder().encode(raw).byteLength > this.byteLimit) throw new Error(`Saved sessions exceed ${this.byteLimit / 1024 / 1024} MiB. Delete an older session to free space.`);
    this.storage?.setItem(SESSION_KEY, raw); this.#memory = raw;
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
  if (item.mode !== undefined && !["general", "python", "dotnet-wasm"].includes(item.mode)) throw new Error("Invalid saved mode.");
  if (item.siteAccess !== undefined && typeof item.siteAccess !== "boolean") throw new Error("Invalid saved interactive sites permission.");
  if (item.nugetAccess !== undefined && typeof item.nugetAccess !== "boolean") throw new Error("Invalid saved NuGet permission.");
  if (item.networkAccess !== undefined && typeof item.networkAccess !== "boolean") throw new Error("Invalid saved network permission.");
  const state = initialState(); state.blocks = { blocks: item.blocks, omitted_before: 0, truncated: false };
  if (!parseServerMessage(JSON.stringify({ type: "state_snapshot", snapshot: {
    protocol_version: WFE_PROTOCOL_VERSION, revision: 1, sequence: 1, state } })).ok) throw new Error("Invalid saved transcript.");
  let connection: SavedSession["connection"] = null;
  if (item.connection !== null) {
    const checked = validateConnection(item.connection);
    connection = connectionMetadata(checked);
  }
  return { id: item.id, name: item.name, label: item.label, updatedAt: item.updatedAt, blocks: item.blocks,
    messages: item.messages, inputs: item.inputs, system: item.system, python: item.python,
    ...(item.loopDetection !== undefined ? {loopDetection: parseLoopMode(item.loopDetection)} : {}),
    ...(item.turnLimits !== undefined ? {turnLimits: parseTurnLimits(item.turnLimits)} : {}),
    ...(item.mode !== undefined ? { mode: item.mode } : {}),
    ...(item.networkAccess !== undefined ? { networkAccess: item.networkAccess } : {}),
    ...(item.siteAccess !== undefined ? { siteAccess: item.siteAccess } : {}),
    ...(item.nugetAccess !== undefined ? { nugetAccess: item.nugetAccess } : {}), theme: item.theme, connection,
    ...(item.dotnetRelayEndpoint !== undefined ? { dotnetRelayEndpoint: item.dotnetRelayEndpoint === null ? null : relayEndpoint(item.dotnetRelayEndpoint) } : {}),
    ...(item.providerTrace !== undefined ? { providerTrace: parseProviderTraces(item.providerTrace) } : {}),
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
