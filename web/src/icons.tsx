import { jsx as h } from "../lib/snabbdom/build/index.js";
import type { VNode } from "../lib/snabbdom/build/index.js";
import {
  SPINNER_FRAMES,
  TOOL_SPINNER_FRAMES,
  UI_ICON_GLYPHS,
  ICON_GLYPHS,
} from "./generated/contracts.js";
// UI-only actions do not change the native protocol's icon vocabulary.
export const BUTTON_ICONS = { ...ICON_GLYPHS, ...UI_ICON_GLYPHS,
  microphone: "\uf130", run: "\uf04b", site: "\uf0ac",
  keyboard: "\uf11c", persona: "\uf118", timer: "\uf017", compact: "\uf066",
  terminal: "\uf120", python: "\ue73c", dotnet: "\ue70c", chip: "\uf2db",
  desktop: "\uf108", network: "\uf0e8", lock: "\uf023", tor: "\uf21b",
  tasks: "\uf0ae", tools: "\uf0ad", book: "\uf02d", server: "\uf233",
  fullscreen: "\uf065", restore: "\uf066", previous: "\uf053", next: "\uf054",
  upload: "\uf093", pin: "\uf08d", copy: "\uf0c5", preview: "\uf06e", folder: "\uf07b", file: "\uf15b",
  refresh: "\uf021", connection: "\uf1e6", history: "\uf1da", code: "\uf121",
  expand: "\uf078", collapse: "\uf054", columns: "\uf0db", list: "\uf03a",
} as const;
export type ButtonIcon = keyof typeof BUTTON_ICONS;

/** For the small native-dialog templates; glyphs are static trusted constants. */
export function buttonIconHtml(icon: ButtonIcon): string {
  return `<span class="ui-icon" aria-hidden="true">${BUTTON_ICONS[icon]}</span>`;
}

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

export function uiIcon(icon: ButtonIcon): VNode {
  return (
    <span attrs={{ class: "ui-icon", "aria-hidden": "true" }}>
      {BUTTON_ICONS[icon]}
    </span>
  );
}


/** Nerd Font file-type glyphs, using only the filename extension. */
export function fileTypeIcon(path: string): VNode {
  const extension = path.split("/").at(-1)?.split(".").at(-1)?.toLowerCase() ?? "";
  const types: Record<string, readonly [string, string]> = {
    py: ["\ue73c", "Python"], pyw: ["\ue73c", "Python"], ipynb: ["\ue73c", "Notebook"],
    js: ["\ue74e", "JavaScript"], mjs: ["\ue74e", "JavaScript"], jsx: ["\ue74e", "JavaScript"],
    ts: ["\ue628", "TypeScript"], tsx: ["\ue628", "TypeScript"], rs: ["\ue7a8", "Rust"],
    md: ["\ue73e", "Markdown"], markdown: ["\ue73e", "Markdown"],
    json: ["\ue60b", "JSON"], html: ["\uf13b", "HTML"], htm: ["\uf13b", "HTML"], css: ["\uf13c", "CSS"],
    csv: ["\uf1c3", "Spreadsheet"], tsv: ["\uf1c3", "Spreadsheet"], xlsx: ["\uf1c3", "Spreadsheet"],
    png: ["\uf1c5", "Image"], jpg: ["\uf1c5", "Image"], jpeg: ["\uf1c5", "Image"],
    gif: ["\uf1c5", "Image"], webp: ["\uf1c5", "Image"], svg: ["\uf1c5", "SVG"],
    pdf: ["\uf1c1", "PDF"], zip: ["\uf1c6", "Archive"], gz: ["\uf1c6", "Archive"], tar: ["\uf1c6", "Archive"],
    txt: ["\uf15c", "Text"], log: ["\uf15c", "Log"],
    yml: ["\uf013", "Configuration"], yaml: ["\uf013", "Configuration"], toml: ["\uf013", "Configuration"],
  };
  const [glyph, label] = types[extension] ?? [BUTTON_ICONS.file, "File"];
  return <span attrs={{ class: "ui-icon file-type-icon", "aria-hidden": "true", title: label }}>{glyph}</span>;
}

/** Distinct UI glyphs without widening the remote command protocol. */
export function commandIcon(id: string, fallback: ButtonIcon): ButtonIcon {
  if (id === "local:dictation") return "microphone";
  const icons: Record<string, ButtonIcon> = {
    hotkeys: "keyboard", themes: "theme", "input-history": "history",
    "loop-detection": "refresh", "system-prompt": "edit", "clear-ui": "trash",
    "clear-context": "trash", "toggle-debugger": "debug", "toggle-todos": "tasks",
    sessions: "history", "name-session": "edit", "latest-files": "folder",
    "toggle-background-tasks": "tasks", "background-mode": "processing",
    "tool-call-mode": "tools", skills: "book", models: "model",
    "lsp-servers": "server", "agent-mode": "terminal", "agent-general": "terminal",
    "python-isolated": "python", "python-nonlocal": "python", "python-permissive": "python",
    "remote-control": "connection", "delete-python-runtime": "trash", quit: "quit",
    personas: "persona", "persona-import": "upload", "persona-export": "download",
    "turn-limits": "timer", "compact-session": "compact",
  };
  return icons[id.replace(/^local:/u, "")] ?? fallback;
}
