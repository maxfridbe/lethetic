import type { Connection } from "./connection.js";

/** Null means the provider did not report a usable value; zero is a real count. */
export interface ProviderUsage {
  input: number | null; output: number | null; total: number | null;
  cached: number | null; cacheWrite: number | null; reasoning: number | null;
}
const object = (value: unknown): Record<string, unknown> =>
  value !== null && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : {};
const count = (value: unknown): number | null =>
  typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : null;
const sum = (...values: number[]): number | null => count(values.reduce((a, b) => a + b, 0));

export function readUsage(provider: Connection["provider"], response: unknown): ProviderUsage {
  const usage = object(object(response)["usage"]);
  const promptDetails = object(usage["prompt_tokens_details"]);
  const completionDetails = object(usage["completion_tokens_details"]);
  const cached = count(provider === "anthropic" ? usage["cache_read_input_tokens"] :
    usage["prompt_cache_hit_tokens"] ?? promptDetails["cached_tokens"]);
  const cacheWrite = count(provider === "anthropic" ? usage["cache_creation_input_tokens"] : promptDetails["cache_write_tokens"]);
  let input = count(provider === "anthropic" ? usage["input_tokens"] : usage["prompt_tokens"]);
  if (provider === "anthropic" && input !== null) input = sum(input, cached ?? 0, cacheWrite ?? 0);
  if (provider === "openai" && input === null && cached !== null && count(usage["prompt_cache_miss_tokens"]) !== null) {
    input = sum(cached, count(usage["prompt_cache_miss_tokens"])!);
  }
  const output = count(provider === "anthropic" ? usage["output_tokens"] : usage["completion_tokens"]);
  return { input, output, total: count(usage["total_tokens"]) ??
    (input !== null && output !== null ? sum(input, output) : null),
    cached: input !== null && cached !== null && cached > input ? null : cached,
    cacheWrite, reasoning: count(completionDetails["reasoning_tokens"]) };
}
