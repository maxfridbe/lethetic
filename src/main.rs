use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyEventKind},
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
    let config_content = fs::read_to_string("config.yml")?;
    let config: Config = serde_yaml::from_str(&config_content)?;

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

async fn run_app(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App, config: &Config) -> Result<(), Box<dyn Error>> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let client = Client::new();
    let mut cancellation_token = CancellationToken::new();
    let mut reader = EventStream::new();
    let mut sys = System::new_all();
    
    let mut last_tick = std::time::Instant::now();
    let mut last_stats_update = std::time::Instant::now();
    let mut full_response_content = String::new();

    app.refresh_system_stats();
    app.git_status = get_git_info().await;

    loop {
        if app.should_redraw {
            terminal.draw(|f| ui(f, app))?;
            app.should_redraw = false;
        }

        let timeout = Duration::from_millis(50);
        
        tokio::select! {
            Some(event) = reader.next() => {
                if let Ok(Event::Key(key)) = event {
                    if key.kind == KeyEventKind::Press {
                        match handle_key(app, key) {
                            AppEventOutcome::Exit => return Ok(()),
                            AppEventOutcome::SendPrompt(prompt) => {
                                app.add_segment(format!("\n{} User: {}\n", icons::INPUT, prompt), BlockType::User);
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
                                            let _ = ctx_tx.send(StreamEvent::ToolResult(Some(tc_id), result));
                                        });
                                        app.is_processing = true;
                                    } else {
                                        app.add_segment(format!("\n{} Tool execution denied by user.\n", icons::WARNING), BlockType::Text);
                                    }
                                }
                                app.show_approval_prompt = false;
                            }
                            AppEventOutcome::Stop => {
                                cancellation_token.cancel();
                                app.is_processing = false;
                                app.add_segment(format!("\n{} [STOPPED]\n", icons::WARNING), BlockType::Text);
                                // Drain the channel to ensure no stale events are processed
                                while let Ok(_) = rx.try_recv() {}
                            }
                            AppEventOutcome::Continue => {}
                        }
                    }
                }
            }
            Some(stream_event) = rx.recv() => {
                match stream_event {
                    StreamEvent::DebugLog(msg) => {
                        app.log_debug(&msg);
                    }
                    StreamEvent::Chunk(chunk) => {
                        if !app.is_processing { continue; }
                        full_response_content.push_str(&chunk);
                        let b_type = if chunk.contains("<thought>") || full_response_content.contains("<thought>") && !full_response_content.contains("</thought>") {
                            BlockType::Thought
                        } else if markdown::sniff_for_markdown(&chunk) || full_response_content.contains("```") {
                            BlockType::Markdown
                        } else {
                            BlockType::Text
                        };
                        app.add_segment(chunk, b_type);

                        // Sniff for manual tool calls (XML, native, or JSON-in-tags)
                        let found_tc = parser::find_tool_call(&full_response_content);
                        if found_tc.is_some() {
                            app.log_debug(&format!("Found manual tool call! processed={}", app.tool_calls_processed_this_request));
                        }
                        if let Some((tc, pos)) = found_tc {
                            if !app.tool_calls_processed_this_request {
                                handle_tool_call(app, vec![tc], pos, tx.clone(), &mut cancellation_token, &full_response_content, false);
                            }
                        }
                    }
                    StreamEvent::ToolCalls(calls) => {
                        if !app.tool_calls_processed_this_request {
                            handle_tool_call(app, calls, full_response_content.len(), tx.clone(), &mut cancellation_token, &full_response_content, true);
                        } else {
                            app.log_debug("Ignoring native tool calls because a manual one was already processed.");
                        }
                    }
                    StreamEvent::ToolResult(id, result) => {
                        app.add_segment(format!("\n{} [TOOL RESULT]\n{}\n", icons::SUCCESS, result), BlockType::ToolResult);
                        if let Some(tc_id) = id {
                            app.context_manager.add_tool_result(tc_id, &result);
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
                        
                        // Finalize the assistant message in context
                        let mut content_to_store = full_response_content.clone();
                        let calls = if app.tool_calls_processed_this_request {
                            // Truncate the content to remove the tool call part from the text context
                            if let Some(pos) = app.tool_call_pos {
                                content_to_store.truncate(pos);
                            }
                            app.pending_tool_call.as_ref().map(|tc| vec![tc.clone()])
                        } else {
                            None
                        };
                        app.context_manager.add_assistant_tool_call(&content_to_store, calls);

                        if let (Some(count), Some(duration)) = (eval_count, eval_duration) {
                            app.tokens_per_s = (count as f64 / (duration as f64 / 1_000_000_000.0)).max(0.0);
                        }
                        app.log_debug("LLM Response Done.");

                        // If we had an intercepted tool call and it's set to Always Allow, trigger it now.
                        if app.tool_calls_processed_this_request && app.shell_approval_mode == ApprovalMode::Always {
                            if let Some(tool_call) = app.pending_tool_call.take() {
                                let tc_id = tool_call.id.clone();
                                let func_name = tool_call.function.name.clone();
                                let args = tool_call.function.arguments.clone();
                                app.log_debug(&format!("TOOL ALLOWED (AUTO): {}", func_name));
                                
                                let ctx_tx = tx.clone();
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
                                    let _ = ctx_tx.send(StreamEvent::ToolResult(Some(tc_id), result));
                                });
                                app.is_processing = true;
                            }
                        }
                    }
                    StreamEvent::Error(e) => {
                        app.is_processing = false;
                        app.add_segment(format!("\n{} ERROR: {}\n", icons::WARNING, e), BlockType::Text);
                    }
                }
            }
            _ = tokio::time::sleep(timeout) => {
                if app.is_processing && last_tick.elapsed() >= Duration::from_millis(100) {
                    app.tick_spinner();
                    last_tick = std::time::Instant::now();
                }
                if last_stats_update.elapsed() >= Duration::from_secs(2) {
                    sys.refresh_memory();
                    app.memory_usage = sys.used_memory() / 1024 / 1024;
                    app.git_status = get_git_info().await;
                    last_stats_update = std::time::Instant::now();
                }
            }
        }
    }
}
