// Side-effect module: must be imported before any application module.
// Application modules reference `window`; under Node the global object stands
// in for it. Node's global object is not a real Window, so install it untyped.
Reflect.set(globalThis, "window", globalThis);
