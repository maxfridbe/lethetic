/** A model endpoint supplied by the user or embedding application. */
export interface Connection {
  name: string;
  provider: "openai" | "anthropic";
  /** Full /v1/chat/completions or /v1/messages URL, including any proxy prefix. */
  endpoint: string;
  model: string;
  /** OpenRouter-compatible reasoning control. Omit to keep the model default. */
  thinking?: "enabled" | "disabled";
  /** Kept out of saved sessions and WASM/Python. The hosted UI uses sessionStorage; Android uses its encrypted native vault. */
  apiKey?: string;
}

/** Accept the token itself, never a page URL or a copied Authorization header. */
export function normalizeApiKey(value: string | undefined): string | undefined {
  if (value === undefined) return undefined;
  if (typeof value !== "string") throw new Error("The API key must be text.");
  const key = value.trim();
  if (!key) return undefined;
  if (/^(?:authorization\s*:|bearer(?:\s|$))/i.test(key)) {
    throw new Error("Enter only the API key, without Authorization: or Bearer.");
  }
  if (/^[a-z][a-z0-9+.-]*:\/\//i.test(key) || /^(?:#\/|\/sessions\/)/i.test(key)) {
    throw new Error("The API key contains a URL or session link. Replace it with the provider's API key.");
  }
  if (/[^\x21-\x7e]/.test(key)) throw new Error("The API key contains spaces or unsupported characters. Paste the token only.");
  return key;
}

/** Metadata may be restored without credentials; authenticated chat must not proceed silently. */
export function requireChatCredential(connection: Connection): void {
  const key = normalizeApiKey(connection.apiKey);
  const url = new URL(connection.endpoint);
  if (!key && ["https://openrouter.ai", "https://api.openai.com", "https://api.anthropic.com"].includes(url.origin)) {
    throw new Error("API key missing for this saved connection. Open Connections, enter the provider key, then Save & use connection. Closing the tab or restarting the app can clear stored keys.");
  }
}

/** Explain what was sent without exposing any part of the credential. */
export function authenticationFailure(connection: Connection): string {
  return normalizeApiKey(connection.apiKey)
    ? "Authentication rejected (HTTP 401). An API key was sent, but the provider did not accept it. Check the key for this endpoint in Connections."
    : "Authentication required (HTTP 401). No API key was sent. Enter the provider key in Connections and choose Save & use connection.";
}

/** Chat, connection tests and model discovery must use the same credential rules. */
export function modelAuthHeaders(connection: Pick<Connection, "provider" | "apiKey">): Record<string, string> {
  const key = normalizeApiKey(connection.apiKey);
  if (connection.provider === "anthropic") return {
    "anthropic-version": "2023-06-01", "anthropic-dangerous-direct-browser-access": "true",
    ...(key ? {"x-api-key": key} : {}),
  };
  return key ? {Authorization: `Bearer ${key}`} : {};
}

export function validateConnection(connection: Connection): Connection {
  if (connection.provider !== "openai" && connection.provider !== "anthropic") {
    throw new Error("Choose an OpenAI-compatible or Anthropic connection.");
  }
  const endpoint = new URL(connection.endpoint);
  if (endpoint.username || endpoint.password || endpoint.hash || endpoint.search) {
    throw new Error("Use an endpoint without credentials, query parameters, or a fragment.");
  }
  const loopback = ["localhost", "127.0.0.1", "[::1]"].includes(endpoint.hostname);
  if (endpoint.protocol !== "https:" && !(endpoint.protocol === "http:" && loopback)) {
    throw new Error("Use HTTPS, or HTTP on localhost for development.");
  }
  if (endpoint.protocol === "http:" && globalThis.location?.protocol === "https:") {
    throw new Error("An HTTPS page needs an HTTPS model endpoint. Use a TLS proxy for local models.");
  }
  if (!connection.name.trim() || !connection.model.trim()) {
    throw new Error("A connection name and model are required.");
  }
  if (connection.thinking !== undefined &&
      !["enabled", "disabled"].includes(connection.thinking)) throw new Error("Choose a valid thinking setting.");
  if (connection.thinking !== undefined && connection.provider !== "openai") {
    throw new Error("The thinking preference requires an OpenRouter-compatible connection.");
  }
  return { ...connection, endpoint: endpoint.href, model: connection.model.trim(), apiKey: normalizeApiKey(connection.apiKey) };
}

const STORAGE_KEY = "lethetic.connections.v1";

/** Persist endpoint metadata only; credentials use a separate optional tab store. */
export function saveConnections(storage: Pick<Storage, "setItem">, connections: Connection[]): void {
  storage.setItem(STORAGE_KEY, JSON.stringify(connections.map(connectionMetadata)));
}

/** Explicit allowlist shared by saved connections and sessions; never persist credentials. */
export function connectionMetadata({ name, provider, endpoint, model, thinking }: Connection): Omit<Connection, "apiKey"> {
  return { name, provider, endpoint, model, ...(thinking !== undefined ? { thinking } : {}) };
}

export function loadConnections(storage: Pick<Storage, "getItem">): Connection[] {
  try {
    const values: unknown = JSON.parse(storage.getItem(STORAGE_KEY) ?? "[]");
    if (!Array.isArray(values)) return [];
    return values.flatMap((value: unknown) => {
      if (typeof value !== "object" || value === null) return [];
      const item = value as Record<string, unknown>;
      if (typeof item["name"] !== "string" || typeof item["model"] !== "string" ||
          typeof item["endpoint"] !== "string" ||
          (item["provider"] !== "openai" && item["provider"] !== "anthropic")) return [];
      try {
        return [validateConnection({ name: item["name"], model: item["model"],
          endpoint: item["endpoint"], provider: item["provider"], thinking: item["thinking"] as Connection["thinking"] })];
      } catch { return []; }
    });
  } catch { return []; }
}

const KEY_STORAGE = "lethetic.connection-keys.v1";
const connectionIdentity = ({name,provider,endpoint,model}: Connection): string => JSON.stringify([name,provider,endpoint,model]);

/** Pass sessionStorage to retain keys through reloads in this tab only. */
export function saveConnectionKeys(storage: Pick<Storage, "setItem">, connections: Connection[]): void {
  storage.setItem(KEY_STORAGE, JSON.stringify(connections.filter(item => item.apiKey)
    .map(item => [connectionIdentity(item), item.apiKey])));
}
export function restoreConnectionKeys(storage: Pick<Storage, "getItem">, connections: Connection[]): Connection[] {
  try {
    const stored: unknown = JSON.parse(storage.getItem(KEY_STORAGE) ?? "[]");
    if (!Array.isArray(stored)) return connections;
    const keys = new Map<string,string>();
    for (const entry of stored) if (Array.isArray(entry) && entry.length === 2 &&
      typeof entry[0] === "string" && typeof entry[1] === "string") keys.set(entry[0],entry[1]);
    return connections.map(item => item.apiKey !== undefined ? item :
      {...item, apiKey: keys.get(connectionIdentity(item))});
  } catch { return connections; }
}

export interface ConnectionHealth { state: "testing" | "valid" | "invalid"; message: string }
/** Ephemeral only: contains credentials. Never put this identity in DOM or storage. */
export const connectionHealthIdentity = (connection: Connection): string =>
  JSON.stringify([connection.provider, connection.endpoint, connection.model, connection.apiKey?.trim() ?? "", connection.thinking ?? "default"]);
