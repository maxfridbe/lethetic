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

See the [architecture diagrams](docs/architecture.md) for isolation boundaries,
remote-control flow, and browser-only execution, storage, and virtual sites.

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

The standalone page shows an animated popup at 70% of the viewport's width and
height, with the workspace visible behind it. It initializes the shared CLI WASM
core and the saved session's selected Pyodide or .NET runtime, restoring session
files without running saved programs. The popup closes automatically after
initialization and a two-second minimum; no click or continue button is needed.
Loading stages appear in the popup and workspace status. Startup failures offer
Retry. Local SVG/CSS respects reduced-motion preferences. Embedded `mountLethetic`
hosts retain their own loading UI; optionally use `startInBackground: true`, await
`view.initialize(onProgress)`, then call `view.activate()` after dismissing your overlay.

Open https://maxfridbe.github.io/lethetic/ and save a connection:

1. Choose **OpenAI-compatible** or **Anthropic Messages**.
2. Enter the complete endpoint, including a proxy path if required:
   `https://your-server/v1/chat/completions` or `https://your-server/v1/messages`.
3. Enter the exact model ID and an optional API key. Save the connection.
4. Use **Test connection** to send a small model request, then close the dialog to chat.
5. In **Runtime & security**, choose **Python · Pyodide** or **.NET 11 · WASM** to enable code execution for this session.

Use **Fetch models** in Connections to query the endpoint with the currently
entered key, even before saving a connection. Filter by model ID or display name,
choose **Use selected model**, then save. Manual model entry remains available
when an endpoint does not support model discovery or CORS blocks it. Discovery uses
the sibling `/models` endpoint, preserving proxy prefixes, following the
[OpenAI models API](https://developers.openai.com/api/reference/resources/models/methods/list)
and [Anthropic models API](https://platform.claude.com/docs/en/api/models/list).

The **Active for this session** banner and **Active** card identify the connection
used by the next turn. **Edit** opens settings without switching; **Use connection**
switches only the current session's connection. Saving an inactive connection does
not activate it. **Duplicate with key** creates a separately editable copy including
its API key and leaves the current connection active. Key badges say **Key present**
or **No key provided** without displaying the secret; **Show** reveals it on request.
Selecting a connection automatically sends a small test request. The plug icon is
green after success, red after failure, and neutral when untested; edited settings
must be tested again. **Test connection** tests the current editor values.

For OpenRouter, choose **Thinking · OpenRouter → On · request thinking** and
**Save changes**. **Model default** sends no override; **Off** requests disabled
reasoning. The preference belongs to the saved connection, survives refresh and
duplication, and updates the current session without replacing its history or files.
Chat and Test connection use the same setting. Other OpenAI-compatible endpoints
should stay on Model default unless they support OpenRouter's `reasoning` object;
this control does not configure direct Anthropic Messages connections.

On sends `reasoning: { enabled: true, exclude: false }`; Off sends
`reasoning: { enabled: false }`, following the
[OpenRouter reasoning API](https://openrouter.ai/docs/guides/best-practices/reasoning-tokens).
Returned `reasoning`, DeepSeek-style `reasoning_content`, or readable
`reasoning_details` appear once in a **Thinking** block before the answer. Direct
Anthropic thinking blocks are also displayed when returned. Signed/encrypted
reasoning details are retained for tool continuation and session restoration, but
encrypted data is never displayed as thinking. Some models do not expose reasoning,
and models with mandatory reasoning may reject Off. Thinking tokens count toward
provider output usage and may increase response time and cost.

Enter the provider's token alone in **API key**, without `Bearer` or an
`Authorization:` prefix. Chat, Test connection, and Fetch models share the same
credential validation. Surrounding whitespace is trimmed; URLs/session links,
header prefixes, and invalid token characters are rejected before a model request.
An invalid key saved by an older version remains visible for correction in
Connections; replacing it and saving keeps the existing chat and files.

Connection names, protocols, endpoint URLs, model IDs, and thinking preferences are saved in localStorage.
The hosted app keeps API keys in tab-scoped sessionStorage so refresh retains them.
Keys are excluded from saved chats and device-local connection metadata. Chats are saved locally.
Changing connections, updating a key, or toggling Python keeps the current session,
history, and files. Use **New session** to start a separate chat. Clearing context resets the current conversation and Python runtime. Editing connection fields takes effect when you save them.

Text and available thinking stream live using the CLI's OpenAI and Anthropic
stream parsers, compiled to WASM. Encrypted reasoning is preserved for provider
continuation but never displayed. A provider can report reasoning-token usage
without returning readable reasoning. JSON responses are also accepted for
endpoints that ignore streaming. Tools execute only after a validated terminal
response; interrupted tool streams never authorize execution.

The browser uses the CLI context projection: older tool results are shortened,
older context is trimmed to the input budget, and reused tool-call IDs are repaired
in outgoing requests. Saved transcript/history is retained. See
[shared-core status](docs/browser-parity.md) for the precise architecture and gaps.
Responses API endpoints, native filesystem tools, shells, LSP, native skills, and
Podman are not available in the browser host. Use the authenticated `lethetic --rc`
connection for those capabilities through the same UI.

## Compact context

Choose **Ctrl+P → Compact session** to summarize the model context while retaining
the browser session, transcript and files. The CLI and WASM host use one Rust
compaction coordinator: the same prompts, bounded UTF-8 windows, file-body elision
and hierarchical merges. No tools run during summarization. Cancellation, provider
errors or failed persistence preserve the previous history. The session turn
timeout covers the entire browser compaction, including all passes.

## Context and status bar

Open **Stats** to pin context used/input budget, model context maximum,
output budget/model maximum, latest-request tokens, thinking, files and file size,
transcript blocks, end-to-end output rate, timeout, rounds, retries and loop mode.
Existing usage, cache, reasoning and timing pins remain available. New/default
bars include context. Custom pin selections and the three-line bar are preserved.

Context is the **latest request input**, not accumulated turn usage. Before a
provider reports usage, it uses the CLI context engine's projected-message estimate
(excluding tool-schema overhead). Provider usage and its source survive refresh.

OpenRouter connections fetch `/api/v1/models` and the selected model's
`/api/v1/models/{author}/{slug}/endpoints`. Advertised model maxima and individual
provider limits are shown separately, as described in the [OpenRouter model API](https://openrouter.ai/docs/api/api-reference/models/get-models)
and [endpoint API](https://openrouter.ai/docs/api/api-reference/endpoints/list-all-endpoints-for-a-model).
Metadata is cached in memory for ten minutes, refreshed on connection changes,
and can be refreshed manually. Missing metadata stays unknown; session budgets
remain usable. No credential is saved with model metadata.

Requests use the lower of your session budget and the advertised model limit,
reserving output tokens within the total context. If the model has one provider,
its reported input/output/context restrictions also cap requests. Saved budgets
are preserved.
**Ctrl+P → Turn limits → Use model context** fills the discovered context minus
the output budget; **Save** applies it. Browser implementation ceilings still apply
(2,000,000 input and 131,072 output tokens). Routed providers can have lower limits.

## Turn limits and loop detection

Choose **Ctrl+P → Turn limits**. Settings belong to the current session, survive
refresh, and apply to its next turn without changing its files or worker variables:

| Setting | Default | Range |
| --- | --- | --- |
| Entire-turn timeout | 10 minutes | 0–1,440 minutes; 0 disables it |
| Model requests per turn | 32 | 1–1,000 |
| Transient retries per request | 2 | 0–10 |
| Context input budget | 131,072 tokens | 256–2,000,000 |
| Maximum output, including reasoning | 16,384 tokens | 1–131,072 |

Connections also shows these editable limits. **Test connection** discovers the
selected model's context/output capacity (including OpenRouter provider caps),
fills the context budget with room reserved for output, and validates a model
response. **Apply limits** saves to the current session without replacing its
history or files. Discovery failure leaves manual budgets available. Stale
results from a different model or endpoint cannot overwrite the editor.
Use **Defaults → Apply limits** to give an existing session the new fourfold
token defaults; saved budgets are otherwise preserved.

Choose budgets supported by the model. Context estimates use the CLI's approximate
byte-based accounting, not the provider's tokenizer. The current user turn is
protected from trimming; an oversized current turn can still exceed a model's
limit. **Stop** works even with an unlimited turn timeout. Python and .NET worker
execution deadlines remain separate (120 seconds by default).

Retries cover transient network failures, HTTP 429/5xx and interrupted streams,
using the CLI's backoff (3, 6, 12 seconds, capped at 30) or a bounded Retry-After.
Only the failed model request is resent: completed tools are not rerun. Failed
partial text is removed before retrying. HTTP authentication and validation errors
are not retried. **Ctrl+P → Loop detection** uses the existing remote UI panel and
CLI repetition detector for streamed thinking; Combined is the default, Off is
available, and a pure block-length cap is opt-in.

## Compacting context

Choose **Ctrl+P → Compact session context** to summarize the current model history
using the active connection. Long histories are summarized in bounded passes. The
same session, visible transcript, custom prompt, files, and live Python variables
are retained. The summary becomes the context for subsequent model turns and is
saved across reloads. Compaction can be stopped like a normal turn; cancellation,
request errors, and failed saves preserve the original model history. The command
is unavailable while another turn runs or before any model history exists.

In **Connections**, Edit and Use populate the saved name, endpoint, model, and key.
Save edits before selecting another connection or closing the dialog; opening a
saved connection always loads its saved values.

## Files, previews, and downloads

Use the existing **Files** directory browser. Choose **Save to Session** beneath a
code block or Python tool call, or **New file** in Files. Enter a relative path such
as `reports/analysis.py`. Folders are derived from these paths. Select a file to
view its source in Monaco with syntax highlighting and line numbers (use **Plain text** for native text selection), **Edit**, **Preview**, **Download File**, **Copy**,
or **Delete** it. Code blocks also have direct Preview, Download, and Copy buttons.
**Expand** fills the window in both the source pane and the rendered preview. Preview
titles show the full relative filename. In expanded rendered previews, swipe horizontally
or use left/right arrow keys to cycle through previewable session files in the selected
sort order. Text selection and modified arrow keys are preserved. In the source pane,
use the Previous/Next controls (or arrows/swipes on those controls) to change files.
Drag the Files edge in Left mode to resize its width, or drag the divider between the
file list and source to adjust their ratio. Focus a divider and use arrow keys for
keyboard resizing. **Sort** supports name, date, and size, with reversible ordering;
folders stay first. Dates record session content changes and survive reloads. Older
files with no recorded date show Unknown. **Download all (.zip)** exports every file
and empty folder in the session, preserving binary bytes and nested paths. The Files
label and Session files breadcrumb include the session's total file count.

Python tool calls default to parsed code with indentation, tab stops, and syntax
highlighting. **Parsed** / **Original** beside the bottom-right **Copy** button
switches between code and raw tool arguments; Copy follows the selected view.
Invalid or incomplete payloads remain in their original form.

Saving from the editor is explicit. The Files browser represents the current
session's Python workspace at `/home/pyodide`. Files created, changed, or deleted
by Python appear automatically after each completed call. Edits in Files are applied
before Python's next call. File edits are disabled during a model turn. Contents
are available to Python; they are not automatically appended to the model prompt.

Selecting a supported file immediately renders it in the Files pane. **Source** /
**Rendered** switches text-based documents between source and rendered views;
**Expand** opens the rendered preview full-size. Previews render static HTML, SVG, Markdown and images (PNG, JPEG, GIF, WebP, AVIF,
BMP and ICO), plus audio/video formats reported playable by the browser (including
MP3, WAV, Ogg, FLAC, MP4 and WebM where supported). Media uses native controls,
loads from session bytes, and does not autoplay. Codec support depends on the
browser; unsupported files remain downloadable. Existing workspace size limits
also apply to media. Code and other text use Monaco syntax highlighting in the pane and Preview dialog, including full-screen views. **Plain text** switches to native text selection; **Monaco** switches back. HTML, SVG and Markdown offer **Source / Rendered** in both preview locations. Arrow keys and touch gestures inside text viewers preserve selection; use the navigation toolbar to change files. Other binary
files can be downloaded without changing their bytes; Edit and Copy apply to text. They use an opaque sandbox and a restrictive content policy:
file scripts, forms, external resources and navigation are disabled. A nonce-scoped,
trusted navigation bridge handles preview arrows and swipes; it has no access to the host. Download the file
to use it elsewhere. This is a static preview, not a JavaScript app runner.

Files are **unencrypted**. The Pages app embeds Turso in the shared Rust/WASM module and stores a separate SQLite-compatible
database image per session in IndexedDB `lethetic.sessions.v2`. Messages, tool calls, transcript blocks and usage are normalized into queryable SQL tables ([schema and SQL examples](docs/storage.md#relational-schema-and-queries)), with file contents stored as raw
binary BLOBs. Sessions offers a **Database (.db)** download. See [embedded storage](docs/storage.md)
for the schema, migration, and extraction examples. Each workspace
supports **100 files, 256 folders, 16 MiB per file and 64 MiB total**. The shared
session snapshot budget is 256 MiB (including base64 used in memory/runtime snapshots
and chat history); the browser's disk quota may be lower. Runtime snapshots retain
base64 for compatibility; yEnc is not used because its low overhead assumes an
8-bit transport, while JSON/localStorage introduce escaping and Unicode overhead.

Existing `lethetic.sessions.v1` localStorage sessions and older binary IndexedDB
records migrate to Turso before the app mounts.
The migrated record is removed only after the IndexedDB transaction commits. If an
IndexedDB database already exists, a later legacy record from an older client is
left untouched rather than overwritten or deleted. A failed
load does not open a blank replacement session. Saves run asynchronously: a banner
shows pending saves or errors, with **Retry save** after failure, and closing a tab
with unsaved changes requests confirmation. A quota or transaction failure preserves
the previous saved record; edits remain in memory and can be exported/downloaded.
Concurrent edits in another tab are detected and rejected rather than overwritten;
export the unsaved tab before reloading it. Oversized workspaces and symbolic links
are reported to the model without replacing the last complete file checkpoint.
Empty directories are preserved; runtime packages and files outside `/home/pyodide`
are not included. Clearing site data deletes local sessions and files.

When an older session has no workspace, its first opening imports a copy of the
legacy `lethetic.files.v1` files, within the workspace limits. The original record
is left untouched. New sessions start with empty workspaces. Older encrypted
records are also left untouched; no encrypted library or passphrase UI is involved.

## Saved sessions and URL routing

**Sessions** uses the existing session screen to resume, name, create, or delete
chats. Completed turns automatically save their transcript, model context, usage
stats, system prompt, code mode, code network permission, theme and connection metadata in
IndexedDB. API keys are excluded. Reopening a chat restores its context
without rerunning tools. Its files and empty directories are restored before the
next Python call. Python variables are recreated, so save durable results to files.

The browser command palette hides native-only commands and runtime operations that
do not apply to the current mode. Mode-switch commands remain available; supported
commands with temporary prerequisites stay visible with their disabled reason.

Open **Ctrl+P → System prompt** to edit your instructions and inspect the exact
read-only instructions appended for the current mode. Expand **Tool definitions
sent separately** to inspect descriptions and schemas. Saving updates subsequent
turns while keeping the session, model history, files, and Python variables.
Only your editable text is saved; mode-specific instructions are composed at
request time, so turning Python off removes its instructions and tool definition.
The default instructions ask the agent to maintain a root `tasks.md` while doing
work with file-writing tools: a Markdown checklist, current step, blockers,
verification results, and output paths. Existing notes and unrelated tasks are
preserved. Open **Files → tasks.md** and leave its rendered Markdown preview open
to follow progress; it refreshes after completed tool calls, not during a running
call. **Source / Rendered** and **Plain text / Monaco** remain available. Python
uses `/home/pyodide/tasks.md` and .NET uses `/home/dotnetdide/tasks.md`; these are
the same relative session file when switching modes. This is model guidance,
not an automatic task recorder; chat without file-writing tools cannot save it.
Rendered file previews default to the active theme's background and text colors;
documents can override these defaults with their own styles.
The .NET instructions include a self-debugging recipe: after running code or
interacting with a hosted site, call `Dotnet.GetDebugInfo()` in a later submission
to read the JSON snapshot of C# and iframe console output, virtual HTTP requests,
and loaded assemblies. Save it as `debug-info.json` for preview/download. The
Sites dialog forwards iframe console messages to the active .NET session; closed
or replaced previews cannot add messages. No extra diagnostic permission is needed.
Snapshots precede the current submission and are bounded, worker-local history;
external HTTP traffic and native browser DevTools are excluded. SDK hosts using
`createDotnetTool` can forward their preview's `onConsole` to `tool.recordConsole`.
Python mode asks the model to write, run, and validate reusable `.py` programs in
`/home/pyodide`, then point you to **Files** to preview or download the results.
The included example creates and executes `solution.py`. The availability example
checks `importlib.util.find_spec("pyodide")`; running-runtime detection follows the
[Pyodide FAQ](https://pyodide.org/en/0.29.3/usage/faq.html#how-to-detect-that-code-is-run-with-pyodide).

Each session has an **Export debug (.tar.gz)** button. The download contains
`session.json` (transcript, model history, settings), `diagnostics.json` (runtime
state and file inventory), and `workspace/` (text and binary files, including empty
folders). Exporting does not switch sessions. During an active turn, files come
from the last completed checkpoint while the visible transcript is included.
Connection keys are excluded and known keys are redacted from text. The archive
is assembled locally; it is not uploaded. Every turn also has **Copy** at
its bottom-right, copying the source text or tool payload without UI labels.

The bundle also contains `provider-traces.jsonl`: redacted request bodies and raw
SSE/JSON response text captured before parsing. Failed tool arguments, partial
streams, retries and cancellations remain available after model-history rollback
and a normal session save/reload. Traces use normalized `provider_traces` rows in
the session database. Byte/request omissions and incomplete reads are explicit;
see [diagnostic limits and SQL examples](docs/storage.md#provider-diagnostics).
Older reports cannot recover streams discarded before capture was introduced.

Provider failures show their public error message/code instead of a generic
transport error. An output-limit failure (including OpenRouter's
`native_finish_reason: "max_output_tokens"`) displays **Output limit reached**,
the requested output budget including reasoning, and **Ctrl+P → Turn limits →
Maximum output tokens**. Adjust that limit within the model's supported maximum,
or split the task into smaller steps. Limits are never raised automatically.
Incomplete tool calls are never executed, even if a provider labels the response
`tool_calls`. Known connection and relay keys are redacted before error display;
arbitrary provider metadata and HTML error pages are kept out of the message.

SDK hosts can receive completed attempts with `createRuntime({ ...,
onTrace(trace) { /* store redacted diagnostics */ } })` and read
`runtime.activeTrace` for a snapshot during a request. `traceSecrets` supplies
additional known credentials to redact, beyond the selected connection's API key.
The UI supplies its known connection and relay keys automatically.

Use **Upload** in the Files toolbar to select one or more device files. They are
imported into the currently open folder and synced to the active session's Pyodide
workspace before its next call. Text and binary data are preserved. Files remain
available after refresh when session storage is enabled. Existing names are kept;
collisions receive numbered filenames, such as `data (2).csv`. The existing limits
apply: 16 MiB per file, 64 MiB total and 100 files per session. A failed upload batch
leaves existing files unchanged. Upload is available while idle; changing sessions
or starting a turn before reading finishes cancels the import. The upload stays in
the browser's session workspace and does not require a native server.

File changes update the directory in place. Unchanged previews stay mounted, preserving selection, scroll and media playback; only changed selected files reload.

Drag the visible handle along the bottom edge in Top mode or the right edge in Left mode to resize Files. Arrow keys also resize a focused handle.

Use the dock icon in the Files toolbar to toggle between Top and Left. Left mode stacks
the file list above the code viewer, with the conversation on the right. When collapsed, the left dock stays as a narrow vertical bar beside the conversation.
The choice survives refresh and session changes; narrow screens use the top layout. File-type
icons appear to the left of filenames in Browse and Changes.

The **Stats** panel lets you pin individual metrics to the bottom bar; pins persist across refreshes and wrap to three lines. The Files list and viewer show human-readable sizes with exact byte counts in tooltips.

The hosted page supports these routes without requiring server-side rewrites:

- `#/sessions/<uuid>` opens that saved session; refresh keeps the same session.
- `#/latest`, or no fragment, opens the most recently saved session, or creates one.
- `#/new` creates a new session and replaces the fragment with its UUID.

Session links identify local records; they do not share conversation contents with
another device. Missing sessions show an error and never silently replace an
existing chat. IndexedDB storage holds up to 50 sessions / 256 MiB of serialized snapshot data, subject to browser disk quota. Save failures appear in
Sessions and Stats; delete old sessions to free space. Old chats are never silently
evicted. Refreshing during a running turn restores the last complete model context and
the file checkpoint from the last completed Python call. Sessions
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

Pushing an ordinary commit deploys Pages. All distributions use the root `VERSION`
file in `yy.mmdd.##` format. Pushing its matching `vyy.mmdd.##` tag publishes one
**Lethetic** release containing the web/WASM embedding ZIP, CLI archive, and native
platform installers. The web archive is `lethetic-web-wasm_v<yy.mmdd.##>.zip` with
its `.sha256` checksum. See [central release versioning](https://github.com/maxfridbe/lethetic/blob/master/docs/releases.md).
A commit does not create a version tag automatically. The ZIP contains both standalone
and embedding modes; it does not contain API keys or downloaded Pyodide packages.
Build outputs live under `target/` and are not committed. `manifest.json` contains
the padded release version and per-file SHA-256 hashes.

## Embed the complete UI

Download `lethetic-web-wasm_v<yy.mmdd.##>.zip` from a release and extract it under your site's
`/vendor/lethetic/` directory. Serve `.wasm` as `application/wasm` and `.js` as
JavaScript. Keep the extracted `src/`, `browser/`, `web/`, and `assets/` trees and stylesheets
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
Pass `credentialStorage: sessionStorage` to retain keys through tab reloads;
embedded hosts otherwise keep keys in memory.
Session routing is off by default so the embed does not take over its host URL. `view.destroy()` aborts pending
work, terminates Python/.NET workers, removes dialogs, and releases application listeners.

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

For an OpenRouter connection, set `connection.thinking: "enabled"` (or `"disabled"`);
omit it for Model default. `onEvent` receives `thinking_delta` and
`assistant_delta` as text arrives, then authoritative `thinking` and `assistant`
texts at request completion. Replace accumulated text with those final values;
do not append it again. `response_reset` discards the current partial response
before a retry/failure, and `retry` reports attempt, delay and error. `send()`
returns the final answer. Set `stream: false` only for endpoints without streaming.
SDK options `timeoutMs` (0 disables), `maxRounds`, `providerRetries`, `contextTokens`
and `maxOutputTokens` match the session controls above. Returned reasoning is displayed even with Model default when the
provider includes it. `history()` includes provider reasoning details needed for
tool continuation; retain these unchanged when restoring a conversation.

Host tool handlers must validate argument fields and authorize their effects.
WASM verifies registered tool names, unique call IDs, JSON objects, and result
ordering; it does not implement full JSON Schema validation. Restoring history allows
completed calls to previously enabled tools without registering or executing them. Tools run sequentially.
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
Enabling Python authorizes execution and virtual file access. **Network access**
is off by default for both Python and .NET; enable it in Connections → Runtime &
security for outbound requests, including requests from site handlers. The saved
HTTP route applies to Python too. Direct requests require destination CORS/TLS.

The worker negotiates workspace, sites and network-policy support before a new
client sends code; stale workers receive no user code. If you see an out-of-date worker error, reload the page; keep
site storage so saved sessions and files remain available. The older
`Invalid Python workspace.` error from mixed deployed assets is fixed in 0.4.1.
Use it for calculations, CSV/JSON processing, NumPy/pandas analysis, and Matplotlib
plots in the session workspace. The tool description tells the model to create
files under `/home/pyodide`, its initial working directory, and explains persistence
and limits. Use relative paths there, or absolute paths under that directory:

```python
from pathlib import Path
Path("reports").mkdir(exist_ok=True)
Path("reports/result.csv").write_text("value\n42\n")
# A plot saved with plt.savefig("reports/plot.png") appears in Files too.
```

The UI saves a file checkpoint after each completed Python call, including a call
that returns a Python exception. Files created by an interrupted call are discarded
when the worker is terminated; the last completed checkpoint remains available.
Reloading, switching sessions, and cancelling never replay code to restore files.
Stdout, stderr, and the final expression are returned to the model, capped at 64 KiB.
Python exceptions are returned as tool output. Standard input receives EOF.
Packages bundled with Pyodide are loaded from imports; arbitrary native pip packages
and OS processes are not available. The initial load can take time and needs network
connectivity even with the code Network access switch off. Exact bundled package
files from the host-selected Pyodide manifest remain allowed; this exemption
strips caller headers/cookies and rejects redirects, query strings and uploads.
Additional `micropip` packages require Network access and use the selected route.
Self-host a complete compatible distribution and pass `indexURL` to
`createPythonTool` or `pyodideIndexURL` to `mountLethetic` to control asset delivery.

Cancellation and the 120-second Python timeout terminate the worker, so even an
infinite loop can be stopped without SharedArrayBuffer, COOP, or COEP headers.
The next execution creates a fresh worker and restores its last file checkpoint.
Variables reset on stop, timeout, reset, reload, and session switches. Files outside
the workspace are temporary. The entire SDK turn also has its own timeout.

Headless hosts can pass `workspace` and `onWorkspace` to `createPythonTool` to
restore and persist the same versioned snapshot. `python.snapshotWorkspace()`
returns a copy of the last completed checkpoint; `python.setWorkspace(snapshot)`
replaces the workspace for the next call while idle. `python.reset()` resets the
worker and globals while retaining that checkpoint. Storage is the host's choice
in the headless SDK; the complete UI handles it through its session store.

Python code can access browser networking and JavaScript worker APIs. A worker is
an execution isolation mechanism, not a security boundary for hostile code. It
receives source, a runtime asset URL, the active session's files, network settings
and any configured relay credential, never model API keys or conversation history. Enable Python only when you intend to run model-generated code. Embedded
hosts should apply their own approval policy if they require per-call consent.

For CSP, allow `data:` in `media-src` for session audio/video previews. Allow your own scripts, `'wasm-unsafe-eval'`, the Pyodide module origin,
`worker-src 'self'`, and `connect-src` for both model and runtime asset origins.
The standalone app includes an appropriate meta CSP for its default CDN. Adapt it
when self-hosting Pyodide. No `unsafe-eval` or cross-origin isolation is required.

### Interactive Python sites

Select **Python · Pyodide**, enable **Interactive sites** in Connections →
Runtime & security, and run a saved `.py` file with **Files → Run**. The
permission is off by default and saved per session. **Sites** opens the same
isolated interactive preview used by .NET. Download and run the complete
[Python notebook example](https://maxfridbe.github.io/lethetic/python/site_example.py),
also included as `python/site_example.py` in the browser ZIP.

```python
from pathlib import Path
from lethetic_wire import listen, Response

Path("site").mkdir(exist_ok=True)
Path("site/index.html").write_text("""
<button onclick="fetch('/api/hello').then(r=>r.json()).then(x=>document.querySelector('output').textContent=x.message)">Call Python</button>
<output></output>
""", encoding="utf-8")

async def handle(request):
    if request.path == "/api/hello":
        return Response.json({"message": "Hello from Python"})
    return Response("Not found", status=404)

listen(8080, handler=handle, web_root="site")
print("Open Sites → :8080")
```

`listen` returns immediately. The virtual port belongs to this worker; it does
not open a TCP socket or expose localhost to other devices. Do not start
`http.server.serve_forever()`, `app.run()`, Uvicorn or a blocking server loop.
Use `stop(8080)` or **Stop site** before rerunning the program.

| API | Behavior |
| --- | --- |
| `listen(port, handler=callable, web_root=None)` | Sync or async function receiving a `Request`, returning `Response` |
| `listen(port, wsgi_app, web_root=None)` | Synchronous WSGI app, including Flask |
| `listen(port, web_root="site")` | Static files, with directory `index.html` |
| `Response(body, status=200, headers={...})` | UTF-8 text or bytes; headers map names to strings or lists |
| `Response.json(value, status=200)` | JSON response with Content-Type |
| `stop(port)` | Unregister a site |
| `await run_file(path)` | Run a session Python file, supporting top-level await |

`Request` exposes `method`, decoded `path`, `query` (dictionary of lists),
`query_string`, `headers` (lowercase dictionary of lists), and `body` (bytes).
`request.text()` decodes UTF-8; `request.json()` parses JSON. GET/HEAD responses
with status 404 fall back to `web_root` files. Traversal and symlinks are rejected.
Run sets cwd to the program's folder, and handlers retain that folder. Bundled
imports are loaded before running a file. Package installation remains explicit.

For Flask, install it first, then register the app object instead of starting its
development server:

```python
import micropip
await micropip.install("flask==3.1.2")
from flask import Flask, request, jsonify
from lethetic_wire import listen

app = Flask("my_site", root_path="/home/pyodide")
@app.post("/echo")
def echo():
    return jsonify(value=request.get_json())

listen(8080, app)  # Optionally add web_root="site" for an existing static directory.
```

For external APIs, use `await pyodide.http.pyfetch(url)` in an async handler;
the preview calls that handler with `fetch('/api/...')`. Enable **Network access**
for the session. Both ordinary Python calls and site handlers use this permission
and the selected direct/relay route. With it off, local routes and static files
still work. Changing the permission or route stops the worker and its sites;
run the saved `.py` again. Chat and completed files remain saved.
Direct requests require destination CORS; relay requests require the configured
page-origin and target-origin allowlists. The [custom-header errata](#errata-custom-http-request-headers)
still applies. [Pyodide's HTTP guide](https://pyodide.org/en/0.29.3/usage/api/python-api/http.html)
describes browser networking.

Requests run serially with code execution, import pending file edits first, and
checkpoint generated files afterward. Reload, cancellation, mode/session changes,
and changing Interactive sites, Network access or HTTP route terminate the worker
and its sites while keeping
completed session files. Sites are never replayed automatically on reload.

The shared preview supports classic scripts, CSS, media and local fetch calls.
Forms must preventDefault and use fetch. Cookies, direct external preview fetch,
ES modules, XHR, ASGI, WebSockets and streaming are unsupported. Request/response
bodies are buffered up to 16 MiB for requests and 64 MiB for responses, with a 15-second cooperative deadline and a
20-second worker timeout; existing smaller session-file limits still apply.
The iframe cannot access parent DOM/storage or model credentials. Python itself
has the session network permission and route applied to fetch, XHR and browser
sockets; the worker policy is a cooperative capability control, not a sandbox
against hostile JavaScript interop or dynamic imports.

Embedding hosts use `createPythonTool({siteAccess:true, networkAccess:false, relay, workspace, onWorkspace})`.
The returned tool exposes `sites()` and `requestSite(request, signal)` with the
same request/response format as the .NET tool. `mountLethetic` also accepts
`{mode:"python", siteAccess:true}`. Keep `python/lethetic_wire.py` alongside the
distribution, and the shared preview assets under `runtimes/dotnetdide/`.

## .NET WASM mode

Choose **Connections → Runtime & security → .NET 11 · WASM**, or select it in
**Agent Mode**. This updates the existing session; connection changes also keep
its ID, chat history and files. The browser loads .NET lazily on the first C# call.
The same Files pane supports uploads, edits, previews, copy, download and ZIP export.
Python and C# share relative session file paths. `System.IO` sees these files under
`/home/dotnetdide`; only that directory is checkpointed after each completed call,
including calls that return an exception. Stop/timeout discards unfinished file
changes and resets variables. Reload restores files and settings, not variables.

```js
import { mountLethetic } from './src/embed.js';
const view = mountLethetic(document.querySelector('#app'), {
  connections: myConnections,
  storage: localStorage,
  mode: 'dotnet-wasm',
  networkAccess: false, // Python and .NET code; does not disable model connections
  nugetAccess: false, // separate permission for optional managed package downloads
  siteAccess: false, // separate permission for interactive Python/C# sites
});
```

Headless hosts can import `createDotnetTool({ networkAccess, nugetAccess, siteAccess, workspace, onWorkspace })`
from `src/sdk.js`, use it in `createRuntime({ tools: [dotnet], ... })`, and call
`dotnet.reset()` on disposal. `setWorkspace()` imports browser edits before the next
call; `snapshotWorkspace()` returns the last completed checkpoint. Execution has a
120-second timeout (configurable), cancellation, a 64 KiB output cap, and the same
session limits as Python: 100 files, 256 folders, 16 MiB/file, 64 MiB total. An invalid
or oversized snapshot is reported without replacing the last saved checkpoint.
The mount composes C# instructions automatically; headless hosts can use
`effectiveSystemPrompt(editable, 'dotnet-wasm', networkAccess, !!relay, nugetAccess, siteAccess)`
from `src/prompts.js` so its permission instructions match the worker options.

```csharp
File.WriteAllText("Program.cs", "Console.WriteLine(6 * 7);");
var result = await Dotnet.Run("Program.cs");
Console.WriteLine(result.Build.Output);
Console.WriteLine(result.ExitCode);
// Or Dotnet.Build(), Dotnet.Build("app.csproj"), await Dotnet.Run("app.csproj").
```

Scripts import `System`, `System.IO`, `System.Linq`, `System.Collections.Generic`
and `System.Threading.Tasks` by default. Use explicit `using` directives for other
available namespaces, such as `System.Text.Json` and `System.IO.Compression`.
Compiled programs don't inherit script variables or script using directives.
Projects use a documented SDK-style subset, without NuGet restore, project/package
references or full MSBuild. Read the bundled `runtimes/dotnetdide/llm.md` and
`docs/runtime-namespaces.md` for supported libraries and project settings.

The injected prompt includes runnable single-file and multi-file project examples,
directory creation, binary/text file I/O, build diagnostics and output paths.
`Dotnet.Run("file.cs")` compiles only that file; a directory or `.csproj` includes
multiple sources. Always await `Run` and inspect `result.Success`: successful
execution of the outer script does not imply that its compiled program succeeded.
Programs run in their source/project directory, and successful builds write
`bin/<name>.dll` and `bin/<name>.pdb` there. These managed artifacts also count
toward session file limits.

### Run saved programs and interactive sites

Select a `.cs` or `.csproj` in **Files** and press **▶ Run** next to its content.
This compiles and runs it in the current session's .NET worker, displays build
output in the transcript and saves generated files. It works without a model
connection, keeps the session ID and enables .NET mode when necessary. It does
not automatically grant Network, NuGet or Interactive sites permissions.

Enable **Interactive sites** in **Connections → Runtime & security** to use
`DotnetDide.Wire`. This is off by default and saved per session. The injected
system prompt shows its current permission and includes a runnable site example.
Create an ASP.NET middleware pipeline with `Wire.CreateApplication()`, register
explicit async `HttpContext` routes, and await `Wire.ListenAsync(8080, app, "site")`.
Return immediately: there is no listening socket, blocking host loop or Kestrel.
Save the program and run it with Files → Run or `await Dotnet.Run(path)`.

**Sites (n)** in the header lists active virtual ports, opens the isolated iframe,
accepts a page path and offers Reload/Stop. It serves index.html and classic JS/CSS,
images/media and `fetch('/api/...')` through C# routes. Requests are serialized,
checkpoint modified files after each response, and import file edits before the
next request. Model turns and site requests share the worker; retry when a turn
finishes. Request timeout/Stop terminates the worker, retaining the last completed
checkpoint. Stop a port and rerun the program to update compiled handlers.

The SDK's preview document keeps its own restrictive CSP. Keep `frame-src 'self'`
in the parent and `wire-preview-host.html` beside the SDK; the helper sets
`sandbox="allow-scripts allow-forms"` with no same-origin access. Form events work,
but native form navigation is blocked: use preventDefault and fetch. The child
cannot access parent DOM/storage/keys or external networking. General outbound
C# HTTP remains controlled separately by Network access.

For a weather page or another site that calls an external API, enable **both**
Interactive sites and **Network access** before running its `.cs` or `.csproj`.
The Sites dialog shows the current C# network permission and HTTP route.
Changing Network access stops the .NET worker: run the site file again, then
reopen Sites. It does not remove the session's files or chat. Use
`fetch('/api/weather')` in the page and `HttpClient` in the C# route; external
requests made directly by preview JavaScript are blocked even with Network access
enabled. Direct C# requests still require destination CORS support.
Keep the original exception message when reporting failures: “Network access is
disabled by the host” means the permission is off; a generic fetch failure alone
does not identify whether CORS, connectivity, or the remote service caused it.

Leave `User-Agent` to the browser when using direct `HttpClient` requests. A
custom header can trigger a rejected CORS preflight in Firefox even when the
same program appears to work in Chrome, which may silently drop that header.
In a Firefox Wire preview test, removing the custom `User-Agent` resolved an
Open-Meteo geocoding fetch failure. An HTTP 200 is still not proof of a successful
ZIP lookup: check the returned locations before making a forecast request.

ES modules, dynamic asset URLs, XHR, CSS @import and WebSockets are
not implemented in this message transport. Use explicit JSON serialization with
`Response.WriteAsync(JsonSerializer.Serialize(value))`; .NET 11 RC2 WASM has a
known failure in the tested automatic object-response/WriteAsJsonAsync path.
Read the bundled `runtimes/dotnetdide/docs/wire.md` and executable
`runtimes/dotnetdide/examples/site.cs` for .cs/.csproj examples and limits.

Embedding hosts can pass `siteAccess: true` to `mountLethetic` or `createDotnetTool`.
The tool exposes `sites()` and `requestSite(request, signal)` alongside its existing
checkpoint APIs. Permission/session/mode changes and reload stop sites and reset
variables, while saved files and the conversation remain. They are not replayed.

### Bundled media and optional NuGet packages

The runtime bundles StbImageSharp/StbImageWriteSharp for images, NAudio.Core for
WAV and signal processing, and Concentus/Oggfile for playable Ogg Opus. These work
without NuGet or general network permission. Generated PNG/JPEG and WAV files
preview in Files; Opus playback depends on browser support. Dispose output streams
and finish Ogg writers before the tool returns. Complete examples ship at
`runtimes/dotnetdide/examples/images.cs` and `examples/audio.cs`.

Additional managed packages can be downloaded directly in the browser:

```csharp
// First tool call (requires NuGet package access):
await DotnetDide.Nuget.Load("Newtonsoft.Json", "13.0.4");
// Next tool call:
Console.WriteLine(Newtonsoft.Json.JsonConvert.SerializeObject(new { answer = 42 }));
```

Enable **NuGet package access** in Connections → Runtime & security, or pass
`nugetAccess: true` to `mountLethetic` / `createDotnetTool`. It defaults off and is
saved per session, independently of `networkAccess`. Direct NuGet.org downloads
bypass the general HTTP relay without sending relay/model credentials. Restricted
embedding CSPs must allow `https://api.nuget.org` in `connect-src`. Managed
compatible dependencies are resolved; native/runtime-specific assets, build
targets, analyzers and version conflicts are rejected. Loaded libraries are also
available to subsequent `Dotnet.Build/Run` calls without PackageReference items.

Exact-version archives persist in browser **Cache Storage**, not localStorage or
session exports (32 archives / 64 MiB total, 16 MiB each). Permission is checked
before cached reuse. Storage restrictions fall back to downloads. After worker
restart, call Load again to reload cached assemblies. To clear persisted archives:

```javascript
const { clearNugetCache } = await import('./runtimes/dotnetdide/dotnetdide.js');
await clearNugetCache();
```

Clearing storage does not unload an active worker; stopping it does. Lethetic
permission changes restart the worker while preserving the session and its files.
The Connections screen links to the bundled `llm.md` and
`docs/media-and-packages.md`, which document API usage, limits and permissions.

### Runtime security

**Network access** is off by default for Python and .NET and is stored per session.
Existing sessions use their saved value (missing means off). The switch covers
worker fetch/HTTP (`pyodide.http.pyfetch`, `js.fetch`, `Dotnetdide.Net` and ordinary
`HttpClient`), XHR and browser sockets, including requests from site handlers.
Runtime assets and Pyodide bundled packages still load. Changing the setting or
HTTP route restarts code workers and stops sites, preserving chat and completed
files. Run the saved program again. The model prompt and Sites dialog show the
current permission and route; local interactive sites work with outbound access off.
Session filesystem read/write is enabled for both code modes. Neither receives
model keys or conversation history. Custom embedding tools remain host-authorized.

```csharp
var text = await Dotnetdide.Net.GetStringAsync("https://example.org/data.txt");
var bytes = await Dotnetdide.Net.GetBytesAsync("https://example.org/image.png");
File.WriteAllBytes("image.png", bytes);
var response = await Dotnetdide.Net.PostJsonAsync("https://example.org/api", new { value = 42 });
// For custom headers/methods: SendAsync(HttpRequestMessage), disposing the response.
```

Direct requests require destination CORS. For APIs without browser CORS, select
**Connections → Runtime & security → HTTP route → HTTP relay**, enter your
relay's HTTPS `/relay` endpoint and token, and choose **Apply HTTP route**. The
same session and files remain selected; code variables restart. The saved route
and token-present state are shown explicitly. Only the endpoint is saved in
session storage/export; the token uses `credentialStorage` (normally
`sessionStorage`) and survives refresh in that tab. With no token, local computation
still works and relay requests fail without falling back to direct networking.

The dotnetdide release includes an optional **native .NET HTTP relay**. Configure
its exact page-origin and target-origin allowlists, a random authentication token,
and optionally `DOTNETDIDE_RELAY_PROXY=socks5://127.0.0.1:9050` on the server.
The server can use HTTP(S) or SOCKS5 upstream proxies, including a Tor client's
SOCKS5 listener. `https://bridges.torproject.org/` supplies bridge addresses, not a
proxy endpoint: configure a bridge in your Tor client first. No Node.js server is
needed. [Full setup and protocol](https://github.com/maxfridbe/dotnetdide/blob/master/docs/networking.md)
are also bundled at `runtimes/dotnetdide/docs/networking.md`.

```js
mountLethetic(root, {
  mode: 'dotnet-wasm', networkAccess: true,
  storage: localStorage, credentialStorage: sessionStorage,
  dotnetRelay: { endpoint: 'https://your-relay.example/relay', token: relayToken }
});
// Headless: createDotnetTool({ networkAccess: true, relay: { endpoint, token } }).
// Python: createPythonTool({ networkAccess: true, relay: { endpoint, token } }).
// mountLethetic with mode: 'python' uses the same dotnetRelay option (legacy name).
```


For Python external APIs, prefer the async API; the relay supports GET, HEAD,
OPTIONS, POST, PUT, PATCH and DELETE, explicit target headers and binary bodies:

```python
from pyodide.http import pyfetch
response = await pyfetch("https://api.example/data", method="POST",
                         headers={"Content-Type": "application/json"},
                         body='{"value":42}')
print(response.status, await response.string())
```

`pyxhr`/`open_url` and libraries using XMLHttpRequest cannot use the HTTP relay;
use `pyfetch` instead. Relay failures and missing credentials produce an error,
never a direct retry. The relay's target allowlist must include any extra package
repositories needed by `micropip`. Leave browser-controlled headers such as
`User-Agent` unset; see the [header errata](#errata-custom-http-request-headers).

The route covers both `Dotnetdide.Net` and ordinary `System.Net.Http.HttpClient`.
Additional helpers include `HeadAsync`, `OptionsAsync`, `DeleteAsync`, `PostAsync`,
`PutAsync`, `PatchAsync`, `PostFormAsync` and `GetJsonAsync<T>`. Use
`SendAsync(request, new Dotnetdide.BrowserRequestOptions { Credentials = "omit",
Mode = "cors", Redirect = "error" })` for custom headers and browser options.
Inspect response status/headers and dispose responses. `OpenReadAsync` returns an
async `System.IO.Stream`; `DownloadFileAsync(url, path)` writes a session file.
`System.IO` itself does not provide TCP networking, and raw `System.Net.Sockets`
or `HttpClientHandler.Proxy` remain unsupported in browser WASM.

Relay bodies are buffered and limited to 8 MiB, with a 30-second upstream timeout;
Lethetic's session file limits still apply to saved downloads. Relay requests
never forward browser cookies, reject redirects and have no direct fallback.
WebSocket/XHR/EventSource/WebTransport are blocked on a relay route; use HTTP.
The page must be allowed by relay CORS; use HTTPS for remote relays. The route
does not change model connections or bundled runtime/package downloads, and
is not a whole-browser Tor route.

This is a capability control,
not a hostile-code sandbox: arbitrary JavaScript interop or dynamic imports can
reach APIs outside these wrappers. Use a dedicated origin and an appropriate
server CSP/network isolation policy if you execute untrusted code.

### Errata: custom HTTP request headers

Direct browser HTTP differs from desktop `HttpClient`. Leave `User-Agent` to
the browser: Firefox can preflight a custom value and fail where Chrome silently
drops it. This caused the reproduced Wire weather-site failure.

Add only API-required custom headers, on individual requests where possible.
`Authorization`, `X-*` headers and JSON Content-Type can trigger a preflight;
the destination must permit the origin, method and headers. Avoid Content-Type
on a bodyless GET. Do not manually set `Host`, `Origin`, `Cookie` or
`Content-Length`, or send Access-Control-Allow-* as a supposed client-side fix.
Network permission does not bypass CORS, and `no-cors` makes responses unreadable.

Preserve the original exception and test the actual request in the affected
browser. [Header restrictions](https://developer.mozilla.org/en-US/docs/Glossary/Forbidden_request_header),
[CORS header rules](https://developer.mozilla.org/en-US/docs/Glossary/CORS-safelisted_request_header)
and [server header approval](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Access-Control-Allow-Headers)
explain the browser behavior.

### Released runtime delivery

`browser/dotnetdide-release.json` pins the GitHub ZIP version and SHA-256.
`build-browser.sh` downloads and verifies that exact release into a build cache,
then bundles its runtime, licenses and guide beneath `runtimes/dotnetdide/`.
No .NET SDK or new npm dependencies are needed to build Lethetic. A deployed site
serves .NET assets from its own origin; it does not fetch a moving GitHub Pages build.
Keep `runtimes/` with the embedding SDK when copying it to another host.
The default browser acceptance suite executes the real bundled .NET release,
checks network denial/opt-in, uploads, persistent variables, cancellation, restored
files and session settings under the standalone page's CSP and project subpath.

### Binary persistence for embedded hosts

The Pages application enables embedded Turso with IndexedDB durability automatically. Embedded hosts opt in before
mounting (use a distinct database name for independent embedded applications):

```javascript
import { mountLethetic, openBrowserStorage } from "./src/embed.js";
const storage = await openBrowserStorage(localStorage);
const app = mountLethetic(document.getElementById("app"), { storage });
// Before programmatic navigation/disposal, await the durable save:
await storage.flush();
app.destroy();
storage.close();
```

Connections and preferences still use localStorage; credentials retain their existing
separate storage. Passing plain `localStorage` keeps the legacy 4 MiB session limit;
use `openBrowserStorage` for the larger workspace capacity. `storage.status` reports
saving/failure, `storage.pending` signals uncommitted edits, and `storage.subscribe`
notifies hosts. A rejected `flush()` must be handled before disposing the view.
Static media previews support the 16 MiB file limit; the text viewer also accepts
16 MiB in browser mode. Virtual sites accept 16 MiB request bodies and 64 MiB response bodies by default.
.NET Sites fetch responses stream in binary chunks up to 64 KiB with backpressure
and a 30-second idle timeout. Python responses remain buffered. The .NET SDK host
can set `siteResponseLimitBytes` up to 512 MiB; Lethetic uses the 64 MiB default.
The legacy `requestSite` API and HTML/media preview preparation still collect
complete responses. Workspace, model-context, and package-cache limits are separate.


### Background execution and native packaging

The [Tauri native host](docs/how-to/native-web-ui.md) packages this same WASM UI for
Windows, macOS, Linux and Android. Virtual folders are the default; a selected
real folder is synchronized through a revocable capability without exposing host
process execution. Native keep-awake is scoped to model responses only, never
idle or tool execution. OS termination remains possible.

Turn/Python/.NET deadlines count responsive foreground time: hidden time and
scheduler gaps longer than five seconds do not consume the budget. While a model
response is pending, visible browser documents request a best-effort screen wake
lock. Debug bundles include page lifecycle events, and in-flight checkpoints keep
transcript/file effects with the last complete model history. Reopened interrupted
work is marked for review; no automatic tool replay is performed.
