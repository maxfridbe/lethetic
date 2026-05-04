# Lethetic Intelligence Engine

> "...so many various filters and enhancements, so many possible patterns, that it was as much an art as a craft. We didn't have the trained personnel that we needed, and as good as the lethetic intelligence engines were, they still lacked the ability to make intuitive leaps. LIs could give you statistical probabilities; they couldn't give you hunches—although the last I'd heard, they were working on adding that function too."

In Greek mythology, Lethe is the underworld river of oblivion and the goddess personifying forgetfulness, daughter of Eris (Strife). Shades (souls) of the dead drank from its waters to erase all memory of their mortal lives before reincarnation or entering the Elysian Fields.

A sophisticated Rust Terminal User Interface (TUI) designed for high-performance interaction with the Gemma 4 26B model. Optimized for **TurboQuant** and native tool-calling, it provides a robust platform for autonomous system engineering tasks.

![Lethetic UI](res/Screenshot.webp)

## Server Setup

The engine is specifically tuned for a **TurboQuant-optimized** server. To set up your remote server:

1. **Automation**: Run `setup_gemma4_server.sh` on your target Linux machine. It automates:
   - Cloning and building the `TheTom/llama-cpp-turboquant` fork with CUDA support.
   - Downloading the `Gemma-4-26B` UD-Q4_K_M model.
   - Setting up a systemd service (`gemma4.service`) on port `12345`.
   - Configuring `turbo3` KV cache quantization for ultra-low memory overhead.
2. **Server Requirements**:
   - **Endpoint**: `http://<server-ip>:12345/completion` (SSE streaming format).
   - **Context**: 262,144 tokens.
   - **Native Reasoning**: Enabled via `--reasoning on` and custom Jinja templates.

## Configuration

Lethetic uses a `config.yml` file for server connection and UI settings.

### Location Priority
1. **Local**: `./config.yml` (checked first).
2. **Global**: `~/.config/lethetic/config.yml` (fallback).

### Example `config.yml`
```yaml
server_url: "http://brainiac-nvidia:12345/completion"
model: "Gemma-4-26B-TurboQuant-262k"
context_size: 262144
tool_wrapper: null
enable_image_processing_tool: false # Enable vision tools if model supports it
```

## Advanced Features

### "Latest Files" Context Management
Lethetic features a unique state-management system to prevent context bloat. Instead of appending full file contents to the linear chat history every time a file is read or patched, the engine maintains a **Latest Files** context:
- **Dynamic Injection**: The most recent version of every file you interact with is automatically injected into a dedicated `<latest_files>` block right before the model's turn.
- **Stub History**: Linear chat history only contains small stubs (e.g., `[File read successfully...]`), keeping the conversation fast and focused.
- **View & Manage**: Use the Command Palette (**F1** / **Ctrl+P**) and select **Latest Files** to see all tracked files, their token counts, and relative age. Press **R** to remove any file from the context.

### Robust Multi-Line Patching
The `apply_patch` tool uses **programmatic diff generation**:
- **Semantic Edits**: The LLM provides the `old_content` and `new_content` blocks.
- **Deterministic Diffing**: The engine uses the `diffy` crate to generate a guaranteed-valid unified diff, which is then applied via the system `patch` command.
- **Resilient**: Automatically strips line numbers and markdown formatting frequently added by LLMs.

### Auto-Summarization
Handle massive outputs without losing context:
- **Safety Truncation**: Outputs >10,000 characters are automatically saved to disk (`.lethetic/tool_responses/`) and truncated in the context.
- **Summarization Tool**: Use the `summarize_content` tool to have the LLM analyze large files or raw text and provide a concise summary of the key findings.

### Enhanced TUI Experience
- **Syntax Highlighting**: Real-time syntax highlighting for all supported languages (powered by `Syntect`), including specialized styling for `read_file` output.
- **Hanging Indents**: Long code lines wrap with automatic indentation to align with line numbers, maintaining readability in narrow terminals.
- **Mouse Support**: Smooth mouse wheel scrolling for the output pane.
- **Input History**: Press **Up/Down** arrows at the boundary of the input field to scroll output, or use the Command Palette to browse and restore previous prompts.

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

Verify tool-calling and context logic:
```bash
cargo test -- --nocapture
```
For live model integration tests:
```bash
cargo test --test test_live_patch_read_integration -- --ignored --nocapture
```
