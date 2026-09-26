//! Live integration tests.
//!
//! The non-ignored tests use a hosted connection (OpenRouter by default, see
//! `live_config.rs`) and never touch the local GPU servers. Tests that need a
//! local llama.cpp, Azure, or the Codex proxy are `#[ignore]`d; run those
//! explicitly with `cargo test --test live -- --ignored <name>`.
//! Run all hosted tests: `cargo test --test live`

mod live_config;

mod test_live_auto_summarize;
mod test_live_azure;
mod test_live_claude_proxy;
mod test_live_client_stream;
mod test_live_extended_coverage;
mod test_live_hello;
mod test_live_parser;
mod test_live_patch;
mod test_live_prompt_write_cs;
mod test_live_qwen3;
