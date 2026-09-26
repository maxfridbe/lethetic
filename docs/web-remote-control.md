# Web remote control

Lethetic can expose one local application/provider session to an HTTPS browser controller. It can either mirror the interactive TUI or run through the browser-only foreground `--service` surface. Controller-token authentication is the default; `--wfe-disable-authtoken` is an explicit reachability-is-authority mode.

```bash
lethetic --rc 127.0.0.1                 # short form; bare --rc asks interactively
lethetic --wfe-remote-control https://127.0.0.1:11223
```

`--rc [TARGET]` accepts `host`, `host:port`, or a full `https://` URL (default port 11223) and is the same as `--wfe-remote-control`. `--rc-open`, `--rc-files`, `--rc-only`, `--rc-tls-cert`, `--rc-tls-key`, and `--rc-token-file` are short spellings of `--wfe-disable-authtoken`, `--wfe-files localonly`, `--service`, `--wfe-tls-cert`, `--wfe-tls-key`, and `--wfe-auth-token-file`. Bare `--rc` lists this machine's addresses and asks for address, port, authentication, file sharing, and surface, then prints the equivalent flags.

Remote control can also be started and stopped inside a running terminal session with **Ctrl+P → Remote Control: start / stop**. The dialog asks the same questions as bare `--rc`, then shows the controller URL, listeners and certificate fingerprint in a popup. While it runs, a status line under the input shows the target, auth mode, file sharing and connected browsers. When `--rc` started it at launch the palette item is disabled, and browsers cannot start or stop it.

The run loop publishes state to browsers only after something changed, at most every 100 ms; command responses publish immediately before replying. Redaction results are memoized per text, so unchanged transcript blocks are not rescanned on each publish.

The web frontend is optional. Without `--wfe-remote-control`, Lethetic does not open a web listener. WFE cannot be combined with `--command`.

## Read-only launch-directory files

Opt in explicitly on Linux:

```bash
lethetic --wfe-remote-control https://127.0.0.1:11223 --wfe-files localonly
# The same option works with --service and either authentication profile.
```

The exact value is `localonly`; missing/unknown values, duplicate flags, and use without `--wfe-remote-control` are rejected. The root is the pinned process launch directory and its descendants. It never follows changes to the model/tool cwd, resumed session, or managed Python workspace. This is a read-only browser capability, independent of the model's tool profile—not a new model tool or a filesystem grant to Python.

**`localonly` limits filesystem scope, not network reachability.** Enabling it intentionally shares eligible project files with controllers. With `--wfe-disable-authtoken`, every reachable peer can obtain that viewing/download authority. Startup warns about both facts. Exclusions and credential scanning are defense in depth, not a guarantee that all unknown project secrets can be recognized.

The **Files** bar opens a collapsible, vertically resizable Monaco pane immediately above chat. Browse folders and breadcrumbs, select a file, or Refresh its current contents. **Download File** saves the selected eligible file; **Download Folder** creates a Stored ZIP of the current folder. **Copy Path** copies the literal launch-root-relative path (for example `src/main.rs`); it does not insert text into or submit the prompt. File contents and selection state stay browser-local, outside WFE mirrored snapshots and provider transcripts. The editor is read-only, supports local basic-language tokenization and Find, and disposes its model when the pane closes. Obsolete reads are aborted and ignored; authentication changes clear file state.

The same disclosure rules govern listing, preview, individual download, and archive:

- `.lethetic`, `.git`, `.claude`, credential stores, `.env` variants, loaded configuration/local overlay, WFE state, and configured TLS/key/token files are protected; explicit protected inode identities remain protected after a rename.
- Absolute/platform paths, dot/traversal components, backslashes, controls, symlinks, hardlinks, mount crossings, sockets, FIFOs, devices, and other special files are rejected. Protected/control roots and filesystem-root selections are refused.
- A Linux `O_PATH` root and whole-relative-path `openat2` with `BENEATH`, `NO_SYMLINKS`, `NO_MAGICLINKS`, and `NO_XDEV` provide confinement. Verified inode snapshots and bounded reads detect changes. Unsupported platforms/kernels fail closed; there is no weaker path-join fallback.
- Complete bounded response bytes are checked for registered configured/controller credentials and established credential/private-key markers. Raw relative paths are checked before JSON escaping; sensitive names are excluded by count. Matching file contents are refused, not silently redacted. Protected names, host roots and raw I/O errors are not returned.

Fixed limits:

| Operation | Limit |
|---|---|
| Request / path | 8 KiB JSON / 4 KiB relative UTF-8 path, depth 64 |
| Directory listing | 1,000 eligible entries, with explicit truncation |
| Monaco preview | 2 MiB UTF-8 |
| Individual download | 32 MiB |
| Folder ZIP | 2,048 files, 2,048 directories, 56 MiB input, 64 MiB encoded output |
| Concurrency | Two file operations total; one archive at a time |
| Read chunk / ordinary deadline / archive deadline | 64 KiB / 15 seconds / 45 seconds |

Binary or oversized previews can still be downloaded within the larger download limit. Archive exclusions are disclosed as protected/unsupported/unreadable counts, never protected names; aggregate-limit exhaustion fails rather than returning a silently partial archive. File work runs outside the application actor, with bounded worker and response lifetimes. Cancellation is checked between operations/chunks; an arbitrary blocking filesystem syscall cannot be forcibly interrupted.

Only authenticated JSON POSTs to `/api/files/list`, `/api/files/read`, `/api/files/download`, and `/api/files/archive` exist, on the same exact HTTPS listener. Each needs the exact Host/Origin, existing secure cookie, and the in-memory proof in `X-Lethetic-WebSocket-Protocol`. The proof stays inside browser transport, not pane state, URLs, or logs. Downloads use bounded authenticated Fetch and short-lived Blob URLs for attachments—not workers. There is no static workspace mount, GET download ticket, extra listener, CORS wildcard, write endpoint, or network fallback. Disabled mode registers no file routes and loads no Monaco editor.

Monaco is directly vendored from a checksum-pinned archive into `web/lib` with Python-standard-library preparation; no Node/npm/Yarn acquisition or bundler is used. Only enabled files mode permits Monaco's required inline CSS (`style-src 'self' 'unsafe-inline'`) and its reviewed escaped dependency renderer. File text enters its model API, never application-created HTML. Inline JavaScript, `unsafe-eval`, CDN/package loading, and blob workers remain forbidden; module workers are same-origin only. Disabled mode retains `style-src 'self'`. Only enabled mode permits `clipboard-write=(self)`; `clipboard-read=()` remains unchanged.

## Interactive and service surfaces

The default invocation starts browser control first and keeps an optional interactive terminal UI attached to the same actor:

```bash
lethetic --wfe-remote-control https://127.0.0.1:11223
```

After Lethetic displays the controller URL/warning, pinned listeners, and certificate fingerprint, the first Enter acknowledges that security information and activates the application/provider/WFE actor. The terminal remains in its ordinary mode, and browser commands, provider/tool work, session activity, and state updates are immediately usable. Continue entirely in the browser, or press Enter at any later time to enter the raw/alternate-screen TUI without restarting or replacing that actor. A normal interactive invocation without WFE still enters the TUI immediately.

For permanently browser-only operation, add `--service`:

```bash
lethetic --service --wfe-remote-control https://127.0.0.1:11223
```

`--service` requires `--wfe-remote-control`. It remains in the foreground with sparse lifecycle and sanitized connection telemetry; it is not a daemon and does not implement a system service-manager protocol. It has one secure Enter gate and never offers a second TUI gate. It never enables Crossterm raw mode, enters the alternate screen, captures mouse/paste input, starts a terminal event stream, warms the terminal highlighter, or draws the TUI.

Both surfaces use the same single application/provider/WFE actor. Stdin and stdout must be attached terminals while Lethetic displays the exact URL, listener set, certificate fingerprint, and any controller credential/warning. Interactive WFE then waits for the security acknowledgement, starts the actor, and installs a cancellation-safe ordinary line reader only for the optional later TUI Enter. EOF/read errors or Unix SIGHUP at either interactive gate shut down gracefully without emitting terminal restoration controls that were never armed. Service mode begins after its sole acknowledgement and constructs no post-bootstrap reader. If its output later returns BrokenPipe/EIO, it permanently silences console logging without weakening authentication or stopping the actor.

Process lifetime is explicit:

| Event | Interactive surface | `--service` surface |
|---|---|---|
| Unix SIGHUP | graceful shutdown | keep running; fixed safe notice when logging is available |
| terminal EOF/read error | graceful shutdown | no post-bootstrap terminal reader |
| SIGTERM or SIGINT | graceful shutdown | graceful shutdown |
| confirmed browser Quit | graceful shutdown | graceful shutdown |
| browser connects/disconnects, including zero clients | keep running | keep running |
| opted-in WFE listener/runtime failure | fatal graceful shutdown | fatal graceful shutdown |

Shutdown rejects new work, cancels pending approval/question/provider/tool activity, checkpoints the session, reconciles retained Python state, and closes WFE within bounded cleanup. Browser connection count is telemetry only and never a lifetime signal.

## Security warning

A browser controller has the same practical authority as the TUI. Depending on the active profile and pending request, it can:

- send prompts and stop generation;
- approve model-requested tools, including shell or unrestricted Host Python execution;
- answer model questions and change models, themes, prompts, or agent mode;
- name, resume, delete, or wipe sessions;
- delete retained Python packages; and
- exit Lethetic.

Treat the bootstrap token, authenticated browser profile, and explicit token file like credentials. Do not share them, paste them into chat, put them in process arguments, or store them in a URL query string. Model/tool content, provider transcripts, and logs never need the token.

`--wfe-disable-authtoken` removes the controller bearer credential. HTTPS still encrypts the connection, and exact Host/Origin plus process-cookie/WebSocket-proof checks still bind a browser session to one Lethetic process, but none of those checks identifies or authorizes the person at the browser. **Every peer that can reach the exact HTTPS listener can obtain TUI-equivalent control.** Lethetic therefore prints the URL, fingerprint, warning, and firewall/VPN guidance before binding, and requires an Enter acknowledgement. Use a host firewall and VPN ACLs; do not treat TLS encryption as controller authentication.

The web projection deliberately omits API keys, OAuth and proxy state, TLS key material, session paths, provider signatures/replay, manifests, internal capability and lease handles, cancellation handles, and unvalidated identity bindings. It exposes only coarse Python policy status and strictly validated operational Podman names. This is a reduction in exposed data, not a reduction in controller authority.

## Generated exact-target profile

With no credential-file flags, every accepted exact IP or DNS target uses generated security. Loopback remains the simplest local example:

```bash
lethetic --wfe-remote-control https://127.0.0.1:11223
```

LAN/VPN addresses and a canonical lowercase machine name use the same one-command flow:

```bash
lethetic --wfe-remote-control https://100.102.242.15:11223
lethetic --wfe-remote-control https://brainiac:11223
```

On Linux, Lethetic creates or reuses a private ECDSA P-256 certificate whose sole subject alternative name is the exact configured identity: one IP SAN for an IP URL or one DNS SAN for a hostname URL. IP identities remain stored per canonical IP under `~/.local/state/lethetic/wfe/ipv4-<hex-address>` or `ipv6-<hex-address>`. DNS identities use `dns-<sha256-canonical-hostname>` and a strict hostname-bound manifest. All use 0700 directories and 0600 certificate, key, manifest, and generation-lock files. The same logical identity reuses its certificate across ports; a DNS address change does not rotate it, while another hostname resolving to the same IP gets a separate certificate. Existing version-2 IP identities remain reusable; DNS identities use version-3 manifests. Generated storage currently requires Linux no-follow filesystem support.

For a DNS target, Lethetic resolves the configured name once at startup under a bounded deadline and answer limit. It validates, sorts, and deduplicates every result, then explicitly binds every returned A/AAAA address with the configured port. All listeners share one cookie/proof, rate limiter, state, and global capacities. If any answer is prohibited or any listener cannot bind/configure, all acquired sockets close and startup fails—there is no wildcard, first-answer, preferred-family, partial-listener, or re-resolution fallback. The pinned set is printed before tokenless acknowledgement and after successful startup. A restart performs a fresh lookup.

To use the generated certificate without a controller token:

```bash
lethetic \
  --wfe-remote-control https://brainiac:11223 \
  --wfe-disable-authtoken
```

In this mode no controller token—generated, persisted, or dormant—exists. The printed controller URL has no fragment. The warning, logical URL, exact pinned listener set, and Enter acknowledgement occur before any listener becomes reachable.

In the default mode, the controller token is never persisted. Lethetic generates a fresh 256-bit token for each process, prints the certificate fingerprint and host-only bootstrap URL once, and places the capability after `#`, so browsers do not transmit it in the initial HTTP request. The dependency-free bootstrap module removes the fragment before loading the rest of the application, then the SPA exchanges the capability over HTTPS for both a process-local cookie marked `Secure`, `HttpOnly`, `SameSite=Strict`, and `Path=/` and an independent random WebSocket proof. The proof is retained only in that page's memory and is never persisted.

Generated certificates are self-signed. On first use, the browser may require an explicit certificate exception. Compare the browser certificate fingerprint with the fingerprint printed by the same Lethetic process before accepting it. Lethetic does not install a trust root or modify browser/OS trust settings. A DNS certificate names only the exact configured hostname—not its resolved IP, CNAME target, search-suffix expansion, trailing-dot form, or another alias.

The listeners are HTTPS/WSS only. There is no plaintext listener, redirect port, wildcard bind, HTTP fallback, reverse-proxy trust, or automatic network fallback.

## Optional explicit profile

Supplying certificate and key files overrides generated TLS for operators who already manage their own exact-target identity. Pair them with either a controller token file (default security model):

```bash
lethetic \
  --wfe-remote-control https://brainiac:11223 \
  --wfe-tls-cert /secure/lethetic/server-chain.pem \
  --wfe-tls-key /secure/lethetic/server-key.pem \
  --wfe-auth-token-file /secure/lethetic/controller.token
```

Replace the example identity and paths with local values. Open the configured base URL, then enter the token-file value in the browser address bar as `#token=<value>`; do not put it in the command line or query string. The bootstrap module erases the fragment before loading the SPA, which sends the token in a bounded same-origin JSON `POST /auth` request over HTTPS.

Or pair the explicit identity with tokenless control:

```bash
lethetic \
  --wfe-remote-control https://brainiac:11223 \
  --wfe-tls-cert /secure/lethetic/server-chain.pem \
  --wfe-tls-key /secure/lethetic/server-key.pem \
  --wfe-disable-authtoken
```

The accepted combinations are exact:

| Options after `--wfe-remote-control <HTTPS_URL>` | TLS identity | Controller authority |
|---|---|---|
| none | generated/reused exact IP or DNS certificate | fresh process token |
| `--wfe-tls-cert`, `--wfe-tls-key`, `--wfe-auth-token-file` | explicit exact IP or DNS certificate | explicit token |
| `--wfe-disable-authtoken` | generated/reused exact IP or DNS certificate | network reachability |
| `--wfe-tls-cert`, `--wfe-tls-key`, `--wfe-disable-authtoken` | explicit exact IP or DNS certificate | network reachability |

The disable flag conflicts with a token file. Certificate and key must always appear together; a token file is not accepted without both, and explicit TLS requires either its token file or the disable flag. All WFE credential/authentication options require `--wfe-remote-control`; `--service` also requires it, and WFE still conflicts with `--command`.

Lethetic accepts only an `https` URL with an exact non-wildcard IP or canonical lowercase ASCII LDH DNS name and an explicit valid port. A single label such as `brainiac` is valid; wildcards, underscores, uppercase/Unicode/percent-escaped names, empty/oversized labels, trailing dots, numeric/IP aliases, user information, IPv4-mapped IPv6 aliases, paths, queries, and fragments are rejected. An explicit certificate must be currently valid for server authentication, match the private key, and have the exact URL identity as its sole SAN of the matching type. DNS identities require exact DNS SNI and never use Common Name fallback.

An explicit token file must contain one URL-safe base64 token representing at least 256 bits, with at most one final newline. Certificate, key, and any token inputs must be bounded regular owner-controlled files; symlinks, hard links, and insecure permissions fail closed. An invalid or partial explicit profile never falls back to generated material.

Any non-loopback listener exposes TUI-equivalent control to every network that can route to it. Use host firewall or Tailscale policy to narrow reachability across the complete printed listener set. The built-in exact SNI, Host, and Origin checks are not a substitute for network access control.

## Browser connection flow

1. Open the exact URL printed by Lethetic in the intended browser profile. Token mode prints a fragment capability; disabled mode prints the fragment-free base URL.
2. Verify the TLS fingerprint if the certificate is not already trusted.
3. The dependency-free static bootstrap removes any fragment before loading the application graph and keeps a valid token only in memory.
4. A valid nonempty fragment is sent only to `POST /auth`. An empty fragment sends exact JSON `{}` to `POST /auth/session`, which exists only for disabled mode. A malformed nonempty fragment fails visibly and never downgrades to tokenless control.
5. The selected exchange returns the same secure process-local cookie and independent process-local WebSocket proof; neither endpoint returns chat state.
6. The SPA retains that proof only in page memory and opens same-origin `/ws` over WSS with it as the sole WebSocket subprotocol.
7. For a DNS target, TLS first requires the exact hostname SNI. The server then requires the exact Host and Origin, the cookie, and the proof, and echoes the selected subprotocol.
8. The server sends a protocol hello and full redacted snapshot, followed by typed revisioned patches.

The auth calls intentionally use Fetch CORS request mode even though each URL is fixed and same-origin. With the page-wide `no-referrer` policy, this makes supporting browsers send the concrete HTTPS `Origin` that Lethetic requires; it does not grant cross-origin access, add CORS response headers, or relax exact Host/Origin admission.

The unauthenticated shell contains no chat state. Both auth routes and `/ws` enforce the exact configured Host and Origin. `/auth` is usable only in token-required mode; `/auth/session` is usable only in disabled mode and accepts only an empty JSON object. `/ws` additionally requires both process-binding factors returned by the selected exchange; the host-scoped cookie alone is insufficient. Authentication failures are generic and rate-limited. Browser authentication Fetches have a 10-second deadline and are aborted when the page is torn down. Connections, authentication bodies, frames, queues, prompts, lifecycle telemetry, and the backend actor mailbox are independently bounded; a lagging browser is disconnected or resynchronized rather than allowed to retain unbounded backend state.

The cookie, in-memory WebSocket proof, and generated bootstrap token are valid only for the current Lethetic process. The controller token itself is not consumed: the same valid token can establish multiple browser sessions while that process remains alive. A reload appears one-use in default mode because bootstrap erases the fragment and reload destroys the in-memory proof; reopen the original token-bearing URL (or append the explicit token again) to authenticate another page. During the initial bootstrap, retryable network, `429`, and server failures retain the token only in page memory for the scheduled retry; success or a terminal rejection clears it.

Disabled mode can recover without a bearer credential. A reload repeats `/auth/session`. Ordinary socket interruptions first reuse the current proof under the existing 400 ms-to-15 second exponential backoff. If a reconnect using a proof that was previously accepted fails before WebSocket `open`, the next attempt performs one single-flight `/auth/session` refresh. A freshly exchanged proof is reused across backed-off admission/network retries and is refreshed only periodically, so a single occupied-client retry loop cannot exhaust the authentication limiter; `429 Retry-After` remains in force across offline/online transitions. This lets a page recover after Lethetic restarts. A page restored from the browser back/forward cache opens a fresh socket rather than displaying a permanently stale live session. A changed WebSocket proof identifies a new auth epoch: uncertain pending commands and local correlations are abandoned and visibly reported rather than replayed into a process that may already have executed them. A same-proof refresh stays in the current process epoch and retains exact request replay state. Tokens and proofs are never persisted in localStorage, sessionStorage, a query, or generated state.

After an idle interval, the browser issues a bounded snapshot probe whose independent deadline is cleared only by its matching response; unrelated state traffic does not mask a half-open connection. A stale-state snapshot request also has a hard 10-second recovery deadline before the socket is replaced. A protocol mismatch, failed liveness check, patch gap, or sequence/revision mismatch triggers reconnect, authentication, or full-snapshot recovery rather than applying uncertain mutations.

## Local connection telemetry

Every accepted WSS socket produces one connect record and one disconnect record. Interactive mode places these records in the TUI's default-visible Debugger pane (toggle with F12); service mode writes the same bounded, sanitized telemetry to its foreground console while console output remains usable. Connect records contain the canonical peer IP, IPv4/IPv6 family, process-monotonic connection ordinal, admitted active-client count, token-required/tokenless mode, initial state sequence/revision, and a real WebSocket Ping/Pong RTT. The server sends a random-nonce Ping after hello/snapshot and accepts only the matching Pong; state and command processing continue while it waits, and timeout or early close is shown as `rtt=unavailable` rather than substituted with authentication/setup time.

Disconnect records contain the same IP/ordinal, post-close active count, socket uptime, and a fixed internal category. The lifecycle lane is bounded separately from the command mailbox, so connection notices cannot consume command capacity. These detailed local-terminal records are runtime-only: they are not chat blocks, provider context, `SessionState`, `logs.txt`, or browser-projected state. Lethetic immediately discards the source port and never logs forwarding headers, tokens, cookies, proofs, URLs, user agents, or client-provided close reasons.

The browser Debugger receives a separate coarse derivative: the newest 50 browser-safe operational diagnostics plus a cumulative omitted-event count. Entries have fixed categories, severity, bounded trusted counts, and messages capped at 1 KiB after redaction. Raw peer IP/port, ordinals, RTT/uptime, request IDs, provider/tool errors, prompts, output/content, user agents, and close reasons never enter this projection. Diagnostic refreshes update the same-revision snapshot lane without advancing command sequence or revision.

## Commands and mirrored state

Browser commands use the generated protocol-v6 contract; the current and minimum accepted WFE protocol versions are both 6:

```text
ICommandRequest(id, expected_revision, type, ...parameters)
ICommandResponse(id, result)
```

State is server-pushed on the same authenticated WSS connection as a protocol hello, an initial `IStateSnapshot`, and revisioned `IStatePatch` messages. Request IDs are replay-protected, and duplicate in-flight requests with the same body coalesce. Reusing an ID with different data is rejected. Every complete encoded command, including its envelope and JSON escaping, must fit the 256 KiB client/server frame limit; a value below its field-level UTF-8 limit can still be rejected if escaping makes the complete command too large.

Race-sensitive operations bind to exact identities. Tool approval requires the current revision, session UUID, approval instance ID, and pending tool-call ID; the server always executes its exact current pending call rather than browser-supplied arguments. A complete approval preview can be approved directly. If privacy redaction replaced values or the 16 KiB preview limit omitted a tail, affirmative decisions require a separate inline confirmation and an explicit protocol acknowledgment; denial remains one click. The notices distinguish substitutions from an unseen tail, and a truncated approval executes the complete server-held call. “Always allow tools” warns that all later tool calls under the current execution policy may execute without another preview. Protected values and omitted approval tails are not exposed by either the approval or duplicated tool-call projection. If a system-prompt editor projection was redacted or truncated, remote editing, creation, and saving are disabled so hidden content cannot be overwritten. Questions still require exact form/question IDs. Lossy questions cannot be answered remotely, but retain a Cancel request action in browser-only service mode; cancellation records one fixed tool-error result without restarting the provider. Destructive confirmations are short-lived and bound to the exact action and payload. A stale browser can always request a fresh snapshot but cannot apply affirmative or destructive mutations at an old revision.

Stop carries the exact session UUID and a fresh operation-scoped `cancel_id`. The ID survives streaming, tool, approval and question phases but not settlement or a successor turn; it is not an OS cancellation handle or a bearer credential. Only Stop targeting that exact live instance bypasses changing presentation revisions, so streaming cannot repeatedly stale a valid Stop. Other revision and approval fences are unchanged. New Session and confirmed Clear Context report success only after their final durable commit; a late save failure is replayed as `SaveFailed`, is not automatically retryable, and requests resynchronization.

History labels are display-only previews. `history_entry_selected` returns the complete original editor content only when it is losslessly viewable and fits the prompt limit. A lossy, oversized, stale or unavailable selection leaves the draft unchanged. The browser applies a recall only if its request, session, selected entry and draft generation still match; a delayed response never overwrites newer typing. Content-bearing replay is session-scoped and aggregate-byte-bounded in addition to the entry-count limit.

The browser palette is driven by the same canonical command registry as the TUI. Its noneditable command list owns case-insensitive `H`, `T`, `C`, and `D` for Hotkeys, Themes, Clear UI (Keep Context), and Toggle Debugger; Arrow Up/Down, Home/End, and Enter navigate and invoke. Single-letter accelerators do not fire from search/editor controls, outside the list, with Ctrl/Alt/Meta, during composition or repeats, after another handler prevented the event, for disabled commands, or while a protected server-owned overlay is active. Protected approval, question, confirmation, session, and session-name panels preempt the local palette. Ctrl+C stops or requests Quit only from a noneditable target with no selected text.

At widths of 1024 px and above, Debugger is a persistent independently scrolling right-hand pane. At narrower widths it is a modal drawer with a backdrop and focus trap; a protected overlay covers and inerts that drawer. Escape closes the protected overlay first, then the drawer, then cancellable work. If transport is stale, the narrow drawer can be hidden locally and its canonical shared `ToggleDebugger` state is reconciled after exact synchronization; the wide pane always follows authoritative `App.show_debug` state.

Two horizontally scrollable rows stay at the viewport bottom without covering the composer or conversation, and no field is hidden at narrow widths. The application row contains Stop, Activity, Model, Provider, Python, Container, Context, Request, Turn, Turn API-eq, Session, Session API-eq, Rate, Memory, Files, Blocks, Git, and any projection-loss Status disclosure. Stop text uses fixed browser-safe activity/outcome categories; raw provider errors, request IDs, question fragments, and other arbitrary `App.stop_reason` suffixes remain local, with filtering disclosed by protocol metadata. The transport row contains browser transport, mirror synchronization, retry, pending commands, server mirror bounds, and bounded mirror detail. Token counters remain decimal strings, and costs use the server-formatted display rather than JavaScript numeric conversion; `estimate_cost: false` suppresses estimates in both TUI and WFE.

Container status is emitted only for Python-only sandboxed Podman. A retained identity is `lethetic-python-<canonical-lowercase-UUID>` and may appear inactive; it is active only when the live retained identity matches. A transient identity is canonical `lethetic-python-transient-<positive-pid>-<counter>` and appears only while its worker is active. Malformed, mismatched-kind, non-Podman, and inactive transient identities are omitted. These names are operational labels, not credentials; raw container/image IDs and runtime/session bindings are never projected.

Session names are display-only schema-4 metadata: naming or renaming never changes the session UUID, directory, inode binding, runtime identity, or resume/delete target.

## Frontend build

Browser libraries are vendored in `web/lib`; the project has no local npm dependencies, package manifest, lockfile, `node_modules`, bundler, CDN, or runtime third-party asset downloads. Browser modules, styles, fonts and workers load only from the embedded same-origin assets. Install only the globally pinned compiler if needed:

```bash
npm install -g typescript@7.0.2
```

Build the deterministic checked-in distribution:

```bash
./build-web.sh
```

Validate contracts, sources, vendored hashes, imports, compiler version, and checked-in distribution drift without changing files:

```bash
./build-web.sh --check
```

Both modes also compile the strict TypeScript tests in `web/tests` (Node `node:test`, no packages; ambient Node declarations in `web/tests/node-env.d.ts`) into a temporary directory outside `web/dist` and run them against the staged build. The script is working-directory independent, requires global TypeScript exactly 7.0.2, rejects a project-local compiler, and runs Rust contract generation with Cargo locked and offline. Normal Cargo builds embed the checked-in `web/dist`; they do not run npm, download browser assets, or silently regenerate the SPA.

Vendored components:

- Snabbdom 3.6.4, MIT license;
- Marked 18.0.11, MIT license, used only as a Markdown lexer;
- Monaco Editor 0.56.0, MIT license, direct native ESM with local icon fonts and worker;
- JetBrains Mono Nerd Font Mono from Nerd Fonts v3.5.0, SIL Open Font License.

Source metadata, licenses, sizes, and SHA-256 checksums are stored alongside the vendored files. Monaco's direct acquisition and reproducible offline preparation are documented in `web/lib/README.md`. The build also derives an exact Rust embed/dependency allowlist from the reviewed distribution manifest.

Run real-browser file acceptance against **new test-owned processes**, with an already acquired Chrome for Testing executable:

```bash
cargo build --locked --offline --bin lethetic
python3 scripts/test_wfe_files_browser.py --browser /path/to/chrome
```

The script compiles its TypeScript CDP driver (`web/tests/tools/wfe-files-browser.ts`) with the global `tsc` into the evidence directory. The Python/Node-standard-library driver uses a new workspace, private home and browser profile, no model prompts, and no existing WFE/controller token. It exercises disabled/enabled and tokenless modes, actual Monaco read-only rendering, hostile text, downloads/ZIP, Copy Path, CSP, module workers, layout and disposal. It installs no dependencies and leaves the user's running sessions alone.

## Rendering and privacy

The SPA preserves user prompts, thoughts, status content, and plain assistant prose as Snabbdom text nodes. Explicit Markdown blocks and ordinary assistant text whose lexer tokens contain meaningful Markdown are normalized into a closed set of fixed Snabbdom nodes; parser-generated HTML is never consumed. Complete structured tool results use the same significant-Markdown detector only after strict JSON inspection has established that the source is not JSON. Supported presentation includes headings, paragraphs, quotes, lists/tasks, code, tables, rules, emphasis, strikethrough, and hard breaks. Complete valid JSON tool calls, tool results, non-Python approval previews, and `json` code fences are tokenized from their original source into fixed text-only spans for keys, strings, numbers, literals, and punctuation. Raw HTML and images remain visible literal text and cannot create active or fetching elements. Links become anchors only after strict absolute credential-free HTTPS validation and receive fixed new-window, no-opener, no-follow, and no-referrer attributes; rejected links remain complete literal Markdown.

Protocol v6 records projection loss independently as `filtered`, `redacted`, and `truncation`. `filtered` means fields outside the browser-safe schema were deliberately omitted; `redacted` means sensitive values were replaced before delivery; `truncation=size_limit` means a fixed limit omitted an unseen tail; and `truncation=invalid_source` means invalid or incomplete input was replaced by a safe projection. Loss comes only from this server-authored metadata, never from attacker-controlled placeholder text. A filtered or redacted value can still be a complete final browser-safe source and retain Markdown or JSON rendering; either truncation kind remains literal and fails closed. Approval previews keep their separate `preview_redacted` and `preview_truncated` flags and the additional hidden-content confirmation described above.

The browser explains each loss with fixed text, in deterministic order:

- `WFE intentionally omitted fields outside this browser-safe projection.`
- `WFE replaced sensitive values before browser delivery.`
- `WFE shortened this value to a fixed browser limit; the omitted tail is not shown.`
- `WFE replaced an invalid or incomplete value with a safe projection.`

Model/tool content is never assigned to `innerHTML`, parsed as DOM, evaluated, or used as browser script/style. Markdown complexity or parser failures fall back to the complete bounded literal text. JSON highlighting preserves every original source slice and limits an individual source to 64 KiB, 64 nesting levels, and 4,096 lexical segments. A bounded valid JSON document remains literal and never falls through to Markdown. All JSON fences within one Markdown block share that segment budget. Across the visible chat window, exact lexical demand is allocated newest-first to complete valid tool calls, then complete valid JSON results; the remainder is shared deterministically by Markdown-capable blocks. The supplied allocations never exceed 4,096 segments, so large prose cannot starve a small valid call and malformed, truncated, or over-budget input remains unchanged. Static assets are embedded and served from a fixed allowlist with a restrictive Content Security Policy and no CORS wildcard; renderer safety does not depend on CSP.

The browser mirrors the TUI's authored activity frames locally: processing uses the regular spinner, while tool execution and LSP management use the tool spinner. External CSS advances one frame every 100 ms without server patches or a JavaScript timer. Activity labels remain visible, and reduced-motion preference disables animation while retaining the first frame.

The conversation follows the newest output by default and renders at most a 180-block browser window. Wheel, touch, pointer, keyboard, or scrollbar movement away from the bottom pauses following and preserves the captured position. Each genuine manual event replaces one five-second inactivity deadline; reaching the bottom resumes immediately, otherwise expiry selects the newest window and moves to the exact bottom even if no server patch arrives. Browser-owned positioning does not extend the hold. Authentication-epoch or session-UUID changes reset to following, while ordinary same-session disconnect/reconnect transitions preserve the remaining hold.

Remote presentation keeps at most the latest 200 blocks and applies per-block, aggregate raw-content, and complete encoded-message limits. JSON escaping is included in transport validation, and older content is omitted rather than allowing a snapshot to exceed the WSS bound. Every omission, redaction-related warning, and truncation is explicit. Volatile spinner, memory, Git, rate, and operational-diagnostic samples update the same-revision snapshot lane without advancing command sequence or revision, so periodic sampling does not invalidate approvals.

## Troubleshooting and recovery

- **Startup rejects the URL:** use a concrete IP or canonical lowercase LDH hostname, an explicit port, and `https`; remove wildcard/uppercase/underscore/trailing-dot/numeric aliases and path/query/fragment/user-info components.
- **Hostname resolution fails or returns a prohibited address:** fix the host's DNS/NSS/hosts-file configuration or use one exact IP. Lethetic does not filter bad answers or adopt another name.
- **One resolved address cannot bind:** stop the conflicting local service or correct the hostname's address set. Lethetic closes every listener and does not continue on a partial family/address set or switch ports.
- **Address already in use:** stop the conflicting local service or choose another explicit port. Lethetic does not switch ports automatically.
- **Certificate warning:** compare the browser fingerprint with the host-only value printed by the same Lethetic process. Do not bypass a mismatch.
- **Controller token rejected:** open the complete fragment URL printed by the currently running Lethetic process. Generated tokens change on every launch but are reusable within that launch; never substitute a URL from another process.
- **Authentication origin rejected:** use the exact printed URL rather than its resolved IP, a CNAME/alias, trailing-dot form, redirect, or alternate port.
- **Too many authentication attempts:** wait 60 seconds before retrying with the current URL.
- **An extension reports an inline-script CSP violation:** Lethetic contains no inline scripts. A source such as an extension-owned `utils.js` is being blocked as intended; retry in a private browser profile with extensions disabled if it interferes.
- **Token-mode authentication fails after restart or reload:** after reload, reopen the same process's token-bearing URL or append the explicit token again. After process restart, use the newly printed generated URL because all process credentials changed.
- **A fragment-free page says a controller token is required:** the current process was not launched with `--wfe-disable-authtoken`. There is no browser-side fallback from token-required to tokenless authority.
- **Tokenless page is retrying after restart:** leave it open; failed pre-open WSS reconnects refresh `/auth/session` under bounded backoff. A `429` waits for the server's `Retry-After` value.
- **Connection log reports `rtt=unavailable`:** the peer disconnected or did not return the exact WebSocket Pong before the bounded probe deadline; authentication/setup duration is intentionally not reported as RTT.
- **Browser reports a state gap:** allow it to request a complete snapshot. Mutations remain disabled until exact revision synchronization.
- **Explicit certificate validation fails:** check validity dates, the sole exact SAN of the target's IP/DNS type, server-auth usage, key pairing, file ownership/mode, and PEM content. DNS certificates cannot add aliases or rely on Common Name. There is no fallback to generated or plaintext credentials.
- **Server terminates unexpectedly:** Lethetic reports the opted-in remote-control failure and performs normal session/runtime finalization instead of silently continuing without the requested controller.
- **Closing the last browser does not stop Lethetic:** this is intentional in both surfaces; reconnect or request Quit from an authenticated browser, send SIGTERM/SIGINT, or end the interactive terminal.
- **Service logging stops after its launcher/pipe closes:** service mode survives SIGHUP, and BrokenPipe/EIO permanently disables only its sparse console output. The browser actor and authentication epoch remain active.

Stop Lethetic before rotating or replacing explicit TLS/token files, changing DNS/NSS answers, or changing the authentication mode. Keep credentials and generated state permissions under operator control; never make them model-writable. The listener set is pinned for one process while browsers resolve the hostname independently, so an address change can make reconnects fail until Lethetic restarts; it never triggers a silent rebind. Invalid or near-expiry generated exact-target material is replaced with a fresh manifest-bound identity, so verify any newly printed fingerprint. Explicit credential problems fail closed; there is no generated-credential fallback for an explicit profile. Tokenless mode is never selected implicitly—only the exact invocation flag enables it.
