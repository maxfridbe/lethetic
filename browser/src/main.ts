import { fitViewport } from "./viewport.js";
import type { mountLethetic } from "./embed.js";

const root = document.getElementById("browser-root");
const splash = document.getElementById("startup-splash") as HTMLDialogElement;
const progress = document.getElementById("splash-progress")!;
if (!root || !splash) throw new Error("Missing application root or splash.");
fitViewport(root);
// Promote the initial static popup into the browser's modal layer before loading the UI.
splash.close(); splash.showModal();
const minimum = new Promise<void>(resolve => setTimeout(resolve, 2000));
let view: ReturnType<typeof mountLethetic> | undefined;
let ready = false;
let storage: Storage | undefined;
try { storage = localStorage; } catch { /* Private or embedded contexts may deny storage. */ }
let credentialStorage: Storage | undefined;
try { credentialStorage = sessionStorage; } catch { /* Memory-only keys when unavailable. */ }

splash.addEventListener("cancel", event => event.preventDefault());
splash.addEventListener("close", () => { if (ready) { splash.remove(); view?.activate(); } });

async function start(): Promise<void> {
  splash.querySelector("button")?.remove();
  delete splash.dataset.failed;
  splash.setAttribute("aria-busy", "true");
  const report = (message: string): void => { progress.textContent = message; };
  try {
    if (!view) {
      report("Loading application…");
      const { mountLethetic, openBrowserStorage } = await import("./embed.js");
      report("Loading saved sessions…");
      const persistent = storage ? await openBrowserStorage(storage) : undefined;
      view = mountLethetic(root!, {storage: persistent, credentialStorage, sessionRouting: true,
        startInBackground: true, compact: new URLSearchParams(location.search).get("embed") === "1"});
    }
    await view.initialize(report);
    await document.fonts.ready;
    await new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
    report("Workspace and runtimes ready.");
    await minimum;
    ready = true; splash.setAttribute("aria-busy", "false"); splash.close();
  } catch (error) {
    splash.dataset.failed = "true"; splash.setAttribute("aria-busy", "false");
    report(`Lethetic could not start: ${String(error)}. Existing data has been kept.`);
    const retry = document.createElement("button");
    retry.id = "splash-retry"; retry.type = "button"; retry.textContent = "Retry loading";
    retry.addEventListener("click", () => { void start(); });
    splash.querySelector(".splash-controls")!.append(retry); retry.focus();
  }
}
void start();
