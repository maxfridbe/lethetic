// Typed Snabbdom VNode traversal helpers shared by the rendering tests.
import type { VNode } from "../../lib/snabbdom/build/index.js";

export type { VNode };

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function childVNodes(vnode: VNode): VNode[] {
  return (vnode.children ?? []).filter(
    (child): child is VNode => typeof child === "object",
  );
}

/** Pre-order traversal over element and text VNodes (string children are skipped). */
export function visit(
  vnode: VNode | null | undefined,
  callback: (candidate: VNode) => void,
): void {
  if (vnode === null || vnode === undefined) {
    return;
  }
  callback(vnode);
  for (const child of childVNodes(vnode)) {
    visit(child, callback);
  }
}

export function all(
  vnode: VNode | null | undefined,
  predicate: (candidate: VNode) => boolean,
): VNode[] {
  const matches: VNode[] = [];
  visit(vnode, (candidate) => {
    if (predicate(candidate)) {
      matches.push(candidate);
    }
  });
  return matches;
}

export function find(
  vnode: VNode | null | undefined,
  predicate: (candidate: VNode) => boolean,
): VNode | null {
  return all(vnode, predicate)[0] ?? null;
}

export function attrs(vnode: VNode): Readonly<Record<string, string | number | boolean>> {
  return vnode.data?.attrs ?? {};
}

export function attr(vnode: VNode | null | undefined, name: string): string | number | boolean | undefined {
  return vnode?.data?.attrs?.[name];
}

/** The `class` attribute, or the empty string when absent or not a string. */
export function classAttr(vnode: VNode | null | undefined): string {
  const value = attr(vnode, "class");
  return typeof value === "string" ? value : "";
}

export function hasClass(vnode: VNode, className: string): boolean {
  const value = attr(vnode, "class");
  return typeof value === "string" && value.split(/\s+/u).includes(className);
}

export function byClass(vnode: VNode, className: string): VNode[] {
  return all(vnode, (candidate) => hasClass(candidate, className));
}

export function bySelector(vnode: VNode, selector: string): VNode[] {
  return all(vnode, (candidate) => candidate.sel === selector);
}

/** Text of element and text VNodes, ignoring raw string children. */
export function textContent(vnode: VNode): string {
  if (typeof vnode.text === "string") {
    return vnode.text;
  }
  return childVNodes(vnode).map(textContent).join("");
}

/** Text of a VNode tree including raw string children. */
export function vnodeText(vnode: VNode | string): string {
  if (typeof vnode === "string") {
    return vnode;
  }
  if (typeof vnode.text === "string") {
    return vnode.text;
  }
  return (vnode.children ?? []).map(vnodeText).join("");
}

export function propValue(vnode: VNode, name: string): unknown {
  const props: unknown = vnode.data?.props;
  return isRecord(props) ? props[name] : undefined;
}

export function domPropValue(vnode: VNode, name: string): unknown {
  const domProps: unknown = vnode.data?.["domProps"];
  return isRecord(domProps) ? domProps[name] : undefined;
}

export function listener(vnode: VNode, event: string): unknown {
  const on: unknown = vnode.data?.on;
  return isRecord(on) ? on[event] : undefined;
}

/** Invoke a VNode event listener the way Snabbdom does, with a synthetic event. */
export function fire(vnode: VNode, event: string): void {
  const handler = listener(vnode, event);
  const handlers = Array.isArray(handler) ? handler : [handler];
  for (const candidate of handlers) {
    if (typeof candidate !== "function") {
      throw new Error(`VNode has no ${event} listener`);
    }
    Reflect.apply(candidate, vnode, [new Event(event), vnode]);
  }
}

export function runInitHook(vnode: VNode): void {
  const init = vnode.data?.hook?.init;
  if (init === undefined) {
    throw new Error("VNode has no init hook");
  }
  init(vnode);
}

export function runPrepatchHook(previous: VNode, next: VNode): void {
  const prepatch = next.data?.hook?.prepatch;
  if (prepatch === undefined) {
    throw new Error("VNode has no prepatch hook");
  }
  prepatch(previous, next);
}
