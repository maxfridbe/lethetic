import { jsx as h } from "../../lib/snabbdom/build/index.js";
import type { VNode } from "../../lib/snabbdom/build/index.js";
import type {
  BackgroundTaskStateView,
  BackgroundTaskView,
  WebAppSnapshot,
} from "../generated/contracts.js";
import { boundedText } from "../safety.js";

export { isBackgroundTaskList, isBackgroundTaskView } from "../background-protocol.js";

const MARKS: Record<BackgroundTaskStateView, string> = {
  running: "⏳", done: "✓", failed: "✗", stopped: "■",
};

/** Ids of tasks that are finished in `next` but were running in `previous`. */
export function newlyFinished(previous: WebAppSnapshot | null, next: WebAppSnapshot): BackgroundTaskView[] {
  const running = new Set((previous?.status.background_tasks ?? [])
    .filter((task) => task.state === "running").map((task) => task.id));
  return next.status.background_tasks.filter((task) => task.state !== "running" && running.has(task.id));
}

function renderTask(task: BackgroundTaskView): VNode {
  const known = task.progress_percent !== null;
  const detail = task.state === "running"
    ? `${task.elapsed} · last activity ${task.idle} ago${task.stalled ? " · may be stalled" : ""}`
    : `${task.state_label} · ${task.elapsed}`;
  return <div key={task.id} attrs={{
    class: `background-task is-${task.state}${task.stalled ? " is-stalled" : ""}`,
    title: task.last_line === "" ? task.description : `${task.description}\n› ${task.last_line}`,
  }}>
    <span attrs={{ class: "background-task-head" }}>
      <span attrs={{ "aria-hidden": "true" }}>{MARKS[task.state]}</span>
      <strong>{task.id}</strong>
      <span attrs={{ class: "background-task-name" }}>{boundedText(task.description, 200)}</span>
    </span>
    <span attrs={{
      class: `background-task-bar${known ? "" : " is-indeterminate"}`,
      role: "progressbar",
      "aria-label": `${task.id} progress`,
      ...(known ? { "aria-valuemin": "0", "aria-valuemax": "100", "aria-valuenow": String(task.progress_percent) } : {}),
    }}>
      <span attrs={{ class: "background-task-fill" }}
        style={{ width: known ? `${task.progress_percent}%` : task.state === "running" ? "30%" : "100%" }}></span>
    </span>
    <span attrs={{ class: "background-task-detail" }}>
      {task.progress_label === "" ? detail : `${boundedText(task.progress_label, 128)} · ${detail}`}
    </span>
  </div>;
}

/** Strip of background task cards above the status row; nothing when empty. */
export function renderBackgroundTasks(snapshot: WebAppSnapshot | null): VNode | null {
  const tasks = snapshot?.status.background_tasks ?? [];
  if (tasks.length === 0) return null;
  return <section key="background-tasks" attrs={{ class: "background-tasks", "aria-label": "Background tasks" }}>
    {tasks.map(renderTask)}
  </section>;
}
