import { fitViewport } from "./viewport.js";
import type { mountLethetic } from "./embed.js";

const root = document.getElementById("browser-root");
const splash = document.getElementById("startup-splash") as HTMLDialogElement;
const progress = document.getElementById("splash-progress")!;
const proceed = document.getElementById("splash-continue") as HTMLButtonElement;
if (!root || !splash || !proceed) throw new Error("Missing application root or splash.");
fitViewport(root);
// Promote the initial static popup into the browser's modal layer before loading the UI.
splash.close(); splash.showModal();
const minimum = new Promise<void>(resolve => setTimeout(resolve, 3000));
let view: ReturnType<typeof mountLethetic> | undefined;
let ready = false, failed = false;
let storage: Storage | undefined;
try { storage = localStorage; } catch { /* Private or embedded contexts may deny storage. */ }
let credentialStorage: Storage | undefined;
try { credentialStorage = sessionStorage; } catch { /* Memory-only keys when unavailable. */ }

const dismiss = (): void => { if (ready) splash.close(); };
splash.addEventListener("cancel", event => { if (!ready) event.preventDefault(); });
splash.addEventListener("click", event => {
  if (ready && event.target instanceof Node && splash.contains(event.target)) dismiss();
});
splash.addEventListener("close", () => { if (ready) { splash.remove(); view?.activate(); } });
proceed.addEventListener("click", () => { if (failed) void start(); else dismiss(); });

async function start(): Promise<void> {
  failed = false; proceed.disabled = true; proceed.textContent = "Loading…";
  splash.setAttribute("aria-busy", "true");
  const report = (message: string): void => { progress.textContent = message; };
  try {
    if (!view) {
      report("Loading application and saved sessions…");
      const { mountLethetic, openBrowserStorage } = await import("./embed.js");
      const persistent = storage ? await openBrowserStorage(storage) : undefined;
      view = mountLethetic(root!, {storage: persistent, credentialStorage, sessionRouting: true,
        startInBackground: true, compact: new URLSearchParams(location.search).get("embed") === "1"});
    }
    await view.initialize(report);
    await document.fonts.ready;
    await new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
    report("Workspace and runtimes ready.");
    await minimum;
    ready = true; splash.dataset.ready = "true"; splash.setAttribute("aria-busy", "false");
    proceed.textContent = "Click to continue"; proceed.disabled = false; proceed.focus();
  } catch (error) {
    failed = true; splash.setAttribute("aria-busy", "false");
    report(`Lethetic could not start: ${String(error)}. Existing data has been kept.`);
    proceed.textContent = "Retry loading"; proceed.disabled = false; proceed.focus();
  }
}
void start();
