# Ollama Gemma 4 Runner

A simple Rust Terminal User Interface (TUI) application designed to test tool calling capabilities with the `prutser/gemma-4-26B-A4B-it-ara-abliterated:Q5_K_M` model running on a local Ollama instance.

## Features

- **Ratatui-based TUI**: A clean, split-pane terminal interface for sending prompts and viewing responses.
- **Native Tool Calling**: Fully configured to send JSON schema tool definitions to the Ollama `/api/chat` endpoint and parse the native OpenAI-compatible tool call responses that Gemma 4 generates.
- **Unit Tested**: Includes built-in unit tests to verify the JSON parsing of the tool call payload.

## Prerequisites

- [Rust & Cargo](https://rustup.rs/)
- A running [Ollama](https://ollama.com/) instance on `localhost:11434`
- The target model installed:
  ```bash
  ollama run prutser/gemma-4-26B-A4B-it-ara-abliterated:Q5_K_M
  ```

## Usage

1. Run the application:
   ```bash
   cargo run
   ```
2. Type a prompt (e.g., `"What is the weather in Paris today?"`) in the input box.
3. Press `Enter` to send the request to Ollama.
4. The application will intercept the tool call and display the parsed function name and arguments in the output box.
5. Press `Esc` to quit.

## Testing

To run the unit tests verifying the JSON parser:
```bash
cargo test
```