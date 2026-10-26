import { fileSize } from "../../web/src/files/size.js";
import { nativeAvailable } from "./native-host.js";
import { h } from "../../web/lib/snabbdom/build/index.js";
import type { VNode } from "../../web/lib/snabbdom/build/index.js";
import { uiIcon } from "../../web/src/icons.js";
import type { LocalTransport } from "./local-transport.js";
import { PinnedStats } from "./pinned-stats.js";
import type { StatId } from "./pinned-stats.js";

/** Session capabilities, separate from provider connectivity or Tor circuit readiness. */
export function browserHeaderStatus(transport: LocalTransport | null, directory: string | null): string {
  const native = nativeAvailable();
  const runtime = transport?.mode === "dotnet-wasm" ? ".NET" : transport?.pythonEnabled ? "PY" : "General";
  const network = !transport?.networkAccess ? "Network off" : transport.networkMode === "tor"
    ? native ? "TOR only" : "TOR unavailable" : "Network";
  return [native ? "App" : "Browser", "WASM", runtime, network, directory ? `Directory: ${directory}` : "Virtual directory"].join(" · ");
}

export function renderBrowserStatus(transport: LocalTransport | null, pins = new PinnedStats(), refresh: () => void = () => {}): VNode {
  const metrics = transport?.metrics, turn = metrics?.turn;
  const seconds = (ms: number): string => `${(ms / 1000).toFixed(1)}s`;
  const started = (turn?.requests ?? 0) > 0;
  const network = !transport?.networkAccess ? "network off" : transport.networkMode === "tor" ? nativeAvailable() ? "Tor only" : "Tor unavailable · blocked" : nativeAvailable() ? "native HTTP" : transport.dotnetRelay ? transport.dotnetRelay.token ? "HTTP relay" : "relay token needed" : "direct network";
  const limits = transport?.effectiveTurnLimits, model = transport?.modelLimits.value;
  const tokens = (value: number | null | undefined): string => value == null ? "Not reported" : value.toLocaleString();
  const source = metrics?.context.source === "server_usage" ? "last provider input" : "CLI estimate";
  const entries: { id: StatId; label: string; value: string; brief?: string }[] = [
    { id: "context", label: "Context used / input budget", value: `${tokens(metrics?.context.tokens ?? 0)} / ${tokens(limits?.contextTokens)} (${source})`, brief: `Context: ${tokens(metrics?.context.tokens ?? 0)} / ${tokens(limits?.contextTokens)}` },
    { id: "requestUsage", label: "Last reported request input / output", value: `${tokens(metrics?.latestUsage?.input)} / ${tokens(metrics?.latestUsage?.output)}` },
    { id: "contextMax", label: "Model context maximum", value: tokens(model?.context) },
    { id: "inputBudget", label: "Input budget (effective / configured)", value: `${tokens(limits?.contextTokens)} / ${tokens(transport?.turnLimits.contextTokens)}` },
    { id: "outputBudget", label: "Output budget / model maximum", value: `${tokens(limits?.maxOutputTokens)} / ${tokens(model?.output)}${transport?.turnLimits.autoOutput ? " (automatic)" : ""}` },
    { id: "stop", label: "Stop reason", value: transport?.state.status.stop_reason ?? "Ready" },
    { id: "activity", label: "Activity", value: transport?.state.activity.kind.replaceAll("_", " ") ?? "Idle" },
    { id: "thinking", label: "Thinking", value: transport?.connection?.thinking ?? "Model default" },
    { id: "files", label: "Session files", value: String(transport?.state.status.file_count ?? 0) },
    { id: "fileBytes", label: "Session file size", value: fileSize(transport?.workspaceBytes ?? 0) },
    { id: "blocks", label: "Transcript blocks", value: String(transport?.state.blocks.blocks.length ?? 0) },
    { id: "rate", label: "Output tokens / response second", value: turn?.sums.output != null && metrics && metrics.modelMs > 0 ? `${(turn.sums.output / (metrics.modelMs / 1000)).toFixed(1)} tok/s (end-to-end)` : "Not reported" },
    { id: "timeout", label: "Turn timeout", value: limits?.timeoutMs === 0 ? "Disabled" : `${(limits?.timeoutMs ?? 0) / 60000} min` },
    { id: "rounds", label: "Maximum model rounds", value: String(limits?.maxRounds ?? "—") },
    { id: "retries", label: "Retries per request", value: String(limits?.providerRetries ?? "—") },
    { id: "loop", label: "Loop detection", value: transport?.loopDetection ?? "—" },
    { id: "connection", label: "Connection", value: transport?.connection?.name ?? "None" },
    { id: "model", label: "Model", value: transport?.connection?.model ?? "None" },
    { id: "input", label: "Input tokens", value: turn?.label("input") ?? "Not reported", brief: started ? `${turn?.label("input")} in` : "Input: —" },
    { id: "output", label: "Output tokens", value: turn?.label("output") ?? "Not reported", brief: started ? `${turn?.label("output")} out` : "Output: —" },
    { id: "cached", label: "Cached input", value: turn?.label("cached") ?? "Not reported" },
    { id: "cacheWrite", label: "Cache writes", value: turn?.label("cacheWrite") ?? "Not reported" },
    { id: "reasoning", label: "Reasoning tokens", value: turn?.label("reasoning") ?? "Not reported" },
    { id: "total", label: "Turn tokens", value: turn?.label("total") ?? "Not reported" },
    { id: "session", label: "Session tokens", value: metrics?.session.label("total") ?? "Not reported" },
    { id: "requests", label: "Requests (turn / session)", value: `${turn?.requests ?? 0} / ${metrics?.session.requests ?? 0}` },
    { id: "elapsed", label: "Turn elapsed", value: started ? seconds(metrics?.elapsed ?? 0) : "—", brief: started ? seconds(metrics?.elapsed ?? 0) : "Elapsed: —" },
    { id: "modelTime", label: "Completed model response time", value: started ? seconds(metrics?.modelMs ?? 0) : "—" },
    { id: "toolTime", label: "Tool execution", value: started ? seconds(metrics?.toolMs ?? 0) : "—" },
    { id: "python", label: "Code runtime", value: transport?.mode === "dotnet-wasm" ? `.NET 11 WASM · ${network}` : transport?.pythonEnabled ? `Pyodide · ${network}` : "Off" },
  ];
  return h("details.browser-status", [
    h("summary", { attrs: { "aria-label": "Usage and timing details" } }, [
      h("span.browser-usage-summary", pins.ids.length ? pins.ids.map(id => {
        const stat = entries.find(entry => entry.id === id)!;
        return h("span.pinned-stat", { key: id, attrs: { "data-pinned-stat": id, title: `${stat.label}: ${stat.value}` } }, stat.brief ?? `${stat.label}: ${stat.value}`);
      }) : "Open Stats to pin values here"), h("span", "Stats"),
    ]),
    h("div.browser-status-body", [
      h("p", transport?.persistenceStatus ?? ""),
      h("p", "Pin the stats you want in the bottom bar. It wraps to three lines; scroll within the bar for additional pins."),
      h("dl", entries.map(({ id, label, value }) => h("div", { key: id }, [
        h("dt", [h("span", label), h("button.stat-pin", { attrs: { type: "button", "data-stat-pin": id,
          "aria-label": `${pins.ids.includes(id) ? "Unpin" : "Pin"} ${label}`, "aria-pressed": String(pins.ids.includes(id)),
          title: `${pins.ids.includes(id) ? "Unpin from" : "Pin to"} bottom bar` },
          on: { click: () => { pins.toggle(id); refresh(); } } }, [uiIcon("pin")])]), h("dd", value),
      ]))),
      pins.error ? h("p", { attrs: { role: "status" } }, pins.error) : null,
      h("p", transport?.modelLimits.status ?? ""),
      h("button", { attrs: { type: "button", "data-refresh-model-limits": "", title: "Refresh model limits" }, on: { click: () => { void transport?.refreshModelLimits(); } } }, [uiIcon("refresh"), " Refresh limits"]),
      ...(model?.endpoints.length ? [h("details", [h("summary", "Provider context / input / output maxima"), h("ul", model.endpoints.map(endpoint => h("li", `${endpoint.name}: ${tokens(endpoint.context)} / ${tokens(endpoint.input)} / ${tokens(endpoint.output)}`)))])] : []),
      h("p", "Tokens are reported by the endpoint. Partial totals exclude requests with missing usage. Model time includes network, queueing and generation; it is not decoding speed. Context is the last request’s input, not cumulative turn tokens. Before provider usage arrives, the CLI context engine supplies an estimate (excluding tool-schema overhead). OpenRouter maxima are advertised limits, not a guarantee for every routed provider. Session budgets reserve room for output."),
    ]),
  ]);
}
