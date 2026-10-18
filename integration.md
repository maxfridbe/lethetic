# Lethetic browser and embedding integration

The browser edition is a static application backed by `lethetic-wasm`, a portable
Rust conversation and tool orchestration runtime. It connects directly to a model
endpoint supplied by the user. GitHub Pages serves the application assets; it does
not relay model traffic or hold API keys.

## Run the hosted application

Open https://maxfridbe.github.io/lethetic/ and save a connection:

1. Choose **OpenAI-compatible** or **Anthropic Messages**.
2. Enter the complete endpoint, including a proxy path if required:
   `https://your-server/v1/chat/completions` or `https://your-server/v1/messages`.
3. Enter the exact model ID and an optional API key. Save the connection.
4. Use **Test saved connection** to send a small model request, or start chatting.
5. Enable **Python tools** to let the model execute Python with Pyodide.

Connection names, protocols, endpoint URLs, and model IDs are saved in localStorage.
API keys and chats stay in memory. Re-enter keys after reloading. Export downloads
conversation JSON, including tool inputs and results, without connection keys.
Changing connections or Python mode, or choosing New chat, clears the conversation
and Python session. Editing connection fields takes effect when you save them.

Responses appear when each model request completes; this edition currently uses
non-streaming Chat Completions and Messages. Responses API endpoints, native
filesystem tools, shells, LSP, native skills, and Podman are not part of the browser
runtime. The existing `lethetic --rc` browser controller remains available for those
native capabilities and uses its own authenticated, same-origin UI.

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
`browser-v0.1.0`) publishes an independent embedding release with those same assets.
A commit does not create a version tag automatically. The ZIP contains both standalone and embedding modes; it does
not contain API keys or downloaded Pyodide packages. Build outputs live under
`target/` and are not committed. `manifest.json` contains per-file SHA-256 hashes.

## Embed the complete UI

Download `lethetic-browser.zip` from a release and extract it under your site's
`/vendor/lethetic/` directory. Serve `.wasm` as `application/wasm` and `.js` as
JavaScript. Keep `src/` and `wasm/` at the same relative locations.

```html
<link rel="stylesheet" href="/vendor/lethetic/styles.css">
<div id="assistant"></div>
<script type="module">
  import { mountLethetic } from '/vendor/lethetic/src/embed.js';
  const view = mountLethetic(document.getElementById('assistant'), {
    compact: true,
    connections: [{
      name: 'My service', provider: 'openai', model: 'my-model',
      endpoint: 'https://models.example.com/v1/chat/completions'
    }],
    // Optional: storage: localStorage. The default embed stores nothing.
    // Optional: python: true, pyodideIndexURL: 'https://your-site/pyodide/'
  });
  // On teardown: view.destroy();
</script>
```

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
