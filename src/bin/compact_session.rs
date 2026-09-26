/// One-shot CLI: compact a session directory's `ui_log.txt` with the configured
/// model and print the summary. Pass `--out FILE` to also write it to disk.
///
/// Usage: cargo run --bin compact_session -- <session_dir> [--out FILE]
#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let src_dir = args.first().cloned().unwrap_or_else(|| {
        eprintln!("Usage: compact_session <session_dir> [--out FILE]");
        std::process::exit(1);
    });
    let out = args
        .iter()
        .position(|a| a == "--out")
        .and_then(|i| args.get(i + 1).cloned());

    let mut config = lethetic::config::Config::load("config.yml").expect("no config.yml");
    config.merge_matching_server_settings();
    let log = lethetic::app::App::compaction_source_text(&src_dir).unwrap_or_else(|error| {
        eprintln!("Error: {error}");
        std::process::exit(1);
    });
    let client = reqwest::Client::new();
    match lethetic::client::compact_llm(&client, &config, &log).await {
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
        Ok(summary) => {
            println!("\n=== COMPACTED SUMMARY ===\n{}\n=== END ===", summary);
            if let Some(path) = out {
                std::fs::write(&path, &summary).expect("could not write summary");
                println!("\nSummary written to: {path}");
            }
        }
    }
}
