# Lethetic: Autonomous Coding CLI

> **NOTE:** Lethetic follows a **reproducible test philosophy**. Every feature and bug fix MUST be empirically verified through automated integration scenarios or unit tests before deployment. This ensures the reliability of the autonomous loop and prevents regressions in tool-calling logic.

**Lethetic** is an experimental coding CLI and autonomous agent runner. The name is inspired by the works of **David Gerrold** (notably the *War Against the Chtorr* series), representing an attempt to build a sophisticated, tool-augmented interface for local LLMs.

This project is a **test-driven application** designed for evaluating and interacting with local LLMs' tool-calling capabilities via a rich TUI (Terminal User Interface) for autonomous agent interactions.

## Supported Models

Lethetic runs against two local LLM servers on `brainiac-nvidia`. Switch between them with the command palette (Ctrl+P).

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
-   **`parser_new.rs`**: State-machine marker parser; handles streaming LLM output, isolating thoughts, text, and tool calls in real-time.
-   **`client.rs`**: SSE streaming client; handles both raw JSON tool calls (Gemma4) and structured `tool_calls` deltas (Qwen3+Jinja).
-   **`loop_detector.rs`**: Multi-mode repetition detection (NGram, Phrase Frequency) to protect against hallucination loops.
-   **`markdown.rs`**: Syntax-aware rendering engine using `pulldown-cmark` and `syntect`.
-   **`tools/`**: Modular tool directory; each tool has its own logic and JSON schema.
-   **`system_prompt.rs`**: Manages prompt templates; resolves `[TOOL_CALL_FORMAT]` placeholder to model-specific instructions.

### External Configuration

-   **`~/.config/lethetic/config.yml`**: Server endpoint, model name, context limits, and `model_servers` list for the switcher.
-   **`.lethetic/sessions/`**: Persistent session storage for UI state and conversation context.

## Key Features

-   **Multi-model switching**: Command palette switches between Gemma4 and Qwen3 without restart.
-   **Autonomous Loop**: "Research → Strategy → Execution" cycle driven by the LLM.
-   **Themed TUI**: `ratatui`-based interface with multi-theme support, real-time streaming, and interactive tool approval.
-   **Advanced Loop Detection**: Real-time monitoring to prevent infinite token generation loops.
-   **Native Tool Calling**: Handles both structured `tool_calls` (Qwen3+Jinja) and raw JSON marker format (Gemma4).
-   **Empirical Verification**: Every tool backed by integration tests in `tests/`.

## Running the Servers

```bash
# Start Gemma 4 (TurboQuant, port 7210)
sudo systemctl start gemma4

# Start Qwen3 MTP+turbo3 (ik_llama.cpp fork, port 7211)
sudo systemctl start qwen3

# Check status
~/Scripts/status_ai.sh
```

Both services use `Restart=always` and load on `brainiac-nvidia`.

---
*Note: This application serves as a benchmark for local LLM tool-calling reliability and autonomous agent interfaces.*
