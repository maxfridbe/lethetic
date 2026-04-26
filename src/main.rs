use std::env;
use crossterm::{
    event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event, EventStream, KeyEventKind, MouseEventKind, KeyCode, KeyModifiers},
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

use lethetic::context;
use lethetic::config::Config;
use lethetic::client::{StreamEvent, trigger_llm_request};
use lethetic::app::{App, AppEventOutcome, BlockType, handle_key, handle_tool_call, ApprovalMode};
use lethetic::ui::ui;
use lethetic::tools::get_git_info;
use lethetic::icons;
use lethetic::parser;
use lethetic::parser_new;

fn handle_large_output(id: &str, result: String) -> (String, String) {
    if result.len() > 10000 {
        let file_id = if id.is_empty() { "unknown" } else { id };
        let dir_path = ".lethetic/tool_responses";
        let _ = std::fs::create_dir_all(dir_path);
        let file_path = format!("{}/{}.txt", dir_path, file_id);
        let _ = std::fs::write(&file_path, &result);
        
        let mut exit_status = String::new();
        if result.starts_with("EXIT_CODE: ") {
            let lines: Vec<&str> = result.lines().collect();
            if !lines.is_empty() {
                exit_status = format!("{}\n", lines[0]);
            }
        }
        
        let truncated = format!("{}... [Output truncated. Full output is {} characters long and has been saved to {}] ...", exit_status, result.len(), file_path);
        (result, truncated)
    } else {
        (result.clone(), result)
    }
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
    let config: Config = serde_yaml::from_str(&config_content)?;

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
    let res = run_app(&mut terminal, &mut app, &config).await;

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        println!("{:?}", err);
    }

    Ok(())
}

async fn run_headless(config: &Config, prompt: String) -> Result<(), Box<dyn Error>> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let client = Client::new();
    let mut cancellation_token = CancellationToken::new();
    let mut app = App::new(config);
    app.context_manager.set_cwd(app.current_dir.clone());
    
    app.context_manager.add_message("user", &prompt);
    println!("\n{} User: {}\n", icons::INPUT, prompt);
    
    let mut full_response_content = String::new();
    app.context_manager.set_cwd(app.current_dir.clone());
    app.parser.reset();
    trigger_llm_request(client.clone(), config.clone(), &app.context_manager, tx.clone(), cancellation_token.clone(), false, app.current_session_dir.clone());

    loop {
        match rx.recv().await {
            Some(StreamEvent::Chunk(chunk)) => {
                print!("{}", chunk);
                io::Write::flush(&mut io::stdout())?;
                full_response_content.push_str(&chunk);
                
                app.parser.parse_chunk(&chunk);

                if app.parser.state == lethetic::parser_new::ParserState::Text {
                    match parser::find_tool_call(&full_response_content, true) {
                        Some(Ok((tc, _))) => {
                            println!("\n\n{} [TOOL CALL: {}]", icons::COMMAND, tc.function.name);
                            println!("Arguments: {}", tc.function.arguments);
                            cancellation_token.cancel();
                            
                            let assistant_content = full_response_content.clone();
                            app.context_manager.add_message("assistant", &assistant_content);

                            let (mut result, new_dir) = lethetic::tools::execute(
                                tc.function.name.as_str(), &tc.function.arguments, &app.current_dir, cancellation_token.clone(), tx.clone()).await;
                            let (full_result, ui_result) = handle_large_output(&tc.id, result);

                            app.current_dir = new_dir;
                            println!("\n{} [TOOL RESULT]\n{}\n", icons::SUCCESS, ui_result);
                            app.context_manager.add_tool_message(tc.id.clone(), &tc.function.name, &full_result);

                            
                            full_response_content.clear();
                            cancellation_token = CancellationToken::new();
                            app.context_manager.set_cwd(app.current_dir.clone());
                            app.parser.reset();
                            trigger_llm_request(client.clone(), config.clone(), &app.context_manager, tx.clone(), cancellation_token.clone(), false, app.current_session_dir.clone());
                            continue; 
                        }
                        Some(Err((err_msg, _))) => {
                            println!("\n\n{} [SYNTAX ERROR: {}]", icons::WARNING, err_msg);
                            cancellation_token.cancel();
                            let assistant_content = full_response_content.clone();
                            app.context_manager.add_message("assistant", &assistant_content);
                            app.context_manager.add_tool_message("raw_call".to_string(), "syntax_error", &format!("Syntax Error in tool call: {}", err_msg));
                            full_response_content.clear();
                            cancellation_token = CancellationToken::new();
                            app.context_manager.set_cwd(app.current_dir.clone());
                            app.parser.reset();
                            trigger_llm_request(client.clone(), config.clone(), &app.context_manager, tx.clone(), cancellation_token.clone(), false, app.current_session_dir.clone());
                            continue;
                        }
                        None => {}
                    }
                }
            }
            Some(StreamEvent::ToolProgress(msg)) => {
                // In headless, we could print progress or just ignore for now
                // but let's print a single line indicator if it changed
                print!("\r[STREAMING] {}          ", msg.replace('\n', " | "));
                io::Write::flush(&mut io::stdout())?;
            }
            Some(StreamEvent::Done(_, _)) => {
                print!("\r                                                                \r");
                if app.parser.state == lethetic::parser_new::ParserState::Text {
                    match parser::find_tool_call(&full_response_content, true) {
                        Some(Ok((tc, _))) => {
                        println!("\n\n{} [TOOL CALL: {}]", icons::COMMAND, tc.function.name);
                        println!("Arguments: {}", tc.function.arguments);
                        cancellation_token.cancel();
                        cancellation_token = CancellationToken::new();

                        let assistant_content = full_response_content.clone();
                        app.context_manager.add_message("assistant", &assistant_content);

                        let (mut result, new_dir) = lethetic::tools::execute(
                            tc.function.name.as_str(), &tc.function.arguments, &app.current_dir, cancellation_token.clone(), tx.clone()).await;
                        let (full_result, ui_result) = handle_large_output(&tc.id, result);
                        
                        app.current_dir = new_dir;
                        println!("\n{} [TOOL RESULT]\n{}\n", icons::SUCCESS, ui_result);
                        app.context_manager.add_tool_message(tc.id.clone(), &tc.function.name, &full_result);
                        
                        full_response_content.clear();
                        cancellation_token = CancellationToken::new();
                        app.context_manager.set_cwd(app.current_dir.clone());
                        app.parser.reset();
                        trigger_llm_request(client.clone(), config.clone(), &app.context_manager, tx.clone(), cancellation_token.clone(), false, app.current_session_dir.clone());
                        continue;
                    }
                    _ => {}
                }
            }

                if !full_response_content.is_empty() {
                     let messages = app.context_manager.get_messages();
                     let last_role = messages.last().map(|m| m.role.as_str());
                     if last_role != Some("assistant") {
                        app.context_manager.add_message("assistant", &full_response_content);
                     }
                }
                
                if full_response_content.is_empty() && app.context_manager.get_messages().last().map_or(false, |m| m.role == "tool") {
                    continue;
                }

                println!("\n[DONE]");
                return Ok(());
            }
            Some(StreamEvent::Error(e)) => {
                println!("\n{} ERROR: {}", icons::WARNING, e);
                return Ok(());
            }
            None => break,
            _ => {}
        }
    }
    Ok(())
}

async fn run_app(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App, config: &Config) -> Result<(), Box<dyn Error>> {
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
                                        app.load_session(&filename);
                                        app.should_redraw = true;
                                    }
                                    AppEventOutcome::DeleteSession(filename) => {
                                        let _ = std::fs::remove_dir_all(filename);
                                        app.refresh_session_list();
                                        app.should_redraw = true;
                                    }
                                    AppEventOutcome::SendPrompt(prompt) => {
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
                                            app.context_manager.add_message("user", &prompt);
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
                                    }
                                    AppEventOutcome::ToolApproved(approved, always) => {
                                        if let Some(tool_call) = app.pending_tool_call.take() {
                                            if approved {
                                                if always { app.shell_approval_mode = ApprovalMode::Always; }
                                                let tc_id = tool_call.id.clone();
                                                let func_name = tool_call.function.name.clone();
                                                let args = tool_call.function.arguments.clone();
                                                let current_dir = app.current_dir.clone();
                                                
                                                let ctx_tx = tx.clone();
                                                let tool_cancel = cancellation_token.clone();
                                                app.is_executing_tool = true;
                                                tokio::spawn(async move {
                                                    let (mut result, new_dir) = lethetic::tools::execute(
                                                        func_name.as_str(), &args, &current_dir, tool_cancel, ctx_tx.clone()).await;
                                                    
                                                    let (full_result, ui_result) = handle_large_output(&tc_id, result);
                                                    
                                                    let _ = ctx_tx.send(StreamEvent::ToolResult(Some(tc_id), func_name, full_result, new_dir.clone()));
                                                    let _ = ctx_tx.send(StreamEvent::DebugLog(format!("DIR_UPDATE|{}", new_dir)));
                                                });
                                                app.is_processing = true;
                                            } else {
                                                app.add_segment(format!("\n{} Tool execution denied by user.\n", icons::WARNING), BlockType::Text);
                                            }
                                        }
                                        app.show_approval_prompt = false;
                                        app.should_redraw = true;
                                    }
                                    AppEventOutcome::Stop => {
                                        cancellation_token.cancel();
                                        app.is_processing = false;
                                        app.add_segment(format!("\n{} [STOPPED]\n", icons::WARNING), BlockType::Text);
                                        while let Ok(_) = rx.try_recv() {}
                                        app.should_redraw = true;
                                    }
                                    AppEventOutcome::Continue => { app.should_redraw = true; }
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
                                    if let Some(reason) = app.loop_detector.check(&app.last_block_content) {
                                        app.log_debug(&format!("LOOP DETECTED: {}", reason));
                                        cancellation_token.cancel();
                                        app.is_processing = false;
                                        
                                        let now = std::time::Instant::now();
                                        let is_rapid_loop = if let Some(last_time) = app.last_loop_detection_time {
                                            now.duration_since(last_time) < std::time::Duration::from_secs(120)
                                        } else {
                                            false
                                        };

                                        if is_rapid_loop && app.loop_detection_count >= 1 {
                                            app.add_segment(format!("\n{} [FORCED STOP] Rapid reasoning loop detected twice within 2 minutes. Stopping engine to prevent waste.\n", icons::WARNING), BlockType::Text);
                                            app.context_manager.add_message("assistant", &full_response_content);
                                            app.context_manager.add_message("system", "The user's system forced a stop because you entered a persistent reasoning loop.");
                                            app.loop_detection_count = 0; // Reset for next attempt
                                            app.last_loop_detection_time = None;
                                        } else {
                                            app.add_segment(format!("\n{} [LOOP DETECTED] {}\n", icons::WARNING, reason), BlockType::Text);
                                            app.context_manager.add_message("assistant", &full_response_content);
                                            app.context_manager.add_message("system", "Note: You were stuck in a reasoning loop. Please choose a single clear path and proceed with a tool call immediately.");
                                            
                                            app.loop_detection_count += 1;
                                            app.last_loop_detection_time = Some(now);
                                            
                                            // Re-trigger the request automatically once
                                            trigger_llm_request(client.clone(), config.clone(), &app.context_manager, tx.clone(), cancellation_token.clone(), app.show_debug, app.current_session_dir.clone());
                                        }
                                        break; 
                                    }
                                }

                                if app.parser.state == lethetic::parser_new::ParserState::Text && !app.tool_calls_processed_this_request {
                                    match parser::find_tool_call(&full_response_content, true) {
                                        Some(Ok((tc, pos))) => {
                                            handle_tool_call(app, vec![tc], pos, tx.clone(), &mut cancellation_token, &full_response_content, false);
                                        }
                                        Some(Err((err_msg, _pos))) => {
                                            app.log_debug(&format!("Tool call syntax error: {}", err_msg));
                                            cancellation_token.cancel();
                                            app.is_processing = false;
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
                                handle_tool_call(app, calls, full_response_content.len(), tx.clone(), &mut cancellation_token, &full_response_content, true);
                            }
                        }
                        StreamEvent::ToolResult(id, func_name, mut result, new_dir) => {
                            app.is_executing_tool = false;
                            app.tool_output_preview.clear();
                            app.current_dir = new_dir;
                            let success = if result.contains("EXIT_CODE: ") { result.contains("EXIT_CODE: 0") } else { true };
                            
                            let tc_id_str = id.clone().unwrap_or_else(|| "unknown".to_string());
                            let (full_result, ui_result) = handle_large_output(&tc_id_str, result);

                            let description = app.pending_tool_call.as_ref()
                                .and_then(|tc| tc.function.arguments["description"].as_str())
                                .unwrap_or("Action").to_string();

                            app.add_segment_with_title(format!("\n{}\n", ui_result), BlockType::ToolResult, description);
                            if let Some(last) = app.blocks.last_mut() { last.success = Some(success); }

                            if let Some(tc_id) = id { 
                                if let Some(tc) = app.pending_tool_call.take() {
                                    app.context_manager.add_assistant_tool_call(&full_response_content, vec![tc]);
                                }
                                app.context_manager.add_tool_message(tc_id, &func_name, &full_result); 
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
                        StreamEvent::Done(eval_count, eval_duration) => {
                            app.is_processing = false;
                            if app.parser.state == lethetic::parser_new::ParserState::Text && !app.tool_calls_processed_this_request {
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
                            }

                            if let (Some(count), Some(duration)) = (eval_count, eval_duration) {
                                app.tokens_per_s = (count as f64 / (duration as f64 / 1_000_000_000.0)).max(0.0);
                            } else if let Some(start) = app.request_start_time {
                                // Fallback for servers that don't provide duration in Done
                                let elapsed = start.elapsed().as_secs_f64();
                                if elapsed > 0.0 {
                                    // Use full_response_content as a proxy for count if eval_count is missing
                                    let count = eval_count.unwrap_or(full_response_content.split_whitespace().count() as u32);
                                    app.tokens_per_s = (count as f64 / elapsed).max(0.0);
                                }
                            }
                            app.request_start_time = None;
                            app.should_redraw = true;
                            app.save_session(); // Final save on completion

                            if app.tool_calls_processed_this_request && app.shell_approval_mode == ApprovalMode::Always {
                                if let Some(tool_call) = app.pending_tool_call.take() {
                                    let tc_id = tool_call.id.clone();
                                    let func_name = tool_call.function.name.clone();
                                    let args = tool_call.function.arguments.clone();
                                    let current_dir = app.current_dir.clone();
                                    
                                    let ctx_tx = tx.clone();
                                    let tool_cancel = cancellation_token.clone();
                                    app.is_executing_tool = true;
                                    tokio::spawn(async move {
                                        let (mut result, new_dir) = lethetic::tools::execute(
                                            func_name.as_str(), &args, &current_dir, tool_cancel, ctx_tx.clone()).await;
                                        let (full_result, ui_result) = handle_large_output(&tc_id, result);
                                        let _ = ctx_tx.send(StreamEvent::ToolResult(Some(tc_id), func_name, full_result, new_dir.clone()));
                                        let _ = ctx_tx.send(StreamEvent::DebugLog(format!("DIR_UPDATE|{}", new_dir)));
                                    });
                                    app.is_processing = true;
                                }
                            }
                        }
                        StreamEvent::Error(e) => {
                            app.is_processing = false;
                            app.add_segment(format!("\n{} ERROR: {}\n", icons::WARNING, e), BlockType::Text);
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
