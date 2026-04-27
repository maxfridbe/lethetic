# Lethetic: Autonomous Coding CLI

> **NOTE:** Lethetic follows a **reproducible test philosophy**. Every feature and bug fix MUST be empirically verified through automated integration scenarios or unit tests before deployment. This ensures the reliability of the autonomous loop and prevents regressions in tool-calling logic.

**Lethetic** is an experimental coding CLI and autonomous agent runner. The name is inspired by the works of **David Gerrold** (notably the *War Against the Chtorr* series), representing an attempt to build a sophisticated, tool-augmented interface for local LLMs.

This project is a **test-driven application** designed for evaluating and interacting with the `gemma-4-26B` model's tool-calling capabilities. It provides a rich TUI (Terminal User Interface) for autonomous agent interactions, prioritizing transparency, empirical verification, and clear separation of concerns.

## System Architecture

The runner is implemented in Rust using a modular approach:

### Core Components (`src/`)

-   **`main.rs`**: The entry point. Manages the high-level orchestrator and tokio runtime.
-   **`app.rs`**: The central state machine. Manages `RenderBlock` history, input buffering, and the TUI event loop logic.
-   **`ui.rs`**: The rendering layer. Implements themed layouts, virtualization for large histories, and interactive popups (Palette, Theme Selector, Session Manager).
-   **`context.rs`**: Implements the `ContextManager` which maintains conversation history, token counting, and assistant/tool message coordination.
-   **`parser_new.rs`**: A robust, state-machine based marker parser that handles streaming LLM output, isolating thoughts, text, and tool calls in real-time.
-   **`client.rs`**: High-performance SSE streaming client optimized for TurboQuant servers.
-   **`loop_detector.rs`**: Implements multi-mode repetition detection (NGram, Phrase Frequency) to protect against LLM "hallucination loops."
-   **`markdown.rs`**: A syntax-aware rendering engine using `pulldown-cmark` and `syntect` for rich TUI display.
-   **`tools/`**: A modular directory where each tool (e.g., `run_shell_command`, `apply_patch`) is defined with its own logic and JSON schema.
-   **`system_prompt.rs`**: Manages the `EXPERT_ENGINEER` templates and capability profiles.

### External Configuration & Data

-   **`config.yml`**: Externalized configuration for server endpoint, model name, and context limits.
-   **`.lethetic/sessions/`**: Persistent storage for UI state and conversation context, allowing for session resumption.

## Key Features

-   **Autonomous Loop**: Support for a "Research -> Strategy -> Execution" cycle driven by the LLM.
-   **Themed TUI Layer**: A `ratatui`-based interface with multi-theme support, real-time streaming, and interactive tool interception/approval.
-   **Advanced Loop Detection**: Real-time monitoring to prevent infinite token generation loops.
-   **Native Tool Calling**: Fully implements the Gemma 4 protocol with strict schema validation.
-   **Empirical Verification**: Every tool is backed by integration tests in `tests/` to ensure schema and behavioral compliance.

## Evaluation Goals

-   **Tool Call Accuracy**: Does the model correctly identify the tool and provide valid JSON arguments?
-   **Schema Adherence**: Does the generated call match the provided JSON schema?
-   **Prompt Engineering**: Refining the `EXPERT_ENGINEER` prompt to ensure reliable autonomous behavior.
-   **UI Responsiveness**: Maintaining 60fps performance during heavy streaming and markdown rendering.

---
*Note: This application serves as a benchmark for local LLM tool-calling reliability and autonomous agent interfaces.*
