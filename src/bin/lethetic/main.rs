mod app_events;
mod cli;
mod compaction;
mod context;
mod entry;
mod formatting;
mod headless;
mod input;
mod internal;
mod lifecycle;
mod line_reader;
mod provider;
mod rc_wizard;
mod runtime;
mod service_console;
mod session_load;
mod session_transition;
mod stream_events;
mod terminal;
mod wfe_commands;
mod wfe_startup;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    entry::run().await
}
