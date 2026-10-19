/** An HTTP relay for .NET code. This never changes the model connection or Pyodide. */
export interface DotnetRelay { endpoint: string; token?: string }
export function relayEndpoint(value: unknown): string {
  if (typeof value !== "string" || value.length > 2048) throw new Error("Enter an HTTPS relay endpoint.");
  let url: URL;
  try { url = new URL(value.trim()); } catch { throw new Error("Enter an HTTPS /relay URL, not a Tor bridge line."); }
  if (url.hostname === "bridges.torproject.org") throw new Error("bridges.torproject.org supplies Tor bridges, not a proxy endpoint. Configure a bridge in your Tor client, then configure the native relay with its SOCKS5 port.");
  if (url.protocol.startsWith("socks")) throw new Error("A browser cannot connect directly to SOCKS. Configure SOCKS5 on the native .NET relay, then enter the relay's HTTPS URL here.");
  if ((url.protocol !== "https:" && !(url.protocol === "http:" && ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname))) || url.username || url.password || url.search || url.hash)
    throw new Error("Use HTTPS (or HTTP on localhost), without URL credentials, query or fragment.");
  return url.href;
}
export function relayToken(value: unknown): string {
  if (value === undefined || value === "") return "";
  if (typeof value !== "string" || !/^[!-~]{24,1024}$/u.test(value)) throw new Error("Relay token must contain 24–1024 printable ASCII characters without spaces.");
  return value;
}
const KEY = "lethetic.relay-keys.v1";
/** Keep relay tokens separate from saved/exported sessions, like model keys. */
export class RelayKeys {
  readonly #values = new Map<string, string>();
  constructor(private readonly storage?: Pick<Storage, "getItem" | "setItem">) {
    try {
      const raw = storage?.getItem(KEY);
      if (raw && raw.length <= 100_000) {
        const records: unknown = JSON.parse(raw);
        if (Array.isArray(records) && records.length <= 32) for (const record of records) {
          if (Array.isArray(record) && record.length === 2) this.#values.set(relayEndpoint(record[0]), relayToken(record[1]));
        }
      }
    } catch { this.#values.clear(); }
  }
  get(endpoint: string): string { return this.#values.get(endpoint) ?? ""; }
  set(endpoint: string, token: string): void {
    const next = new Map(this.#values); next.delete(endpoint); if (token) next.set(endpoint, token);
    if (next.size > 32) throw new Error("Relay key limit reached (32). Clear an unused relay token first.");
    // Never report a successful save if the tab's key storage rejected it.
    this.storage?.setItem(KEY, JSON.stringify([...next]));
    this.#values.clear(); for (const [key, value] of next) this.#values.set(key, value);
  }
  values(): string[] { return [...this.#values.values()].filter(Boolean); }
}
