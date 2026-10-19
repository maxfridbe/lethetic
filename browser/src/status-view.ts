import { h } from "../../web/lib/snabbdom/build/index.js";
import type { VNode } from "../../web/lib/snabbdom/build/index.js";
import { uiIcon } from "../../web/src/icons.js";
import type { LocalTransport } from "./local-transport.js";
import { PinnedStats } from "./pinned-stats.js";
import type { StatId } from "./pinned-stats.js";

export function renderBrowserStatus(transport: LocalTransport | null, pins = new PinnedStats(), refresh: () => void = () => {}): VNode {
  const metrics = transport?.metrics, turn = metrics?.turn;
  const seconds = (ms: number): string => `${(ms / 1000).toFixed(1)}s`;
  const started = (turn?.requests ?? 0) > 0;
  const entries: { id: StatId; label: string; value: string; brief?: string }[] = [
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
    { id: "modelTime", label: "Model responses", value: started ? seconds(metrics?.modelMs ?? 0) : "—" },
    { id: "toolTime", label: "Tool execution", value: started ? seconds(metrics?.toolMs ?? 0) : "—" },
    { id: "python", label: "Code runtime", value: transport?.mode === "dotnet-wasm" ? `.NET 11 WASM · ${transport.networkAccess ? transport.dotnetRelay ? transport.dotnetRelay.token ? "HTTP relay" : "relay token needed" : "direct network" : "network off"}` : transport?.pythonEnabled ? "Pyodide worker" : "Off" },
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
      h("p", "Tokens are reported by the endpoint. Partial totals exclude requests with missing usage. Model time includes network, queueing and generation; it is not decoding speed. Context limits and pricing are not supplied by this runtime."),
    ]),
  ]);
}
