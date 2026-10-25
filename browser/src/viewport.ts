/** Follow both browser visual-viewport changes and native WebView layout resizing.
 * Only the standalone full-page mount opts in; embedded hosts own their sizing.
 */
export function fitViewport(root: HTMLElement, viewport = globalThis.visualViewport): () => void {
  let fullHeight = globalThis.innerHeight, width = globalThis.innerWidth;
  const update = (): void => {
    // Accessibility zoom overrides are not keyboard resizes.
    if (viewport && Math.abs(viewport.scale - 1) > .01) return;
    const layoutHeight = globalThis.innerHeight;
    const editing = document.activeElement?.matches('textarea, input:not([type=checkbox]):not([type=radio]):not([type=range]):not([type=button]):not([type=submit]), [contenteditable="true"]') ?? false;
    if (width !== globalThis.innerWidth) fullHeight = layoutHeight;
    else fullHeight = Math.max(fullHeight, layoutHeight);
    width = globalThis.innerWidth;
    // Native adjustResize can shrink innerHeight before visualViewport catches up.
    const height = Math.min(viewport?.height ?? layoutHeight, layoutHeight);
    const top = Math.max(0, Math.min(viewport?.offsetTop ?? 0, layoutHeight - height));
    root.style.height = `${height}px`;
    root.style.top = `${top}px`;
    // Android shrinks BOTH viewports, so comparing them to each other misses its IME.
    root.classList.toggle("is-keyboard-open", editing && fullHeight - height > 120);
    document.documentElement.style.setProperty("--browser-visible-height", `${height}px`);
    document.documentElement.style.setProperty("--browser-visible-top", `${top}px`);
  };
  viewport?.addEventListener("resize", update);
  viewport?.addEventListener("scroll", update);
  globalThis.addEventListener("resize", update);
  document.addEventListener("focusin", update);
  document.addEventListener("focusout", update);
  update();
  return () => {
    viewport?.removeEventListener("resize", update); viewport?.removeEventListener("scroll", update);
    globalThis.removeEventListener("resize", update);
    document.removeEventListener("focusin", update); document.removeEventListener("focusout", update);
    root.style.removeProperty("height"); root.style.removeProperty("top");
    root.classList.remove("is-keyboard-open");
    document.documentElement.style.removeProperty("--browser-visible-height");
    document.documentElement.style.removeProperty("--browser-visible-top");
  };
}
