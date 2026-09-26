// Side-effect module: must be imported before the file-pane modules are evaluated.
// Only requestAnimationFrame is provided so any other window access fails loudly.
Reflect.set(globalThis, "window", {
  requestAnimationFrame: (callback: () => void) => setTimeout(callback, 0),
});
