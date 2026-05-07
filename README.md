# Lethetic Intelligence Engine

> "...so many various filters and enhancements, so many possible patterns, that it was as much an art as a craft. We didn't have the trained personnel that we needed, and as good as the lethetic intelligence engines were, they still lacked the ability to make intuitive leaps. LIs could give you statistical probabilities; they couldn't give you hunches—although the last I'd heard, they were working on adding that function too."

In Greek mythology, Lethe is the underworld river of oblivion and the goddess personifying forgetfulness, daughter of Eris (Strife). Shades (souls) of the dead drank from its waters to erase all memory of their mortal lives before reincarnation or entering the Elysian Fields.

A sophisticated Rust Terminal User Interface (TUI) designed for high-performance interaction with the Gemma 4 26B model. Optimized for **TurboQuant** and native tool-calling, it provides a robust platform for autonomous system engineering tasks.

![Lethetic UI](res/Screenshot.webp)

## Server Setup

The engine is specifically tuned for a **TurboQuant-optimized** llama.cpp server running Gemma 4 26B. To set up a new server:

1. **Automation**: Run `setup_gemma4_server.sh` on your target Linux machine. It automates:
   - Cloning and building the `TheTom/llama-cpp-turboquant` fork with CUDA support.
   - Downloading `gemma-4-26B-A4B-it-UD-Q5_K_S.gguf` from Unsloth.
   - Installing the bundled `chat_template.jinja` (handles tool calls, reasoning, multi-turn).
   - Creating a systemd service (`gemma4.service`) on port `12345`.
   - Configuring `turbo3` KV cache quantization for ultra-low memory overhead.

2. **Server parameters** (as used on brainiac-nvidia):
   - Model: `gemma-4-26B-A4B-it-UD-Q5_K_S.gguf`
   - `--reasoning on` — enables chain-of-thought
   - `--jinja --chat-template-file chat_template.jinja` — custom tool-call template
   - `--cache-type-k turbo3 --cache-type-v turbo3` — TurboQuant KV quantization
   - `--ctx-size 262144` — 256k token context
   - `--temp 0.1 --repeat-penalty 1.09`

3. **API endpoint**: the server exposes a standard OpenAI-compatible API at `/v1/chat/completions`.

## Configuration

Lethetic uses a `config.yml` file for server connection and UI settings.

### Location Priority
1. **Local**: `./config.yml` (checked first).
2. **Global**: `~/.config/lethetic/config.yml` (fallback).

### Example `config.yml`
```yaml
server_url: "http://brainiac-nvidia:7210/v1/responses"
model: "Gemma-4-26B-TurboQuant-262k"
context_size: 262144
theme: "Default"               # optional — sets the startup theme
enable_image_processing_tool: false
```

The `theme` field accepts any theme name from the list in the Themes section. If omitted, `Default` is used.

## Architecture

### gemma-chat library (`gemma-chat/`)

A standalone Rust library that implements the OpenAI-compatible streaming client for Gemma 4:

- **SSE parser** (`sse.rs`) — parses `data:` lines from HTTP server-sent events.
- **Stream parser** (`stream.rs`) — stateful parser converting raw SSE chunks to typed events:
  - `ReasoningDelta` — model thinking tokens (displayed in thought blocks)
  - `TextDelta` — actual response text
  - `ToolCallStart / ToolCallDelta / ToolCallComplete` — structured tool invocations
  - `Done` — carries `completion_tokens`, `prompt_tokens`, `tg_per_s`, `pp_per_s`
- **Client** (`client.rs`) — `stream_chat()` and `complete()` over `/v1/chat/completions`.

The library is a workspace member and can be tested independently:
```bash
cargo test -p gemma-chat -- --nocapture
```

## Tools

The model has access to the following tools. All tools accept a `tool_call_id` (unique string identifier) and `description` (short action summary) alongside their specific parameters.

### File System

| Tool | Description |
|---|---|
| `read_file` | Read a complete file. Output includes line numbers for patching. |
| `read_file_lines` | Read a specific line range from a file (start/end line, inclusive). |
| `read_folder` | List files and subdirectories at a path (non-recursive). |
| `write_file` | Create or overwrite a file. Parent directories are created automatically. |
| `replace_text` | Replace an exact literal string in a file. Must match exactly one occurrence. |
| `apply_patch` | Modify a file by specifying `old_content` and `new_content` blocks. Uses `diffy` to generate a deterministic unified diff — resilient to LLM-added formatting. |
| `search_text` | Search for a regex pattern across files in a directory. |

### Shell & Calculation

| Tool | Description |
|---|---|
| `run_shell_command` | Execute a bash command on the local system and return stdout/stderr. Output is streamed to the UI in real time. |
| `calculate` | Evaluate a mathematical expression (e.g. `sin(pi/2)`, `2**10`). |

### Web

| Tool | Description |
|---|---|
| `read_page` | Fetch a URL and convert the page to clean Markdown. Preferred for information retrieval. |
| `web_fetch` | Fetch raw HTML/text content from a URL. |
| `web_search` | Search the web via DuckDuckGo and return result snippets. |

### Document & Vision *(requires `enable_image_processing_tool: true`)*

| Tool | Description |
|---|---|
| `get_pdf_text` | Extract the full text layer from a PDF (pure Rust, no external deps). |
| `process_image` | Analyze an image with vision capabilities. Long edge resized to `max_size` (default 1024) before processing. |
| `process_pdf_image` | Render a specific PDF page to an image and analyze it with vision. |

### Interaction & Context

| Tool | Description |
|---|---|
| `ask_the_user` | Pause execution and ask the user a question. The response is injected back into the conversation. |
| `summarize_content` | Summarize a file or large string using the LLM. Used automatically when tool output exceeds the truncation limit. |

### Tool Approval

Shell commands (`run_shell_command`) require user approval before execution. Press **A** to always allow for the session, **O** for once, or **D** to deny. The approval mode can be locked to *Always* from the Command Palette.

## Advanced Features

### "Latest Files" Context Management
Lethetic features a unique state-management system to prevent context bloat. Instead of appending full file contents to the linear chat history every time a file is read or patched, the engine maintains a **Latest Files** context:
- **Dynamic Injection**: The most recent version of every file you interact with is automatically injected into a dedicated `<latest_files>` block right before the model's turn.
- **Stub History**: Linear chat history only contains small stubs (e.g., `[File read successfully...]`), keeping the conversation fast and focused.
- **View & Manage**: Use the Command Palette (**F1** / **Ctrl+P**) and select **Latest Files** to see all tracked files, their token counts, and relative age. Press **R** to remove any file from the context.

### Robust Multi-Line Patching
The `apply_patch` tool uses **programmatic diff generation**:
- **Semantic Edits**: The LLM provides `old_content` and `new_content` blocks.
- **Deterministic Diffing**: The engine uses the `diffy` crate to generate a guaranteed-valid unified diff applied via the system `patch` command.
- **Resilient**: Automatically strips line numbers and markdown formatting frequently added by LLMs.

### Auto-Summarization
Handle massive outputs without losing context:
- **Safety Truncation**: Outputs >10,000 characters are automatically saved to disk (`.lethetic/tool_responses/`) and truncated in the context.
- **Summarization Tool**: Use the `summarize_content` tool to have the LLM analyze large files or raw text and provide a concise summary.

### Enhanced TUI Experience
- **Solid background**: Each theme applies a solid fill color to the entire terminal — no bleed-through from the underlying terminal background.
- **Themes**: 30 built-in themes including dark (Default, Matrix, Cyberpunk, Ocean, Sunset, Forest, Lavender, Mono, Gold, Deep Sea, Dracula, Nord, Gruvbox, Tokyo Night, Monokai, Obsidian, Ash, Infrared) and light (Paper, Solarized Light, GitHub Light, Ivory, Rose, Mint, Sky, Linen, Chalk, Parchment, Clay, Fog). Theme is persisted per session and loaded automatically on resume.
- **Performance stats**: Status bar shows server-reported `tg` (token generation, t/s) and `pp` (prompt processing, t/s) from the server's timings, plus accurate context token count from the server's usage field.
- **Syntax Highlighting**: Real-time syntax highlighting for `sh`, `rs`, `cs`, `js`, `ts`, `py`, `cpp`, `json`, `toml`, `yaml`, and `md` in code blocks (powered by Syntect).
- **Hanging Indents**: Long code lines wrap with automatic indentation to align with line numbers.
- **Mouse Support**: Smooth mouse wheel scrolling for the output pane.
- **Input History**: Press **Up/Down** at the boundary of the input field to scroll output, or use the Command Palette to browse and restore previous prompts.

## Key Hotkeys

- **TAB**: Switch focus between Input and Output panes.
- **UP / DOWN**:
    - **At Input Boundary**: Scroll output line-by-line.
    - **In Input**: Move cursor or navigate history.
- **ALT + UP / DOWN**: Scroll output line-by-line at any time.
- **PAGE UP / DOWN**: Scroll output by 20 lines.
- **F1 / CTRL+P**: Open the Command Palette.
- **F12**: Toggle the Debugger pane.
- **CTRL+C**: Stop output (1st press) / Quit (2nd press).

## Usage

1. **Run**:
   ```bash
   cargo run --bin lethetic
   ```
2. **Headless Mode**: Execute single tasks directly from your shell:
   ```bash
   cargo run --bin lethetic -- --command "Analyze the main loop in src/main.rs"
   ```

## Testing

Unit and non-live tests:
```bash
cargo test
```

Live integration tests (require the server to be running):
```bash
# Run all live tests one at a time
cargo test --test test_live_extended_coverage -- --ignored --nocapture
cargo test --test test_live_parser          -- --ignored --nocapture
cargo test --test test_live_patch           -- --ignored --nocapture

# Full pipeline test (prompt → model → tool call → file write → verify)
cargo test --test test_live_prompt_write_cs -- --ignored --nocapture
```

All integration tests live in `tests/integration/` and follow the `test_live_*` naming convention.
