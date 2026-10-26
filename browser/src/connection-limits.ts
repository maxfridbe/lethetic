import { buttonIconHtml } from "../../web/src/icons.js";
import { field, errorMessage } from "./dialog.js";
import { DEFAULT_TURN_LIMITS, MAX_OUTPUT_TOKENS, parseTurnLimits } from "./turn-limits.js";
import { effectiveLimits, fetchModelLimits, isOpenRouter } from "./model-limits.js";
import type { ModelLimits } from "./model-limits.js";
import { fetchModels } from "./models.js";
import type { Connection } from "./connection.js";
import type { LocalTransport } from "./local-transport.js";

export const connectionLimitsHtml = `<form data-field="connection-limits-form">
  <fieldset class="browser-runtime"><legend>Context &amp; turn limits · current session</legend>
    <p class="browser-note" data-field="connection-model-limits" role="status">Test the connection to discover model limits.</p>
    <label>Context input budget (tokens)<input data-field="connection-context" type="number" min="256" max="2000000" required></label>
    <label><input data-field="connection-auto-output" type="checkbox">Automatic output cap (model maximum)</label>
    <p class="browser-note">Automatic includes reasoning and reserves the advertised maximum from input capacity. Without model limits it uses 65,536 tokens. Uncheck for a manual cap.</p>
    <label>Manual output cap (including reasoning)<input data-field="connection-output" type="number" min="1" max="${MAX_OUTPUT_TOKENS}" required></label>
    <label>Turn timeout (minutes, 0 disables)<input data-field="connection-timeout" type="number" min="0" max="1440" required></label>
    <label>Maximum model rounds per turn<input data-field="connection-rounds" type="number" min="1" max="1000" required></label>
    <label>Retries per model request<input data-field="connection-retries" type="number" min="0" max="10" required></label>
    <p class="browser-note" data-field="connection-effective-limits"></p>
    <div class="panel-toolbar"><button type="submit" data-field="connection-apply-limits">${buttonIconHtml("save")}Apply limits</button><button type="button" data-field="connection-default-limits">${buttonIconHtml("refresh")}Defaults</button></div>
    <p data-field="connection-limits-status" role="status">Test fills available context, reserving output tokens. Apply limits saves to this session.</p>
  </fieldset>
</form>`;

/** Session budgets remain independent of the connection editor and test conversation. */
export function connectionLimits(dialog: HTMLDialogElement, transport: LocalTransport, current: () => Connection) {
  let model: ModelLimits | null = null, generation = 0;
  const input = (name: string) => field<HTMLInputElement>(dialog, `connection-${name}`);
  const status = (text: string) => { field(dialog,"connection-limits-status").textContent = text; };
  const same = (a: Connection | undefined, b: Connection) => !!a && a.endpoint === b.endpoint && a.provider === b.provider && a.model === b.model && (a.apiKey?.trim() ?? "") === (b.apiKey?.trim() ?? "");
  const fill = (limits = transport.turnLimits): void => {
    input("context").value=String(limits.contextTokens); input("output").value=String(limits.maxOutputTokens);
    input("auto-output").checked=!!limits.autoOutput;input("output").disabled=!!limits.autoOutput;
    input("timeout").value=String(limits.timeoutMs/60000); input("rounds").value=String(limits.maxRounds); input("retries").value=String(limits.providerRetries);
  };
  const describe = (): void => {
    field<HTMLButtonElement>(dialog,"connection-apply-limits").disabled=transport.busy || !same(transport.connection,current());
    const limits=transport.effectiveTurnLimits;
    field(dialog,"connection-effective-limits").textContent=`Active session: context ${limits.contextTokens.toLocaleString()} · output ${limits.maxOutputTokens.toLocaleString()} tokens. ${same(transport.connection,current()) ? "" : "Save and use the edited connection before applying limits."}`;
  };
  const knownModel = () => model ?? (same(transport.connection,current()) ? transport.modelLimits.value : null);
  const read = () => parseTurnLimits({contextTokens:Number(input("context").value),autoOutput:input("auto-output").checked,maxOutputTokens:input("auto-output").checked ? DEFAULT_TURN_LIMITS.maxOutputTokens : Number(input("output").value),timeoutMs:Number(input("timeout").value)*60000,maxRounds:Number(input("rounds").value),providerRetries:Number(input("retries").value)});
  const reset = (): void => { generation++; model=null;field(dialog,"connection-model-limits").textContent="Test the connection to discover model limits."; describe(); };
  input("auto-output").addEventListener("change",()=>{input("output").disabled=input("auto-output").checked;});
  field(dialog,"connection-default-limits").addEventListener("click",()=>{fill(DEFAULT_TURN_LIMITS);status("Defaults filled. Apply limits to save.");});
  field(dialog,"connection-limits-form").addEventListener("submit",event=>{
    event.preventDefault();
    try {
      if (!same(transport.connection,current())) throw Error("Save and use this connection before applying its limits.");
      const configured=read();
      transport.setTurnLimits(configured); fill(); describe();status("Limits saved for this session. History and files are unchanged.");
    }catch(error){status(errorMessage(error));}
  });
  return {
    fill(): void { reset(); fill(); status("Test fills available context, reserving output tokens. Apply limits saves to this session."); }, reset, describe,
    testLimits: () => effectiveLimits(read(),knownModel()),
    async discover(connection: Connection, signal: AbortSignal): Promise<void> {
      const revision=++generation, originalContext=input("context").value, originalOutput=input("output").value;
      const controller=new AbortController(), abort=()=>controller.abort(signal.reason);
      signal.addEventListener("abort",abort,{once:true});if(signal.aborted)abort();
      const timer=setTimeout(()=>controller.abort(),10000);
      field(dialog,"connection-model-limits").textContent=`Fetching limits for ${connection.model}…`;
      try {
        let found: ModelLimits;
        if(isOpenRouter(connection))found=await fetchModelLimits(connection,controller.signal);
        else {
          const item=(await fetchModels(connection,controller.signal)).find(item=>item.id===connection.model);
          found={context:item?.contextTokens ?? null,output:item?.maxOutputTokens ?? null,endpoints:[],fetchedAt:Date.now()};
        }
        if(signal.aborted || revision!==generation || !same(connection,current()))return;
        model=found;
        const provider=found.endpoints.length===1 ? found.endpoints[0] : undefined;
        const providerInfo=provider ? ` Provider ${provider.name}: context ${provider.context?.toLocaleString() ?? "unknown"}, input ${provider.input?.toLocaleString() ?? "unknown"}, output ${provider.output?.toLocaleString() ?? "unknown"}.` : found.endpoints.length>1 ? ` ${found.endpoints.length} provider routes; capacities vary.` : "";
        field(dialog,"connection-model-limits").textContent=`${connection.model} · context maximum: ${found.context?.toLocaleString() ?? "not reported"} · output maximum: ${found.output?.toLocaleString() ?? "not reported"}.${providerInfo}`;
        const filled=effectiveLimits({...read(),contextTokens:found.context ? 2_000_000 : transport.turnLimits.contextTokens,maxOutputTokens:Number(originalOutput)||DEFAULT_TURN_LIMITS.maxOutputTokens},found);
        if(input("context").value===originalContext && found.context)input("context").value=String(filled.contextTokens);
        if(input("output").value===originalOutput)input("output").value=String(Math.min(filled.maxOutputTokens,MAX_OUTPUT_TOKENS));
        status(found.context ? "Model context filled with room reserved for output. Apply limits to save." : "This endpoint did not report context capacity. Enter a supported budget manually.");
      }catch{
        if(!signal.aborted && revision===generation){model=null;field(dialog,"connection-model-limits").textContent="Model limits unavailable. Connection testing continues with entered budgets and any cached model limits.";}
      }finally{clearTimeout(timer);signal.removeEventListener("abort",abort);if(!signal.aborted && revision===generation)describe();}
    },
  };
}
