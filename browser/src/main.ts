import { mountLethetic } from "./embed.js";

const root = document.getElementById("app");
if (!root) throw new Error("Missing application root.");
let storage: Storage | undefined;
try { storage = localStorage; } catch { /* Private or embedded contexts may deny storage. */ }
mountLethetic(root, { storage, compact: new URLSearchParams(location.search).get("embed") === "1" });
