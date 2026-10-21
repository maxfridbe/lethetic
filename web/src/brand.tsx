import { jsx as h } from "../lib/snabbdom/build/index.js";
import type { VNode } from "../lib/snabbdom/build/index.js";

/** The header emblem shares the active theme's accent and surface colors. */
export function letheticLogo(): VNode {
  const background = "var(--surface)";
  return (
    <svg attrs={{ xmlns: "http://www.w3.org/2000/svg", viewBox: "0 0 160 160", width: "30", height: "30", class: "lethetic-brand-icon", "aria-hidden": "true", focusable: "false" }}>
      <rect attrs={{ width: "160", height: "160", fill: background }} />
      <g attrs={{ transform: "translate(80, 80)", stroke: "currentColor", fill: "none" }}>
        <circle attrs={{ r: "70", "stroke-width": "5" }} />
        <g attrs={{ "stroke-width": "4.5", "stroke-linecap": "round" }}>
          <line attrs={{ x1: "-70", y1: "0", x2: "-58", y2: "0" }} />
          <line attrs={{ x1: "58", y1: "0", x2: "70", y2: "0" }} />
          <line attrs={{ x1: "0", y1: "-70", x2: "0", y2: "-58" }} />
          <line attrs={{ x1: "0", y1: "58", x2: "0", y2: "70" }} />
          <line attrs={{ x1: "-36", y1: "-26", x2: "-10", y2: "-12" }} />
          <line attrs={{ x1: "-36", y1: "0", x2: "-10", y2: "0" }} />
          <line attrs={{ x1: "-36", y1: "26", x2: "-10", y2: "12" }} />
        </g>
        <g attrs={{ fill: "currentColor", stroke: "none" }}>
          <circle attrs={{ cx: "-36", cy: "-26", r: "9" }} />
          <circle attrs={{ cx: "-36", cy: "0", r: "9.5" }} />
          <circle attrs={{ cx: "-36", cy: "26", r: "9" }} />
        </g>
        <g attrs={{ fill: background, stroke: "none" }}>
          <circle attrs={{ cx: "-36", cy: "-26", r: "3" }} />
          <circle attrs={{ cx: "-36", cy: "0", r: "3.2" }} />
          <circle attrs={{ cx: "-36", cy: "26", r: "3" }} />
        </g>
        <path attrs={{ d: "M -10,-46 C 12,-22 -10,18 12,46", stroke: background, "stroke-width": "18", "stroke-linecap": "round" }} />
        <path attrs={{ d: "M -10,-46 C 12,-22 -10,18 12,46", "stroke-width": "8.5", "stroke-linecap": "round" }} />
        <g attrs={{ "stroke-width": "3.5", "stroke-dasharray": "3 4", "stroke-linecap": "round" }}>
          <line attrs={{ x1: "18", y1: "-9", x2: "33", y2: "-15" }} />
          <line attrs={{ x1: "18", y1: "9", x2: "33", y2: "15" }} />
        </g>
        <circle attrs={{ cx: "42", cy: "-18", r: "8", "stroke-width": "4", "stroke-dasharray": "5 4" }} />
        <circle attrs={{ cx: "42", cy: "18", r: "8", "stroke-width": "4", "stroke-dasharray": "5 4" }} />
      </g>
    </svg>
  );
}
