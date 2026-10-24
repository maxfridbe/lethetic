import { invokeNative, nativeAvailable, nativeNetworkReady } from "./native-host.js";
import type { DotnetRelay } from "./network-route.js";

/** One revocable HTTP capability per runtime. Never saved in session state. */
export class NativeNetwork {
  #pending: Promise<DotnetRelay> | null = null;
  #generation = 0;
  async route(enabled: boolean, browserRoute: DotnetRelay | null, signal: AbortSignal): Promise<DotnetRelay | null> {
    signal.throwIfAborted();
    if (!nativeAvailable()) return browserRoute;
    if (!enabled) return null;
    const generation = this.#generation;
    await nativeNetworkReady();
    signal.throwIfAborted();
    if (generation !== this.#generation) throw new DOMException("Network runtime reset", "AbortError");
    this.#pending ??= invokeNative<DotnetRelay>("grant_network");
    const route = await this.#pending;
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
