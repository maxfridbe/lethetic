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

mod context;
mod tools;
mod icons;
mod system_prompt;
mod markdown;
mod config;
mod client;
mod parser;
mod tool_executor;
mod app;
mod ui;

use config::Config;
use client::{StreamEvent, trigger_llm_request};
use app::{App, AppEventOutcome, BlockType, handle_key, handle_tool_call, ApprovalMode};
use ui::ui;
use tool_executor::get_git_info;

fn handle_large_output(id: &str, mut result: String) -> String {
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
        
        result = format!("{}... [Output truncated. Full output is {} characters long and has been saved to {}] ...", exit_status, result.len(), file_path);
    }
    result
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
    trigger_llm_request(client.clone(), config.clone(), &app.context_manager, tx.clone(), cancellation_token.clone(), false, app.current_session_dir.clone());

    loop {
        match rx.recv().await {
            Some(StreamEvent::Chunk(chunk)) => {
                print!("{}", chunk);
                io::Write::flush(&mut io::stdout())?;
                full_response_content.push_str(&chunk);

                let is_complete = full_response_content.contains("<|tool_call|>") 
                    || full_response_content.contains("<tool_call|>");

                if is_complete {
                    match parser::find_tool_call(&full_response_content, true) {
                        Some(Ok((tc, _))) => {
                            println!("\n\n{} [TOOL CALL: {}]", icons::COMMAND, tc.function.name);
                            println!("Arguments: {}", tc.function.arguments);
                            cancellation_token.cancel();
                            
                            let assistant_content = full_response_content.clone();
                            app.context_manager.add_message("assistant", &assistant_content);

                            let (mut result, new_dir) = match tc.function.name.as_str() {
                                "run_shell_command" => {
                                    let cmd = tc.function.arguments["command"].as_str().unwrap_or("");
                                    tool_executor::execute_shell(cmd, &app.current_dir, cancellation_token.clone()).await
                                },
                                "read_folder" => {
                                    let path = tc.function.arguments["path"].as_str().unwrap_or(".");
                                    (tool_executor::execute_read_folder(path, &app.current_dir).await, app.current_dir.clone())
                                },
                                "read_file_lines" => {
                                    let path = tc.function.arguments["path"].as_str().unwrap_or("");
                                    let start = tc.function.arguments["start_line"].as_u64().unwrap_or(1) as usize;
                                    let end = tc.function.arguments["end_line"].as_u64().unwrap_or(1) as usize;
                                    (tool_executor::execute_read_file_lines(path, start, end, &app.current_dir).await, app.current_dir.clone())
                                },
                                "search_text" => {
                                    let pattern = tc.function.arguments["pattern"].as_str().unwrap_or("");
                                    let path = tc.function.arguments["path"].as_str().unwrap_or(".");
                                    (tool_executor::execute_search_text(pattern, path, &app.current_dir).await, app.current_dir.clone())
                                },
                                "apply_patch" => {
                                    let path = tc.function.arguments["path"].as_str().unwrap_or("");
                                    let patch = tc.function.arguments["patch"].as_str().unwrap_or("");
                                    (tool_executor::execute_apply_patch(path, patch, &app.current_dir).await, app.current_dir.clone())
                                },
                                "replace_text" => {
                                    let path = tc.function.arguments["path"].as_str().unwrap_or("");
                                    let old_string = tc.function.arguments["old_string"].as_str().unwrap_or("");
                                    let new_string = tc.function.arguments["new_string"].as_str().unwrap_or("");
                                    (tool_executor::execute_replace_text(path, old_string, new_string, &app.current_dir).await, app.current_dir.clone())
                                },
                                "write_file" => {
                                    let path = tc.function.arguments["path"].as_str().unwrap_or("");
                                    let content = tc.function.arguments["content"].as_str().unwrap_or("");
                                    (tool_executor::execute_write_file(path, content, &app.current_dir).await, app.current_dir.clone())
                                },
                                "web_fetch" => {
                                    let url = tc.function.arguments["url"].as_str().unwrap_or("");
                                    (tool_executor::execute_web_fetch(url).await, app.current_dir.clone())
                                },
                                "calculate" => (format!("Calculation result for: {}", tc.function.arguments["expression"]), app.current_dir.clone()),
                                _ => (format!("Unknown tool: {}", tc.function.name), app.current_dir.clone()),
                            };
                            
                            result = handle_large_output(&tc.id, result);
                            
                            app.current_dir = new_dir;
                            println!("\n{} [TOOL RESULT]\n{}\n", icons::SUCCESS, result);
                            app.context_manager.add_tool_message(tc.id.clone(), &tc.function.name, &result);
                            
                            full_response_content.clear();
                            cancellation_token = CancellationToken::new();
                            app.context_manager.set_cwd(app.current_dir.clone());
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
                            trigger_llm_request(client.clone(), config.clone(), &app.context_manager, tx.clone(), cancellation_token.clone(), false, app.current_session_dir.clone());
                            continue;
                        }
                        None => {}
                    }
                }
            }
            Some(StreamEvent::Done(_, _)) => {
                match parser::find_tool_call(&full_response_content, true) {
                    Some(Ok((tc, _))) => {
                        println!("\n\n{} [TOOL CALL: {}]", icons::COMMAND, tc.function.name);
                        println!("Arguments: {}", tc.function.arguments);
                        cancellation_token.cancel();
                        cancellation_token = CancellationToken::new();

                        let assistant_content = full_response_content.clone();                        app.context_manager.add_message("assistant", &assistant_content);

                        let (mut result, new_dir) = match tc.function.name.as_str() {
                            "run_shell_command" => {
                                let cmd = tc.function.arguments["command"].as_str().unwrap_or("");
                                tool_executor::execute_shell(cmd, &app.current_dir, cancellation_token.clone()).await
                            },
                            "read_folder" => {
                                let path = tc.function.arguments["path"].as_str().unwrap_or(".");
                                (tool_executor::execute_read_folder(path, &app.current_dir).await, app.current_dir.clone())
                            },
                            "read_file_lines" => {
                                let path = tc.function.arguments["path"].as_str().unwrap_or("");
                                let start = tc.function.arguments["start_line"].as_u64().unwrap_or(1) as usize;
                                let end = tc.function.arguments["end_line"].as_u64().unwrap_or(1) as usize;
                                (tool_executor::execute_read_file_lines(path, start, end, &app.current_dir).await, app.current_dir.clone())
                            },
                            "apply_patch" => {
                                let path = tc.function.arguments["path"].as_str().unwrap_or("");
                                let patch = tc.function.arguments["patch"].as_str().unwrap_or("");
                                (tool_executor::execute_apply_patch(path, patch, &app.current_dir).await, app.current_dir.clone())
                            },
                            "write_file" => {
                                let path = tc.function.arguments["path"].as_str().unwrap_or("");
                                let content = tc.function.arguments["content"].as_str().unwrap_or("");
                                (tool_executor::execute_write_file(path, content, &app.current_dir).await, app.current_dir.clone())
                            },
                            "web_fetch" => {
                                let url = tc.function.arguments["url"].as_str().unwrap_or("");
                                (tool_executor::execute_web_fetch(url).await, app.current_dir.clone())
                            },
                            "calculate" => (format!("Calculation result for: {}", tc.function.arguments["expression"]), app.current_dir.clone()),
                            _ => (format!("Unknown tool: {}", tc.function.name), app.current_dir.clone()),
                        };
                        
                        result = handle_large_output(&tc.id, result);
                        
                        app.current_dir = new_dir;
                        println!("\n{} [TOOL RESULT]\n{}\n", icons::SUCCESS, result);
                        app.context_manager.add_tool_message(tc.id.clone(), &tc.function.name, &result);
                        
                        full_response_content.clear();
                        cancellation_token = CancellationToken::new();
                        app.context_manager.set_cwd(app.current_dir.clone());
    trigger_llm_request(client.clone(), config.clone(), &app.context_manager, tx.clone(), cancellation_token.clone(), false, app.current_session_dir.clone());
                        continue;
                    }
                    Some(Err((err_msg, _))) => {
                        println!("\n\n{} [SYNTAX ERROR: {}]", icons::WARNING, err_msg);
                        let assistant_content = full_response_content.clone();
                        app.context_manager.add_message("assistant", &assistant_content);
                        app.context_manager.add_tool_message("raw_call".to_string(), "syntax_error", &format!("Syntax Error in tool call: {}", err_msg));
                        full_response_content.clear();
                        cancellation_token = CancellationToken::new();
                        app.context_manager.set_cwd(app.current_dir.clone());
    trigger_llm_request(client.clone(), config.clone(), &app.context_manager, tx.clone(), cancellation_token.clone(), false, app.current_session_dir.clone());
                        continue;
                    }
                    None => {}
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
                                                tokio::spawn(async move {
                                                    let (mut result, new_dir) = match func_name.as_str() {
                                                        "run_shell_command" => {
                                                            let cmd = args["command"].as_str().unwrap_or("");
                                                            tool_executor::execute_shell(cmd, &current_dir, tool_cancel).await
                                                        },
                                                        "read_folder" => {
                                                            let path = args["path"].as_str().unwrap_or(".");
                                                            (tool_executor::execute_read_folder(path, &current_dir).await, current_dir.clone())
                                                        },
                                                        "read_file" => {
                                                            let path = args["path"].as_str().unwrap_or("");
                                                            (tool_executor::execute_read_file(path, &current_dir).await, current_dir.clone())
                                                        },
                                                        "read_file_lines" => {
                                                            let path = args["path"].as_str().unwrap_or("");
                                                            let start = args["start_line"].as_u64().unwrap_or(1) as usize;
                                                            let end = args["end_line"].as_u64().unwrap_or(1) as usize;
                                                            (tool_executor::execute_read_file_lines(path, start, end, &current_dir).await, current_dir.clone())
                                                        },
                                                        "search_text" => {
                                                            let pattern = args["pattern"].as_str().unwrap_or("");
                                                            let path = args["path"].as_str().unwrap_or(".");
                                                            (tool_executor::execute_search_text(pattern, path, &current_dir).await, current_dir.clone())
                                                        },
                                                        "apply_patch" => {
                                                            let path = args["path"].as_str().unwrap_or("");
                                                            let patch = args["patch"].as_str().unwrap_or("");
                                                            (tool_executor::execute_apply_patch(path, patch, &current_dir).await, current_dir.clone())
                                                        },
                                                        "replace_text" => {
                                                            let path = args["path"].as_str().unwrap_or("");
                                                            let old_string = args["old_string"].as_str().unwrap_or("");
                                                            let new_string = args["new_string"].as_str().unwrap_or("");
                                                            (tool_executor::execute_replace_text(path, old_string, new_string, &current_dir).await, current_dir.clone())
                                                        },
                                                        "write_file" => {
                                                            let path = args["path"].as_str().unwrap_or("");
                                                            let content = args["content"].as_str().unwrap_or("");
                                                            (tool_executor::execute_write_file(path, content, &current_dir).await, current_dir.clone())
                                                        },
                                                        "web_fetch" => {
                                                            let url = args["url"].as_str().unwrap_or("");
                                                            (tool_executor::execute_web_fetch(url).await, current_dir.clone())
                                                        },
                                                        "calculate" => (format!("Calculation result for: {}", args["expression"]), current_dir.clone()),
                                                        _ => (format!("Unknown tool: {}", func_name), current_dir.clone()),
                                                    };
                                                    
                                                    result = handle_large_output(&tc_id, result);
                                                    
                                                    let _ = ctx_tx.send(StreamEvent::ToolResult(Some(tc_id), func_name, result, new_dir.clone()));
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
                                
                                // Robust detection of whether we are currently inside a special block
                                let last_tool_open = full_response_content.rfind("<|tool_call").or_else(|| full_response_content.rfind("<tool_call"));
                                let last_tool_close = full_response_content.rfind("<|tool_call|>").or_else(|| full_response_content.rfind("<tool_call|>"));
                                let is_in_tool_call = last_tool_open.is_some() && (last_tool_close.is_none() || last_tool_open > last_tool_close);

                                let last_thought_open = full_response_content.rfind("<|channel>thought")
                                    .or_else(|| full_response_content.rfind("<thought>"))
                                    .or_else(|| full_response_content.rfind("<think>"));
                                let last_thought_close = full_response_content.rfind("<channel|>")
                                    .or_else(|| full_response_content.rfind("</thought>"))
                                    .or_else(|| full_response_content.rfind("</think>"));
                                
                                // In Gemma 4, the response starts with the thought channel by default.
                                // If we haven't seen any close tags yet, and we aren't in a tool call, we are thinking.
                                let is_in_thought = if let Some(close_pos) = last_thought_close {
                                    // If we saw a close, we are only in thought if a new one opened AFTER that close
                                    last_thought_open.is_some() && last_thought_open.unwrap() > close_pos
                                } else {
                                    // No close tag seen yet. If we also haven't seen a tool call start, 
                                    // assume we are in the default initial thought channel.
                                    last_tool_open.is_none()
                                };

                                let b_type = if is_in_tool_call {
                                    BlockType::Formulating
                                } else if is_in_thought {
                                    BlockType::Thought
                                } else if full_response_content.contains("```") {
                                    BlockType::Markdown
                                } else {
                                    BlockType::Text
                                };
                                
                                app.add_segment(chunk, b_type);

                                let is_complete = last_tool_close.is_some() && (last_tool_open.is_none() || last_tool_close > last_tool_open);
                                if is_complete && !app.tool_calls_processed_this_request {
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
                            let success = if result.contains("EXIT_CODE: ") { result.contains("EXIT_CODE: 0") } else { true };
                            
                            app.current_dir = new_dir;
                            let tc_id_str = id.clone().unwrap_or_else(|| "unknown".to_string());
                            result = handle_large_output(&tc_id_str, result);
                            
                            app.add_segment(format!("\n{} [TOOL RESULT]\n{}\n", icons::SUCCESS, result), BlockType::ToolResult);
                            if let Some(last) = app.blocks.last_mut() { last.success = Some(success); }
                            if let Some(tc_id) = id { app.context_manager.add_tool_message(tc_id, &func_name, &result); }
                            
                            app.is_processing = true;
                            app.tool_calls_processed_this_request = false;
                            app.tool_call_pos = None;
                            full_response_content.clear();
                            cancellation_token = CancellationToken::new();
                            app.request_start_time = Some(tokio::time::Instant::now());
                            app.context_manager.set_cwd(app.current_dir.clone());
                            trigger_llm_request(client.clone(), config.clone(), &app.context_manager, tx.clone(), cancellation_token.clone(), app.show_debug, app.current_session_dir.clone());
                        }
                        StreamEvent::Done(eval_count, eval_duration) => {
                            app.is_processing = false;
                            if !app.tool_calls_processed_this_request {
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
                                    tokio::spawn(async move {
                                        let (result, new_dir) = match func_name.as_str() {
                                            "run_shell_command" => {
                                                let cmd = args["command"].as_str().unwrap_or("");
                                                tool_executor::execute_shell(cmd, &current_dir, tool_cancel).await
                                            },
                                            "read_folder" => {
                                                let path = args["path"].as_str().unwrap_or(".");
                                                (tool_executor::execute_read_folder(path, &current_dir).await, current_dir.clone())
                                            },
                                            "read_file_lines" => {
                                                let path = args["path"].as_str().unwrap_or("");
                                                let start = args["start_line"].as_u64().unwrap_or(1) as usize;
                                                let end = args["end_line"].as_u64().unwrap_or(1) as usize;
                                                (tool_executor::execute_read_file_lines(path, start, end, &current_dir).await, current_dir.clone())
                                            },
                                            "search_text" => {
                                                let pattern = args["pattern"].as_str().unwrap_or("");
                                                let path = args["path"].as_str().unwrap_or(".");
                                                (tool_executor::execute_search_text(pattern, path, &current_dir).await, current_dir.clone())
                                            },
                                            "apply_patch" => {
                                                let path = args["path"].as_str().unwrap_or("");
                                                let patch = args["patch"].as_str().unwrap_or("");
                                                (tool_executor::execute_apply_patch(path, patch, &current_dir).await, current_dir.clone())
                                            },
                                            "replace_text" => {
                                                let path = args["path"].as_str().unwrap_or("");
                                                let old_string = args["old_string"].as_str().unwrap_or("");
                                                let new_string = args["new_string"].as_str().unwrap_or("");
                                                (tool_executor::execute_replace_text(path, old_string, new_string, &current_dir).await, current_dir.clone())
                                            },
                                            "write_file" => {
                                                let path = args["path"].as_str().unwrap_or("");
                                                let content = args["content"].as_str().unwrap_or("");
                                                (tool_executor::execute_write_file(path, content, &current_dir).await, current_dir.clone())
                                            },
                                            "web_fetch" => {
                                                let url = args["url"].as_str().unwrap_or("");
                                                (tool_executor::execute_web_fetch(url).await, current_dir.clone())
                                            },
                                            "calculate" => (format!("Calculation result for: {}", args["expression"]), current_dir.clone()),
                                            _ => (format!("Unknown tool: {}", func_name), current_dir.clone()),
                                        };
                                        let _ = ctx_tx.send(StreamEvent::ToolResult(Some(tc_id), func_name, result, new_dir.clone()));
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
