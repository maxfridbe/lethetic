export interface TurnLimits { timeoutMs: number; maxRounds: number; providerRetries: number; contextTokens: number; maxOutputTokens: number; autoOutput?: boolean }
export const MAX_OUTPUT_TOKENS = 2_000_000;
export const DEFAULT_TURN_LIMITS: Readonly<TurnLimits> = { timeoutMs: 600_000, maxRounds: 32, providerRetries: 2, contextTokens: 131_072, maxOutputTokens: 65_536, autoOutput: true };
export function parseTurnLimits(value: unknown): TurnLimits {
  if (typeof value !== "object" || value === null || Array.isArray(value)) throw new Error("Invalid turn limits.");
  // Existing saved numeric caps remain manual unless the user enables automatic.
  const supplied = value as Partial<TurnLimits>;
  const limits = {...DEFAULT_TURN_LIMITS, ...supplied,
    autoOutput: supplied.autoOutput ?? (supplied.maxOutputTokens === undefined ? true : undefined)};
  if ((supplied.autoOutput !== undefined && typeof supplied.autoOutput !== "boolean") ||
      !Number.isSafeInteger(limits.timeoutMs) || limits.timeoutMs < 0 || limits.timeoutMs > 86_400_000 ||
      !Number.isInteger(limits.maxRounds) || limits.maxRounds < 1 || limits.maxRounds > 1000 ||
      !Number.isInteger(limits.contextTokens) || limits.contextTokens < 256 || limits.contextTokens > 2_000_000 ||
      !Number.isInteger(limits.maxOutputTokens) || limits.maxOutputTokens < 1 || limits.maxOutputTokens > MAX_OUTPUT_TOKENS ||
      !Number.isInteger(limits.providerRetries) || limits.providerRetries < 0 || limits.providerRetries > 10) {
    throw new Error("Use 0–1,440 minutes (0 disables the timeout), 1–1,000 model rounds and 0–10 retries; context 256–2,000,000 tokens and output 1–2,000,000 tokens.");
  }
  return { timeoutMs: limits.timeoutMs, maxRounds: limits.maxRounds, providerRetries: limits.providerRetries, contextTokens: limits.contextTokens, maxOutputTokens: limits.maxOutputTokens, ...(limits.autoOutput !== undefined ? {autoOutput: limits.autoOutput} : {}) };
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
