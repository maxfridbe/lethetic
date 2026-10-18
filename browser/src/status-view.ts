import { h } from "../../web/lib/snabbdom/build/index.js";
import type { VNode } from "../../web/lib/snabbdom/build/index.js";
import type { LocalTransport } from "./local-transport.js";

export function renderBrowserStatus(transport: LocalTransport | null): VNode {
  const metrics = transport?.metrics;
  const turn = metrics?.turn;
  const seconds = (ms: number): string => `${(ms / 1000).toFixed(1)}s`;
  const started = (turn?.requests ?? 0) > 0;
  const brief = !started ? "Usage appears after your first reply" :
    `${turn?.label("input")} in · ${turn?.label("output")} out · ${seconds(metrics?.elapsed ?? 0)}`;
  const entries = [
    ["Connection", transport?.connection?.name ?? "None"],
    ["Model", transport?.connection?.model ?? "None"],
    ["Input tokens", turn?.label("input") ?? "Not reported"],
    ["Output tokens", turn?.label("output") ?? "Not reported"],
    ["Cached input", turn?.label("cached") ?? "Not reported"],
    ["Cache writes", turn?.label("cacheWrite") ?? "Not reported"],
    ["Reasoning tokens", turn?.label("reasoning") ?? "Not reported"],
    ["Turn tokens", turn?.label("total") ?? "Not reported"],
    ["Session tokens", metrics?.session.label("total") ?? "Not reported"],
    ["Requests (turn / session)", `${turn?.requests ?? 0} / ${metrics?.session.requests ?? 0}`],
    ["Turn elapsed", started ? seconds(metrics?.elapsed ?? 0) : "—"],
    ["Model responses", started ? seconds(metrics?.modelMs ?? 0) : "—"],
    ["Tool execution", started ? seconds(metrics?.toolMs ?? 0) : "—"],
    ["Python", transport?.pythonEnabled ? "Pyodide worker" : "Off"],
  ];
  return h("details.browser-status", [
    h("summary", { attrs: { "aria-label": "Usage and timing details" } }, [
      h("span", { attrs: { class: "browser-usage-summary" } }, brief), h("span", "Stats"),
    ]),
    h("div.browser-status-body", [
      h("dl", entries.map(([label, value]) => h("div", [h("dt", label ?? ""), h("dd", value ?? "")]))),
      h("p", "Tokens are reported by the endpoint. Partial totals exclude requests with missing usage. Model time includes network, queueing and generation; it is not decoding speed. Context limits and pricing are not supplied by this runtime."),
    ]),
  ]);
}
