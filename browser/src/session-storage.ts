import { SESSION_KEY, SESSION_BYTES, SessionStore } from "./sessions.js";
import type { SavedSession } from "./sessions.js";
import { decodeBytes, encodeBytes, FILE_BYTES } from "./workspace.js";


type BinarySession = Omit<SavedSession, "workspace"> & {
  workspace?: { version: 1; directories: string[]; files: { path: string; data: Uint8Array; modifiedAt?: number }[] };
};
type RecordData = { revision: number; sessions: BinarySession[] };

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
 * in IndexedDB; base64 exists only in the compatible runtime/export snapshots.
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
  constructor(private readonly preferences: Storage, private readonly db: IDBDatabase, record: RecordData | undefined) {
    if (record && (!Number.isSafeInteger(record.revision) || record.revision < 1)) throw new Error("Damaged session database revision.");
    this.#revision = record?.revision ?? 0;
    this.#raw = this.#committed = record ? fromBinary(record.sessions) : null;
    db.onversionchange = () => { db.close(); this.#error = new Error("Session database changed. Export unsaved sessions and reload this tab."); this.#emit(); };
  }
  get pending(): boolean { return this.#raw !== this.#committed; }
  get status(): string {
    if (this.#error) return `Session not saved: ${this.#error.message} Edits remain in this tab; export them before closing, or retry saving.`;
    return this.pending ? "Saving chats and files to this device…" : "Chats and files saved unencrypted in IndexedDB. API keys are excluded. Files sync with the selected code runtime; variables reset on reload.";
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
          await this.#write(toBinary(raw));
          this.#committed = raw;
        }
      }).catch((error: unknown) => { this.#error = error instanceof Error ? error : new Error(String(error)); throw this.#error; })
        .finally(() => { this.#running = null; this.#emit(); });
      this.#emit();
    }
    await this.#running;
  }
  #write(sessions: BinarySession[]): Promise<void> {
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
        try { store.put({ revision: this.#revision + 1, sessions } satisfies RecordData, "all"); }
        catch (error) { failure = error instanceof Error ? error : new Error(String(error)); transaction.abort(); }
      };
      transaction.oncomplete = () => { this.#revision++; resolve(); };
      transaction.onabort = () => reject(failure ?? transaction.error ?? new Error("Session save was aborted."));
      transaction.onerror = () => { /* onabort reports the transaction failure. */ };
    });
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
  const db = await new Promise<IDBDatabase>((resolve, reject) => {
    const request = indexedDB.open(databaseName, 1);
    let blocked = false;
    request.onupgradeneeded = () => request.result.createObjectStore("sessions");
    request.onsuccess = () => { if (blocked) request.result.close(); else resolve(request.result); };
    request.onerror = () => reject(request.error);
    request.onblocked = () => { blocked = true; reject(new Error("Session database upgrade is blocked by another tab. Close that tab and retry.")); };
  });
  try {
    const record = await new Promise<RecordData | undefined>((resolve, reject) => {
      const tx = db.transaction("sessions", "readonly");
      const read = tx.objectStore("sessions").get("all");
      tx.oncomplete = () => resolve(read.result as RecordData | undefined);
      tx.onabort = () => reject(tx.error);
    });
    const storage = new BrowserSessionStorage(preferences, db, record);
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
