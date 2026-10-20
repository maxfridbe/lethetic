import type { DotnetRelay } from "./network-route.js";

type RelayFetch = (fetch: typeof globalThis.fetch, relay: DotnetRelay, input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;
export const PYTHON_NETWORK_DENIED = "Network access is disabled for this session. Enable Network access in Connections → Runtime & security, then run the Python program again.";

/** Only immutable package files from the host-selected Pyodide distribution are exempt. */
export function pythonPackageDownloads(lock: { packages: Record<string, { file_name: string }> }, indexURL: string): Set<string> {
  const downloads = new Set<string>();
  for (const entry of Object.values(lock.packages)) {
    if (typeof entry.file_name !== "string" || !/^[a-zA-Z0-9_.+-]+\.(whl|zip|tar(?:\.gz)?)$/u.test(entry.file_name))
      throw new Error("Invalid Pyodide package filename.");
    downloads.add(new URL(entry.file_name, indexURL).href);
  }
  return downloads;
}

/** Install before Pyodide captures browser APIs. Like .NET's policy, this controls
 * cooperative code; JavaScript interop/dynamic imports are not a hostile-code sandbox.
 * Configuration is fixed for this worker; permission/route changes terminate it.
 */
export function installPythonNetworkPolicy(options: {
  networkAccess: boolean;
  relay?: DotnetRelay | null;
  downloads: ReadonlySet<string>;
  relayFetch?: RelayFetch;
  onDenied?: () => void;
}, scope: typeof globalThis = globalThis): void {
  const originalFetch = scope.fetch.bind(scope);
  const downloads = new Set(options.downloads);
  const allowed = options.networkAccess;
  const relay = options.relay ? { ...options.relay } : null;
  const denied = (): DOMException => {
    options.onDenied?.();
    return new DOMException(PYTHON_NETWORK_DENIED, "NotAllowedError");
  };
  Object.defineProperty(scope, "fetch", { configurable: false, writable: false,
    value: async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
      const resolved = typeof input === "string" ? new URL(input, scope.location?.href) : input;
      const request = new Request(resolved, init);
      // This is an exact URL allowlist, never a CDN-origin wildcard. No caller
      // headers, cookies, query parameters, uploads or redirects can use it.
      if (request.method === "GET" && downloads.has(request.url)) {
        return originalFetch(new Request(request, {
          headers: {}, credentials: "omit", redirect: "error", referrer: "", referrerPolicy: "no-referrer",
        }));
      }
      if (!allowed) throw denied();
      if (relay) {
        if (!options.relayFetch) throw new Error("Python HTTP relay support is unavailable; no direct fallback.");
        return options.relayFetch(originalFetch, relay, request);
      }
      return originalFetch(request);
    } });
  for (const name of ["XMLHttpRequest", "WebSocket", "EventSource", "WebTransport"]) {
    const original = (scope as unknown as Record<string, unknown>)[name];
    if (typeof original !== "function") continue;
    Object.defineProperty(scope, name, { configurable: false, writable: false,
      value: new Proxy(original, { construct(target, args): object {
        if (!allowed) throw denied();
        if (relay) throw new DOMException(
          "The HTTP relay supports async HTTP fetch only. Use await pyodide.http.pyfetch; sockets and XMLHttpRequest cannot bypass the configured route.",
          "NotSupportedError");
        return Reflect.construct(target, args) as object;
      } }) });
  }
}
