// Side-effect module: must be imported before src/transport.js is evaluated.
export const TEST_ORIGIN = "https://brainiac:11223";

Object.defineProperty(globalThis, "location", {
  configurable: true,
  value: { origin: TEST_ORIGIN, protocol: "https:" },
});
globalThis.addEventListener = () => {};
