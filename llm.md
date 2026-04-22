# Lethetic: Autonomous Coding CLI

**Lethetic** is an experimental coding CLI and autonomous agent runner. The name is inspired by the works of **David Gerrold** (notably the *War Against the Chtorr* series), representing an attempt to build a sophisticated, tool-augmented interface for local LLMs.

This project is a **test-driven application** designed for evaluating and interacting with the `gemma-4-26B` model's tool-calling capabilities. It provides a rich TUI (Terminal User Interface) for autonomous agent interactions, prioritizing transparency, empirical verification, and clear separation of concerns.

## System Architecture

The runner is implemented in Rust using a modular approach:

### Core Components (`src/`)

-   **`main.rs`**: The entry point and orchestrator. It manages the `ratatui` TUI event loop, handles asynchronous communication with the Ollama server, manages themes, and coordinates tool execution flow.
-   **`context.rs`**: Implements the `ContextManager` which maintains the conversation history. It handles token counting (using `tiktoken-rs`), context trimming, and the injection of tool results into the message stream.
-   **`tools.rs`**: Defines the available tool set (e.g., `read_file_lines`, `apply_patch`, `run_shell_command`, `calculate`) and their JSON schemas. It ensures tool calls follow the required format, including mandatory tracking IDs.
-   **`markdown.rs`**: A robust markdown rendering engine for the TUI. It uses `pulldown-cmark` for parsing and `syntect` for syntax highlighting within code blocks, supporting tables, headings, and basic styling.
-   **`system_prompt.rs`**: Contains the `EXPERT_ENGINEER` system prompt, defining the agent's behavior, planning requirements, and tool call syntax.
-   **`icons.rs`**: Defines Nerd Font icons used throughout the TUI for visual feedback (status, tools, system info).

### External Configuration & Data

-   **`config.yml`**: Externalized configuration for server endpoint (`server_url`), model name (`model`), and context limits.
-   **`Cargo.toml`**: Project dependencies including `ratatui`, `tokio`, `reqwest`, `serde`, and `syntect`.

## Key Features

-   **Autonomous Loop**: Support for a "Research -> Strategy -> Execution" cycle driven by the LLM.
-   **TUI Layer**: A `ratatui`-based interface with multi-theme support, real-time streaming, and interactive tool interception/approval.
-   **Markdown Rendering**: Rich text formatting in the terminal, including syntax-highlighted code blocks and tables.
-   **Tool Calling**: Native tool calling support with strict JSON-in-XML-block requirements as defined in the system prompt.
-   **Verification**: Extensive test suite (including root-level `test_*.rs` files) to confirm JSON parsing, XML extraction, and tool logic integrity.

## Evaluation Goals

-   **Tool Call Accuracy**: Does the model correctly identify the tool and provide valid JSON arguments?
-   **Schema Adherence**: Does the generated call match the provided JSON schema, specifically regarding `tool_call_id`?
-   **Prompt Engineering**: Refining the `EXPERT_ENGINEER` prompt to ensure reliable autonomous behavior.
-   **UI Responsiveness**: Maintaining high performance during streaming and complex markdown rendering.

---
*Note: This application serves as a benchmark for local LLM tool-calling reliability and autonomous agent interfaces.*
