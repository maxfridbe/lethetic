
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
use std::{error::Error, io, time::Duration, path::{Path, PathBuf}};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use lethetic::config::Config;
use lethetic::client::{ModelChoice, StreamEvent, trigger_llm_request};
use lethetic::app::{App, AppEventOutcome, BlockType, handle_key, handle_tool_call, ApprovalMode};

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

/// Dispatch `pending_tool_call` immediately (used when ApprovalMode::Always auto-approved it).
fn dispatch_auto_approved_tool(
    app: &mut App,
    tx: &mpsc::UnboundedSender<StreamEvent>,
    cancellation_token: &CancellationToken,
    client: &Client,
    config: &Config,
) {
    let Some(tool_call) = app.pending_tool_call.as_ref() else { return };
    let tc_id = tool_call.id.clone();
    let func_name = tool_call.function.name.clone();
    let args = tool_call.function.arguments.clone();
    let current_dir = app.current_dir.clone();
    let ctx_tx = tx.clone();
    let tool_cancel = cancellation_token.clone();
    let client = client.clone();
    let config = config.clone();
    app.is_executing_tool = true;
    app.tool_call_dispatched = true;
    tokio::spawn(async move {
        let (result, new_dir) = lethetic::tools::execute(
            func_name.as_str(), &args, &current_dir, tool_cancel, ctx_tx.clone(), &client, &config).await;
        let (full_result, _) = lethetic::tools::handle_large_output(&tc_id, result);
        let _ = ctx_tx.send(StreamEvent::ToolResult { id: Some(tc_id), func_name, result: full_result, cwd: new_dir.clone() });
        let _ = ctx_tx.send(StreamEvent::DebugLog(format!("DIR_UPDATE|{}", new_dir)));
    });
    app.is_processing = true;
}
fn setup_panic_hook() {
    std::panic::set_hook(Box::new(|panic_info| {
        // 1. Reset terminal so it's not garbled
        #[cfg(not(test))]
        {
            let mut stdout = std::io::stdout();
            let _ = crossterm::terminal::disable_raw_mode();
            let _ = crossterm::execute!(
                stdout,
                crossterm::terminal::LeaveAlternateScreen,
                crossterm::event::DisableBracketedPaste,
                crossterm::cursor::Show
            );
        }

        // 2. Prepare payload
        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string();
        let payload = panic_info.payload();
        let message = if let Some(s) = payload.downcast_ref::<&str>() {
            *s
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.as_str()
        } else {
            "Unknown panic payload"
        };

        let location = if let Some(loc) = panic_info.location() {
            format!("{}:{}", loc.file(), loc.line())
        } else {
            "unknown location".to_string()
        };

        let backtrace = std::backtrace::Backtrace::capture();
        let backtrace_str = format!("{}", backtrace);

        // 3. Log to .lethetic/panic.log
        let _ = std::fs::create_dir_all(".lethetic");
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(".lethetic/panic.log")
        {
            use std::io::Write;
            let _ = writeln!(
                file,
                "========================================\n\
                 Timestamp: {}\n\
                 Panic at {}\n\
                 Message: {}\n\n\
                 Backtrace:\n{}\n\n",
                timestamp, location, message, backtrace_str
            );
        }

        // 4. Print beautiful crash dialog box to stderr
        #[cfg(not(test))]
        {
            let message_clean = message.replace('\n', " ").replace('\r', "");
            let msg_trimmed = if message_clean.len() > 43 { format!("{}...", &message_clean[..40]) } else { message_clean };
            let loc_trimmed = if location.len() > 42 { format!("{}...", &location[..39]) } else { location.clone() };

            eprintln!(
                "\n\
                ┌────────────────────────────────────────────────────────┐\n\
                │                   APPLICATION CRASH                    │\n\
                ├────────────────────────────────────────────────────────┤\n\
                │ Lethetic encountered an unrecoverable error (panic).   │\n\
                │                                                        │\n\
                │ Message: {:<46} │\n\
                │ Location: {:<45} │\n\
                │                                                        │\n\
                │ A detailed crash log with backtrace has been saved to: │\n\
                │ .lethetic/panic.log                                    │\n\
                └────────────────────────────────────────────────────────┘\n",
                msg_trimmed, loc_trimmed
            );
        }
    }));
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    setup_panic_hook();
    let args: Vec<String> = env::args().collect();
    
    let config_path = if Path::new("config.yml").exists() {
        PathBuf::from("config.yml")
    } else {
        let home = env::var("HOME").expect("HOME env var not set");
        PathBuf::from(home).join(".config/lethetic/config.yml")
    };

    let mut config = Config::load(&config_path)?;
    config.merge_matching_server_settings();

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

/// Current process resident set size in MB, read from /proc/self/status (VmRSS is in kB).
fn process_rss_mb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            let line = s.lines().find(|l| l.starts_with("VmRSS:"))?;
            line.split_whitespace().nth(1)?.parse::<u64>().ok()
        })
        .map(|kb| kb / 1024)
        .unwrap_or(0)
}

async fn run_app(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App, config: &mut Config) -> Result<(), Box<dyn Error>> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let client = Client::new();
    let mut cancellation_token = CancellationToken::new();
    let mut reader = EventStream::new();

    let mut last_tick = std::time::Instant::now();
    let mut last_save = std::time::Instant::now();
    let mut full_response_content = String::new();

    // Load syntect syntax/theme dumps off the render thread so the first
    // code-fence render doesn't hitch for 100-300ms.
    std::thread::spawn(lethetic::markdown::warm_highlighter);

    let stats_tx = tx.clone();
    tokio::spawn(async move {
        loop {
            let proc_mem = process_rss_mb();
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
                                        while rx.try_recv().is_ok() {}
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
                                            let _ = tx_clone.send(StreamEvent::LoadProgress(10.0, "Reading session state...".to_string()));

                                            let load_dir = filename_clone.clone();
                                            let state = tokio::task::spawn_blocking(move || {
                                                let mut state = lethetic::app::SessionState::load(&load_dir);
                                                if terminal_width > 0 {
                                                    for block in &mut state.blocks {
                                                        block.cached_lines = Some(lethetic::ui::render_block_to_lines(block, terminal_width, &theme_clone, None));
                                                    }
                                                }
                                                state
                                            }).await.unwrap_or_default();

                                            if state.blocks.is_empty() && state.messages.is_empty() {
                                                let _ = tx_clone.send(StreamEvent::Error(format!("Session {} is empty or unreadable", filename_clone)));
                                            }

                                            let _ = tx_clone.send(StreamEvent::LoadProgress(100.0, "Finishing...".to_string()));
                                            let _ = tx_clone.send(StreamEvent::SessionLoaded { dir: filename_clone, state });
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
                                        let config_clone = config.clone();
                                        tokio::spawn(async move {
                                            let mut models: Vec<ModelChoice> = Vec::new();
                                            for (name, url, default_model) in &servers {
                                                let api_key = config_clone.model_servers.iter()
                                                    .find(|s| &s.url == url)
                                                    .and_then(|s| s.api_key.as_deref())
                                                    .or_else(|| {
                                                        if url == &config_clone.server_url {
                                                            config_clone.api_key.as_deref()
                                                        } else {
                                                            None
                                                        }
                                                    });
                                                let live = lethetic::client::get_available_models(&client_clone, url, api_key).await;
                                                if live.is_empty() {
                                                    // Server not reachable — still show from config
                                                    models.push(ModelChoice {
                                                        display: format!("{} (offline)", name),
                                                        url: url.clone(),
                                                        model_id: default_model.clone(),
                                                    });
                                                } else {
                                                    for (id, _) in live {
                                                        models.push(ModelChoice {
                                                            display: format!("{} — {}", name, id),
                                                            url: url.clone(),
                                                            model_id: id,
                                                        });
                                                    }
                                                }
                                            }
                                            let _ = tx_clone.send(StreamEvent::ModelsReady(models));
                                        });
                                    }
                                    AppEventOutcome::SwitchModel(new_url, new_model, parser) => {
                                        config.server_url = new_url.clone();
                                        config.model = new_model.clone();
                                        let matching_server = config.model_servers.iter().find(|s| s.url == new_url);
                                        config.api_key = matching_server.and_then(|s| s.api_key.clone());
                                        config.input_cost_per_1m = matching_server.and_then(|s| s.input_cost_per_1m);
                                        config.output_cost_per_1m = matching_server.and_then(|s| s.output_cost_per_1m);
                                        config.thinking = matching_server.and_then(|s| s.thinking);
                                        config.extra_body = matching_server.and_then(|s| s.extra_body.clone());
                                        config.context_mode = matching_server.and_then(|s| s.context_mode);
                                        if let Some(matching_server) = matching_server
                                            && let Some(t) = &matching_server.theme {
                                                config.theme = Some(t.clone());
                                            }
                                        if let Some(Some(sz)) = matching_server.map(|s| s.context_size) {
                                            config.context_size = sz;
                                            app.max_tokens = sz;
                                        }
                                        app.server_url = new_url.clone();
                                        app.model_name = new_model.clone();
                                        app.config = config.clone();
                                        app.context_manager.mode = config.context_mode.unwrap_or(lethetic::context::ContextMode::Lethetic);
                                        if let Some(name) = &config.theme
                                            && let Some(found) = app.themes.iter().find(|t| t.name.eq_ignore_ascii_case(name)) {
                                                app.theme = found.clone();
                                            }
                                        let mode = lethetic::parser::ParserMode::from(parser.as_str());
                                        app.parser.set_mode(mode);
                                        app.parser.reset();
                                        // Re-resolve system prompt with updated config so the
                                        // correct tool call format (gemma4 <|"|> vs qwen3 JSON)
                                        // is injected for the new model.
                                        let refreshed = lethetic::system_prompt::SystemPromptManager::resolve_prompt(
                                            &app.system_prompt, &app.current_dir, config);
                                        app.context_manager.update_system_prompt(refreshed);
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
                                                let _ = tx.send(StreamEvent::ToolResult { id: Some(tc_id), func_name, result: prompt, cwd: app.current_dir.clone() });
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
                                            app.tool_call_dispatched = false;
                                            app.tool_call_pos = None;
                                            app.server_prompt_tokens = None;
                                            app.server_completion_tokens = None;
                                            full_response_content.clear();
                                            cancellation_token = CancellationToken::new();
                                            app.request_start_time = Some(tokio::time::Instant::now());
                                            app.parser.reset();
                            trigger_llm_request(client.clone(), config.clone(), &app.context_manager, tx.clone(), cancellation_token.clone(), app.show_debug, app.current_session_dir.clone());
                                        }
                                    }
                                    AppEventOutcome::ToolApproved(approved, always) => {
                                        if app.pending_tool_call.is_some() {
                                            if approved {
                                                if always { app.shell_approval_mode = ApprovalMode::Always; }
                                                dispatch_auto_approved_tool(app, &tx, &cancellation_token, &client, config);
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
                                        while rx.try_recv().is_ok() {}
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
                                        let _ = ctx_tx.send(StreamEvent::ToolResult { id: None, func_name: "lsp_install".to_string(), result, cwd: new_dir });
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
                            } else if let Some(dir) = msg.strip_prefix("DIR_UPDATE|") {
                                app.current_dir = dir.to_string();
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
                                            app.server_prompt_tokens = None;
                                            app.server_completion_tokens = None;
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
                                                dispatch_auto_approved_tool(app, &tx, &cancellation_token, &client, config);
                                            }
                                        }
                                        Some(Err((err_msg, _pos))) => {
                                            // Only log error if we are sure it should have finished (Text state)
                                            app.log_debug(&format!("Tool call syntax error: {}", err_msg));
                                            cancellation_token.cancel();
                                            app.is_processing = false;
                                            app.stop_reason = "⚠ Tool call syntax error — re-prompting".to_string();
                                            app.add_segment(format!("\n{} [SYNTAX ERROR] {}\n", icons::WARNING, err_msg), BlockType::Text);
                                            app.context_manager.add_message("assistant", &full_response_content);
                                            let _ = tx.send(StreamEvent::ToolResult { id: Some("raw_call".to_string()), func_name: "syntax_error".to_string(), result: format!("Syntax Error in tool call: {}", err_msg), cwd: app.current_dir.clone() });
                                        }
                                        None => {}
                                    }
                                }
                            }
                        }
                        StreamEvent::ToolCalls(calls) => {
                            if !app.tool_calls_processed_this_request
                                && let AppEventOutcome::ToolApproved(..) = handle_tool_call(app, calls, full_response_content.len(), tx.clone(), &mut cancellation_token, &full_response_content, true) {
                                    dispatch_auto_approved_tool(app, &tx, &cancellation_token, &client, config);
                                }
                        }
                        StreamEvent::ToolResult { id, func_name, result, cwd } => {
                            app.is_executing_tool = false;
                            app.tool_output_preview.clear();
                            app.current_dir = cwd;
                            let success = if result.contains("EXIT_CODE: ") { result.contains("EXIT_CODE: 0") } else { true };
                            
                            let tc_id_str = id.clone().unwrap_or_else(|| "unknown".to_string());
                            let tool_args = app.pending_tool_call.as_ref().map(|tc| tc.function.arguments.clone()).unwrap_or(serde_json::json!({}));
                            // read_file: never truncate — content always goes into the file cache via
                            // update_latest_file below, so the model accesses it through the context
                            // header rather than the tool result. Token budget eviction handles large files.
                            let (mut full_result, ui_result) = if func_name == "read_file" && result.len() < 500_000 {
                                (result.clone(), result)
                            } else {
                                lethetic::tools::handle_large_output(&tc_id_str, result)
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
                                        if let Some(path) = tool_args["path"].as_str()
                                            && let Some(content) = tool_args["content"].as_str() {
                                                app.context_manager.update_latest_file(path.to_string(), content.to_string());
                                                app.add_segment(format!("\n{} File `{}` has been placed in context.\n", icons::SUCCESS, path), BlockType::Text);
                                            }
                                    } else if func_name == "apply_patch" && full_result.contains("Successfully patched") {
                                        if let Some(path) = tool_args["file_path"].as_str() {
                                            let full_path = std::path::Path::new(&app.current_dir).join(path);
                                            if let Ok(content) = std::fs::read_to_string(&full_path) {
                                                app.context_manager.update_latest_file(path.to_string(), content);
                                                app.add_segment(format!("\n{} File `{}` has been updated in context.\n", icons::SUCCESS, path), BlockType::Text);
                                            }
                                        }
                                    } else if func_name == "replace_text" && full_result.contains("Successfully replaced")
                                        && let Some(path) = tool_args["path"].as_str() {
                                            let full_path = std::path::Path::new(&app.current_dir).join(path);
                                            if let Ok(content) = std::fs::read_to_string(&full_path) {
                                                app.context_manager.update_latest_file(path.to_string(), content);
                                                app.add_segment(format!("\n{} File `{}` has been updated in context.\n", icons::SUCCESS, path), BlockType::Text);
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
                                        let msg = "⚠ EDIT ALREADY APPLIED: This exact `old_string` was successfully replaced in a prior call. \
                                             The file already contains your updated version. \
                                             Do not retry this edit — move on to the next issue.".to_string();
                                        app.context_manager.add_message("user", &msg);
                                        app.add_segment("\n⚠ [EDIT ALREADY APPLIED] old_string was replaced earlier this session — move on.\n".to_string(), BlockType::Text);
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
                            app.tool_call_dispatched = false;
                            app.tool_call_pos = None;
                            app.server_prompt_tokens = None;
                            app.server_completion_tokens = None;
                            full_response_content.clear();
                            cancellation_token = CancellationToken::new();
                            app.request_start_time = Some(tokio::time::Instant::now());
                            app.context_manager.set_cwd(app.current_dir.clone());
                            app.parser.reset();
                            trigger_llm_request(client.clone(), config.clone(), &app.context_manager, tx.clone(), cancellation_token.clone(), app.show_debug, app.current_session_dir.clone());
                        }
                        StreamEvent::PreparingToolCall(name) => {
                            app.stop_reason = format!("Lethetic Intelligence Engine Processing (Preparing tool call: {})…", name);
                            app.should_redraw = true;
                        }
                        StreamEvent::ToolProgress(msg) => {
                            app.tool_output_preview = msg;
                            app.should_redraw = true;
                        }
                        StreamEvent::Done { completion_tokens, prompt_tokens, tg_per_s, pp_per_s } => {
                            app.is_processing = false;
                            if (app.parser.state == lethetic::parser::ParserState::Text || app.parser.state == lethetic::parser::ParserState::ToolCall) && !app.tool_calls_processed_this_request
                                && let Some(Ok((tc, pos))) = parser::find_tool_call(&full_response_content, true) {
                                    handle_tool_call(app, vec![tc], pos, tx.clone(), &mut cancellation_token, &full_response_content, false);
                                }

                            if !app.tool_calls_processed_this_request {
                                let last_is_assistant = app.context_manager.get_messages().last().is_some_and(|m| m.role == "assistant");
                                if !last_is_assistant {
                                    app.context_manager.add_message("assistant", &full_response_content);
                                }

                                // Detect "intention text": model described an action in plain text
                                // but never issued a tool call. Re-prompt once so it acts.
                                if looks_like_intention_without_action(&full_response_content) {
                                    app.log_debug("INTENT_TEXT_DETECTED: model described action without tool call — re-prompting");
                                    app.stop_reason = "→ Described action without tool call — re-prompting".to_string();
                                    app.context_manager.add_message("user", "You described an action but did not call a tool. Please call the appropriate tool now.");
                                    app.is_processing = true;
                                    app.tool_calls_processed_this_request = false;
                                    app.tool_call_dispatched = false;
                                    app.server_prompt_tokens = None;
                                    app.server_completion_tokens = None;
                                    full_response_content.clear();
                                    cancellation_token = CancellationToken::new();
                                    app.parser.reset();
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
                            if let Some(ct) = completion_tokens {
                                app.server_completion_tokens = Some(ct);
                            }
                            if let Some(user_block) = app.blocks.iter_mut().rev().find(|b| b.block_type == BlockType::User) {
                                user_block.prompt_tokens = prompt_tokens;
                                user_block.completion_tokens = completion_tokens;
                                user_block.cached_lines = None;
                            }
                            app.request_start_time = None;
                            app.should_redraw = true;
                            app.save_session(); // Final save on completion

                            if app.tool_calls_processed_this_request && app.shell_approval_mode == ApprovalMode::Always && !app.tool_call_dispatched {
                                dispatch_auto_approved_tool(app, &tx, &cancellation_token, &client, config);
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
                        StreamEvent::SessionLoaded { dir, state } => {
                            app.current_session_dir = Some(dir);
                            app.blocks = state.blocks;
                            app.history = state.history;
                            // Restore the session's theme; cached_lines were pre-rendered
                            // with the current theme, so invalidate them on a theme change.
                            if !state.theme_name.is_empty() && state.theme_name != app.theme.name
                                && let Some(idx) = app.themes.iter().position(|t| t.name == state.theme_name) {
                                    app.theme = app.themes[idx].clone();
                                    app.theme_state.select(Some(idx));
                                    for block in &mut app.blocks { block.cached_lines = None; }
                                }
                            app.context_manager.clear();
                            app.context_manager.set_messages(state.messages);
                            app.scroll = 0;
                            app.output_state.select(Some(app.blocks.len().saturating_sub(1)));
                            app.is_loading_session = false;
                            app.needs_save = false;
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


#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    #[test]
    #[serial_test::serial]
    fn test_panic_logging() {
        setup_panic_hook();

        let log_path = Path::new(".lethetic/panic.log");
        let initial_len = if log_path.exists() {
            fs::metadata(log_path).unwrap().len()
        } else {
            0
        };

        let result = std::panic::catch_unwind(|| {
            panic!("Test panic message for hook validation");
        });
        assert!(result.is_err());

        assert!(log_path.exists());
        let log_content = fs::read_to_string(log_path).expect("Failed to read panic.log");
        assert!(log_content.len() as u64 > initial_len);
        assert!(log_content.contains("Test panic message for hook validation"));
        assert!(log_content.contains("Timestamp:"));

        let _ = std::panic::take_hook();
    }
}
