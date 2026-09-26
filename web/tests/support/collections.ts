import assert from "node:assert/strict";

/** Index an array, failing the test (instead of yielding undefined) when absent. */
export function nth<T>(items: readonly T[], index: number): T {
  const value = index < 0 ? items.at(index) : items[index];
  assert.ok(value !== undefined, `missing item at index ${String(index)}`);
  return value;
}

export interface Deferred<T> {
  readonly promise: Promise<T>;
  readonly resolve: (value: T) => void;
}

export function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}
