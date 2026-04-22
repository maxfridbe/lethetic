use std::env;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyEventKind, MouseEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures_util::StreamExt;
use ratatui::{
    backend::CrosstermBackend,
    Terminal,
};
use reqwest::Client;
use std::{error::Error, fs, io, time::Duration};
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = env::args().collect();
    let config_content = fs::read_to_string("config.yml")?;
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
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(&config);
    let res = run_app(&mut terminal, &mut app, &config).await;

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
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
    
    app.context_manager.add_message("user", &prompt);
    println!("\n{} User: {}\n", icons::INPUT, prompt);
    
    let mut full_response_content = String::new();
    trigger_llm_request(client.clone(), config.clone(), &app.context_manager, tx.clone(), cancellation_token.clone(), false);

    loop {
        match rx.recv().await {
            Some(StreamEvent::Chunk(chunk)) => {
                print!("{}", chunk);
                io::Write::flush(&mut io::stdout())?;
                full_response_content.push_str(&chunk);

                let has_start = full_response_content.contains("<|tool_call>") || full_response_content.contains("<tool_call>");
                let has_end = full_response_content.contains("<|tool_call|>") 
                    || full_response_content.contains("<tool_call|>")
                    || (full_response_content.contains('}') && (full_response_content.contains("<|channel>") || full_response_content.contains("<channel|>")));

                if has_start && has_end {
                    if let Some((tc, _)) = parser::find_tool_call(&full_response_content, true) {
                        println!("\n\n{} [TOOL CALL: {}]", icons::COMMAND, tc.function.name);
                        println!("Arguments: {}", tc.function.arguments);
                        cancellation_token.cancel();
                        
                        let assistant_content = full_response_content.clone();
                        app.context_manager.add_message("assistant", &assistant_content);

                        let result = match tc.function.name.as_str() {
                            "run_shell_command" => {
                                let cmd = tc.function.arguments["command"].as_str().unwrap_or("");
                                tool_executor::execute_shell(cmd).await
                            },
                            "read_file_lines" => {
                                let path = tc.function.arguments["path"].as_str().unwrap_or("");
                                let start = tc.function.arguments["start_line"].as_u64().unwrap_or(1) as usize;
                                let end = tc.function.arguments["end_line"].as_u64().unwrap_or(1) as usize;
                                tool_executor::execute_read_file_lines(path, start, end).await
                            },
                            "apply_patch" => {
                                let path = tc.function.arguments["path"].as_str().unwrap_or("");
                                let patch = tc.function.arguments["patch"].as_str().unwrap_or("");
                                tool_executor::execute_apply_patch(path, patch).await
                            },
                            "calculate" => format!("Calculation result for: {}", tc.function.arguments["expression"]),
                            _ => format!("Unknown tool: {}", tc.function.name),
                        };
                        
                        println!("\n{} [TOOL RESULT]\n{}\n", icons::SUCCESS, result);
                        app.context_manager.add_tool_message(tc.id.clone(), &tc.function.name, &result);
                        
                        full_response_content.clear();
                        cancellation_token = CancellationToken::new();
                        trigger_llm_request(client.clone(), config.clone(), &app.context_manager, tx.clone(), cancellation_token.clone(), false);
                        continue; 
                    }
                }
            }
            Some(StreamEvent::Done(_, _)) => {
                if let Some((tc, _)) = parser::find_tool_call(&full_response_content, true) {
                    println!("\n\n{} [TOOL CALL: {}]", icons::COMMAND, tc.function.name);
                    println!("Arguments: {}", tc.function.arguments);
                    
                    let assistant_content = full_response_content.clone();
                    app.context_manager.add_message("assistant", &assistant_content);

                    let result = match tc.function.name.as_str() {
                        "run_shell_command" => {
                            let cmd = tc.function.arguments["command"].as_str().unwrap_or("");
                            tool_executor::execute_shell(cmd).await
                        },
                        "read_file_lines" => {
                            let path = tc.function.arguments["path"].as_str().unwrap_or("");
                            let start = tc.function.arguments["start_line"].as_u64().unwrap_or(1) as usize;
                            let end = tc.function.arguments["end_line"].as_u64().unwrap_or(1) as usize;
                            tool_executor::execute_read_file_lines(path, start, end).await
                        },
                        "apply_patch" => {
                            let path = tc.function.arguments["path"].as_str().unwrap_or("");
                            let patch = tc.function.arguments["patch"].as_str().unwrap_or("");
                            tool_executor::execute_apply_patch(path, patch).await
                        },
                        "calculate" => format!("Calculation result for: {}", tc.function.arguments["expression"]),
                        _ => format!("Unknown tool: {}", tc.function.name),
                    };
                    
                    println!("\n{} [TOOL RESULT]\n{}\n", icons::SUCCESS, result);
                    app.context_manager.add_tool_message(tc.id.clone(), &tc.function.name, &result);
                    
                    full_response_content.clear();
                    cancellation_token = CancellationToken::new();
                    trigger_llm_request(client.clone(), config.clone(), &app.context_manager, tx.clone(), cancellation_token.clone(), false);
                    continue;
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
    let mut full_response_content = String::new();

    let stats_tx = tx.clone();
    tokio::spawn(async move {
        let mut sys = System::new_all();
        loop {
            sys.refresh_memory();
            let mem = sys.used_memory() / 1024 / 1024;
            let git = get_git_info().await;
            let _ = stats_tx.send(StreamEvent::DebugLog(format!("STATS|{}|{}", mem, git)));
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });

    app.refresh_system_stats();

    loop {
        if app.should_redraw {
            terminal.draw(|f| ui(f, app))?;
            app.should_redraw = false;
        }

        let timeout = Duration::from_millis(16);
        
        tokio::select! {
            Some(event) = reader.next() => {
                match event {
                    Ok(Event::Key(key)) => {
                        if key.kind == KeyEventKind::Press {
                            match handle_key(app, key) {
                                AppEventOutcome::Exit => return Ok(()),
                                AppEventOutcome::ToggleMouse => {
                                    app.mouse_enabled = !app.mouse_enabled;
                                    if app.mouse_enabled {
                                        execute!(io::stdout(), EnableMouseCapture)?;
                                    } else {
                                        execute!(io::stdout(), DisableMouseCapture)?;
                                    }
                                    app.should_redraw = true;
                                }
                                AppEventOutcome::SendPrompt(prompt) => {
                                    app.add_segment(prompt.clone(), BlockType::User);
                                    app.context_manager.add_message("user", &prompt);
                                    app.is_processing = true;
                                    app.tool_calls_processed_this_request = false;
                                    app.tool_call_pos = None;
                                    full_response_content.clear();
                                    cancellation_token = CancellationToken::new();
                                    trigger_llm_request(client.clone(), config.clone(), &app.context_manager, tx.clone(), cancellation_token.clone(), app.show_debug);
                                }
                                AppEventOutcome::ToolApproved(approved, always) => {
                                    if let Some(tool_call) = app.pending_tool_call.take() {
                                        if approved {
                                            if always { app.shell_approval_mode = ApprovalMode::Always; }
                                            let tc_id = tool_call.id.clone();
                                            let func_name = tool_call.function.name.clone();
                                            let args = tool_call.function.arguments.clone();
                                            app.log_debug(&format!("TOOL ALLOWED: {}", func_name));
                                            
                                            let ctx_tx = tx.clone();
                                            let func_name_clone = func_name.clone();
                                            tokio::spawn(async move {
                                                let result = match func_name.as_str() {
                                                    "run_shell_command" => {
                                                        let cmd = args["command"].as_str().unwrap_or("");
                                                        tool_executor::execute_shell(cmd).await
                                                    },
                                                    "read_file_lines" => {
                                                        let path = args["path"].as_str().unwrap_or("");
                                                        let start = args["start_line"].as_u64().unwrap_or(1) as usize;
                                                        let end = args["end_line"].as_u64().unwrap_or(1) as usize;
                                                        tool_executor::execute_read_file_lines(path, start, end).await
                                                    },
                                                    "apply_patch" => {
                                                        let path = args["path"].as_str().unwrap_or("");
                                                        let patch = args["patch"].as_str().unwrap_or("");
                                                        tool_executor::execute_apply_patch(path, patch).await
                                                    },
                                                    "calculate" => format!("Calculation result for: {}", args["expression"]),
                                                    _ => format!("Unknown tool: {}", func_name),
                                                };
                                                let _ = ctx_tx.send(StreamEvent::ToolResult(Some(tc_id), func_name_clone, result));
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
                                AppEventOutcome::Continue => {
                                    app.should_redraw = true;
                                }
                            }
                        }
                    }
                    Ok(Event::Mouse(mouse)) => {
                        if app.mouse_enabled {
                            match mouse.kind {
                                MouseEventKind::ScrollDown => {
                                    app.scroll_output_down();
                                    app.should_redraw = true;
                                }
                                MouseEventKind::ScrollUp => {
                                    app.scroll_output_up();
                                    app.should_redraw = true;
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            Some(stream_event) = rx.recv() => {
                match stream_event {
                    StreamEvent::DebugLog(msg) => {
                        if msg.starts_with("STATS|") {
                            let parts: Vec<&str> = msg.split('|').collect();
                            if parts.len() == 3 {
                                app.memory_usage = parts[1].parse().unwrap_or(0);
                                app.git_status = parts[2].to_string();
                                app.should_redraw = true;
                            }
                        } else {
                            app.log_debug(&msg);
                        }
                    }
                    StreamEvent::Chunk(chunk) => {
                        if !app.is_processing { continue; }
                        full_response_content.push_str(&chunk);
                        
                        let is_in_thought = (full_response_content.contains("<thought>") && !full_response_content.contains("</thought>"))
                            || (full_response_content.contains("<|channel>thought") && !full_response_content.contains("<channel|>"));

                        let b_type = if is_in_thought || chunk.contains("<thought>") || chunk.contains("<|channel>thought") {
                            BlockType::Thought
                        } else if full_response_content.contains("```") {
                            BlockType::Markdown
                        } else {
                            BlockType::Text
                        };
                        app.add_segment(chunk, b_type);

                        let is_complete = full_response_content.contains("<|tool_call|>") 
                            || full_response_content.contains("<tool_call|>");

                        if is_complete && !app.tool_calls_processed_this_request {
                            if let Some((tc, pos)) = parser::find_tool_call(&full_response_content, true) {
                                app.log_debug(&format!("Found complete manual tool call at pos {}!", pos));
                                handle_tool_call(app, vec![tc], pos, tx.clone(), &mut cancellation_token, &full_response_content, false);
                            }
                        } else if full_response_content.contains("<tool_call|>") || full_response_content.contains("<|tool_call|>") {
                            if full_response_content.matches("<tool_call|>").count() > 3 || full_response_content.matches("<|tool_call|>").count() > 3 {
                                app.log_debug("Panic stop: excessive control tokens detected.");
                                cancellation_token.cancel();
                                app.is_processing = false;
                                app.add_segment("\n[PANIC STOP] Model is looping control tokens.\n".to_string(), BlockType::Text);
                            }
                        }
                    }
                    StreamEvent::ToolCalls(calls) => {
                        if !app.tool_calls_processed_this_request {
                            handle_tool_call(app, calls, full_response_content.len(), tx.clone(), &mut cancellation_token, &full_response_content, true);
                        }
                    }
                    StreamEvent::ToolResult(id, func_name, result) => {
                        let success = if result.contains("EXIT_CODE: ") {
                            result.contains("EXIT_CODE: 0")
                        } else {
                            true
                        };

                        app.add_segment(format!("\n{} [TOOL RESULT]\n{}\n", icons::SUCCESS, result), BlockType::ToolResult);
                        if let Some(last) = app.blocks.last_mut() {
                            last.success = Some(success);
                        }
                        
                        if let Some(tc_id) = id {
                            app.context_manager.add_tool_message(tc_id, &func_name, &result);
                        }
                        
                        app.is_processing = true;
                        app.tool_calls_processed_this_request = false;
                        app.tool_call_pos = None;
                        full_response_content.clear();
                        cancellation_token = CancellationToken::new();
                        trigger_llm_request(client.clone(), config.clone(), &app.context_manager, tx.clone(), cancellation_token.clone(), app.show_debug);
                    }
                    StreamEvent::Done(eval_count, eval_duration) => {
                        app.is_processing = false;
                        
                        if !app.tool_calls_processed_this_request {
                            if let Some((tc, pos)) = parser::find_tool_call(&full_response_content, true) {
                                app.log_debug(&format!("Found manual tool call on Done at pos {}!", pos));
                                handle_tool_call(app, vec![tc], pos, tx.clone(), &mut cancellation_token, &full_response_content, false);
                            }
                        }

                        if !app.tool_calls_processed_this_request {
                            let messages = app.context_manager.get_messages();
                            let last_role = messages.last().map(|m| m.role.as_str());
                            if last_role != Some("assistant") {
                                app.context_manager.add_message("assistant", &full_response_content);
                            }
                        }

                        if let (Some(count), Some(duration)) = (eval_count, eval_duration) {
                            app.tokens_per_s = (count as f64 / (duration as f64 / 1_000_000_000.0)).max(0.0);
                        }
                        app.log_debug("LLM Response Done.");
                        app.should_redraw = true;

                        if app.tool_calls_processed_this_request && app.shell_approval_mode == ApprovalMode::Always {
                            if let Some(tool_call) = app.pending_tool_call.take() {
                                let tc_id = tool_call.id.clone();
                                let func_name = tool_call.function.name.clone();
                                let args = tool_call.function.arguments.clone();
                                app.log_debug(&format!("TOOL ALLOWED (AUTO): {}", func_name));
                                
                                let messages = app.context_manager.get_messages();
                                let last_role = messages.last().map(|m| m.role.as_str());
                                if last_role != Some("assistant") {
                                    app.context_manager.add_message("assistant", &full_response_content);
                                }

                                let ctx_tx = tx.clone();
                                let func_name_clone = func_name.clone();
                                tokio::spawn(async move {
                                    let result = match func_name.as_str() {
                                        "run_shell_command" => {
                                            let cmd = args["command"].as_str().unwrap_or("");
                                            tool_executor::execute_shell(cmd).await
                                        },
                                        "read_file_lines" => {
                                            let path = args["path"].as_str().unwrap_or("");
                                            let start = args["start_line"].as_u64().unwrap_or(1) as usize;
                                            let end = args["end_line"].as_u64().unwrap_or(1) as usize;
                                            tool_executor::execute_read_file_lines(path, start, end).await
                                        },
                                        "apply_patch" => {
                                            let path = args["path"].as_str().unwrap_or("");
                                            let patch = args["patch"].as_str().unwrap_or("");
                                            tool_executor::execute_apply_patch(path, patch).await
                                        },
                                        "calculate" => format!("Calculation result for: {}", args["expression"]),
                                        _ => format!("Unknown tool: {}", func_name),
                                    };
                                    let _ = ctx_tx.send(StreamEvent::ToolResult(Some(tc_id), func_name_clone, result));
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
