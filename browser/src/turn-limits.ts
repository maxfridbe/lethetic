export interface TurnLimits { timeoutMs: number; maxRounds: number; providerRetries: number; contextTokens: number; maxOutputTokens: number }
export const DEFAULT_TURN_LIMITS: Readonly<TurnLimits> = { timeoutMs: 600_000, maxRounds: 32, providerRetries: 2, contextTokens: 131_072, maxOutputTokens: 16_384 };
export function parseTurnLimits(value: unknown): TurnLimits {
  if (typeof value !== "object" || value === null || Array.isArray(value)) throw new Error("Invalid turn limits.");
  const limits = {...DEFAULT_TURN_LIMITS, ...value} as TurnLimits;
  if (!Number.isSafeInteger(limits.timeoutMs) || limits.timeoutMs < 0 || limits.timeoutMs > 86_400_000 ||
      !Number.isInteger(limits.maxRounds) || limits.maxRounds < 1 || limits.maxRounds > 1000 ||
      !Number.isInteger(limits.contextTokens) || limits.contextTokens < 256 || limits.contextTokens > 2_000_000 ||
      !Number.isInteger(limits.maxOutputTokens) || limits.maxOutputTokens < 1 || limits.maxOutputTokens > 131_072 ||
      !Number.isInteger(limits.providerRetries) || limits.providerRetries < 0 || limits.providerRetries > 10) {
    throw new Error("Use 0–1,440 minutes (0 disables the timeout), 1–1,000 model rounds and 0–10 retries; context 256–2,000,000 tokens and output 1–131,072 tokens.");
  }
  return { timeoutMs: limits.timeoutMs, maxRounds: limits.maxRounds, providerRetries: limits.providerRetries, contextTokens: limits.contextTokens, maxOutputTokens: limits.maxOutputTokens };
}

export const LOOP_MODES = [
  ["off", "Off", "Off"], ["block-limit", "Block limit", "BlockLimit"], ["n-gram", "N-gram", "NGram"],
  ["phrase-frequency", "Phrase frequency", "PhraseFrequency"], ["combined", "Combined", "Combined"],
  ["combined-block-limit", "Combined + block limit", "CombinedWithBlockLimit"],
] as const;
export function parseLoopMode(value: unknown): string {
  if (typeof value !== "string" || !LOOP_MODES.some(mode => mode[0] === value)) throw new Error("Invalid loop detection mode.");
  return value;
}
