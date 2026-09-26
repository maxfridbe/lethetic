export interface FragmentLocation {
  readonly hash: string;
  readonly origin: string;
  readonly pathname: string;
  readonly search: string;
}

export interface FragmentHistory {
  replaceState(data: unknown, unused: string, url?: string | URL | null): void;
}

const TOKEN_LIMIT = 16_384;

/**
 * Takes the one-time credential out of the current fragment and erases the
 * fragment before its value can be used for authentication.
 */
export function takeBootstrapToken(
  locationView: FragmentLocation,
  historyView: FragmentHistory,
): string | null {
  const fragment = locationView.hash.startsWith("#")
    ? locationView.hash.slice(1)
    : locationView.hash;

  if (locationView.hash.length > 0) {
    const cleanUrl = new URL(locationView.origin);
    cleanUrl.pathname = locationView.pathname;
    cleanUrl.search = locationView.search;
    cleanUrl.hash = "";
    historyView.replaceState(null, "", cleanUrl);
  }

  return parseBootstrapFragment(fragment);
}

export type BootstrapAuthentication =
  | { readonly type: "session" }
  | { readonly type: "token"; readonly token: string }
  | { readonly type: "malformed" };

export function classifyBootstrapFragment(
  fragment: string,
): BootstrapAuthentication {
  if (fragment.length === 0) {
    return { type: "session" };
  }
  const encoded = fragment.startsWith("#") ? fragment.slice(1) : fragment;
  const token = parseBootstrapFragment(encoded);
  return token === null ? { type: "malformed" } : { type: "token", token };
}

export function parseBootstrapFragment(fragment: string): string | null {
  if (fragment.length === 0 || fragment.length > TOKEN_LIMIT) {
    return null;
  }

  let encoded: string;
  let explicit = false;
  if (fragment.startsWith("token=")) {
    explicit = true;
    encoded = fragment.slice("token=".length);
    if (encoded.length === 0 || encoded.includes("&")) {
      return null;
    }
  } else {
    // A bare value is only well-defined when it cannot be interpreted as a
    // fragment parameter list.
    if (
      fragment.includes("&") ||
      fragment.includes("=") ||
      fragment.includes("?")
    ) {
      return null;
    }
    encoded = fragment;
  }

  try {
    const token = decodeURIComponent(encoded);
    const canonical = explicit
      ? /^[A-Za-z0-9_-]+={0,2}$/u.test(token)
      : /^[A-Za-z0-9_-]+$/u.test(token);
    return canonical && token.length <= TOKEN_LIMIT ? token : null;
  } catch {
    return null;
  }
}

export function newRequestId(): string {
  if (typeof globalThis.crypto.randomUUID === "function") {
    return globalThis.crypto.randomUUID();
  }

  const bytes = new Uint8Array(16);
  globalThis.crypto.getRandomValues(bytes);
  // RFC 4122 variant/version bits make fallback IDs recognizable and keep the
  // full construction independent of timestamps or application state.
  bytes[6] = (bytes[6]! & 0x0f) | 0x40;
  bytes[8] = (bytes[8]! & 0x3f) | 0x80;
  const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0"));
  return [
    hex.slice(0, 4).join(""),
    hex.slice(4, 6).join(""),
    hex.slice(6, 8).join(""),
    hex.slice(8, 10).join(""),
    hex.slice(10).join(""),
  ].join("-");
}

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function isFiniteInteger(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}

export function isString(value: unknown): value is string {
  return typeof value === "string";
}

export function isBoolean(value: unknown): value is boolean {
  return typeof value === "boolean";
}

export function assertNever(value: never, context: string): never {
  throw new Error(`Unreachable ${context}`);
}

export function boundedText(value: string, maximum = 1_000_000): string {
  return value.length <= maximum ? value : `${value.slice(0, maximum)}\n…`;
}

export function isSafeCssColor(value: string): boolean {
  if (
    value.length === 0 ||
    value.length > 96 ||
    /[;{}]/u.test(value) ||
    /url\s*\(/iu.test(value) ||
    /var\s*\(/iu.test(value)
  ) {
    return false;
  }
  return globalThis.CSS.supports("color", value);
}
