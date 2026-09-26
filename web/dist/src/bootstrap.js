// This module intentionally has no static imports. It executes before the
// application graph is fetched so the controller credential cannot remain in
// the address bar when a later module is blocked or fails to load.
const bootstrapFragment = globalThis.location.hash;
if (bootstrapFragment.length > 0) {
    const cleanUrl = new URL(globalThis.location.origin);
    cleanUrl.pathname = globalThis.location.pathname;
    cleanUrl.search = globalThis.location.search;
    cleanUrl.hash = "";
    globalThis.history.replaceState(null, "", cleanUrl);
}
void import("./main.js")
    .then((application) => {
    application.bootstrap(bootstrapFragment);
})
    .catch(() => {
    const root = globalThis.document.getElementById("app");
    if (root !== null) {
        root.textContent = "Lethetic web could not load.";
        root.setAttribute("aria-busy", "false");
    }
});
export {};
