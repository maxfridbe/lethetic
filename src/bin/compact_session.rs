/// One-shot test: compact a session directory using the configured model.
/// Usage: cargo run --bin compact_session -- <session_dir>
#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let src_dir = args.get(1).cloned().unwrap_or_else(|| {
        eprintln!("Usage: compact_session <session_dir>");
        std::process::exit(1);
    });

    let mut config = lethetic::config::Config::load("config.yml").expect("no config.yml");
    config.merge_matching_server_settings();

    let ui_log = std::fs::read_to_string(format!("{}/ui_log.txt", src_dir))
        .expect("ui_log.txt not found");

    let spm = lethetic::system_prompt::SystemPromptManager::new();
    let prompt = spm.load_prompt("compaction")
        .unwrap_or_else(|| lethetic::system_prompt::DEFAULT_COMPACTION_PROMPT.to_string());

    let stripped = lethetic::client::strip_file_content_from_log(&ui_log);
    println!("Log: {} chars → {} chars after stripping file content ({:.0}% reduction)",
        ui_log.len(), stripped.len(),
        (1.0 - stripped.len() as f64 / ui_log.len() as f64) * 100.0);
    let ui_log = stripped; // shadow with stripped version
    let client = reqwest::Client::new();
    match lethetic::client::compact_llm(&client, &config, &ui_log, &prompt).await {
        Err(e) => eprintln!("Error: {}", e),
        Ok(summary) => {
            println!("\n=== COMPACTED SUMMARY ===\n{}\n=== END ===", summary);

            // Write a new session
            let sessions_root = std::path::Path::new(&src_dir).parent().unwrap();
            let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
            let src_name = std::path::Path::new(&src_dir).file_name().unwrap().to_string_lossy();
            let new_dir = sessions_root.join(format!("session_{}_compacted_{}", ts, src_name));
            std::fs::create_dir_all(&new_dir).unwrap();

            let context_msg = format!("Context from compacted session ({}):\n\n{}", src_name, summary);
            let new_state = lethetic::app::SessionState {
                messages: vec![
                    lethetic::context::Message { role: "user".to_string(), content: context_msg, tool_calls: None },
                    lethetic::context::Message { role: "assistant".to_string(), content: "Understood. I have the context from the previous session and am ready to continue.".to_string(), tool_calls: None },
                ],
                blocks: vec![lethetic::app::RenderBlock {
                    block_type: lethetic::app::BlockType::Text,
                    content: format!("**Compacted from `{}`**\n\n{}", src_name, summary),
                    title: None,
                    success: Some(true),
                    prompt_tokens: None,
                    completion_tokens: None,
                    cached_lines: None,
                    cached_line_count: None,
                }],
                history: vec![],
                ..Default::default()
            };
            let json = serde_json::to_string_pretty(&new_state).unwrap();
            std::fs::write(new_dir.join("session_state.json"), &json).unwrap();
            let _ = std::fs::copy(format!("{}/ui_log.txt", src_dir), new_dir.join("ui_log_original.txt"));
            println!("\nNew session written to: {}", new_dir.display());
        }
    }
}
