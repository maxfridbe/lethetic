# Lethetic browser and embedding integration

The browser edition is a static application backed by `lethetic-wasm`, a portable
Rust conversation and tool orchestration runtime. It connects directly to a model
endpoint supplied by the user. GitHub Pages serves the application assets; it does
not relay model traffic or hold API keys.

**[Try it live](https://maxfridbe.github.io/lethetic/) · [Embed the UI](#embed-the-complete-ui) · [Run locally](#build-and-test)**

Standalone and embedded browser modes reuse the existing remote-mode
`SpaApplication`, chat renderer, command palette, themes, debugger, and styles.
A local transport connects that UI to WASM through its existing typed commands and
snapshots. The native remote controller continues to use its WebSocket transport.

The browser footer shows endpoint-reported input/output tokens and measured turn
time. Expand **Stats** for cache and reasoning counts, turn/session totals, request
counts, model response time, and tool time. Missing or invalid usage is **Not
reported**; totals are marked **partial** when some requests have no usage. Clearing
the chat or switching connections resets these totals. Model time includes network,
queueing, and generation; it is not a decoding-speed estimate. Native-only counters
(memory, Git, containers), unknown context limits, and price estimates are omitted.
Usage supports [DeepSeek cache fields](https://api-docs.deepseek.com/guides/kv_cache/),
OpenAI-compatible token details, and Anthropic cache read/write accounting.

On phones the prompt and Send button share a row, and the full-page application
tracks the visual viewport when the keyboard opens or pans the page. The viewport
also opts into [keyboard-driven layout resizing](https://developer.chrome.com/blog/viewport-resize-behavior).
Embedded hosts retain control of their own container sizing.

## Run the hosted application

Open https://maxfridbe.github.io/lethetic/ and save a connection:

1. Choose **OpenAI-compatible** or **Anthropic Messages**.
2. Enter the complete endpoint, including a proxy path if required:
   `https://your-server/v1/chat/completions` or `https://your-server/v1/messages`.
3. Enter the exact model ID and an optional API key. Save the connection.
4. Use **Test saved connection** to send a small model request, then close the dialog to chat.
5. Enable **Python tools** to let the model execute Python with Pyodide.

Connection names, protocols, endpoint URLs, and model IDs are saved in localStorage.
API keys stay in memory. Re-enter keys after reloading. Chats are saved locally.
Changing connections or Python mode starts a new session, keeping the previous one
in **Sessions**. Clearing context resets the current conversation and Python runtime. Editing connection fields takes effect when you save them.

Responses appear when each model request completes; this edition currently uses
non-streaming Chat Completions and Messages. Responses API endpoints, native
filesystem tools, shells, LSP, native skills, and Podman are not part of the browser
runtime. The existing `lethetic --rc` browser controller remains available for those
native capabilities through the same UI backed by its authenticated native connection.

## Files, previews, and downloads

Use the existing **Files** directory browser. Choose **Save to Files** beneath a
code block or Python tool call, or **New file** in Files. Enter a relative path such
as `reports/analysis.py`. Folders are derived from these paths. Select a file to
view its source in Monaco, **Edit**, **Preview**, **Download File**, **Copy contents**,
or **Delete** it. Code blocks also have direct Preview, Download, and Copy buttons.
Saving is explicit; file contents are not automatically sent to the model.

Previews render static HTML, SVG and Markdown; JSON is formatted and other code is
shown as text. They use an opaque sandbox and a restrictive content policy:
scripts, forms, external resources and navigation are disabled. Download the file
to use it elsewhere. This is a static preview, not a JavaScript app runner.

Files are **unencrypted** in localStorage under `lethetic.files.v1`: up to 100 files,
1 MiB per file and 4 MiB total. No passphrase or encrypted-library UI is involved.
Older encrypted records are left untouched and are not automatically migrated.
A failed save preserves the previous record. Browser storage is device/origin
specific; clearing site data deletes it. Download files you want to keep elsewhere.

## Saved sessions and URL routing

**Sessions** uses the existing session screen to resume, name, create, or delete
chats. Completed turns automatically save their transcript, model context, usage
stats, system prompt, Python mode, theme and connection metadata in
`lethetic.sessions.v1`. API keys are excluded. Reopening a chat restores its context
without rerunning tools. Python variables and virtual files are not restored.

The hosted page supports these routes without requiring server-side rewrites:

- `#/sessions/<uuid>` opens that saved session; refresh keeps the same session.
- `#/latest`, or no fragment, opens the most recently saved session, or creates one.
- `#/new` creates a new session and replaces the fragment with its UUID.

Session links identify local records; they do not share conversation contents with
another device. Missing sessions show an error and never silently replace an
existing chat. Storage holds up to 50 sessions / 4 MiB. Save failures appear in
Sessions and Stats; delete old sessions to free space. Old chats are never silently
evicted. Refreshing during a running turn restores the last saved state. Sessions
and files are stored plainly on this device, with no encryption for now.

## Endpoint requirements

The model server or your proxy must allow the page's origin, for example
`https://maxfridbe.github.io`, with CORS. The repository path `/lethetic/` is not part
of the origin. Allow `POST`, `OPTIONS`, and the required request headers:

- OpenAI-compatible: `Content-Type`, `Authorization`.
- Anthropic: `Content-Type`, `x-api-key`, `anthropic-version`,
  `anthropic-dangerous-direct-browser-access`.

The SDK omits cookies and refuses redirects. Use the final HTTPS endpoint and a
valid certificate. HTTP loopback endpoints work with an HTTP local development
page; the hosted HTTPS app requires HTTPS endpoints. Endpoints containing userinfo,
query parameters, or fragments are rejected, so credentials cannot be saved in a URL.
For servers requiring query parameters or another authentication scheme, expose a
compatible proxy endpoint or provide a custom SDK `fetch` implementation.

Browser CORS and TLS rules still apply to Pyodide and to WASM: neither bypasses
them. An SDK connection key is sent only in the authentication header to the selected
endpoint. Supply a user-owned or short-lived scoped key. For applications serving
multiple users, keep shared service credentials on your own authenticated proxy.

## Build and test

Prerequisites: current stable Rust, Node.js 22+, Python 3, TypeScript 7.0.2, and
wasm-bindgen CLI 0.2.128 (must match the Rust dependency).

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.128 --locked
npm install --global typescript@7.0.2
./build-browser.sh
python3 -m http.server 8080 --directory target/browser
```

Open http://localhost:8080/. Do not open the HTML through `file://`.
The build runs native Rust unit tests, compiles the actual WASM module, checks
TypeScript, and runs SDK integration tests against that module in Node, plus worker
lifecycle tests. No provider keys or paid requests are needed for those tests.

For browser acceptance tests, install Chrome/Chromium and run:

```bash
CHROME_BIN=/path/to/chrome python3 scripts/test_browser.py
```

These tests use a disposable browser profile, a local mock provider, and a project
subpath to exercise loading, connections, chat, embedding, and Python worker behavior.
Add `--pyodide` to additionally download the pinned Pyodide runtime from its CDN and
test real Python execution, persistence, error output, cancellation, and restart.
The optional test needs network access; the default suite uses a local fixture for
the worker's external Python dependency.

## GitHub Pages and releases

In repository **Settings → Pages → Build and deployment**, select **GitHub Actions**.
`.github/workflows/browser.yml` builds and tests pull requests, deploys pushes to
`master`, and can be run manually. Relative module, stylesheet, and WASM URLs work
at `/lethetic/`, custom subpaths, and domain roots. Pages deployment requires the
`github-pages` environment to allow the deploying branch. See GitHub's
[custom workflow instructions](https://docs.github.com/en/pages/getting-started-with-github-pages/using-custom-workflows-with-github-pages).

Pushing an ordinary commit deploys Pages. Pushing a `v*` tag additionally attaches
`lethetic-browser.zip` and `lethetic-browser.zip.sha256` to the GitHub release,
alongside the existing native release workflow. A `browser-v*` tag (for example,
`browser-v0.3.0`) publishes an independent embedding release with those same assets.
A commit does not create a version tag automatically. The ZIP contains both standalone and embedding modes; it does
not contain API keys or downloaded Pyodide packages. Build outputs live under
`target/` and are not committed. `manifest.json` contains per-file SHA-256 hashes.

## Embed the complete UI

Download `lethetic-browser.zip` from a release and extract it under your site's
`/vendor/lethetic/` directory. Serve `.wasm` as `application/wasm` and `.js` as
JavaScript. Keep the extracted `src/`, `browser/`, and `web/` trees and stylesheets
together. The `src/` entry points remain stable; the shared UI and libraries live
under `web/`, and the WASM module lives under `browser/wasm/`.

```html
<link rel="stylesheet" href="/vendor/lethetic/styles.css">
<div id="assistant"></div>
<script type="module">
  import { mountLethetic } from '/vendor/lethetic/src/embed.js';
  const view = mountLethetic(document.getElementById('assistant'), {
    connections: [{
      name: 'My service', provider: 'openai', model: 'my-model',
      endpoint: 'https://models.example.com/v1/chat/completions'
    }],
    storage: localStorage, // Saves connections, sessions and files unencrypted.
    // Optional: sessionRouting: true to use the session hash routes.
    // Optional: python: true, pyodideIndexURL: 'https://your-site/pyodide/'
  });
  // On teardown: view.destroy();
</script>
```

Mount one UI per document: the shared UI owns document-level keyboard shortcuts,
focus handling, theme variables, and styles. Give its container a definite height
(for example, `height: 100vh` in your stylesheet). Use an iframe for isolated styles
or multiple instances. Omit `storage` to keep files and sessions only in memory.
Session routing is off by default so the embed does not take over its host URL. `view.destroy()` aborts pending
work, terminates Python, removes dialogs, and releases application listeners.

An iframe also works: `<iframe src="https://maxfridbe.github.io/lethetic/?embed=1"
title="Lethetic" width="100%" height="900"></iframe>`. Users configure connections
inside the iframe. No keys are accepted via query parameters or `postMessage`.
An iframe sandbox needs `allow-scripts allow-same-origin allow-downloads` for the
application, WASM, and exports; browser storage policy may still limit persistence.

## Use the headless embedding SDK

```javascript
import { createRuntime, createPythonTool } from '/vendor/lethetic/src/sdk.js';

const python = createPythonTool(); // Optional persistent Pyodide tool.
const agent = await createRuntime({
  connection: {
    name: 'My model', provider: 'openai',
    endpoint: 'https://models.example.com/v1/chat/completions',
    model: 'my-model', apiKey: userSuppliedKey,
  },
  systemPrompt: 'You are a careful analysis assistant. Use Python for calculations.',
  tools: [python, {
    name: 'lookup_record',
    description: 'Read a record that the current user can access.',
    parameters: {
      type: 'object', properties: { id: { type: 'string' } }, required: ['id'],
    },
    async execute(args, signal) {
      if (typeof args.id !== 'string') throw new Error('Expected a string ID');
      const response = await fetch('/api/records/' + encodeURIComponent(args.id), { signal });
      if (!response.ok) throw new Error('Record lookup failed');
      return response.text();
    },
  }],
  onEvent(event) { console.log(event.type); },
  maxRounds: 12,
  timeoutMs: 180000,
});

try {
  const reply = await agent.send('Calculate the first ten Fibonacci numbers.');
  console.log(reply);
  // agent.history() returns conversation JSON data without credentials.
  // agent.restore(savedHistory) restores complete turns while idle; it never runs tools.
  // agent.cancel() stops a turn; agent.clear() clears an idle conversation.
} finally {
  agent.dispose();
  python.reset();
}
```

Host tool handlers must validate argument fields and authorize their effects.
WASM verifies registered tool names, unique call IDs, JSON objects, and result
ordering; it does not implement full JSON Schema validation. Tools run sequentially.
Thrown tool errors stop the turn. Return an error string if the model should recover.
Stopping, timeout, malformed responses, or exhausting the round limit rolls back the
incomplete user turn. Already executed host effects are not undone or automatically
retried. Each conversation allows one in-flight turn. Event observer exceptions are
isolated from the runtime. Responses and assembled request bodies are capped at
4 MiB; the message count is capped at 512. Start a new chat when full.

The SDK supports injected `fetch` and `wasm` bytes for controlled hosts and Node.
It exports TypeScript declarations. Import the ESM entry files directly; no npm
installation or bundler is required. The native web-controller assets in `web/dist`
are a separate build and are not required by this package.

## Python mode with Pyodide

The browser tool uses [Pyodide 0.29.3](https://pyodide.org/en/0.29.3/usage/quickstart.html),
loaded lazily from `https://cdn.jsdelivr.net/pyodide/v0.29.3/full/`. It runs in a
dedicated module Web Worker with persistent globals and a virtual filesystem.
Use it for calculations, CSV/JSON processing, NumPy/pandas analysis, and Matplotlib
plots in its virtual filesystem. That filesystem is separate from the saved Files
browser; this edition returns text and does not automatically export Python files.
Stdout, stderr, and the final expression are returned to the model, capped at 64 KiB.
Python exceptions are returned as tool output. Standard input receives EOF.
Packages bundled with Pyodide are loaded from imports; arbitrary native pip packages
and OS processes are not available. The initial load can take time and needs network
access. Self-host a complete compatible distribution and pass `indexURL` to
`createPythonTool` or `pyodideIndexURL` to `mountLethetic` to control asset delivery.

Cancellation and the 120-second Python timeout terminate the worker, so even an
infinite loop can be stopped without SharedArrayBuffer, COOP, or COEP headers.
The next execution creates a fresh worker. Variables and files are lost on stop,
timeout, reset, or page reload. The entire SDK turn also has its own timeout.

Python code can access browser networking and JavaScript worker APIs. A worker is
an execution isolation mechanism, not a security boundary for hostile code. It
receives source and a runtime asset URL, never model API keys or conversation
history. Enable Python only when you intend to run model-generated code. Embedded
hosts should apply their own approval policy if they require per-call consent.

For CSP, allow your own scripts, `'wasm-unsafe-eval'`, the Pyodide module origin,
`worker-src 'self'`, and `connect-src` for both model and runtime asset origins.
The standalone app includes an appropriate meta CSP for its default CDN. Adapt it
when self-hosting Pyodide. No `unsafe-eval` or cross-origin isolation is required.
