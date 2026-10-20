import { validateConnection, modelAuthHeaders } from "./connection.js";
import type { Connection } from "./connection.js";

export interface AvailableModel { id: string; name: string }

/** Discover models using the draft endpoint/key, without requiring a model first. */
export async function fetchModels(connection: Pick<Connection, "provider" | "endpoint" | "apiKey">,
  signal: AbortSignal, fetcher: typeof fetch = fetch): Promise<AvailableModel[]> {
  const validated = validateConnection({ ...connection, name: "Model discovery", model: "discovery" });
  const url = new URL(validated.endpoint);
  const suffix = validated.provider === "anthropic" ? /\/messages\/?$/ : /\/chat\/completions\/?$/;
  if (!suffix.test(url.pathname)) throw new Error("Use a full chat/completions or messages endpoint to fetch models.");
  url.pathname = url.pathname.replace(suffix, "/models");
  const headers: Record<string, string> = { Accept: "application/json", ...modelAuthHeaders(validated) };
  if (validated.provider === "anthropic") {
    url.searchParams.set("limit", "1000");
  }
  const models = new Map<string, AvailableModel>();
  const cursors = new Set<string>();
  for (let page = 0; page < 100; page++) {
    const response = await fetcher(url.href, { headers, signal, credentials: "omit", redirect: "error", cache: "no-store" });
    if (!response.ok) throw new Error(`Model list request failed (HTTP ${response.status}). Check the endpoint and key, or enter a model manually.`);
    const body: unknown = await response.json();
    if (!body || typeof body !== "object" || !("data" in body) || !Array.isArray(body.data)) {
      throw new Error("The endpoint did not return a model list. You can still enter a model manually.");
    }
    for (const item of body.data) {
      if (!item || typeof item !== "object" || typeof item.id !== "string" || !item.id.trim()) continue;
      models.set(item.id, { id: item.id, name: typeof item.display_name === "string" ? item.display_name :
        typeof item.name === "string" ? item.name : item.id });
    }
    if (validated.provider !== "anthropic" || !("has_more" in body) || body.has_more !== true) {
      return [...models.values()].sort((a, b) => a.id.localeCompare(b.id));
    }
    if (!("last_id" in body) || typeof body.last_id !== "string" || !body.last_id || cursors.has(body.last_id)) {
      throw new Error("The model list returned an invalid pagination cursor.");
    }
    cursors.add(body.last_id); url.searchParams.set("after_id", body.last_id);
  }
  throw new Error("The model list is too large. Enter a model manually.");
}
