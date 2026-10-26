import { contentDatabaseFactory } from "./sdk.js";
import type { WasmDatabase } from "./sdk.js";
import { SESSION_KEY, SESSION_BYTES, SessionStore } from "./sessions.js";
import type { SavedSession } from "./sessions.js";
import { decodeBytes, encodeBytes, FILE_BYTES } from "./workspace.js";


type BinarySession = Omit<SavedSession, "workspace"> & {
  workspace?: { version: 1; directories: string[]; files: { path: string; data: Uint8Array; modifiedAt?: number }[] };
};
type RecordData = { revision: number; ids?: string[]; database?: Uint8Array; sessions?: BinarySession[] };
type SessionImage = {sessionId: string; database: Uint8Array};
type DatabaseFactory = new (bytes: Uint8Array) => WasmDatabase;
const textEncoder = new TextEncoder(), textDecoder = new TextDecoder("utf-8", {fatal: true});

function databaseBytes(sessions: BinarySession[], Database: DatabaseFactory): Uint8Array {
  const db = new Database(new Uint8Array());
  try {
    for (const session of sessions) {
      const metadata = {...session, ...(session.workspace ? {workspace: {...session.workspace,
        files: session.workspace.files.map(({data: _data, ...file}) => file)}} : {})};
      db.put("browser/sessions", session.id, textEncoder.encode(JSON.stringify(metadata)));
      for (const file of session.workspace?.files ?? []) db.put(`browser/files/${session.id}`, file.path, file.data);
    }
    return db.export();
  } finally { db.free(); }
}

function databaseSessions(bytes: Uint8Array, Database: DatabaseFactory): BinarySession[] {
  if (!(bytes instanceof Uint8Array)) throw new Error("Damaged Turso database image.");
  const db = new Database(bytes);
  try {
    const ids: string[] = JSON.parse(db.names("browser/sessions"));
    if (ids.length > 50) throw new Error("Damaged session database.");
    return ids.map(id => {
      const data = db.read("browser/sessions", id);
      if (!data) throw new Error("Missing session record.");
      const session = JSON.parse(textDecoder.decode(data)) as BinarySession;
      if (session.id !== id) throw new Error("Session identity mismatch.");
      if (session.workspace) session.workspace.files = session.workspace.files.map(file => {
        const data = db.read(`browser/files/${id}`, file.path);
        if (!data) throw new Error("Missing session file.");
        return {...file, data};
      });
      return session;
    });
  } finally { db.free(); }
}

function toBinary(raw: string): BinarySession[] {
  const store = new SessionStore({ sessionByteLimit: SESSION_BYTES, getItem: () => raw, setItem() {} });
  return store.list().map(session => ({ ...session, ...(session.workspace ? {
    workspace: { ...session.workspace, files: session.workspace.files.map(file => ({ ...file, data: decodeBytes(file.data) })) },
  } : {}) })) as BinarySession[];
}
function fromBinary(sessions: BinarySession[]): string {
  if (!Array.isArray(sessions) || sessions.length > 50) throw new Error("Damaged session database.");
  const raw = JSON.stringify({ version: 1, sessions: sessions.map(session => ({ ...session, ...(session.workspace ? {
    workspace: { ...session.workspace, files: session.workspace.files.map(file => {
      if (!(file.data instanceof Uint8Array) || file.data.byteLength > FILE_BYTES) throw new Error("Damaged session file.");
      return { ...file, data: encodeBytes(file.data) };
    }) },
  } : {}) })) });
  new SessionStore({ sessionByteLimit: SESSION_BYTES, getItem: () => raw, setItem() {} }).list();
  return raw;
}

/** Synchronous UI cache, with explicit asynchronous durability. Files are raw bytes
 * in a separate embedded Turso database image per session, saved atomically in IndexedDB; base64
 * exists only in compatible runtime/export snapshots.
 * A failed transaction leaves the previous database intact and edits in memory.
 */
export class BrowserSessionStorage implements Storage {
  readonly sessionByteLimit = SESSION_BYTES;
  #raw: string | null;
  #committed: string | null;
  #revision: number;
  #running: Promise<void> | null = null;
  #error: Error | null = null;
  #closed = false;
  #listeners = new Set<() => void>();
  constructor(private readonly preferences: Storage, private readonly db: IDBDatabase, record: RecordData | undefined, private readonly Database: DatabaseFactory, sessions?: BinarySession[]) {
    if (record && (!Number.isSafeInteger(record.revision) || record.revision < 1)) throw new Error("Damaged session database revision.");
    this.#revision = record?.revision ?? 0;
    this.#raw = this.#committed = record ? fromBinary(sessions!) : null;
    db.onversionchange = () => { db.close(); this.#error = new Error("Session database changed. Export unsaved sessions and reload this tab."); this.#emit(); };
  }
  get pending(): boolean { return this.#raw !== this.#committed; }
  get status(): string {
    if (this.#error) return `Session not saved: ${this.#error.message} Edits remain in this tab; export them before closing, or retry saving.`;
    return this.pending ? "Saving chats and files to this device…" : "Each session and its files are saved in their own local Turso SQLite database (unencrypted). API keys are excluded. Files sync with the selected code runtime; variables reset on reload.";
  }
  subscribe(listener: () => void): () => void { this.#listeners.add(listener); return () => this.#listeners.delete(listener); }
  #emit(): void { for (const listener of this.#listeners) listener(); }
  get length(): number { return this.#keys().length; }
  #keys(): string[] { return [...new Set([...Array.from({ length: this.preferences.length }, (_, i) => this.preferences.key(i)!).filter(Boolean), ...(this.#raw === null ? [] : [SESSION_KEY])])]; }
  key(index: number): string | null { return this.#keys()[index] ?? null; }
  getItem(key: string): string | null { return key === SESSION_KEY ? this.#raw : this.preferences.getItem(key); }
  setItem(key: string, value: string): void {
    if (this.#closed) throw new Error("Session storage is closed.");
    if (key !== SESSION_KEY) { this.preferences.setItem(key, value); return; }
    if (new TextEncoder().encode(value).byteLength > SESSION_BYTES) throw new Error("Saved sessions exceed 256 MiB. Delete older sessions to free space.");
    this.#raw = value;
    void this.flush().catch(() => { /* Exposed through status, pending and flush(). */ });
  }
  removeItem(key: string): void { if (key === SESSION_KEY) this.setItem(key, '{"version":1,"sessions":[]}'); else this.preferences.removeItem(key); }
  clear(): void { this.removeItem(SESSION_KEY); this.preferences.clear(); }
  /** Resolves only after the latest queued state commits. Also retries a failed save. */
  async flush(): Promise<void> {
    if (this.#closed) throw new Error("Session storage is closed.");
    if (!this.#running) {
      this.#error = null;
      this.#running = Promise.resolve().then(async () => {
        while (this.pending) {
          const raw = this.#raw!;
          await this.#write(raw);
          this.#committed = raw;
        }
      }).catch((error: unknown) => { this.#error = error instanceof Error ? error : new Error(String(error)); throw this.#error; })
        .finally(() => { this.#running = null; this.#emit(); });
      this.#emit();
    }
    await this.#running;
  }
  #write(raw: string, migration = false): Promise<void> {
    const list = (value: string): SavedSession[] => new SessionStore({sessionByteLimit: SESSION_BYTES, getItem: () => value, setItem() {}}).list();
    const sessions = list(raw), ids = sessions.map(session => session.id);
    const previous = new Map((!migration && this.#committed ? list(this.#committed) : []).map(session => [session.id, JSON.stringify(session)]));
    // Build only changed session databases, before starting the IndexedDB
    // transaction. Unrelated sessions keep their existing image bytes.
    const images: SessionImage[] = sessions.filter(session => previous.get(session.id) !== JSON.stringify(session)).map(session => ({
      sessionId: session.id,
      database: databaseBytes(toBinary(JSON.stringify({version: 1, sessions: [session]})), this.Database),
    }));
    return new Promise((resolve, reject) => {
      const transaction = this.db.transaction("sessions", "readwrite");
      const store = transaction.objectStore("sessions");
      let failure: Error | undefined;
      const read = store.get("all");
      read.onsuccess = () => {
        const current = read.result as RecordData | undefined;
        if ((current?.revision ?? 0) !== this.#revision) {
          failure = new Error("Another tab saved sessions. Export this tab's edits before reloading to avoid overwriting them.");
          transaction.abort(); return;
        }
        try {
          for (const id of current?.ids ?? []) if (!ids.includes(id)) store.delete(`session:${id}`);
          for (const image of images) store.put(image, `session:${image.sessionId}`);
          store.put({ revision: this.#revision + 1, ids } satisfies RecordData, "all");
        } catch (error) { failure = error instanceof Error ? error : new Error(String(error)); transaction.abort(); }
      };
      transaction.oncomplete = () => { this.#revision++; resolve(); };
      transaction.onabort = () => reject(failure ?? transaction.error ?? new Error("Session save was aborted."));
      transaction.onerror = () => { /* onabort reports the transaction failure. */ };
    });
  }
  async migrate(): Promise<void> {
    if (this.#raw === null) return;
    await this.#write(this.#raw, true);
  }
  /** One portable SQLite database per session; keys and preferences are excluded. */
  exportDatabase(sessionId: string): Uint8Array {
    const selected = (this.#raw === null ? [] : toBinary(this.#raw)).filter(session => session.id === sessionId);
    if (selected.length !== 1) throw new Error("Session no longer exists.");
    return databaseBytes(selected, this.Database);
  }
  close(): void {
    if (this.pending || this.#running) throw new Error("Wait for session storage.flush() before closing.");
    this.#closed = true; this.db.close(); this.#listeners.clear();
  }
}

/** Await before mounting. Never fall back to an empty session if loading fails.
 * The legacy localStorage record is retained until migration commits successfully.
 */
export async function openBrowserStorage(preferences: Storage, databaseName = "lethetic.sessions.v2"): Promise<BrowserSessionStorage> {
  const Database = await contentDatabaseFactory();
  const db = await new Promise<IDBDatabase>((resolve, reject) => {
    const request = indexedDB.open(databaseName, 1);
    let blocked = false;
    request.onupgradeneeded = () => request.result.createObjectStore("sessions");
    request.onsuccess = () => { if (blocked) request.result.close(); else resolve(request.result); };
    request.onerror = () => reject(request.error);
    request.onblocked = () => { blocked = true; reject(new Error("Session database upgrade is blocked by another tab. Close that tab and retry.")); };
  });
  try {
    const {record, images} = await new Promise<{record: RecordData | undefined; images: SessionImage[]}>((resolve, reject) => {
      const tx = db.transaction("sessions", "readonly"), store = tx.objectStore("sessions");
      const read = store.get("all"), all = store.getAll();
      tx.oncomplete = () => resolve({record: read.result as RecordData | undefined,
        images: (all.result as SessionImage[]).filter(value => typeof value.sessionId === "string")});
      tx.onabort = () => reject(tx.error);
    });
    let sessions: BinarySession[] | undefined;
    if (record?.ids !== undefined) {
      if (!Array.isArray(record.ids) || record.ids.length > 50 || new Set(record.ids).size !== record.ids.length) throw new Error("Damaged session catalog.");
      sessions = record.ids.map(id => {
        const image = images.find(image => image.sessionId === id);
        if (!image) throw new Error("Missing session database.");
        const contents = databaseSessions(image.database, Database);
        if (contents.length !== 1 || contents[0]!.id !== id) throw new Error("Session database identity mismatch.");
        return contents[0]!;
      });
    } else if (record) {
      sessions = record.database ? databaseSessions(record.database, Database) : record.sessions;
    }
    const storage = new BrowserSessionStorage(preferences, db, record, Database, sessions);
    const schemaUpgrade = images.some(image => image.database.byteLength >= 100 &&
      new DataView(image.database.buffer, image.database.byteOffset, image.database.byteLength).getUint32(60) < 4);
    if (record && (record.ids === undefined || schemaUpgrade)) {
      // Split the previous aggregate record in one transaction. Its source is
      // replaced only when every individual database has committed.
      await storage.migrate();
    }
    const legacy = preferences.getItem(SESSION_KEY);
    if (!record && legacy !== null) {
      // Validate before copying; a corrupt record must not be silently replaced.
      new SessionStore(preferences).list();
      storage.setItem(SESSION_KEY, legacy); await storage.flush();
    }
    if (!record && legacy !== null) preferences.removeItem(SESSION_KEY);
    return storage;
  } catch (error) { db.close(); throw error; }
}
