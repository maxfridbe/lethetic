import { parseWorkspace, type Workspace } from "./workspace.js";
type HostWindow = Window & { __LETHETIC_HOST_TOKEN__?: string; __TAURI__?: { core: { invoke<T>(command: string, args: Record<string, unknown>): Promise<T> } } };
export function nativeAvailable(): boolean {
  return typeof window !== "undefined" && window === window.top && typeof (window as HostWindow).__LETHETIC_HOST_TOKEN__ === "string";
}
async function invoke<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
  if (!nativeAvailable()) throw new Error("Native host unavailable");
  const host = window as HostWindow;
  if (!host.__TAURI__) throw new Error("Native bridge unavailable");
  return host.__TAURI__.core.invoke<T>(`plugin:lethetic-host|${command}`, { ...args, token: host.__LETHETIC_HOST_TOKEN__ });
}
let wakeQueue = Promise.resolve();
let networkReset = Promise.resolve();
export { invoke as invokeNative };
export function nativeNetworkReady(): Promise<void> { return networkReset; }
let turns = 0, responses = 0, hostedSites = 0;
function updateNativeWork(): Promise<void> {
  if (!nativeAvailable()) return Promise.resolve();
  const working = responses > 0, active = turns > 0 || hostedSites > 0 || working;
  const next = wakeQueue.then(() => invoke<void>("set_working", { working, active }));
  wakeQueue = next.catch(() => {}); return next;
}
export function nativeWorking(working: boolean): Promise<void> {
  responses = Math.max(0, responses + (working ? 1 : -1));
  return updateNativeWork();
}
/** The Android foreground notification tracks a user-initiated turn. Its CPU
 * wake lock is separate and is held only for the individual provider responses. */
export function beginNativeTurn(): () => void {
  turns++; void updateNativeWork().catch(() => {});
  let ended = false;
  return () => { if (!ended) { ended = true; turns = Math.max(0, turns - 1); void updateNativeWork().catch(() => {}); } };
}
/** Keep the WebView available for explicit site hosting without a CPU wake lock. */
export async function beginNativeSite(): Promise<() => void> {
  hostedSites++; try { await updateNativeWork(); } catch (error) { hostedSites--; throw error; }
  let ended = false;
  return () => { if (!ended) { ended = true; hostedSites = Math.max(0, hostedSites - 1); void updateNativeWork().catch(() => {}); } };
}
/** A reload must not inherit the old document's native sleep inhibitor. */
export function resetNativeWork(): void {
  turns = 0; responses = 0; hostedSites = 0; void updateNativeWork().catch(() => {});
  if (nativeAvailable()) {
    void invoke<void>("cancel_download", { id: null }).catch(() => {});
    void invoke<void>("cancel_speech", { id: null }).catch(() => {});
    networkReset = Promise.all([invoke<void>("revoke_network", { grant: null }), invoke<void>("stop_forwarded_sites", { id: null })]).then(() => {});
    void networkReset.catch(() => {});
  }
}
/** A grant lasts only until session change, explicit revoke or app exit. */
export class NativeDirectory {
  name: string | null = null;
  status = "Virtual directory";
  #queue = Promise.resolve();
  #generation = 0;
  async choose(): Promise<Workspace | null> {
    await this.#queue;
    const result = await invoke<{ name: string; workspace: unknown } | null>("choose_directory");
    if (!result) return null;
    try {
      const workspace = parseWorkspace(result.workspace);
      this.name = result.name; this.status = `Folder: ${result.name}`; this.#generation++;
      return workspace;
    } catch (error) { await this.revoke(); throw error; }
  }
  sync(workspace: Workspace): Promise<void> {
    if (!this.name) return Promise.resolve();
    const generation = this.#generation;
    const snapshot = parseWorkspace(workspace);
    const pending = this.#queue.then(async () => {
      if (generation !== this.#generation || !this.name) return;
      await invoke<void>("sync_directory", { workspace: snapshot });
      this.status = `Folder: ${this.name} · saved`;
    });
    this.#queue = pending.catch(error => { this.status = `Folder save failed: ${String(error)}`; });
    return pending;
  }
  async revoke(): Promise<void> {
    this.#generation++; this.name = null; this.status = "Virtual directory";
    await this.#queue;
    if (nativeAvailable()) await invoke<void>("revoke_directory");
  }
}
