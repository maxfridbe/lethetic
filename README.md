# Lethetic Intelligence Engine

> "...so many various filters and enhancements, so many possible patterns, that it was as much an art as a craft. We didn't have the trained personnel that we needed, and as good as the lethetic intelligence engines were, they still lacked the ability to make intuitive leaps. LIs could give you statistical probabilities; they couldn't give you hunches—although the last I'd heard, they were working on adding that function too."

A sophisticated Rust Terminal User Interface (TUI) designed for high-performance interaction with the Gemma 4 26B model. Optimized for **TurboQuant** and native tool-calling, it provides a robust platform for autonomous system engineering tasks.

## Features

- **Interactive TUI**: Powered by Ratatui, featuring full-width background styling, horizontal dividers, and a searchable/selectable output history.
- **TurboQuant Support**: Custom-built for the Unsloth/TurboQuant llama.cpp fork, supporting `tbq3` and `tbqp3` KV cache quantization for massive context windows (up to 262K).
- **Native Tool Calling**: Fully implements the Gemma 4 native protocol, allowing the model to perform complex tasks like reading files, applying patches, and executing shell commands.
- **Robust Parsing**: "Marker-Aware" parser that safely handles multiline shell scripts, internal braces, and UTF-8 characters without UI freezes.
- **Performance Optimized**: Features line-based caching, background status updates, and a "sliding window" for large tool outputs to ensure a fluid 60fps experience.
- **Security First**: Includes a granular "Security Confirmation" prompt for tool execution, with support for one-time or permanent approval.

## Prerequisites

- [Rust & Cargo](https://rustup.rs/) (2024 edition)
- A running `llama-server` instance (TurboQuant fork recommended) on port `7210`.
- The target model: `Gemma-4-26B-TurboQuant-262k`.

## Usage

1. **Setup**: Use the provided `setup_gemma4_server.sh` on your Debian/Ubuntu server to automate the compilation and configuration.
2. **Run**: 
   ```bash
   cargo run --bin lethetic
   ```
3. **Headless Mode**: Execute single tasks directly from your shell:
   ```bash
   cargo run --bin lethetic -- --command "Create a hello world program in Rust"
   ```

## Key Hotkeys

- **TAB**: Switch focus between Input and Output panes.
- **UP/DOWN**: Scroll through output history (when focused).
- **F12**: Toggle the Debugger pane.
- **F10**: Toggle Mouse Capture (for native terminal selection).
- **ESC / CTRL+P**: Open the Command Palette.
- **CTRL+C**: Stop output (1st press) / Quit (2nd press).

## Testing

Comprehensive integration scenarios for tool-calling:
```bash
cargo run --bin eval_scenarios
```
