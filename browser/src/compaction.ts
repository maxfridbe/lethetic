import { executionTimeout } from "./lifecycle.js";
import { createCompactionPlan, createRuntime } from "./sdk.js";
import type { RuntimeOptions, WasmCompaction } from "./sdk.js";
import { DEFAULT_TURN_LIMITS, parseTurnLimits } from "./turn-limits.js";

interface CompactionPass { id: number; system: string; user: string; max_tokens: number; merge: boolean }

/** Browser host I/O only. Windows, prompts, merges and result validation are CLI/WASM policy. */
export async function summarizeContext(options: RuntimeOptions, messages: readonly unknown[], signal: AbortSignal,
  progress: (current: number, total: number) => void): Promise<string> {
  const limits = parseTurnLimits({ ...DEFAULT_TURN_LIMITS, ...Object.fromEntries(Object.entries(options).filter(([, value]) => value !== undefined)) });
  const controller = new AbortController();
  const abort = (): void => controller.abort(signal.reason);
  signal.addEventListener("abort", abort, {once: true}); if (signal.aborted) abort();
  const cancelDeadline = executionTimeout(() => controller.abort(new Error("Compaction timed out. Original context kept.")), limits.timeoutMs);
  let plan: WasmCompaction | undefined;
  try {
    controller.signal.throwIfAborted();
    plan = await createCompactionPlan(messages, options);
    while (plan.result() === undefined) {
      controller.signal.throwIfAborted();
      const passes = JSON.parse(plan.requests()) as CompactionPass[];
      for (const pass of passes) {
        controller.signal.throwIfAborted();
        const [done, total] = JSON.parse(plan.progress()) as [number, number]; progress(done + 1, total);
        const runtime = await createRuntime({...options, timeoutMs: 0, tools: [], systemPrompt: pass.system, maxOutputTokens: pass.max_tokens});
        try {
          const summary = await runtime.send(pass.user, controller.signal);
          controller.signal.throwIfAborted(); plan.accept(pass.id, summary);
        } finally { runtime.dispose(); }
      }
    }
    controller.signal.throwIfAborted();
    return plan.result()!;
  } finally {
    plan?.free(); cancelDeadline(); signal.removeEventListener("abort", abort);
  }
}
