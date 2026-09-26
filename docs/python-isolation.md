# Python isolation

This document is the source of truth for Lethetic's Python-only execution profile, its isolation modes, the Python notebook audit, and retained package runtimes.

## Python-only tool surface

Python-only mode is a tool profile, not a model connection. It works with both OpenAI-compatible connections and native Anthropic Messages connections. The model is advertised and allowed to dispatch exactly one tool:

| Tool | Purpose |
|---|---|
| `python` | Execute one cell in the chat's persistent Python runspace. |

Every other model-originated tool name is rejected at final dispatch. General mode keeps its existing tool set, including the legacy host-side `todowrite` tool, and does not advertise `python`.

Before provider I/O, Lethetic verifies that a Python-only agent request contains the exact host-generated guidance, exactly one advertised tool named `python`, and provider-native parallel tool use disabled. The final serialized body is checked again and that same checked byte sequence is both diagnosed and sent. `extra_body` cannot set `model`, `messages`, `system`, `max_tokens`, `max_completion_tokens`, `stream`, `stream_options`, `tools`, `tool_choice`, `parallel_tool_calls`, `functions`, `function_call`, `mcp_servers`, `n`, or `thinking`. A stale or provider-injected General call remains blocked at dispatch even if request admission were bypassed.

The worker is notebook-style but does not require Jupyter or IPython. Imports, variables, `_`, and `os.chdir()` persist while one chat remains attached to one healthy worker. The worker has no interactive stdin and does not implement rich MIME display.

Python globals reset when:

- a chat is created, loaded, detached, or resumed;
- the Python policy, backend, workspace binding, or security fingerprint changes;
- an executing Python cell is cancelled;
- the worker exits, crashes, or violates the framed protocol; or
- the runspace is explicitly reset.

Changing only the model does not reset Python globals. Cancelling provider generation before a Python cell begins does not reset an otherwise healthy worker. Cancelling a cell that is already executing terminates that worker because Python cannot safely unwind arbitrary code. Host cleanup also settles the worker's inherited Unix process group or Windows Job members, including ordinary subprocess children, before reporting completion; a bounded cleanup failure is surfaced. Host mode is not a sandbox: deliberately detached Unix processes (for example a new session created with `setsid`) are outside that inherited-group mechanism.

### Effective model guidance

Lethetic appends policy-derived, noneditable Python guidance after the selected prompt template. The suffix always identifies `python` as the sole model tool, identifies `lethetic_todo` as an imported host module and `lethetic_output` as worker-local output recovery, rejects invented shell/file/web/task/todowrite calls, and describes the effective target, backend, network, workspace access, grant count, policy source, lifetime, and reset behavior. Invalid or unresolved policy tells the model not to call Python.

Only an exact valid Python-only + Sandbox + Podman + Nonlocal + read/write workspace + session package access + no-extra-grant policy advertises `lethetic-pkg refresh` and `lethetic-pkg install NAME...` through a Python subprocess. Every other mode states that the Lethetic-managed package path is unavailable; this does not falsely claim that ordinary Python file/network operations are impossible under a broader Host or Full policy. `lethetic_todo` and `lethetic-pkg` are not additional model tools. Exact runtime, session, container, path, and capability identities are excluded from model context.

## `lethetic_todo`

Python-only mode replaces the separate model tool with an importable module:

```python
import lethetic_todo

snapshot = lethetic_todo.get()
# {"revision": 3, "todos": [...]}

updated = lethetic_todo.set(
    [
        {
            "content": "Run tests",
            "status": "in_progress",
            "priority": "high",
        }
    ],
    expected_revision=snapshot["revision"],
)
```

`get()` returns the current list and its revision. `set(todos, expected_revision=...)` atomically replaces the list only if the revision still matches, then returns the new snapshot. A stale revision fails rather than overwriting a newer update.

The data remains host-backed at `<launch-cwd>/.lethetic/todos.json`. The module uses a bounded, typed worker/host exchange that recognizes only todo reads and compare-and-swap replacements. Python cannot provide a path, command, socket destination, or network target through this interface; it is not a general host bridge. Todo values are validated by the same host-side rules used by General mode. After a successful audited `set`, Lethetic emits a typed host-only snapshot event so TUI/headless output refreshes with the new revision and task count.

## `lethetic_output`

Large results include bounded display excerpts and an opaque `artifact_id`. Recover captured output through an imported module in the existing `python` tool, without a host file path or another model tool:

```python
import lethetic_output

# Replace this example with the exact quoted artifact_id from the result.
artifact_id = "11111111-2222-4333-8444-555555555555"
info = lethetic_output.info(artifact_id)
chunk = lethetic_output.read(artifact_id, "stdout", 0, 65536)
print(chunk["text"])
# Continue from chunk["next_offset"] while chunk["eof"] is false.
```

`info()` returns `cell`, `artifact_id`, `retained_bytes`, `max_read_bytes`, and per-section `captured_bytes`, `original_bytes`, `excerpt_bytes`, and `truncated`. The sections are `stdout`, `stderr`, `repr`, and `traceback`. `read()` returns `cell`, `artifact_id`, `section`, `offset`, `next_offset`, `total_bytes`, `eof`, and `text`. Offsets address the retained UTF-8 bytes: start at zero, then use the returned `next_offset`; limits must be 1–65,536 bytes and large enough for the next character. Error displays retain useful stderr/traceback tails.

Artifact IDs are host-issued canonical UUIDv4 strings and are checked against the worker response. Numeric `cell` values are display-only and are rejected by the retrieval API. A reset or replacement worker cannot reuse an old artifact ID, even if its cell numbering starts at one again.

The worker retains a FIFO of at most eight artifacts and 8 MiB of captured output. Each stdout/stderr capture is bounded to 1 MiB, and repr/traceback to 256 KiB each; the initial excerpt is at most 64 KiB per section. Capture truncation retains bounded head/tail text with an omission marker: discarded bytes cannot be recovered. Retrieval reads captured output, not necessarily the complete original stream. New cells—including retrieval cells—consume ring capacity. Eviction or any worker reset makes the old ID unavailable with `KeyError`. This is an ephemeral worker-local facility, not durable artifact storage, a filesystem grant, or a host bridge.

## Notebook audit

Lethetic writes a standard Jupyter notebook named `python.ipynb`:

- durable chat: `<launch-cwd>/.lethetic/sessions/<session-id>/python.ipynb`
- non-durable run: `<launch-cwd>/.lethetic/python-sessions/<run-uuid>/python.ipynb`

The file uses nbformat 4.5. Each recognized `python` tool attempt becomes one code cell identified by the canonical provider-envelope tool-call ID. The exact source supplied by the provider is stored without formatting or normalization. Standard Jupyter `stream`, `execute_result`, and `error` outputs represent worker results.

Lethetic checkpoints a pending cell durably before approval or execution. If that checkpoint cannot be written safely, the cell is not executed. Denied, cancelled, interrupted, launch-failed, errored, and successful attempts all remain in the notebook. On recovery, an unfinished cell is marked interrupted; notebook source is never replayed automatically. Execution counts continue across chat resume even though Python globals start in a new worker generation.

Notebook metadata records the call ID, description, status, timestamps, canonical cwd, backend and policy identity, worker generation, container identity when applicable, truncation state, and narrow host-call outcomes. It does not store provider thinking blocks or thinking signatures, and Lethetic does not inject the notebook into model context automatically.

The notebook and Python output can contain source code, credentials, tokens, local paths, or other secrets. Review and redact it before sharing it with another person or agent.

## Display formatting versus execution

The TUI and approval detail can show a syntax-highlighted, formatted Python preview. Formatting is in-process, uses a pinned formatter with deterministic options, and does not discover project Ruff configuration.

Formatting is presentation-only. Lethetic always uses the original source bytes for:

- approval identity and hashes;
- execution;
- provider tool replay;
- notebook source;
- traceback line mapping; and
- duplicate-call detection.

The approval detail can show the exact original. Invalid, oversized, or unformattable input falls back to a normalized display of the original and never blocks approval or execution. Formatter thread-start failure, queue pressure, timeout, disconnect, or panic takes the same fallback path rather than terminating Lethetic.

## Backends

| Backend | Python installation | Filesystem boundary | Network implementation | Lifetime |
|---|---|---|---|---|
| Host | Host Python | None beyond configured process cwd | Host network | Worker process |
| Bubblewrap | Host Python and selected runtime files | Linux user/mount/process namespaces | Separate network namespace for None; host network for Full | Worker process |
| Transient Podman | Already-local OCI image | Rootless, read-only container plus explicit mounts | `--network=none` or `--network=host` | Worker/container |
| Retained Podman | Trusted already-local Lethetic runtime image | Rootless container plus a chat-owned COW package layer | Direct network disabled; constrained broker | Chat runtime, 14-day TTL |

Backend selection is exact and fail-closed. Lethetic does not silently fall back to Host, another sandbox backend, or a less restrictive network mode.

Podman must be rootless and available through a trusted system invocation path. Every automatic container creation uses `--pull=never`; inspect, start, attach, stop, and remove commands do not resolve or pull images. If an image is absent, activation fails with an instruction to use the separately confirmed pull action; Lethetic never pulls an image as a side effect of selecting or running Python.

Containers and Linux namespaces reduce exposure but are not virtual machines. They share the host kernel and do not by themselves guarantee CPU, memory, wall-clock, side-channel, or kernel-exploit isolation.

## Network modes

`NetworkAccess` has three distinct values:

| Mode | Direct container network | Reachability | Lethetic-managed package path |
|---|---|---|---|
| None | Disabled | No host, localhost, LAN, VPN, or Internet | Unavailable |
| Nonlocal | Disabled | Constrained public HTTP(S) through the broker only | Session layer; explicit `lethetic-pkg` invocation |
| Full | Podman host networking | Host/localhost/LAN/VPN/Internet reachable according to host routes | Unavailable; no automatic installation |

### None

None uses an isolated network namespace (`--network=none` for Podman). Existing None modes do not install packages automatically.

### Nonlocal

Nonlocal is constrained public HTTP(S), not an origin or domain allowlist. The retained container itself still has `--network=none`. Its only egress capability is a typed Unix-domain broker:

- DNS and route validation happen outside the container.
- HTTP permits bounded absolute-form GET and HEAD requests on port 80.
- HTTPS permits CONNECT only to canonical port 443.
- Localhost, host, RFC1918/LAN, CGNAT, route-visible VPN/local, link-local, metadata, multicast, transition, documentation/benchmark, reserved, mixed-answer, and otherwise non-global destinations are denied.
- DNS, route, audit, peer-identity, capability, and broker failures deny traffic; they do not downgrade to Full.

This is not a domain allowlist. A public endpoint can relay a request elsewhere, and some public-address VPN or NAT-hairpin destinations may be indistinguishable from ordinary public services.

Lethetic-managed distro packages in the retained layer are changed only after Nonlocal has been selected explicitly and model Python invokes the narrow helper itself:

```text
lethetic-pkg refresh
lethetic-pkg install NAME...
```

The helper accepts strict package names and fixed package-manager argv. It rejects options, URLs, paths, repository edits, and shell syntax. Signed packages may still execute maintainer scripts as namespaced container root and mutate that chat's retained layer; the helper does not make package maintainer scripts safe.

### Full

Full uses Podman host networking. Code can reach services bound to host localhost, LAN peers, VPN/local routes, and the Internet whenever the host can reach them. It does not mount the Podman or Docker engine socket, the host root, extra devices, or grant privileged mode. The process remains the invoking unprivileged UID with dropped capabilities, no-new-privileges, and a read-only container root.

Full network does not enable Lethetic's package broker and does not automatically install anything. Python can still download or modify files using its ordinary network and writable-workspace permissions, so Full materially reduces isolation.

## Invocation-locked literal modes

The following exact CLI flags are mutually exclusive. They are immutable process-scoped overrides and are never persisted into project or global Python policy sidecars.

| Flag | Network | Workspace | Lethetic-managed package layer | Container lifetime |
|---|---|---|---|---|
| `--python-fully-isolated` | None | launch cwd, read/write | Unavailable | Transient |
| `--python-isolated-with-nonlocal-network` | Nonlocal broker | launch cwd, read/write | Per-chat retained COW layer | Retained; 14-day TTL |
| `--python-isolated-permissive` | Full host network | launch cwd, read/write | Unavailable | Transient |

All three force Python-only mode, rootless Podman, no extra path grants, the same canonical launch cwd inside the container, and no backend/network fallback. Fully isolated and permissive can be used without a durable headless session. Any effective Nonlocal headless policy—including an ordinary YAML/global selection—requires exactly one durable identity selected with `--new-session` or `--session-id` before provider work because package-layer ownership and cleanup must be bound to a chat.

The literal policy has higher precedence than chat one-time, project, global, and model-server policy for the lifetime of the Lethetic process. It survives New Session, Resume/load, deletion or wipe replacement, WFE clear-context, and model switching. A chat transition still detaches the existing worker, so notebook globals reset while the Python-only profile and exact invocation-only shared-cwd binding remain. TUI and WFE expose its source as `cli_locked`; Agent Mode controls are disabled, and direct or stale mutation commands fail. Restart without the literal flag to select another profile.

Examples:

```bash
# Transient and networkless.
lethetic --python-fully-isolated \
  --timeout-seconds 900 --command "Inspect and test this project"

# Retained public-package layer; print a new session UUID.
lethetic --python-isolated-with-nonlocal-network --new-session \
  --timeout-seconds 1800 --command "Build this project"

# Resume the exact retained layer. Python globals start fresh.
lethetic --python-isolated-with-nonlocal-network \
  --session-id 01234567-89ab-4def-8123-456789abcdef \
  --timeout-seconds 1800 --command "Continue"

# Transient with host/localhost/LAN/VPN/Internet reachability.
lethetic --python-isolated-permissive \
  --timeout-seconds 900 --command "Exercise the local development service"
```

`--command` consumes the remaining command-line text, so put it last.

Ordinary YAML or TUI-selected Nonlocal mode keeps its Lethetic-owned managed workspace semantics. Sharing the launch cwd at the same path occurs only when one of the literal flags selected it for that invocation.

## Shared launch cwd and control-state masking

For literal modes, Lethetic canonicalizes the launch directory once, records its device/inode identity, mounts it read/write at the identical absolute path, and makes that path the container and worker cwd. A retained layer cannot be resumed from a different path or replacement inode.

The real host `<launch-cwd>/.lethetic` remains available to Lethetic for sessions, runtime manifests, todo state, and notebooks. If it is absent, Lethetic creates it durably as an owner-controlled mode-0700 directory before container preflight. Python sees an independently mounted, inaccessible nested tmpfs at that exact path instead. The mask is attested with `notmpcopyup`, `nosuid`, `nodev`, `noexec`, a bounded size, and inaccessible permissions. Lethetic rejects a launch cwd that is either an ancestor or descendant of its configuration root, runtime-state root, or `~/tmp/lethetic-sessions` managed-workspace store. It also rejects nested `.lethetic` trees and special host IPC/device entries, because the single root mask cannot protect nested control data.

The external project directory is not Lethetic-owned. Runtime, chat, TTL, or wipe-all cleanup may delete only a separately bound managed workspace and exact manifest-owned runtime resources. It never recursively deletes the shared launch directory.

Managed retained workspaces use Podman's private relabeling profile (`:Z`) and the broker requires the exact paired Podman ProcessLabel and MountLabel. External shared-cwd runtimes instead use an explicitly attested `label=disable` profile so Lethetic does not recursively relabel user source. In that one profile, the broker skips only the SELinux peer-label check. It still requires a PIDFD-pinned process and verifies `SO_PEERCRED` UID/GID/PID, process start time, cgroup identity, executable device/inode, the framed protocol, runtime identity, and the unguessable capability. Disabling the peer-label check removes one SELinux defense-in-depth layer; users who require that additional boundary should use a Lethetic-managed workspace instead of an external shared cwd.

Broad manually configured grants can weaken isolation. They are not available in the three literal modes.

## Runtime notices and lifecycle

After exact container attestation and worker handshake, Lethetic emits an application notice. It is displayed in TUI/headless output and its browser-safe projection, but is not added to provider/model context. Rendering uses the validated operational name; the raw container ID remains internal to attestation and exact cleanup. Examples:

```text
Podman container lethetic-python-transient-<pid>-<counter> created; network: none; mounted R/W cwd: <path>
Podman container lethetic-python-<canonical-lowercase-UUID> created; direct network: disabled; constrained public HTTP(S) broker: available; mounted R/W cwd: <path>
Podman container lethetic-python-transient-<pid>-<counter> created; network: full (host/localhost/LAN/VPN/Internet reachable); mounted R/W cwd: <path>
Python notebook audit: <exact-path>
```

A retained layer reports `resumed` instead of `created` when an existing exact container is attached. Capability probes do not emit runtime notices. WFE redacts the mounted cwd and any legacy/contextual raw Podman ID before browser delivery.

WFE operational status applies a separate fail-closed identity rule. It can show retained `lethetic-python-<canonical-lowercase-UUID>` while active or inactive, and active transient `lethetic-python-transient-<positive-pid>-<counter>` only while that worker exists. It omits malformed names, a mismatched identity kind, non-Podman Host/Bubblewrap modes, and inactive transient identities. These status names are non-secret operational labels; they are not authentication material and never enter model guidance. Raw container/image IDs, runtime/session IDs, labels, fingerprints, broker paths, mounts, and capabilities are not projected.

Transient Podman uses create, exact-ID inspect/attestation, and start/attach rather than relying on a name. Once the exact ID and Lethetic ownership labels are verified, startup failure, cancellation, reset, and drop remove only that ID and verify its absence; drop performs bounded exact-ID retries independently of the caller's Tokio runtime. If identity/ownership attestation fails, Lethetic reports the mismatch and refuses to remove the untrusted object. Retained lifecycle operations likewise require the exact manifest-bound, label-verified ID. Lethetic never uses `podman prune`, a name prefix, or broad label deletion.

Before Drop cleanup begins, Lethetic durably records the transient cleanup identity under `~/.local/state/lethetic/transient-python-cleanups/`. Each private, mode-0600, versioned record is bound by filename to either the deterministic create name or the verified full container ID, is size bounded, and is retained until exact absence has been verified. Pending records are retried before a later transient Podman capability probe. Cleanup work is limited to 128 records per retry pass; a durable attempt counter rotates failures behind untried records so a larger queue cannot permanently starve later identities. Record reads, writes, and removals are descriptor-relative and no-follow; malformed, oversized, substituted, or identity-mismatched records fail closed rather than authorizing cleanup of another object. A record for a different normalized Podman invocation path remains queued for that path without blocking a currently valid backend.

Podman's normalized inspect output is not byte-for-byte stable across create and start, so attestation combines exact create argv with semantic inspection. The exact CreateCommand must prove `--rm`, `notmpcopyup`, and managed-mount `:Z`; normalized inspection may represent autoremove as `io.podman.annotations.autoremove=TRUE`, omit `notmpcopyup`, and omit `:Z` after start. Collection fields returned as JSON `null` are normalized to empty only before required semantic checks, so a missing required item still fails. An explicitly label-disabled external runtime may report an empty ProcessLabel and a generated private mount-only label; any such mount label is independently validated.

## Retained package layers

One durable chat owns one stopped rootless Podman container and its writable native COW layer. Source is a bind mount and is not copied into the layer. Detaching stops the container and resets globals; packages remain. Successful use/resume refreshes the 14-day TTL.

Startup and hourly in-process maintenance may remove only expired exact manifest-bound resources. There is no background daemon or broad Podman cleanup. Deleting the Python runtime/packages removes the exact package layer while preserving source and chat state. Deleting or wiping a chat first removes its exact retained runtime, then its Lethetic-owned state; externally shared source survives.

Runtime manifests bind the exact image, worker/runtime ABI, container ID, labels, UID/GID and user namespace, mounts, workspace identities and ownership, network posture, SELinux profile, broker layout, and security fingerprint. This source tree uses framing protocol v3, worker ABI `lethetic-python-worker-v4`, output capability `lethetic-output-v2`, and retained runtime ABI `lethetic-python-runtime-v4`, required for the typed todo bridge and UUID-bound worker-local output recovery metadata. Known ABI-v2/v3 layers remain exactly inspectable and eligible for TTL or explicit deletion but are attach-disabled; they are never silently rebuilt or reinterpreted. Replacing an old layer requires explicit operator action and a separately built trusted local image carrying the v4 ABI label. The host does not pull, rebuild, or delete a runtime automatically to resolve an ABI mismatch.

## Policy precedence

The effective Python policy is selected in this order:

1. one invocation-locked literal CLI mode (`cli_locked`) for this process;
2. one-time TUI selection for the active chat;
3. managed project sidecar;
4. managed global sidecar; and
5. safe defaults.

A managed sidecar is a complete versioned snapshot. Repository-controlled policy cannot silently enable Host execution or choose a host executable. Invocation-only same-path sharing is deliberately excluded from sidecar serialization and policy restore.

## Building the trusted runtime image

Nonlocal requires the configured trusted Lethetic runtime image to already exist locally. First build the release helper, then build from an explicitly selected **already-local exact base image ID** with pulls disabled:

```bash
cargo build --release --locked --bin lethetic-runtime
BASE_IMAGE='sha256:<64-lowercase-hex-local-base-image-id>'
podman build --pull=never \
  --build-arg "BASE_IMAGE=${BASE_IMAGE}" \
  -f Containerfile.runtime \
  -t localhost/lethetic-python-runtime:dev .
```

Resolve and inspect the base locally before substituting its exact ID; do not use a floating remote base reference for this trusted build. `localhost/lethetic-python-runtime:dev` is the source default. Building, replacing, pulling, committing, pushing, or publishing an image is never an automatic consequence of running a cell. After a worker/image ABI change, old retained layers must be explicitly deleted before they can be replaced.

## Troubleshooting

- **Podman unavailable or not rootless:** install/configure rootless Podman outside Lethetic, then retry. A supported Distrobox `podman` compatibility symlink may forward to host Podman.
- **Image is not installed:** use the explicit, separately confirmed pull/build action. Lethetic will not pull automatically.
- **Wrong cwd or replacement inode on resume:** return to the original canonical launch directory, or explicitly delete the retained runtime and create a new session/layer.
- **Old runtime ABI:** explicitly delete the exact old package layer, rebuild the trusted local image from an already-local base, then retry. No migration attaches old executable state.
- **Broker denied a destination:** Nonlocal accepts only its constrained public HTTP(S) semantics. Select permissive mode explicitly only if host/LAN/VPN/Internet reachability is intended.
- **Notebook checkpoint failed:** fix ownership, permissions, symlink replacement, or disk-space problems under the host `.lethetic` control root. Lethetic will not run an unrecorded Python attempt.
- **Cell cancellation:** the active worker and its globals are intentionally discarded. Retained installed packages are unaffected.

## Implementation and tests

The primary implementation lives under `src/python/`, with policy and binding integration in `src/config.rs`, `src/python_policy.rs`, `src/tool_runtime.rs`, `src/app.rs`, and `src/headless_session.rs`. Tool schema/dispatch is in `src/tools/`; TUI rendering is in `src/ui.rs` and `src/markdown.rs`.

Deterministic tests cover policy parsing and non-persistence, workspace and inode bindings, mount/mask attestation, exact-ID lifecycle behavior, retained schema migration and cleanup, notebook nbformat/recovery/output cases, todo protocol validation and revision conflicts, display-only formatting, model token limits, and Python-only final dispatch. Rootless-Podman integration tests are opt-in/ignored because they create real local containers and require an explicitly selected already-local image.
