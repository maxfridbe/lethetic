/** Follow the visible viewport when a mobile keyboard shrinks or pans it.
 * Only the standalone full-page mount opts in; embedded hosts own their sizing.
 */
export function fitViewport(root: HTMLElement, viewport = globalThis.visualViewport): () => void {
  const update = (): void => {
    // Preserve pinch zoom and its normal panning behavior.
    if (viewport && viewport.scale !== 1) return;
    root.style.height = `${viewport?.height ?? globalThis.innerHeight}px`;
    root.style.top = `${viewport?.offsetTop ?? 0}px`;
    root.classList.toggle("is-keyboard-open", (viewport?.height ?? globalThis.innerHeight) < globalThis.innerHeight - 120);
    document.documentElement.style.setProperty("--browser-visible-height", `${viewport?.height ?? globalThis.innerHeight}px`);
    document.documentElement.style.setProperty("--browser-visible-top", `${viewport?.offsetTop ?? 0}px`);
  };
  viewport?.addEventListener("resize", update);
  viewport?.addEventListener("scroll", update);
  globalThis.addEventListener("resize", update);
  update();
  return () => {
    viewport?.removeEventListener("resize", update); viewport?.removeEventListener("scroll", update);
    globalThis.removeEventListener("resize", update);
    root.style.removeProperty("height"); root.style.removeProperty("top");
    root.classList.remove("is-keyboard-open");
    document.documentElement.style.removeProperty("--browser-visible-height");
    document.documentElement.style.removeProperty("--browser-visible-top");
  };
}
