/** Android credentials are encrypted by Keystore, never put in localStorage or session exports. */
type Host = Window & {__LETHETIC_ANDROID__?: boolean; __LETHETIC_HOST_TOKEN__?: string;
  __TAURI__?: {core: {invoke<T>(command: string, args: Record<string, unknown>): Promise<T>}}};
export function androidHost(): boolean {
  return typeof window !== "undefined" && window === window.top && (window as Host).__LETHETIC_ANDROID__ === true && typeof (window as Host).__LETHETIC_HOST_TOKEN__ === "string";
}
export async function androidCommand<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
  const host = window as Host;
  if (!androidHost() || !host.__TAURI__) throw Error("Android native bridge unavailable. Update the app.");
  try { return await host.__TAURI__.core.invoke<T>(`plugin:lethetic-host|${command}`, {...args, token: host.__LETHETIC_HOST_TOKEN__}); }
  catch (error) { throw Error(typeof error === "string" ? error : "Android operation failed"); }
}
const ALLOWED = ["lethetic.connection-keys.v1", "lethetic.relay-keys.v1", "lethetic.speech-keys.v1"];
let current: CredentialStorage | undefined;
export function nativeCredentials(): CredentialStorage | undefined { return current; }
export class CredentialStorage implements Storage {
  #values: Map<string, string>;
  #pending: Promise<void> = Promise.resolve();
  #errors = new Set<string>();
  constructor(values: Record<string, string>, readonly write: (key: string, value: string | null) => Promise<void>) {
    this.#values = new Map(Object.entries(values).filter(([key, value]) => ALLOWED.includes(key) && typeof value === "string"));
  }
  get length(): number { return this.#values.size; }
  key(index: number): string | null { return [...this.#values.keys()][index] ?? null; }
  getItem(key: string): string | null { return this.#values.get(key) ?? null; }
  setItem(key: string, value: string): void { this.#save(key, String(value)); }
  removeItem(key: string): void { this.#save(key, null); }
  clear(): void { for (const key of this.#values.keys()) this.removeItem(key); }
  #save(key: string, value: string | null): void {
    if (!ALLOWED.includes(key) || (value?.length ?? 0) > 256 * 1024) throw Error("Invalid credential storage entry");
    if (value === null) this.#values.delete(key); else this.#values.set(key, value);
    this.#pending = this.#pending.then(async () => {
      try { await this.write(key, value); this.#errors.delete(key); }
      catch { this.#errors.add(key); }
    });
  }
  async flush(): Promise<void> { await this.#pending; if (this.#errors.size) throw Error("Credentials could not be saved securely. Keep the app open and retry Save."); }
}
export async function openNativeCredentials(previous?: Storage): Promise<CredentialStorage> {
  const values = await androidCommand<Record<string, string>>("load_credentials");
  const store = new CredentialStorage(values, (key, value) => androidCommand("save_credentials", {key, value}));
  // Migrate only keys still present in the current tab, then remove the unencrypted copies.
  for (const key of ALLOWED) {
    const value = previous?.getItem(key);
    if (value && store.getItem(key) === null) store.setItem(key, value);
  }
  await store.flush();
  for (const key of ALLOWED) previous?.removeItem(key);
  current = store; return store;
}
export async function flushCredentials(storage?: Storage): Promise<void> {
  if (storage instanceof CredentialStorage) await storage.flush();
}
