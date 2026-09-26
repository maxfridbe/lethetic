import { jsx as h } from "../../lib/snabbdom/build/index.js";
import type { VNode } from "../../lib/snabbdom/build/index.js";
import type {
  DebuggerView,
  DiagnosticView,
} from "../generated/contracts.js";
import { boundedText } from "../safety.js";
import type { ChatViewContext } from "./state.js";

function diagnosticCodeLabel(code: DiagnosticView["code"]): string {
  return code.replaceAll("_", " ");
}

function renderDebuggerContent(
  debuggerState: DebuggerView,
  close: () => void,
  autofocus: boolean,
): readonly VNode[] {
  return [
    <header attrs={{ class: "debugger-header" }}>
      <div>
        <span attrs={{ class: "eyebrow" }}>Operational diagnostics</span>
        <strong>Debugger</strong>
      </div>
      <button
        attrs={{
          type: "button",
          class: "text-button debugger-close",
          "aria-label": "Close debugger",
          title: "Close debugger",
          ...(autofocus ? { "data-autofocus": "true" } : {}),
        }}
        on={{ click: close }}
      >
        Close
      </button>
    </header>,
    <p attrs={{ class: "debugger-summary" }}>
      {boundedText(debuggerState.summary)}
    </p>,
    debuggerState.omitted_before === 0 ? (
      <span attrs={{ class: "debugger-omission-placeholder", "aria-hidden": "true" }}></span>
    ) : (
      <p attrs={{ class: "bounded-notice" }}>
        {`${debuggerState.omitted_before} older diagnostic event(s) were omitted.`}
      </p>
    ),
    debuggerState.entries.length === 0 ? (
      <p attrs={{ class: "empty-state debugger-empty" }}>
        No browser-safe operational events have been recorded.
      </p>
    ) : (
      <ol attrs={{ class: "debugger-entries" }}>
        {debuggerState.entries.map((entry, index) => (
          <li
            key={`diagnostic-${index}-${entry.code}`}
            attrs={{ class: `diagnostic diagnostic-${entry.severity}` }}
          >
            <span attrs={{ class: "diagnostic-heading" }}>
              <strong>{diagnosticCodeLabel(entry.code)}</strong>
              <span>{entry.severity}</span>
            </span>
            <p>{boundedText(entry.message)}</p>
          </li>
        ))}
      </ol>
    ),
  ];
}

export function renderDebugger(context: ChatViewContext): VNode | null {
  const snapshot = context.state.snapshot;
  if (
    snapshot === null ||
    !snapshot.debugger.open ||
    (!context.state.debuggerWide && context.state.debuggerDrawerDismissed)
  ) {
    return null;
  }
  const close = context.actions.toggleDebugger;
  const covered = !context.state.debuggerWide && context.state.activePanel !== null;
  const content = renderDebuggerContent(
    snapshot.debugger,
    close,
    !context.state.debuggerWide && !covered,
  );
  if (context.state.debuggerWide) {
    return (
      <aside
        attrs={{
          class: "debugger-pane",
          "aria-label": "Debugger",
        }}
      >
        {content}
      </aside>
    );
  }
  return (
    <div
      attrs={{
        class: `debugger-drawer-layer${covered ? " is-covered" : ""}`,
        ...(covered ? { "aria-hidden": "true", inert: "" } : {}),
      }}
    >
      <button
        attrs={{
          type: "button",
          class: "debugger-backdrop",
          "aria-label": "Close debugger",
          tabindex: "-1",
        }}
        on={{ click: close }}
      ></button>
      <aside
        attrs={{
          class: "debugger-drawer",
          role: "dialog",
          "aria-label": "Debugger",
          ...(covered
            ? {}
            : { "aria-modal": "true", "data-focus-trap": "true" }),
        }}
      >
        {content}
      </aside>
    </div>
  );
}
