/** Device-local interface sizing; independent of browser/page zoom and session state. */
export const UI_SCALES = [75, 90, 100, 110, 125, 150, 175, 200];
const KEY = "lethetic.ui-scale.v1";
const normalized = (value) => UI_SCALES.includes(value) ? value : 100;
export function uiScale() {
    return normalized(Number(document.documentElement.dataset["uiScale"] ?? 100));
}
export function setUiScale(percent, persist = true) {
    const value = normalized(percent);
    document.documentElement.dataset["uiScale"] = String(value);
    document.documentElement.style.setProperty("--ui-scale", String(value / 100));
    if (persist) {
        try {
            localStorage.setItem(KEY, String(value));
        }
        catch { /* Keep sizing usable without storage. */ }
    }
    globalThis.dispatchEvent(new Event("resize"));
}
export function restoreUiScale() {
    let value = 100;
    try {
        value = Number(localStorage.getItem(KEY) ?? 100);
    }
    catch { /* Default sizing when storage is unavailable. */ }
    setUiScale(value, false);
}
