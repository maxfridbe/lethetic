import { mountLethetic, openBrowserStorage } from "./embed.js";
import { fitViewport } from "./viewport.js";

const root = document.getElementById("browser-root");
if (!root) throw new Error("Missing application root.");
fitViewport(root);
let storage: Storage | undefined;
try { storage = localStorage; } catch { /* Private or embedded contexts may deny storage. */ }
let credentialStorage: Storage | undefined;
try { credentialStorage = sessionStorage; } catch { /* Memory-only keys when unavailable. */ }
async function start(): Promise<void> {
  root!.textContent = "Loading saved sessions…";
  try {
    const persistent = storage ? await openBrowserStorage(storage) : undefined;
    mountLethetic(root!, { storage: persistent, credentialStorage, sessionRouting: true, compact: new URLSearchParams(location.search).get("embed") === "1" });
  } catch (error) {
    const message = document.createElement("p"), retry = document.createElement("button");
    message.textContent = `Saved sessions could not be loaded: ${String(error)}. Existing data has been kept.`;
    retry.textContent = "Retry loading sessions"; retry.onclick = () => { void start(); };
    root!.replaceChildren(message, retry);
  }
}
void start();
