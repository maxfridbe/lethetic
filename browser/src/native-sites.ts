import { beginNativeSite, invokeNative, nativeNetworkReady } from "./native-host.js";
import { encodeBytes } from "./workspace.js";
import type { WireRequest, WireResponse } from "./dotnet.js";
export interface ForwardedSite { id: string; port: number; url: string }
type NativeRequest = Omit<WireRequest, "port"> & {id: string};

/** A top-level capability; site pages never receive the native IPC token. */
export class NativeSites {
  route: ForwardedSite | null = null;
  #generation = 0;
  #endWork: (() => void) | null = null;
  constructor(readonly onError: (message: string) => void) {}
  async start(port: number, network: boolean, virtualPort: number, handler: (request: WireRequest) => Promise<WireResponse>): Promise<void> {
    await this.stop();
    const generation = ++this.#generation;
    await nativeNetworkReady();
    const route = await invokeNative<ForwardedSite>("forward_site", {port, network});
    if (generation !== this.#generation) { await invokeNative("stop_forwarded_sites", {id: route.id}); return; }
    this.route = route;
    try { this.#endWork = await beginNativeSite(); }
    catch (error) { await this.stop(); throw error; }
    if (generation !== this.#generation) { this.#endWork?.(); this.#endWork = null; return; }
    void this.#pump(route, generation, virtualPort, handler);
  }
  async #pump(route: ForwardedSite, generation: number, port: number, handler: (request: WireRequest) => Promise<WireResponse>): Promise<void> {
    try {
      while (generation === this.#generation) {
        const request = await invokeNative<NativeRequest>("next_site_request", {id: route.id});
        if (generation !== this.#generation) return;
        let response: WireResponse;
        try { response = await handler({port, method: request.method, path: request.path, headers: request.headers, body: request.body}); }
        catch (error) { response = {status: 503, headers: {"content-type": ["text/plain; charset=utf-8"]}, body: encodeBytes(new TextEncoder().encode(String(error)))}; }
        if (generation !== this.#generation) return;
        // A browser tab can close or time out while its virtual handler finishes.
        await invokeNative("reply_site_request", {id: route.id, requestId: request.id, response}).catch(error => {
          if (!String(error).includes("expired")) throw error;
        });
      }
    } catch (error) {
      if (generation === this.#generation) { this.onError(String(error)); void this.stop().catch(failure => this.onError(String(failure))); }
    }
  }
  async openBrowser(): Promise<void> {
    if (!this.route) throw new Error("Start forwarding first.");
    await invokeNative("open_site_browser", {id: this.route.id});
  }
  async stop(): Promise<void> {
    this.#generation++;
    const route = this.route;
    if (route) await invokeNative("stop_forwarded_sites", {id: route.id});
    this.route = null; this.#endWork?.(); this.#endWork = null;
  }
}
