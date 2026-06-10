//! Live integration tests — they require the model servers from config.yml to be running.
//! Run all: `cargo test --test live`
//! Run one module: `cargo test --test live test_live_qwen3`

mod test_live_auto_summarize;
mod test_live_azure;
mod test_live_client_stream;
mod test_live_extended_coverage;
mod test_live_hello;
mod test_live_parser;
mod test_live_patch;
mod test_live_prompt_write_cs;
mod test_live_qwen3;
