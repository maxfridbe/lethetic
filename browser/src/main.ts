import { mountLethetic } from "./embed.js";
import { fitViewport } from "./viewport.js";

const root = document.getElementById("browser-root");
if (!root) throw new Error("Missing application root.");
fitViewport(root);
let storage: Storage | undefined;
try { storage = localStorage; } catch { /* Private or embedded contexts may deny storage. */ }
let credentialStorage: Storage | undefined;
try { credentialStorage = sessionStorage; } catch { /* Memory-only keys when unavailable. */ }
mountLethetic(root, { storage, credentialStorage, sessionRouting: true, compact: new URLSearchParams(location.search).get("embed") === "1" });
