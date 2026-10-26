import type { WebAppSnapshot } from "../generated/contracts.js";
import type { PersonaEvent } from "./schema.js";
/** Observe live transitions only: hydration, reconnects, and session changes are silent. */
export class PersonaEvents {
  #session: string | null = null;
  #activity = "";
  #busy = false;
  observe(snapshot: WebAppSnapshot | null, live: boolean): PersonaEvent | null {
    if (!snapshot || !live) { this.reset(); return null; }
    const session = snapshot.session.session_id, activity = snapshot.activity.kind, busy = !snapshot.activity.fully_idle;
    if (session !== this.#session) { this.#session = session; this.#activity = activity; this.#busy = busy; return null; }
    const wasBusy = this.#busy, previous = this.#activity;
    this.#busy = busy; this.#activity = activity;
    if (wasBusy && !busy) {
      const reason = snapshot.status.stop_reason;
      if (/stop|cancel|abort/iu.test(reason)) return "when_stopped";
      if (snapshot.blocks.blocks.at(-1)?.success === false || /error|fail|timed? ?out|timeout|incomplete|✗/iu.test(reason)) return "when_error";
      return "when_model_done_responding";
    }
    if (activity !== previous && activity === "processing") return "when_model_thinking";
    if (activity !== previous && activity === "executing_tool") return "when_tool_started";
    return null;
  }
  reset(): void { this.#session = null; this.#activity = ""; this.#busy = false; }
}
