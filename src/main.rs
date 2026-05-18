use std::env;
use crossterm::{
    event::{DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyEventKind, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures_util::StreamExt;
use ratatui::{
    backend::CrosstermBackend,
    Terminal,
};
use reqwest::Client;
use std::{error::Error, fs, io, time::Duration, path::{Path, PathBuf}};
use tokio;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use sysinfo::System;

use lethetic::config::Config;
use lethetic::client::{StreamEvent, trigger_llm_request};
use lethetic::app::{App, AppEventOutcome, BlockType, handle_key, handle_tool_call, ApprovalMode};

/// Dispatch `pending_tool_call` immediately (used when ApprovalMode::Always auto-approved it).
macro_rules! dispatch_auto_approved_tool {
    ($app:expr, $tx:expr, $cancellation_token:expr, $client:expr, $config:expr) => {
        if let Some(tool_call) = $app.pending_tool_call.as_ref() {
            let tc_id    = tool_call.id.clone();
            let func_name = tool_call.function.name.clone();
            let args      = tool_call.function.arguments.clone();
            let current_dir = $app.current_dir.clone();
            let ctx_tx    = $tx.clone();
            let tool_cancel = $cancellation_token.clone();
            let client_cl = $client.clone();
            let config_cl = $config.clone();
            $app.is_executing_tool = true;
            tokio::spawn(async move {
                let (result, new_dir) = lethetic::tools::execute(
                    func_name.as_str(), &args, &current_dir, tool_cancel, ctx_tx.clone(), &client_cl, &config_cl).await;
                let (full_result, _) = handle_large_output(&tc_id, result, &client_cl, &config_cl, &args).await;
                let _ = ctx_tx.send(StreamEvent::ToolResult(Some(tc_id), func_name, full_result, new_dir.clone()));
                let _ = ctx_tx.send(StreamEvent::DebugLog(format!("DIR_UPDATE|{}", new_dir)));
            });
            $app.is_processing = true;
        }
    };
}
use lethetic::ui::ui;
use lethetic::tools::get_git_info;
use lethetic::icons;
use lethetic::parser;

/// Heuristic analysis of why the engine stopped.
/// Covers ~100 distinct outcomes by examining token counts, content, and state flags.
fn classify_done_reason(
    completion_tokens: Option<u32>,
    prompt_tokens: Option<u32>,
    content: &str,
    tool_processed: bool,
    max_tokens: usize,
) -> String {
    let comp   = completion_tokens.unwrap_or(0) as usize;
    let prompt = prompt_tokens.unwrap_or(0) as usize;

    // Text after the thinking block (what the user actually saw)
    let text = if let Some(pos) = content.rfind("</think>") {
        content[pos + "</think>".len()..].trim()
    } else {
        content.trim()
    };

    // ── Errors / degenerate cases ─────────────────────────────────────────────
    if comp == 0 {
        return "⚠ Empty response — model produced no tokens".to_string();
    }
    if comp <= 5 && !tool_processed {
        if prompt > 0 && prompt as f64 / max_tokens as f64 > 0.88 {
            return format!("⚠ Context saturated ({:.0}% full) — model emitted immediate EOS", prompt as f64 / max_tokens as f64 * 100.0);
        }
        if text.is_empty() {
            return format!("⚠ Near-empty response ({} tokens) — possible prompt/template mismatch", comp);
        }
    }

    // ── Tool dispatched ───────────────────────────────────────────────────────
    if tool_processed {
        return "Tool dispatched → awaiting result".to_string();
    }

    // ── Context pressure ─────────────────────────────────────────────────────
    let ctx_pct = if prompt > 0 { prompt as f64 / max_tokens as f64 * 100.0 } else { 0.0 };
    if ctx_pct > 90.0 {
        return format!("⚠ Context {:.0}% full — consider /new to reset", ctx_pct);
    }

    // ── Response length heuristics ────────────────────────────────────────────
    let word_count = text.split_whitespace().count();
    if comp < 20 && word_count < 5 {
        return format!("⚠ Minimal response ({} tokens, {} words) — model may be confused", comp, word_count);
    }

    // ── Normal completion ─────────────────────────────────────────────────────
    if ctx_pct > 70.0 {
        format!("Response complete ({} tokens, context {:.0}% full)", comp, ctx_pct)
    } else {
        format!("Response complete ({} tokens)", comp)
    }
}

/// Returns true when the model wrote its intention in text without issuing a tool call.
/// Looks at the text portion only (after any </think> block) to avoid false positives
/// from reasoning content.
fn looks_like_intention_without_action(content: &str) -> bool {
    // Extract text after the thought block
    let text = if let Some(pos) = content.rfind("</think>") {
        &content[pos + "</think>".len()..]
    } else {
        content
    };
    let text = text.trim();

    // Only flag short responses — a long response is likely a legitimate answer
    if text.is_empty() || text.len() > 400 {
        return false;
    }

    let lower = text.to_lowercase();
    let intent_phrases = [
        "let's read", "let me read", "i will read", "i'll read",
        "let's write", "let me write", "i will write", "i'll write",
        "let's run", "let me run", "i will run", "i'll run",
        "let's search", "let me search", "i will search",
        "let's call", "let me call", "i will call", "i'll call",
        "now i'll", "now let's", "i need to call", "i should call",
        "i'm going to call", "i will use", "i'll use",
    ];
    intent_phrases.iter().any(|p| lower.contains(p))
}

// Delegated to lethetic::tools::handle_large_output; keep thin async wrapper for call-site compat.
async fn handle_large_output(
    id: &str,
    result: String,
    _client: &reqwest::Client,
    _config: &Config,
    _args: &serde_json::Value,
) -> (String, String) {
    lethetic::tools::handle_large_output(id, result)
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = env::args().collect();
    
    let config_path = if Path::new("config.yml").exists() {
        PathBuf::from("config.yml")
    } else {
        let home = env::var("HOME").expect("HOME env var not set");
        PathBuf::from(home).join(".config/lethetic/config.yml")
    };

    let config_content = fs::read_to_string(config_path)?;
    let mut config: Config = serde_yaml::from_str(&config_content)?;

    if args.len() > 2 && args[1] == "--command" {
        let prompt = args[2..].join(" ");
        return tokio::time::timeout(Duration::from_secs(30), run_headless(&config, prompt))
            .await
            .unwrap_or_else(|_| {
                println!("\n{} [TIMEOUT] Operation took longer than 30s. Stopping.", icons::WARNING);
                Ok(())
            });
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(&config);
    let res = run_app(&mut terminal, &mut app, &mut config).await;

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableBracketedPaste
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        println!("{:?}", err);
    }

    Ok(())
}

async fn run_headless(config: &Config, prompt: String) -> Result<(), Box<dyn Error>> {
    println!("\n{} User: {}\n", icons::INPUT, prompt);
    let client = Client::new();
    match lethetic::headless::run_agent(prompt, &client, config, true, None).await {
        Ok(_) => { println!("\n[DONE]"); }
        Err(e) => { println!("\n{} ERROR: {}", icons::WARNING, e); }
    }
    Ok(())
}

async fn run_app(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App, config: &mut Config) -> Result<(), Box<dyn Error>> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let client = Client::new();
    let mut cancellation_token = CancellationToken::new();
    let mut reader = EventStream::new();
    
    let mut last_tick = std::time::Instant::now();
    let mut last_save = std::time::Instant::now();
    let mut full_response_content = String::new();

    let stats_tx = tx.clone();
    tokio::spawn(async move {
        let mut sys = System::new_all();
        let pid = sysinfo::get_current_pid().ok();
        loop {
            sys.refresh_all();
            let proc_mem = if let Some(p) = pid {
                if let Some(process) = sys.process(p) {
                    process.memory() / 1024 / 1024
                } else { 0 }
            } else { 0 };
            let git = get_git_info().await;
            let _ = stats_tx.send(StreamEvent::DebugLog(format!("STATS|{}|{}", proc_mem, git)));
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });

    app.refresh_system_stats();

    loop {
        if app.should_redraw {
            terminal.draw(|f| ui(f, app))?;
            app.should_redraw = false;
        }

        // Periodic save check (every 2 seconds if needed)
        if app.needs_save && last_save.elapsed() >= Duration::from_secs(2) {
            app.save_session();
            last_save = std::time::Instant::now();
        }

        let timeout = Duration::from_millis(16);
        
        tokio::select! {
            Some(event_res) = reader.next() => {
                if let Ok(event) = event_res {
                    match event {
                        Event::Key(key) => {
                            if key.kind == KeyEventKind::Press {
                                // Global Ctrl+C handler
                                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                                    if app.is_processing {
                                        cancellation_token.cancel();
                                        app.is_processing = false;
                                        app.stop_reason = "Cancelled by user".to_string();
                                        app.add_segment(format!("\n{} [STOPPED]\n", icons::WARNING), BlockType::Text);
                                        while let Ok(_) = rx.try_recv() {}
                                        app.should_redraw = true;
                                        continue;
                                    } else {
                                        app.save_session();
                                        return Ok(());
                                    }
                                }

                                match handle_key(app, key) {
                                    AppEventOutcome::Exit => { app.save_session(); return Ok(()); },
                                    AppEventOutcome::NewSession => {
                                        app.start_new_session();
                                        app.should_redraw = true;
                                    }
                                    AppEventOutcome::ResumeSession(filename) => {
                                        app.is_loading_session = true;
                                        app.load_progress = 0.0;
                                        app.load_status = "Starting to load session...".to_string();
                                        app.should_redraw = true;

                                        let tx_clone = tx.clone();
                                        let filename_clone = filename.clone();
                                        let theme_clone = app.theme.clone();
                                        let terminal_width = app.last_rendered_width;

                                        tokio::spawn(async move {
                                            let _ = tx_clone.send(StreamEvent::LoadProgress(10.0, "Reading UI state...".to_string()));
                                            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                                            
                                            let ui_content = tokio::fs::read_to_string(format!("{}/ui_state.json", filename_clone)).await.unwrap_or_default();
                                            let _ = tx_clone.send(StreamEvent::LoadProgress(40.0, "Parsing & rendering UI state...".to_string()));
                                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                                            
                                            let blocks: Vec<lethetic::app::RenderBlock> = tokio::task::spawn_blocking(move || {
                                                let mut parsed_blocks = serde_json::from_str::<Vec<lethetic::app::RenderBlock>>(&ui_content).unwrap_or_default();
                                                if terminal_width > 0 {
                                                    for block in &mut parsed_blocks {
                                                        let rendered = lethetic::ui::render_block_to_lines(block, terminal_width, &theme_clone, None);
                                                        block.cached_lines = Some(rendered);
                                                    }
                                                }
                                                parsed_blocks
                                            }).await.unwrap_or_default();

                                            let _ = tx_clone.send(StreamEvent::LoadProgress(60.0, "Reading context...".to_string()));
                                            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                                            
                                            let ctx_content = tokio::fs::read_to_string(format!("{}/context.json", filename_clone)).await.unwrap_or_default();
                                            let _ = tx_clone.send(StreamEvent::LoadProgress(80.0, "Parsing context...".to_string()));
                                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                                            
                                            let messages: Vec<lethetic::context::Message> = tokio::task::spawn_blocking(move || {
                                                serde_json::from_str::<Vec<lethetic::context::Message>>(&ctx_content).unwrap_or_default()
                                            }).await.unwrap_or_default();

                                            let _ = tx_clone.send(StreamEvent::LoadProgress(100.0, "Finishing...".to_string()));
                                            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                                            let _ = tx_clone.send(StreamEvent::SessionLoaded(filename_clone, blocks, messages));
                                        });
                                    }
                                    AppEventOutcome::ToggleHistory => {
                                        app.show_history = !app.show_history;
                                        if app.show_history {
                                            app.history_state.select(Some(0));
                                        }
                                        app.should_redraw = true;
                                    }
                                    AppEventOutcome::DeleteSession(filename) => {
                                        let _ = std::fs::remove_dir_all(filename);
                                        app.refresh_session_list();
                                        app.should_redraw = true;
                                    }
                                    AppEventOutcome::FetchModels => {
                                        app.show_palette = false;
                                        app.show_model_switcher = true;
                                        app.available_models.clear();
                                        app.model_switcher_state.select(Some(0));
                                        app.should_redraw = true;
                                        // Collect servers: config model_servers + current server_url
                                        let mut servers: Vec<(String, String, String)> = config.model_servers
                                            .iter()
                                            .map(|s| (s.name.clone(), s.url.clone(), s.model.clone()))
                                            .collect();
                                        if servers.is_empty() {
                                            servers.push((config.model.clone(), config.server_url.clone(), config.model.clone()));
                                        }
                                        // Query each server for live model list
                                        let client_clone = client.clone();
                                        let tx_clone = tx.clone();
                                        tokio::spawn(async move {
                                            let mut models: Vec<(String, String, String)> = Vec::new();
                                            for (name, url, _model) in &servers {
                                                let live = lethetic::client::get_available_models(&client_clone, url).await;
                                                if live.is_empty() {
                                                    // Server not reachable — still show from config
                                                    models.push((format!("{} (offline)", name), url.clone(), _model.clone()));
                                                } else {
                                                    for (id, _) in live {
                                                        models.push((format!("{} — {}", name, id), url.clone(), id));
                                                    }
                                                }
                                            }
                                            let _ = tx_clone.send(StreamEvent::ModelsReady(models));
                                        });
                                    }
                                    AppEventOutcome::SwitchModel(new_url, new_model, parser) => {
                                        config.server_url = new_url.clone();
                                        config.model = new_model.clone();
                                        app.server_url = new_url.clone();
                                        app.model_name = new_model.clone();
                                        let mode = lethetic::parser::ParserMode::from_str(&parser);
                                        app.parser.set_mode(mode);
                                        app.parser.reset();
                                        app.add_segment(
                                            format!("\n{} Switched to model: {} ({}) — parser: {}\n", icons::SUCCESS, new_model, new_url, parser),
                                            BlockType::Text,
                                        );
                                        app.stop_reason = format!("Model: {}", new_model);
                                        app.should_redraw = true;
                                    }
                                    AppEventOutcome::SendPrompt(prompt) => {
                                        app.add_to_history(prompt.clone());
                                        if app.is_asking_user {
                                            app.is_asking_user = false;
                                            app.add_segment(prompt.clone(), BlockType::User);
                                            app.context_manager.add_message("user", &prompt);
                                            
                                            if let Some(tool_call) = app.pending_tool_call.take() {
                                                let tc_id = tool_call.id.clone();
                                                let func_name = tool_call.function.name.clone();
                                                let _ = tx.send(StreamEvent::ToolResult(Some(tc_id), func_name, prompt, app.current_dir.clone()));
                                            }
                                        } else {
                                            app.add_segment(prompt.clone(), BlockType::User);
                                            app.context_manager.set_cwd(app.current_dir.clone());
                                            app.context_manager.add_message("user", &prompt);
                                            app.stop_reason = "Processing…".to_string();
                                            app.tool_call_fingerprints.clear();
                                            app.applied_edits.clear();
                                            app.is_processing = true;
                                            app.tool_calls_processed_this_request = false;
                                            app.tool_call_pos = None;
                                            full_response_content.clear();
                                            cancellation_token = CancellationToken::new();
                                            app.request_start_time = Some(tokio::time::Instant::now());
                                            app.parser.reset();
                            trigger_llm_request(client.clone(), config.clone(), &app.context_manager, tx.clone(), cancellation_token.clone(), app.show_debug, app.current_session_dir.clone());
                                        }
                                    }
                                    AppEventOutcome::ToolApproved(approved, always) => {
                                        if let Some(tool_call) = app.pending_tool_call.as_ref() {
                                            if approved {
                                                if always { app.shell_approval_mode = ApprovalMode::Always; }
                                                let tc_id = tool_call.id.clone();
                                                let func_name = tool_call.function.name.clone();
                                                let args = tool_call.function.arguments.clone();
                                                let current_dir = app.current_dir.clone();
                                                
                                                let ctx_tx = tx.clone();
                                                let tool_cancel = cancellation_token.clone();
                                                let client_clone = client.clone();
                                                let config_clone = config.clone();
                                                app.is_executing_tool = true;
                                                tokio::spawn(async move {
                                                    let (result, new_dir) = lethetic::tools::execute(
                                                        func_name.as_str(), &args, &current_dir, tool_cancel, ctx_tx.clone(), &client_clone, &config_clone).await;
                                                    
                                                    let (full_result, _) = handle_large_output(&tc_id, result, &client_clone, &config_clone, &args).await;
                                                    
                                                    let _ = ctx_tx.send(StreamEvent::ToolResult(Some(tc_id), func_name, full_result, new_dir.clone()));
                                                    let _ = ctx_tx.send(StreamEvent::DebugLog(format!("DIR_UPDATE|{}", new_dir)));
                                                });
                                                app.is_processing = true;
                                            } else {
                                                app.pending_tool_call.take();
                                                app.add_segment(format!("\n{} Tool execution denied by user.\n", icons::WARNING), BlockType::Text);
                                            }
                                        }
                                        app.show_approval_prompt = false;
                                        app.should_redraw = true;
                                    }
                                    AppEventOutcome::Stop => {
                                        cancellation_token.cancel();
                                        app.is_processing = false;
                                        app.is_executing_tool = false;
                                        app.tool_output_preview.clear();
                                        app.add_segment(format!("\n{} [STOPPED]\n", icons::WARNING), BlockType::Text);
                                        while let Ok(_) = rx.try_recv() {}
                                        app.should_redraw = true;
                                    }
                                    AppEventOutcome::Continue => { app.should_redraw = true; }
                                }

                                // LSP install requested from the server panel
                                if let Some(cmd) = app.lsp_install_cmd.take() {
                                    app.add_segment(format!("\n⟳ Installing LSP server…\n$ {}\n", cmd), BlockType::Text);
                                    app.stop_reason = "⟳ Installing LSP server…".to_string();
                                    app.is_executing_tool = true;
                                    let ctx_tx = tx.clone();
                                    let tool_cancel = cancellation_token.clone();
                                    let cwd = app.current_dir.clone();
                                    tokio::spawn(async move {
                                        let (result, new_dir) = lethetic::tools::execute(
                                            "run_shell_command",
                                            &serde_json::json!({"command": cmd, "description": "Install LSP server", "tool_call_id": "lsp_install"}),
                                            &cwd, tool_cancel, ctx_tx.clone(), &reqwest::Client::new(), &lethetic::config::Config::default(),
                                        ).await;
                                        let _ = ctx_tx.send(StreamEvent::ToolResult(None, "lsp_install".to_string(), result, new_dir));
                                    });
                                }
                            }
                        }
                        Event::Paste(text) => {
                            if !app.is_processing {
                                app.input.insert_str(app.cursor_pos, &text);
                                app.cursor_pos += text.len();
                                app.should_redraw = true;
                            }
                        }
                        _ => {}
                    }
                }
            }

            // DRAIN THE CHANNEL: Process all waiting events before drawing
            Some(mut stream_event) = rx.recv() => {
                loop {
                    match stream_event {
                        StreamEvent::ModelsReady(models) => {
                            app.available_models = models;
                            if !app.available_models.is_empty() {
                                app.model_switcher_state.select(Some(0));
                            }
                            app.should_redraw = true;
                        }
                        StreamEvent::DebugLog(msg) => {
                            if msg.starts_with("STATS|") {
                                let parts: Vec<&str> = msg.split('|').collect();
                                if parts.len() == 3 {
                                    app.memory_usage = parts[1].parse().unwrap_or(0);
                                    app.git_status = parts[2].to_string();
                                    app.should_redraw = true;
                                }
                            } else if msg.starts_with("DIR_UPDATE|") {
                                app.current_dir = msg[11..].to_string();
                                app.should_redraw = true;
                            } else {
                                app.log_debug(&msg);
                            }
                        }
                        StreamEvent::TokenUpdate(count, ms) => {
                            if ms > 0.0 {
                                app.tokens_per_s = (count as f64 / (ms / 1000.0)).max(0.0);
                                app.should_redraw = true;
                            } else if let Some(start) = app.request_start_time {
                                let elapsed = start.elapsed().as_secs_f64();
                                if elapsed > 0.0 {
                                    app.tokens_per_s = (count as f64 / elapsed).max(0.0);
                                    app.should_redraw = true;
                                }
                            }
                        }
                        StreamEvent::Chunk(chunk) => {
                            if app.is_processing {
                                full_response_content.push_str(&chunk);
                                app.should_redraw = true;
                                
                                let segments = app.parser.parse_chunk(&chunk);
                                for (b_type, content) in segments {
                                    app.add_segment(content, b_type);
                                    
                                    // Check for loops after adding content
                                    if let Some(detection) = app.loop_detector.check(&app.last_block_content) {
                                        app.log_debug(&format!("LOOP DETECTED: {}", detection.reason));
                                        cancellation_token.cancel();
                                        app.is_processing = false;
                                        
                                        let now = std::time::Instant::now();
                                        let is_rapid_loop = if let Some(last_time) = app.last_loop_detection_time {
                                            now.duration_since(last_time) < std::time::Duration::from_secs(120)
                                        } else {
                                            false
                                        };

                                        if is_rapid_loop && app.loop_detection_count >= 1 {
                                            let mut stop_msg = format!("\n{} [WATCHDOG TERMINATED] Persistent loop detected after multiple auto-correction attempts. Handing control to user.\n", icons::WARNING);
                                            if let Some(sample) = detection.sample {
                                                stop_msg.push_str(&format!("{} Last looping sequence: \"{}\"\n", icons::DEBUG, sample));
                                            }
                                            app.add_segment(stop_msg, BlockType::Text);
                                            app.context_manager.add_message("assistant", &full_response_content);
                                            app.context_manager.add_message("user", "The watchdog terminated your generation because you were unable to break out of a loop. Please proceed with a tool call immediately.");
                                            app.stop_reason = format!("⚠ Persistent loop terminated after {} detections — waiting for input", app.loop_detection_count + 1);
                                            app.loop_detection_count = 0;
                                            app.last_loop_detection_time = None;
                                        } else {
                                            let mut loop_msg = format!("\n{} [LOOP DETECTED] {}\n", icons::WARNING, detection.reason);
                                            if let Some(sample) = detection.sample {
                                                loop_msg.push_str(&format!("{} Sample: \"{}\"\n", icons::DEBUG, sample));
                                            }
                                            app.add_segment(loop_msg, BlockType::Text);
                                            app.context_manager.add_message("assistant", &full_response_content);
                                            app.context_manager.add_message("user", "Note: You were stuck in a reasoning loop. Please choose a single clear path and proceed with a tool call immediately.");
                                            app.stop_reason = format!("→ Loop #{} detected — auto-correcting", app.loop_detection_count + 1);
                                            app.loop_detection_count += 1;
                                            app.last_loop_detection_time = Some(now);

                                            // Correct re-triggering logic:
                                            app.is_processing = true;
                                            full_response_content.clear();
                                            cancellation_token = CancellationToken::new(); // NEW TOKEN
                                            app.parser.reset();
                                            
                                            trigger_llm_request(client.clone(), config.clone(), &app.context_manager, tx.clone(), cancellation_token.clone(), app.show_debug, app.current_session_dir.clone());
                                        }
                                        break; 
                                    }
                                }

                                if app.parser.state == lethetic::parser::ParserState::Text && !app.tool_calls_processed_this_request {
                                    match parser::find_tool_call(&full_response_content, false) {
                                        Some(Ok((tc, pos))) => {
                                            if let AppEventOutcome::ToolApproved(..) = handle_tool_call(app, vec![tc], pos, tx.clone(), &mut cancellation_token, &full_response_content, false) {
                                                dispatch_auto_approved_tool!(app, tx, cancellation_token, client, config);
                                            }
                                        }
                                        Some(Err((err_msg, _pos))) => {
                                            // Only log error if we are sure it should have finished (Text state)
                                            app.log_debug(&format!("Tool call syntax error: {}", err_msg));
                                            cancellation_token.cancel();
                                            app.is_processing = false;
                                            app.stop_reason = format!("⚠ Tool call syntax error — re-prompting");
                                            app.add_segment(format!("\n{} [SYNTAX ERROR] {}\n", icons::WARNING, err_msg), BlockType::Text);
                                            app.context_manager.add_message("assistant", &full_response_content);
                                            let _ = tx.send(StreamEvent::ToolResult(Some("raw_call".to_string()), "syntax_error".to_string(), format!("Syntax Error in tool call: {}", err_msg), app.current_dir.clone()));
                                        }
                                        None => {}
                                    }
                                }
                            }
                        }
                        StreamEvent::ToolCalls(calls) => {
                            if !app.tool_calls_processed_this_request {
                                if let AppEventOutcome::ToolApproved(..) = handle_tool_call(app, calls, full_response_content.len(), tx.clone(), &mut cancellation_token, &full_response_content, true) {
                                    dispatch_auto_approved_tool!(app, tx, cancellation_token, client, config);
                                }
                            }
                        }
                        StreamEvent::ToolResult(id, func_name, result, new_dir) => {
                            app.is_executing_tool = false;
                            app.tool_output_preview.clear();
                            app.current_dir = new_dir;
                            let success = if result.contains("EXIT_CODE: ") { result.contains("EXIT_CODE: 0") } else { true };
                            
                            let tc_id_str = id.clone().unwrap_or_else(|| "unknown".to_string());
                            let tool_args = app.pending_tool_call.as_ref().map(|tc| tc.function.arguments.clone()).unwrap_or(serde_json::json!({}));
                            // read_file: never truncate — content always goes into the file cache via
                            // update_latest_file below, so the model accesses it through the context
                            // header rather than the tool result. Token budget eviction handles large files.
                            let (mut full_result, ui_result) = if func_name == "read_file" && result.len() < 500_000 {
                                (result.clone(), result)
                            } else {
                                handle_large_output(&tc_id_str, result, &client, &config, &tool_args).await
                            };

                            let description = app.pending_tool_call.as_ref()
                                .and_then(|tc| tc.function.arguments["description"].as_str())
                                .unwrap_or("Action").to_string();

                            app.add_segment_with_title(format!("\n{}\n", ui_result), BlockType::ToolResult, description);
                            if let Some(last) = app.blocks.last_mut() { last.success = Some(success); }

                            if let Some(tc_id) = id {
                                app.pending_tool_call.take();

                                if success && !full_result.contains("OUTPUT TRUNCATED") {
                                    if func_name == "read_file" {
                                        if let Some(path) = tool_args["path"].as_str() {
                                            let full_path = std::path::Path::new(&app.current_dir).join(path);
                                            if let Ok(content) = std::fs::read_to_string(&full_path) {
                                                app.context_manager.update_latest_file(path.to_string(), content);
                                                app.add_segment(format!("\n{} File `{}` has been placed in context.\n", icons::SUCCESS, path), BlockType::Text);
                                                full_result = "[File read successfully. Contents are now available in your Latest Files context.]".to_string();
                                            }
                                        }
                                    } else if func_name == "write_file" {
                                        if let Some(path) = tool_args["path"].as_str() {
                                            if let Some(content) = tool_args["content"].as_str() {
                                                app.context_manager.update_latest_file(path.to_string(), content.to_string());
                                                app.add_segment(format!("\n{} File `{}` has been placed in context.\n", icons::SUCCESS, path), BlockType::Text);
                                            }
                                        }
                                    } else if func_name == "apply_patch" && full_result.contains("Successfully patched") {
                                        if let Some(path) = tool_args["file_path"].as_str() {
                                            let full_path = std::path::Path::new(&app.current_dir).join(path);
                                            if let Ok(content) = std::fs::read_to_string(&full_path) {
                                                app.context_manager.update_latest_file(path.to_string(), content);
                                                app.add_segment(format!("\n{} File `{}` has been updated in context.\n", icons::SUCCESS, path), BlockType::Text);
                                            }
                                        }
                                    } else if func_name == "replace_text" && full_result.contains("Successfully replaced") {
                                        if let Some(path) = tool_args["path"].as_str() {
                                            let full_path = std::path::Path::new(&app.current_dir).join(path);
                                            if let Ok(content) = std::fs::read_to_string(&full_path) {
                                                app.context_manager.update_latest_file(path.to_string(), content);
                                                app.add_segment(format!("\n{} File `{}` has been updated in context.\n", icons::SUCCESS, path), BlockType::Text);
                                            }
                                        }
                                    }
                                }

                                // Track successfully applied edits for "already applied" detection
                                if (func_name == "edit" || func_name == "replace_text")
                                    && full_result.contains("Successfully")
                                {
                                    let old_str = tool_args["old_string"].as_str().unwrap_or("").to_string();
                                    if !old_str.is_empty() {
                                        app.applied_edits.insert(old_str);
                                    }
                                }

                                // Detect "already applied" — edit fails with "not found" but we already applied it
                                if (func_name == "edit" || func_name == "replace_text")
                                    && full_result.contains("not found")
                                {
                                    let old_str = tool_args["old_string"].as_str().unwrap_or("");
                                    if !old_str.is_empty() && app.applied_edits.contains(old_str) {
                                        let msg = format!(
                                            "⚠ EDIT ALREADY APPLIED: This exact `old_string` was successfully replaced in a prior call. \
                                             The file already contains your updated version. \
                                             Do not retry this edit — move on to the next issue."
                                        );
                                        app.context_manager.add_message("user", &msg);
                                        app.add_segment(format!("\n⚠ [EDIT ALREADY APPLIED] old_string was replaced earlier this session — move on.\n"), BlockType::Text);
                                        full_result = msg;
                                    }
                                }

                                app.context_manager.add_tool_message(tc_id, &func_name, &full_result);
                            }

                            // Duplicate tool call detection: same (tool, key-args) called 2+ times for edit/replace_text, 3+ for others
                            {
                                let path = tool_args["path"].as_str()
                                    .or_else(|| tool_args["file_path"].as_str())
                                    .unwrap_or("");
                                let fingerprint = match func_name.as_str() {
                                    "read_file" => format!("read_file:{}", path),
                                    "read_file_lines" => format!("read_file_lines:{}:{}-{}",
                                        path,
                                        tool_args["start_line"].as_u64().unwrap_or(0),
                                        tool_args["end_line"].as_u64().unwrap_or(0)),
                                    "search_text" => format!("search_text:{}:{}",
                                        tool_args["pattern"].as_str().unwrap_or(""),
                                        path),
                                    "run_shell_command" => format!("run_shell_command:{}",
                                        tool_args["command"].as_str().unwrap_or("")),
                                    other => format!("{}:{}",
                                        other,
                                        serde_json::to_string(&tool_args).unwrap_or_default()),
                                };
                                let count = {
                                    let c = app.tool_call_fingerprints.entry(fingerprint).or_insert(0);
                                    *c += 1;
                                    *c
                                };
                                let dup_threshold = match func_name.as_str() {
                                    "edit" | "replace_text" => 2,
                                    "run_shell_command" => {
                                        let cmd = tool_args["command"].as_str().unwrap_or("");
                                        if cmd.contains("rm ") || cmd.contains("unlink ")
                                            || cmd.contains(" mv ") || cmd.contains("del ")
                                        { 2 } else { 3 }
                                    }
                                    _ => 3,
                                };
                                if count >= dup_threshold {
                                    let path_hint = tool_args["path"].as_str()
                                        .or_else(|| tool_args["file_path"].as_str())
                                        .unwrap_or("this file");
                                    let hint = match func_name.as_str() {
                                        "read_file" | "read_file_lines" => format!(
                                            "You have called `{}` on `{}` {} times and received the same result. \
                                             The file may be too large for this approach. Try: \
                                             `search_text` with a specific pattern to locate the code you need, \
                                             `read_file_lines` with a narrower range (50–100 lines at a time), \
                                             or `summarize_content` with the file path for an overview.",
                                            func_name, path_hint, count
                                        ),
                                        "search_text" => format!(
                                            "You have run this search {} times and received the same result. \
                                             Try a more specific pattern or use `find_symbol` for definition/reference lookup.",
                                            count
                                        ),
                                        other => format!(
                                            "You have called `{}` with identical parameters {} times. \
                                             Try a different approach or a different tool.",
                                            other, count
                                        ),
                                    };
                                    let warn = format!("⚠ DUPLICATE TOOL CALL: {}", hint);
                                    app.context_manager.add_message("user", &warn);
                                    app.add_segment(
                                        format!("\n⚠ [DUPLICATE TOOL CALL x{}] {}\n", count, hint),
                                        BlockType::Text,
                                    );
                                }
                            }

                            app.is_processing = true;
                            app.tool_calls_processed_this_request = false;
                            app.tool_call_pos = None;
                            full_response_content.clear();
                            cancellation_token = CancellationToken::new();
                            app.request_start_time = Some(tokio::time::Instant::now());
                            app.context_manager.set_cwd(app.current_dir.clone());
                            app.parser.reset();
                            trigger_llm_request(client.clone(), config.clone(), &app.context_manager, tx.clone(), cancellation_token.clone(), app.show_debug, app.current_session_dir.clone());
                        }
                        StreamEvent::ToolProgress(msg) => {
                            app.tool_output_preview = msg;
                            app.should_redraw = true;
                        }
                        StreamEvent::Done { completion_tokens, prompt_tokens, tg_per_s, pp_per_s } => {
                            app.is_processing = false;
                            if (app.parser.state == lethetic::parser::ParserState::Text || app.parser.state == lethetic::parser::ParserState::ToolCall) && !app.tool_calls_processed_this_request {
                                match parser::find_tool_call(&full_response_content, true) {
                                    Some(Ok((tc, pos))) => {
                                        handle_tool_call(app, vec![tc], pos, tx.clone(), &mut cancellation_token, &full_response_content, false);
                                    }
                                    _ => {}
                                }
                            }

                            if !app.tool_calls_processed_this_request {
                                let messages = app.context_manager.get_messages();
                                if messages.last().map_or(true, |m| m.role != "assistant") {
                                    app.context_manager.add_message("assistant", &full_response_content);
                                }

                                // Detect "intention text": model described an action in plain text
                                // but never issued a tool call. Re-prompt once so it acts.
                                if looks_like_intention_without_action(&full_response_content) {
                                    app.log_debug("INTENT_TEXT_DETECTED: model described action without tool call — re-prompting");
                                    app.stop_reason = "→ Described action without tool call — re-prompting".to_string();
                                    app.context_manager.add_message("user", "You described an action but did not call a tool. Please call the appropriate tool now.");
                                    full_response_content.clear();
                                    cancellation_token = CancellationToken::new();
                                    app.parser.reset();
                                    app.is_processing = true;
                                    app.tool_calls_processed_this_request = false;
                                    trigger_llm_request(client.clone(), config.clone(), &app.context_manager, tx.clone(), cancellation_token.clone(), app.show_debug, app.current_session_dir.clone());
                                    continue;
                                }

                                // Set heuristic stop reason for normal / degenerate completion
                                app.stop_reason = classify_done_reason(
                                    completion_tokens,
                                    prompt_tokens,
                                    &full_response_content,
                                    false,
                                    app.max_tokens,
                                );
                            } else {
                                // Tool was dispatched — set reason based on tool name
                                let tool_name = app.pending_tool_call.as_ref()
                                    .map(|tc| tc.function.name.as_str())
                                    .unwrap_or("unknown");
                                app.stop_reason = classify_done_reason(
                                    completion_tokens,
                                    prompt_tokens,
                                    &full_response_content,
                                    true,
                                    app.max_tokens,
                                );
                                // Enrich with the tool name
                                if app.stop_reason.starts_with("Tool dispatched") {
                                    app.stop_reason = format!("→ Tool dispatched: {} — awaiting result", tool_name);
                                }
                            }

                            // Use server-reported speeds if available, else fall back to wall-clock
                            if let Some(tg) = tg_per_s {
                                app.tokens_per_s = tg;
                            } else if let Some(start) = app.request_start_time {
                                let elapsed = start.elapsed().as_secs_f64();
                                if elapsed > 0.0 {
                                    let count = completion_tokens.unwrap_or(full_response_content.split_whitespace().count() as u32);
                                    app.tokens_per_s = (count as f64 / elapsed).max(0.0);
                                }
                            }
                            if let Some(pp) = pp_per_s {
                                app.pp_tokens_per_s = pp;
                            }
                            if let Some(pt) = prompt_tokens {
                                app.server_prompt_tokens = Some(pt);
                            }
                            app.request_start_time = None;
                            app.should_redraw = true;
                            app.save_session(); // Final save on completion

                            if app.tool_calls_processed_this_request && app.shell_approval_mode == ApprovalMode::Always {
                                if let Some(tool_call) = app.pending_tool_call.as_ref() {
                                    let tc_id = tool_call.id.clone();
                                    let func_name = tool_call.function.name.clone();
                                    let args = tool_call.function.arguments.clone();
                                    let current_dir = app.current_dir.clone();
                                    
                                    let ctx_tx = tx.clone();
                                    let tool_cancel = cancellation_token.clone();
                                    let client_clone = client.clone();
                                    let config_clone = config.clone();
                                    app.is_executing_tool = true;
                                    tokio::spawn(async move {
                                        let (result, new_dir) = lethetic::tools::execute(
                                            func_name.as_str(), &args, &current_dir, tool_cancel, ctx_tx.clone(), &client_clone, &config_clone).await;
                                        let (full_result, _) = handle_large_output(&tc_id, result, &client_clone, &config_clone, &args).await;
                                        let _ = ctx_tx.send(StreamEvent::ToolResult(Some(tc_id), func_name, full_result, new_dir.clone()));
                                        let _ = ctx_tx.send(StreamEvent::DebugLog(format!("DIR_UPDATE|{}", new_dir)));
                                    });
                                    app.is_processing = true;
                                }
                            }
                        }
                        StreamEvent::Error(e) => {
                            app.is_processing = false;
                            let short = if e.len() > 80 { format!("{}…", &e[..77]) } else { e.clone() };
                            app.stop_reason = format!("✗ Server error: {}", short);
                            app.add_segment(format!("\n{} ERROR: {}\n", icons::WARNING, e), BlockType::Text);
                            app.should_redraw = true;
                        }
                        StreamEvent::LoadProgress(pct, status) => {
                            app.load_progress = pct;
                            app.load_status = status;
                            app.should_redraw = true;
                        }
                        StreamEvent::SessionLoaded(dir, blocks, messages) => {
                            app.current_session_dir = Some(dir);
                            app.blocks = blocks;
                            app.context_manager.clear();
                            app.context_manager.set_messages(messages);
                            app.scroll = 0;
                            app.output_state.select(Some(app.blocks.len().saturating_sub(1)));
                            app.is_loading_session = false;
                            app.should_redraw = true;
                        }
                    }


                    // Attempt to process next available event without yielding
                    if let Ok(next_event) = rx.try_recv() {
                        stream_event = next_event;
                    } else {
                        break;
                    }
                }
            }
            _ = tokio::time::sleep(timeout) => {
                if app.is_processing && last_tick.elapsed() >= Duration::from_millis(100) {
                    app.tick_spinner();
                    last_tick = std::time::Instant::now();
                }
            }
        }
    }
}
