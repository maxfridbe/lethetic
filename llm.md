# Lethetic: Autonomous Coding CLI

> **NOTE:** Lethetic follows a **reproducible test philosophy**. Every feature and bug fix MUST be empirically verified through automated integration scenarios or unit tests before deployment. This ensures the reliability of the autonomous loop and prevents regressions in tool-calling logic.

**Lethetic** is an experimental coding CLI and autonomous agent runner. The name is inspired by the works of **David Gerrold** (notably the *War Against the Chtorr* series), representing an attempt to build a sophisticated, tool-augmented interface for local LLMs.

This project is a **test-driven application** designed for evaluating and interacting with local LLMs' tool-calling capabilities via a rich TUI (Terminal User Interface) for autonomous agent interactions.

## Supported Models

Lethetic can switch among configured OpenAI-compatible local servers and a native `claude-code-proxy` connection from the command palette (Ctrl+P). Connection protocol, model/parser choice, and agent tool profile are independent settings.

### Gemma 4 26B — `port 7210`

- **Binary**: `~/llama-cpp-turboquant/build/bin/llama-server` (TheTom/llama-cpp-turboquant fork)
- **Quantization**: UD-Q5_K_S (Unsloth), turbo3 KV cache
- **Context**: 262 144 tokens
- **Service**: `sudo systemctl start gemma4`
- **Parser**: `gemma4` — uses asymmetric `<|"|>` tool call markers
- **Strengths**: Highest output quality, reliable tool calls, excellent code generation

### Qwen3 27B MTP + TurboQuant KV — `port 7211`

- **Binary**: `~/ik_llama.cpp/build/bin/llama-server` (ikawrakow/ik_llama.cpp + turboquant-kv branch)
- **Quantization**: Q4_K_M MTP model, turbo3 KV cache
- **Context**: 262 144 tokens
- **Service**: `sudo systemctl start qwen3`
- **Parser**: `qwen3` — plain JSON tool calls (no special markers); system messages must be merged into one
- **Strengths**: ~30% faster than Gemma4 via MTP speculative decoding (~35 tps), reasoning/thinking mode, strong at multi-step tasks

### Claude Code Proxy — `127.0.0.1:18765`

- **Connection kind**: `claude_code_proxy`
- **Protocol**: native Anthropic Messages (`/v1/messages`) and model discovery (`/v1/models`)
- **Default model/limits**: `gpt-5.6-sol`; 1,050,000 total context, 922,000 maximum input, 128,000 maximum output, 24,576 request output, and a conservative 900,000-token Lethetic input/history budget; adaptive summarized thinking, max effort
- **Lifecycle**: the proxy is started and authorized separately; Lethetic only attaches over loopback and never reads its OAuth store
- **Tool continuation**: native API-issued IDs plus lossless replay of thinking signatures, redacted thinking, text, and `tool_use` blocks

Structured `context_limits` metadata is exact-model-scoped. Legacy `context_size` remains an input/history fallback for unlisted discovered models; model switching updates and immediately re-trims `ContextManager` rather than leaving the prior model's budget active.

#### How MTP + TurboQuant works

`-mtp --draft-max 1 --draft-p-min 0.0` enables Multi-Token Prediction speculative decoding (~20% generation speedup). `--cache-type-k turbo3 --cache-type-v turbo3` compresses the KV cache ~8× via Walsh-Hadamard Transform + 3-bit PolarQuant, enabling 262k context within 24 GB VRAM. The combined support required patching ik_llama.cpp's flash-attention kernels and CPY dispatch; those fixes live on `feature/turboquant-kv` at `git@github.com:maxfridbe/ik_llama_tq.cpp.git`.

**Key fixes applied** (see `feature/turboquant-kv` branch):
- Flash attention: `Q_q8_1=false` for turbo K types; correct qs bit-shift `2*(jj%4)`; explicit 128/256 head-dim instance files; `FA_ALL_QUANTS=ON`
- CPY kernel: flatten source to `[ne0, n_rows]` to match KV view layout; override `nb[1]=ggml_row_size(type,ne0)` for 1D V cache

## System Architecture

### Core Components (`src/`)

-   **`main.rs`**: Entry point; manages the high-level orchestrator and tokio runtime.
-   **`app.rs`**: Central state machine; manages `RenderBlock` history, input buffering, and TUI event loop.
-   **`ui.rs`**: Rendering layer; themed layouts, virtualization for large histories, interactive popups (Palette, Theme Selector, Session Manager).
-   **`context.rs`**: `ContextManager` — conversation history, token counting, assistant/tool message coordination. Merges all system messages into one before dispatch (required by Qwen3's Jinja template).
-   **`parser.rs`**: State-machine marker parser for raw OpenAI-compatible model output.
-   **`transport/`**: Provider-neutral messages/tools/events plus OpenAI and native Anthropic adapters.
-   **`client.rs`**: Transport orchestration and application stream events.
-   **`accounting.rs`**: Cache-aware per-request fixed-point API-equivalent pricing, idempotent request ledger, logical-turn totals, and cumulative session totals.
-   **`python/` + `tool_runtime.rs`**: Per-chat Python worker plus Host/Bubblewrap/transient Podman and retained Nonlocal Podman launch policy.
-   **`session_store.rs` + `headless_session.rs`**: External path/UUID advisory leases, durable directory identity registry, locked resume/delete, and headless session/accounting persistence.
-   **`python/runtime_store.rs` + `python/retained_runtime.rs`**: Exact manifest-bound container identity, COW lifecycle, crash reconciliation, and 14-day cleanup.
-   **`python/egress_broker.rs` + `python/supervisor.rs`**: Public-only HTTP(S) broker, route validation, host lease, unprivileged notebook worker, and narrow package supervisor.
-   **`python_policy.rs`**: Versioned global/project managed sidecars with atomic persistence.
-   **`wfe/` + `commands.rs`**: Canonical TUI/browser commands, redacted presentation contracts, single-owner remote actor, revisioned mirror, strict TLS/token profiles, and embedded HTTPS/WSS frontend.
-   **`loop_detector.rs`**: Multi-mode repetition detection (NGram, Phrase Frequency) to protect against hallucination loops.
-   **`markdown.rs`**: Syntax-aware rendering engine using `pulldown-cmark` and `syntect`.
-   **`tools/`**: Modular tool directory; each tool has its own logic and JSON schema.
-   **`system_prompt.rs`**: Manages prompt templates; resolves `[TOOL_CALL_FORMAT]` placeholder to model-specific instructions.

### External Configuration

-   **`~/.config/lethetic/config.yml`**: Fallback server endpoint, model, context, stable connection IDs/kinds, and switcher catalog.
-   **`~/.config/lethetic/python-mode.yml`**: Optional global managed Python tool/runtime policy.
-   **`<workspace>/.lethetic/python-mode.yml`**: Optional project policy overriding global; never stored in portable session JSON.
-   **`.lethetic/sessions/`**: Persistent UI/conversation/accounting state and opaque provider replay blocks; never Python globals.
-   **`~/.local/state/lethetic/session-locks/`**: External path/UUID locks and permanent UUID→directory identity bindings, outside model-writable sessions.
-   **`~/.local/state/lethetic/python-runtimes/`**: Exact retained-runtime manifests and redacted egress audit logs; locks live in sibling `python-runtime-locks/`, while per-runtime capability/socket bridges use the short sibling `b/<runtime-uuid>/` path so Linux `sockaddr_un` limits are checked before provider work.
-   **`~/tmp/lethetic-sessions/<session-uuid>/workspace/`**: Managed source workspace bound by canonical path/device/inode/SHA-256.

## Key Features

-   **Multi-connection switching**: Stable connection IDs switch among OpenAI-compatible servers and native `claude-code-proxy` without URL ambiguity.
-   **Provider-native replay**: Signed/redacted thinking and native tool blocks survive tool continuations and session save/load.
-   **Request-derived accounting**: Provider cache categories and every continuation are persisted idempotently; exact-model pricing yields latest-turn and cumulative-session API-equivalent estimates, never claimed subscription charges.
-   **Python-only profile**: Exactly one model tool, `python`; task state is available inside cells through the narrow host-backed `lethetic_todo` module. General mode may retain legacy `todowrite`.
-   **Optional Python isolation**: Host, Bubblewrap, transient rootless Podman, or retained Nonlocal Podman; explicit None/Public-only/Full semantics with no backend/network fallback or automatic pull.
-   **Autonomous Loop**: "Research → Strategy → Execution" cycle driven by the LLM.
-   **Authenticated web mirror**: Optional embedded Snabbdom SPA over exact-origin HTTPS/WSS, driven by the same command registry and sole `App` actor through bounded, revision-checked, redacted contracts.
-   **Themed TUI**: `ratatui`-based interface with multi-theme support, real-time streaming, and interactive tool approval.
-   **Advanced Loop Detection**: Real-time monitoring to prevent infinite token generation loops.
-   **Native Tool Calling**: Handles Anthropic `tool_use`, OpenAI structured deltas, and raw Gemma/Qwen marker formats behind one canonical transport surface.
-   **Empirical Verification**: Every tool backed by integration tests in `tests/`.

## Python Runtime Security Model

[`docs/python-isolation.md`](docs/python-isolation.md) is the complete source of truth for Python modes, shared-cwd masking, notebook auditing, `lethetic_todo`, retained layers, and operational troubleshooting. [`docs/web-remote-control.md`](docs/web-remote-control.md) documents the optional browser controller, TLS/authentication profiles, protocol, frontend build, and TUI-equivalent authority.

Python globals exist only for the active chat. New/load session, cancellation of an executing cell, crash, or profile/backend/policy changes kill the worker; model switches do not. General mode never advertises `python`. Python-only prompt/API schemas and the final dispatcher independently enforce exactly `python`, preventing stale or hallucinated calls from escaping through `task`, `todowrite`, or shell tools.

Host execution is deliberately unrestricted and remains approval-gated. Bubblewrap and Podman policies are complete snapshots loaded global → project → one-time. Repository policy cannot activate Host, select an interpreter, or enable retained session packages. A Distrobox `podman` compatibility symlink may forward to host Podman only after rootless/image/hardened probes. Paths are canonicalized and revalidated, special files are rejected, process commands use direct argv, environments are curated, and every automatic Podman container creation uses `--pull=never`; image pulling requires separate explicit confirmation.

Ordinary managed-policy `NetworkAccess::Nonlocal` is separate from Full. It requires Podman, package access `session`, a Lethetic-managed read/write workspace, and zero extra grants; the invocation-only `--python-isolated-with-nonlocal-network` profile is the explicit exception that binds the externally owned launch cwd at the same path and masks its `.lethetic` directory. In both cases the container has `--network=none`; a mounted typed UDS is its sole egress channel. The host broker accepts bounded absolute-form HTTP GET/HEAD on port 80 and CONNECT on canonical port 443, performs DNS itself, rejects mixed or non-global answers, validates route/interface state through netlink, connects a validated numeric address once, and verifies the peer. Broker/DNS/route/audit/lease failure tears down the worker rather than retrying as Full. Residual limits remain: a public service can relay traffic, and some public-address VPN/NAT-hairpin destinations cannot be distinguished from ordinary public endpoints.

The retained image is immutable. One stopped named container/COW layer belongs to one locked chat identity; exact manifest ID, labels, image, ABI, user namespace, mounts, network, workspace, SELinux profile, and explicit broker-layout profile are attested before attach/removal. Managed mounts use private relabeling and exact ProcessLabel/MountLabel peer verification. The external shared-cwd profile uses `label=disable` to avoid relabeling user source; only the broker's SELinux peer-label check is omitted there, while PIDFD-pinned process, credentials, start time, cgroup, executable identity, protocol, and capability checks remain. Schema-v2 manifests are parsed separately and migrated only when exactly one complete frozen old or short layout matches; runtime-local broker layouts remain delete-only. Python runs as the invoking UID with empty capability sets; only the supervisor accepts strict `lethetic-pkg refresh` or `lethetic-pkg install NAME...` requests and executes fixed apt argv as namespaced root. Maintainer scripts are not made safe. Detach resets globals and stops the container but preserves packages; successful resume/use refreshes a 14-day TTL. Startup/hourly reconciliation uses external identity locks and exact IDs, never `podman prune` or broad name/label deletion.

The sandbox contains only the Python worker. LLM HTTP transport stays in Lethetic on the host. `lethetic_todo` reaches only typed get/compare-and-swap operations for the pinned host todo store and remains usable while container Python cannot see the masked `.lethetic` directory. Namespace/container isolation is not a VM and does not promise CPU, memory, time, or kernel isolation.

## Running the Servers

```bash
# Start Gemma 4 (TurboQuant, port 7210)
sudo systemctl start gemma4

# Start Qwen3 MTP+turbo3 (ik_llama.cpp fork, port 7211)
sudo systemctl start qwen3

# Check status
~/Scripts/status_ai.sh
```

`claude-code-proxy` is intentionally not managed by Lethetic. Start/authorize it through its own launcher; Lethetic attaches to the configured loopback listener and reports it offline when absent.

Both services use `Restart=always` and load on `brainiac-nvidia`.

---
*Note: This application serves as a benchmark for local LLM tool-calling reliability and autonomous agent interfaces.*
