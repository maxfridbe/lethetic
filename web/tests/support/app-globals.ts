// Side-effect module: must be imported before any application module.
// The application registers global listeners at load time; tests need no DOM.
import "./window-global.js";

globalThis.addEventListener = () => {};
