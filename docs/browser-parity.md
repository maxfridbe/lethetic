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
| Context budgeting, older tool-output shortening, call-ID repair | `src/context.rs`, `src/context/api.rs` |
| Transient error classification and retry backoff | `gemma-chat/src/retry.rs` |
| Thinking-loop detection | `src/loop_detector.rs` |
| Transcript UI, composer, files, panels and command protocol | `web/src`, generated WFE contracts |

The browser's saved JSON history is translated into the CLI `ContextManager` by
`src/engine/context.rs`; the input budget affects requests, not the saved transcript.
Native filesystem reads are isolated in `src/context/filesystem.rs`.
`prepare_api_context_with_files` accepts a host snapshot reader. The browser does
not yet automatically put session files into the CLI active/latest-file cache;
its code workers access the session workspace directly.

The existing CLI text/tool-tag parser, accounting types and calculator also compile
in the portable library. Compilation alone does not mean their complete UI/tool
features are exposed in the browser.

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
- Share automatic compaction and provider retry UI state. Manual browser compaction
  is available, but still uses its existing bounded-pass browser coordinator.
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
