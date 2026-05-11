# Lethetic Intelligence Engine

> "...so many various filters and enhancements, so many possible patterns, that it was as much an art as a craft. We didn't have the trained personnel that we needed, and as good as the lethetic intelligence engines were, they still lacked the ability to make intuitive leaps. LIs could give you statistical probabilities; they couldn't give you hunches—although the last I'd heard, they were working on adding that function too."

A Rust TUI coding agent for locally-hosted LLMs. Optimized for **TurboQuant** llama.cpp servers and native tool-calling. Supports multiple model backends simultaneously via hot-swap GPU sleep.

![Lethetic UI](res/Screenshot.webp)

---

## Quick Start

```bash
# Build
cargo build --release

# Run (reads config from ./config.yml or ~/.config/lethetic/config.yml)
cargo run --bin lethetic

# Headless / scripted
cargo run --bin lethetic -- --command "Fix all TypeScript errors in src/"
```

---

## Server Setup

Lethetic is tuned for a **TurboQuant llama.cpp** server. Two models are supported simultaneously via GPU hot-swap (each sleeps when idle, wakes on request).

### Gemma 4 26B — port 7210

```bash
bash setup_gemma4_server.sh
```

Automates: build TurboQuant llama.cpp fork with CUDA, download `gemma-4-26B-A4B-it-UD-Q5_K_S.gguf`, install chat template, create `gemma4.service`.

Key parameters:
- `--cache-type-k turbo3 --cache-type-v turbo3` — TurboQuant KV quantization
- `--ctx-size 262144` — 256k context
- `--reasoning on --jinja` — chain-of-thought + custom tool-call template
- `--temp 0.1 --repeat-penalty 1.09`

### Qwen3 27B — port 7211

```bash
bash setup_qwen3_server.sh
```

Downloads `Qwen3.6-27B-Q5_K_M.gguf` and creates `qwen3.service`.

Key parameters:
- `--cache-type-k turbo3 --cache-type-v turbo3` — required to fit 131k KV cache in VRAM
- `--ctx-size 131072` — 128k context
- `--reasoning on --jinja`
- `--temp 0.6 --repeat-penalty 1.05`

### GPU memory notes

Both models use `--sleep-idle-seconds 30s`. When idle, each releases GPU VRAM. With two RTX cards (≈24GB total), only one model is resident at a time. Switching models causes a ~5–10s reload pause on first request.

---

## Configuration

### Location priority
1. `./config.yml` (checked first)
2. `~/.config/lethetic/config.yml`

### Example `config.yml`

```yaml
server_url: http://brainiac-nvidia:7210/v1/responses
model: Gemma-4-26B-TurboQuant-262k
context_size: 262144
theme: Default

model_servers:
  - name: Gemma 4 26B
    url: http://brainiac-nvidia:7210/v1/responses
    model: Gemma-4-26B-TurboQuant-262k
    parser: gemma4

  - name: Qwen3 27B
    url: http://brainiac-nvidia:7211/v1/responses
    model: Qwen3-27B-Q5
    parser: qwen3
```

### `parser` dialect

Each `model_servers` entry has a `parser` field that controls two things:

| `parser` | Initial state | Tool call format |
|---|---|---|
| `gemma4` (default) | Thought block | `<\|"\|>string<\|"\|>` asymmetric markers |
| `qwen3` / `default` | Text | Standard JSON strings |

The system prompt's **Tool call format** section is automatically tailored to the active model — Qwen3 receives plain JSON instructions; Gemma4 receives the asymmetric marker instructions. Switching models via the palette reloads the parser and context.

---

## Model Switcher

**Ctrl+P → Models** — opens a panel that queries `/v1/models` on every configured server and shows a combined list. The active model is marked `▶`. Selecting a new entry:
- Switches `server_url` and `model` for the session
- Resets the stream parser to the new dialect
- Updates the status bar with the active model name

---

## Architecture

### gemma-chat library (`gemma-chat/`)

Standalone Rust library for OpenAI-compatible streaming over llama.cpp:

- **SSE parser** — `data:` line parsing from HTTP server-sent events
- **Stream parser** — converts raw SSE to typed events: `ReasoningDelta`, `TextDelta`, `ToolCallComplete`, `Done`
- **Client** — `stream_chat()` and `complete()` over `/v1/chat/completions`

```bash
cargo test -p gemma-chat -- --nocapture
```

### Stream parser (`src/parser.rs`)

Stateful chunk parser. Mode controls initial state and which token markers are recognised:

- **Gemma4**: starts in `Thought`; markers: `<|channel>thought`, `<channel|>`, `<|tool_call>`, `<|channel>text`
- **Qwen3 / default**: starts in `Text`; markers: `<think>`, `</think>`, `<tool_call>`

---

## Context Management

### Two-tier file cache

Files read or written during a session are tracked in two tiers:
- **active_files** — accessed ≤3 turns ago; injected as `<active_file>` immediately before the model turn (highest attention)
- **latest_files** — older; injected as `<latest_files>` before the system prompt (background context)

Files are always re-read from disk at context-build time. If a file was deleted or moved, the context shows `⚠ File was deleted or no longer exists on disk.`

### Token budget

Files are evicted (oldest first) when the total file token budget exceeds 35% of `context_size`. The prompt order is:

```
latest_files → system_prompt → messages → active_file → [model turn]
```

### Large tool output

Tool outputs > 20,000 chars are saved to `.lethetic/tool_responses/<id>.txt` and replaced in context with a truncation message and navigation hint. `read_file` is exempt — file content always goes into the cache regardless of size (up to 500k chars).

---

## Tools

All tools accept `tool_call_id` (unique string identifier) and `description` (short action summary).

### File System

| Tool | Description |
|---|---|
| `read_file` | Read a complete file with line numbers. Always placed in file cache — no truncation. |
| `read_file_lines` | Read a line range (start–end, inclusive). |
| `read_folder` | List files and subdirectories (non-recursive). |
| `write_file` | Create or overwrite a file. Parent dirs created automatically. |
| `edit` | Fuzzy-match file edit: tolerates whitespace drift. Three-tier matching: exact → normalized → similarity-scored. |
| `replace_text` | Replace an exact string occurrence. `replace_all: true` to replace every match. Error includes line numbers on multi-match. |
| `apply_patch` | Block-level replace via `old_content`/`new_content`. Uses `diffy` to generate a deterministic diff. |
| `glob` | ripgrep-based file pattern search (`**/*.ts`). Results sorted by mtime, capped at 200. |
| `search_text` | Regex search across files. Prefers `rg`; excludes `target/`, `.git/`, `node_modules/`. |

### Code Intelligence

| Tool | Description |
|---|---|
| `find_symbol` | Definition, references, or all-symbols scan via `rg` patterns. |
| `lsp` | Language Server Protocol: `goToDefinition`, `findReferences`, `hover`, `documentSymbol`, `workspaceSymbol`. Auto-installs missing server on first use. Falls back to `find_symbol` if unavailable. Supported: Rust (rust-analyzer), TypeScript (typescript-language-server), Python (pyright), Go (gopls), C/C++ (clangd), C# (csharp-ls), Lua. |

### Shell & Math

| Tool | Description |
|---|---|
| `run_shell_command` | Execute a bash command; output streamed to UI in real time. Requires approval. |
| `calculate` | Evaluate math: `sin(pi/2)`, `sqrt(9)`, `2^10`, `log(100,10)`. Powered by `meval`. |

### Web

| Tool | Description |
|---|---|
| `fetch_url` | Fetch a URL and convert to Markdown (default), plain text, or raw HTML. |
| `web_search` | DuckDuckGo search. `num_results` param (1–20, default 10). |

### Project & Tasks

| Tool | Description |
|---|---|
| `repo_overview` | Ecosystem detection, 2-level dir tree, README preview, entry points. |
| `todowrite` | Write a structured todo list (status + priority) to `.lethetic/todos.json`. |
| `task` | Spawn an autonomous sub-agent with all tools except `task` and `ask_the_user`. 5-minute timeout; sub-agent progress streamed to parent UI. |

### Document & Vision *(requires `enable_image_processing_tool: true`)*

| Tool | Description |
|---|---|
| `get_pdf_text` | Extract full text layer from a PDF. |
| `process_image` | Analyze an image with vision. |
| `process_pdf_image` | Render a PDF page to image and analyze it. |

### Interaction

| Tool | Description |
|---|---|
| `ask_the_user` | Pause and ask the user a question. Response is injected back into context. |
| `summarize_content` | LLM summarization of a file or text. `prompt` required. |

---

## Engine Reliability

### Loop detection

Combined NGram + phrase-frequency watchdog. Only model text/thought output is checked — tool results (compiler errors, stack traces) are excluded to prevent false positives.

- NGram window: 128 chars, threshold: 4 occurrences
- Phrase frequency: tracks self-correction phrases (`"Actually,"`, `"Wait,"`, etc.)
- On detection: auto-injects correction prompt; on persistent loop: hands control to user

### Duplicate tool call detection

Same tool + same key parameters called repeatedly:
- `edit` / `replace_text`: warns at 2nd identical call
- `run_shell_command` with `rm`/`mv`/`unlink`: warns at 2nd call
- All others: warns at 3rd call

If an `edit`/`replace_text` was already applied earlier in the session and the model tries it again (with the same `old_string`), it receives: *"EDIT ALREADY APPLIED — move on to the next issue."*

### Intent-text detection

If the model responds with a short text describing what it's about to do (without calling a tool), lethetic re-prompts: *"You described an action without calling a tool. Call the tool now."*

### Stop-reason status bar

The status bar shows why the engine stopped:
- `Response complete (N tokens)` / `Response complete (N tokens, context X% full)`
- `→ Tool dispatched: <tool>` / `→ Loop #N detected — auto-correcting`
- `⚠ Context saturated` / `⚠ Persistent loop terminated` / `⚠ Minimal response`
- `⏸ Waiting for your answer: <question>` / `⏸ Awaiting approval: <tool>`
- `✗ Server error: <msg>` / `Cancelled by user`

---

## Hotkeys

| Key | Action |
|---|---|
| **TAB** | Switch focus: Input ↔ Output |
| **Up / Down** (at input boundary) | Scroll output line by line |
| **Alt + Up / Down** | Scroll output at any time |
| **Page Up / Down** | Scroll output 20 lines |
| **F1 / Ctrl+P** | Command Palette |
| **F12** | Toggle debugger pane |
| **Ctrl+C** | Stop generation (1st) / Quit (2nd) |

### Command Palette items

| # | Item | Action |
|---|---|---|
| 0 | Hotkeys | Show key reference |
| 1 | Themes | Pick from 30 built-in themes |
| 2 | Input History | Browse and restore previous prompts |
| 3 | Loop Detection | Cycle detection mode (Off/NGram/Phrase/Combined) |
| 4 | System Prompt | Edit or switch prompt template |
| 5 | Clear UI | Clear display, keep context |
| 6 | Clear All | Clear display and context, start fresh |
| 7 | Toggle Debugger | Show/hide debug log pane |
| 8 | Sessions | Load, resume, or delete sessions |
| 9 | Latest Files | View and manage file context cache |
| 10 | Models | Switch between configured model servers |
| 11 | LSP Servers | View install status; Enter to auto-install |
| 12 | Quit | Exit |

---

## Testing

### Unit tests

```bash
cargo test --lib
```

### Live integration tests

Require the server(s) to be running. Tests within and across test binaries are serialized via the `llm` named lock — Gemma4 and Qwen3 tests cannot run simultaneously.

```bash
# Gemma4 tests
cargo test --test test_live_extended_coverage -- --nocapture
cargo test --test test_live_hello            -- --nocapture
cargo test --test test_live_client_stream    -- --nocapture

# Qwen3 tests (requires qwen3.service running on port 7211)
cargo test --test test_live_qwen3            -- --nocapture

# Patch/parser/summarize tests
cargo test --test test_live_patch            -- --nocapture
cargo test --test test_live_parser           -- --ignored --nocapture
cargo test --test test_live_prompt_write_cs  -- --ignored --nocapture
```

### Diagnostic tools

```bash
# Replay a session's token stream through the parser
cargo run --bin playback -- .lethetic/sessions/<session>/tokens.jsonl
```
