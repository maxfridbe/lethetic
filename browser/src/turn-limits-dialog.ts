import { effectiveLimits } from "./model-limits.js";
import { createDialog, field, errorMessage } from "./dialog.js";
import { buttonIconHtml } from "../../web/src/icons.js";
import { DEFAULT_TURN_LIMITS } from "./turn-limits.js";
import type { LocalTransport } from "./local-transport.js";

export function createTurnLimitsDialog(transport: LocalTransport) {
  const dialog = createDialog("Turn limits", `<form data-field="limits-form">
    <label>Turn timeout (minutes)<input data-field="timeout" type="number" min="0" max="1440" step="1" required></label>
    <p>Includes model thinking, retries and tool execution. Use 0 for no turn timeout; Stop remains available. Python and .NET workers keep their separate execution limits.</p>
    <label>Maximum model rounds per turn<input data-field="rounds" type="number" min="1" max="1000" step="1" required></label>
    <label>Retries per model request<input data-field="retries" type="number" min="0" max="10" step="1" required></label>
    <p>Retries cover temporary connection failures, rate limits, server errors and interrupted streams. Completed tools are not replayed.</p>
    <p data-field="model-limits" role="status"></p>
    <div class="panel-toolbar"><button type="button" data-field="model-refresh">${buttonIconHtml("refresh")}Refresh model limits</button><button type="button" data-field="model-context">${buttonIconHtml("save")}Use model context</button></div>
    <label>Context input budget (tokens)<input data-field="context" type="number" min="256" max="2000000" step="1" required></label>
    <label>Maximum output tokens<input data-field="output" type="number" min="1" max="131072" step="1" required></label>
    <p>Use limits supported by your model. The CLI context engine trims older context and shortens older tool results; saved session history remains available. Output includes reasoning tokens.</p>
    <div class="panel-toolbar"><button type="submit" data-field="save">${buttonIconHtml("save")}Save</button><button type="button" data-field="defaults">${buttonIconHtml("refresh")}Defaults</button></div>
    <p data-field="status" role="status"></p></form>`);
  const input = (name: string) => field<HTMLInputElement>(dialog, name);
  const fill = (limits = transport.turnLimits): void => {
    input("timeout").value = String(limits.timeoutMs / 60000);
    input("rounds").value = String(limits.maxRounds); input("retries").value = String(limits.providerRetries);
    input("context").value = String(limits.contextTokens); input("output").value = String(limits.maxOutputTokens);
  };
  const describe = (): void => {
    const model = transport.modelLimits.value;
    field(dialog, "model-limits").textContent = `${transport.modelLimits.status}. Context maximum: ${model?.context?.toLocaleString() ?? "not reported"}; output maximum: ${model?.output?.toLocaleString() ?? "not reported"}. Effective session input/output: ${transport.effectiveTurnLimits.contextTokens.toLocaleString()} / ${transport.effectiveTurnLimits.maxOutputTokens.toLocaleString()}.`;
    field<HTMLButtonElement>(dialog, "model-context").disabled = !model?.context;
  };
  field(dialog, "model-refresh").addEventListener("click", async () => { const pending = transport.refreshModelLimits(); describe(); await pending; describe(); });
  field(dialog, "model-context").addEventListener("click", () => {
    const model = transport.modelLimits.value;
    if (!model?.context) return;
    const limits = effectiveLimits({...transport.turnLimits, contextTokens: 2000000,
      maxOutputTokens: Math.min(Number(input("output").value) || DEFAULT_TURN_LIMITS.maxOutputTokens, 131072)}, model);
    input("output").value = String(limits.maxOutputTokens);
    input("context").value = String(limits.contextTokens);
    field(dialog, "status").textContent = "Model context filled, reserving output tokens. Save to apply to this session.";
  });
  field(dialog, "defaults").addEventListener("click", () => fill(DEFAULT_TURN_LIMITS));
  field(dialog, "limits-form").addEventListener("submit", event => {
    event.preventDefault();
    try {
      transport.setTurnLimits({ timeoutMs: Number(input("timeout").value) * 60000,
        maxRounds: Number(input("rounds").value), providerRetries: Number(input("retries").value),
        contextTokens: Number(input("context").value), maxOutputTokens: Number(input("output").value) });
      field(dialog, "status").textContent = "Saved for this session. Applies to the next turn.";
    } catch (error) { field(dialog, "status").textContent = errorMessage(error); }
  });
  return { open(): void { fill(); describe(); void transport.modelLimits.ensure(transport.connection).then(describe); field(dialog, "status").textContent = transport.busy ? "Finish or stop the current turn before saving." : "Saved per session; survives refresh."; dialog.showModal(); },
    destroy(): void { dialog.remove(); } };
}
