import type { BackgroundTaskStateView, BackgroundTaskView } from "./generated/contracts.js";
import { isBoolean, isFiniteInteger, isRecord, isString } from "./safety.js";

const STATES: readonly BackgroundTaskStateView[] = ["running", "done", "failed", "stopped"];
const KEYS = [
  "id", "description", "state", "state_label", "progress_percent", "progress_label",
  "elapsed", "idle", "stalled", "last_line",
] as const;

export function isBackgroundTaskView(value: unknown): value is BackgroundTaskView {
  if (!isRecord(value) || Object.keys(value).length !== KEYS.length ||
      !KEYS.every((key) => Object.hasOwn(value, key))) return false;
  const percent = value["progress_percent"];
  return /^bg[0-9]{1,9}$/u.test(String(value["id"])) &&
    isString(value["description"]) &&
    typeof value["state"] === "string" && (STATES as readonly string[]).includes(value["state"]) &&
    isString(value["state_label"]) &&
    (percent === null || (isFiniteInteger(percent) && percent >= 0 && percent <= 100)) &&
    isString(value["progress_label"]) && isString(value["elapsed"]) && isString(value["idle"]) &&
    isBoolean(value["stalled"]) && isString(value["last_line"]);
}

export function isBackgroundTaskList(value: unknown): value is BackgroundTaskView[] {
  return Array.isArray(value) && value.length <= 20 && value.every(isBackgroundTaskView);
}
