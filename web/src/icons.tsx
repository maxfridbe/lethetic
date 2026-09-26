import { jsx as h } from "../lib/snabbdom/build/index.js";
import type { VNode } from "../lib/snabbdom/build/index.js";
import {
  SPINNER_FRAMES,
  TOOL_SPINNER_FRAMES,
  UI_ICON_GLYPHS,
} from "./generated/contracts.js";
import type { UiIconId } from "./generated/contracts.js";

export type ActivitySpinnerKind = "regular" | "tool";

export function activitySpinner(kind: ActivitySpinnerKind): VNode {
  const frames = kind === "regular" ? SPINNER_FRAMES : TOOL_SPINNER_FRAMES;
  return (
    <span
      attrs={{
        class: `activity-spinner activity-spinner-${kind}`,
        "aria-hidden": "true",
      }}
    >
      {frames.map((frame, index) => (
        <span
          key={`${kind}-${String(index)}`}
          attrs={{ class: "activity-spinner-frame" }}
        >
          {frame}
        </span>
      ))}
    </span>
  );
}

export function uiIcon(icon: UiIconId): VNode {
  return (
    <span attrs={{ class: "ui-icon", "aria-hidden": "true" }}>
      {UI_ICON_GLYPHS[icon]}
    </span>
  );
}
