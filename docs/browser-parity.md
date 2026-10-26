# CLI core and browser host

The CLI is the source of agent behavior. Portable code belongs in the `lethetic`
library and compiles for both native and `wasm32-unknown-unknown`. The
`lethetic-wasm` package contains JavaScript bindings. New browser agent features
should reuse CLI code or first extract a host interface from it.

This migration is **not complete CLI parity**. The native application and browser
host still have different orchestration and persistence adapters. Moving a module
into the root crate alone does not make its behavior shared with the native CLI.

## Code already used by both hosts

| Behavior | Source |
| --- | --- |
| OpenAI SSE validation, usage framing, tool-call assembly | `gemma-chat/src/stream.rs` |
| Anthropic thinking/signatures, tool blocks, usage and terminal validation | `src/engine/anthropic.rs` |
| OpenAI request builder and extra-body validation | `gemma-chat/src/request.rs` |
| Provider message serialization | `src/engine/messages.rs` |
| Context budgeting, projected token estimate, older tool-output shortening, call-ID repair | `src/context.rs`, `src/context/api.rs` |
| Transient error classification and retry backoff | `gemma-chat/src/retry.rs` |
| Manual compaction planning, prompts, file-body elision, bounded merges and summary validation | `src/engine/compaction.rs`, `src/engine/compaction_plan.rs` |
| Thinking-loop detection | `src/loop_detector.rs` |
| Transcript UI, composer, files, panels and command protocol | `web/src`, generated WFE contracts |

The browser's saved JSON history is translated into the CLI `ContextManager` by
`src/engine/context.rs`; the input budget affects requests, not the saved transcript.
The browser status uses that same projected token estimate until provider input
usage arrives; it persists the latest request count separately from cumulative
turn/session totals. The browser host discovers OpenRouter model/provider limits
and reserves the output budget within the advertised context ceiling. See
[context and status controls](../integration.md#context-and-status-bar).
Native filesystem reads are isolated in `src/context/filesystem.rs`.
`prepare_api_context_with_files` accepts a host snapshot reader. The browser does
not yet automatically put session files into the CLI active/latest-file cache;
its code workers access the session workspace directly.

The existing CLI text/tool-tag parser, accounting types and calculator also compile
in the portable library. Compilation alone does not mean their complete UI/tool
features are exposed in the browser.

## Shared manual compaction

Native CLI and browser use the same prompts, UTF-8-safe transcript windows and
hierarchical merge plan. Requests contain no executable tools. Each pass uses the configured output budget up to
8,192 tokens, including hidden reasoning. Browser chat defaults to the advertised model/provider output maximum, with a 65,536-token fallback; older saved manual caps remain unchanged. Compaction still caps each pass at 8,192;
low output budgets can cause reasoning models to exhaust the budget before
producing a summary. Truncated responses fail rather than replace history. Input windows respect the configured input budget and
an 80,000-byte ceiling. Empty, oversized or non-shrinking summaries fail without
replacing the original context. Reasoning text is excluded from the summary.

The native adapter runs up to four independent passes concurrently and streams
the final merge to the compaction dialog. The browser executes passes sequentially
using its fetch adapter; its turn timeout covers the entire compaction. Browser
compaction commits in the current session after persistence succeeds. The native
CLI retains its existing compacted-session workflow and settings inheritance.
Cancellation and provider failures leave source history intact in both hosts.

## Remaining work

- Unify native application/headless and browser turn orchestration, cancellation,
  checkpointing and automatic context compaction behind host services. The browser
  still drives model/tool rounds in `browser/src/sdk.ts`; its transactional history
  adapter is `src/engine/Conversation`, not the native application's turn loop.
- Share prompt assembly and configuration fully. Python/Pyodide and .NET capability
  instructions must continue to describe the actual selected host accurately.
- Expose CLI general-purpose file tools through a session-filesystem adapter,
  including the active/latest-file context cache. Browser code workers already
  synchronize session files, but do not provide the CLI filesystem tool surface.
- Port task/TODO/skill and approval state machines with browser storage and tool
  authority adapters; keep the same remote command contracts where applicable.
- Share automatic compaction triggers and provider retry UI state. Manual
  compaction now uses the same Rust coordinator in both hosts.
- Add native connection selection to the standalone host only with the existing
  authenticated remote-control origin/session protections preserved.

Shells, processes, LSP servers, containers and unrestricted OS sockets require a
native host. The browser cannot implement those by compiling Rust alone. Select a
native `lethetic --rc` connection for full host capabilities; browser networking
uses fetch/CORS, Python uses Pyodide, and .NET uses DotnetDide.

## Verification

`./build-browser.sh` runs portable CLI and parser unit tests, WASM compilation,
SDK tests and Python worker lifecycle tests. Browser CI also checks the CLI library
with Clippy on the WASM target. `scripts/test_browser.py --pyodide` exercises the
shared UI, streaming, settings persistence, real Pyodide and .NET workers.
Native `cargo test -p lethetic --lib` and strict all-target Clippy guard the existing
CLI behavior. Live-provider tests supplement deterministic tests; provider keys
belong in private process environments, never fixtures, screenshots or Git.

For an opt-in native provider check, set `LETHETIC_TEST_ENDPOINT`,
`LETHETIC_TEST_MODEL` and `LETHETIC_TEST_API_KEY` in a private environment, then run
`cargo test -p lethetic --lib live_environment_compaction -- --ignored`. This makes
a provider request and may incur charges; it is excluded from normal CI.
