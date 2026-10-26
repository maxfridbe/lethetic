import { invokeNative, nativeAvailable, nativeNetworkReady } from "./native-host.js";
import type { DotnetRelay, NativeNetworkMode } from "./network-route.js";

/** One revocable HTTP capability per runtime. Never saved in session state. */
export class NativeNetwork {
  #pending: Promise<DotnetRelay> | null = null;
  #generation = 0;
  #mode: NativeNetworkMode = "direct";
  async route(enabled: boolean, browserRoute: DotnetRelay | null, signal: AbortSignal, mode: NativeNetworkMode = "direct"): Promise<DotnetRelay | null> {
    signal.throwIfAborted();
    if (mode !== "direct" && mode !== "tor") throw new Error("Invalid network mode");
    if (enabled && mode === "tor" && !nativeAvailable()) throw new Error("Tor-only access requires the native app; direct networking is blocked.");
    if (!nativeAvailable()) return browserRoute;
    if (!enabled) return null;
    if (mode !== this.#mode) { this.reset(); this.#mode = mode; }
    const generation = this.#generation;
    await nativeNetworkReady();
    signal.throwIfAborted();
    if (generation !== this.#generation) throw new DOMException("Network runtime reset", "AbortError");
    this.#pending ??= invokeNative<DotnetRelay>("grant_network", {mode});
    const pending = this.#pending;
    let route: DotnetRelay;
    try { route = await pending; }
    catch (error) { if (this.#pending === pending) this.#pending = null; throw error; }
    signal.throwIfAborted();
    if (generation !== this.#generation) throw new DOMException("Network runtime reset", "AbortError");
    return route;
  }
  reset(): void {
    this.#generation++;
    const pending = this.#pending; this.#pending = null;
    if (pending) void pending.then(route => invokeNative<void>("revoke_network", { grant: route.token })).catch(() => {});
  }
}
