import { fitViewport } from "./viewport.js";

const root = document.getElementById("browser-root");
if (!root) throw new Error("Missing application root.");
fitViewport(root);
let storage: Storage | undefined;
try { storage = localStorage; } catch { /* Private or embedded contexts may deny storage. */ }
let credentialStorage: Storage | undefined;
try { credentialStorage = sessionStorage; } catch { /* Memory-only keys when unavailable. */ }
async function start(): Promise<void> {
  const splash = document.getElementById("startup-splash");
  // Start imports and storage immediately; the minimum display time runs alongside them.
  const minimum = new Promise<void>(resolve => setTimeout(resolve, splash ? 3000 : 0));
  root!.setAttribute("aria-busy", "true");
  try {
    const { mountLethetic, openBrowserStorage } = await import("./embed.js");
    const persistent = storage ? await openBrowserStorage(storage) : undefined;
    await minimum;
    root!.removeAttribute("aria-busy");
    mountLethetic(root!, { storage: persistent, credentialStorage, sessionRouting: true, compact: new URLSearchParams(location.search).get("embed") === "1" });
  } catch (error) {
    const message = document.createElement("p"), retry = document.createElement("button");
    message.textContent = `Lethetic could not start: ${String(error)}. Existing data has been kept.`;
    message.setAttribute("role", "alert");
    retry.textContent = "Retry loading sessions"; retry.onclick = () => { retry.disabled = true; void start(); };
    root!.removeAttribute("aria-busy");
    root!.replaceChildren(message, retry);
  }
}
void start();
