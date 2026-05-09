use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::client::{trigger_llm_request, StreamEvent};
use crate::config::Config;
use crate::context::ContextManager;
use crate::parser::{self, StreamParser};
use crate::system_prompt::SystemPromptManager;
use crate::tools;

/// Run a self-contained agent loop with the given prompt.
///
/// If `progress_tx` is Some, ToolProgress events from tool execution are
/// forwarded so the parent TUI session can show sub-agent activity.
/// Returns the final assistant text response.
pub async fn run_agent(
    prompt: String,
    client: &reqwest::Client,
    config: &Config,
    print_output: bool,
    progress_tx: Option<mpsc::UnboundedSender<StreamEvent>>,
) -> Result<String, String> {
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| ".".to_string());

    // Build system prompt excluding task + ask_the_user (sub-agent must not recurse or pause)
    let spm = SystemPromptManager::new();
    let template = spm.load_prompt("software_engineer")
        .unwrap_or_else(|| crate::system_prompt::DEFAULT_PROMPT_TEMPLATE.to_string());
    let tool_decls = tools::get_prompt_templates_excluding(config, &["task", "ask_the_user"]);
    let resolved = template
        .replace("[TOOLS_DEFINITIONS]", &tool_decls)
        .replace("[CWD]", &cwd);

    let mut context = ContextManager::new(config.context_size, Some(resolved));
    context.set_cwd(cwd.clone());
    context.add_message("user", &prompt);

    let mut parser = StreamParser::new();
    let mut current_dir = cwd;
    let mut full_response = String::new();

    let (tx, mut rx) = mpsc::unbounded_channel::<StreamEvent>();
    let mut cancel = CancellationToken::new();

    trigger_llm_request(
        client.clone(), config.clone(), &context,
        tx.clone(), cancel.clone(), false, None,
    );

    loop {
        match rx.recv().await {
            Some(StreamEvent::Chunk(chunk)) => {
                if print_output {
                    print!("{}", chunk);
                    let _ = std::io::Write::flush(&mut std::io::stdout());
                }
                full_response.push_str(&chunk);
                parser.parse_chunk(&chunk);

                if parser.state == crate::parser::ParserState::Text {
                    match parser::find_tool_call(&full_response, false) {
                        Some(Ok((tc, _))) => {
                            cancel.cancel();
                            context.add_message("assistant", &full_response);
                            let func_name = tc.function.name.clone();
                            let tc_id = tc.id.clone();
                            let tool_tx = progress_tx.as_ref().map(|p| p.clone()).unwrap_or_else(|| tx.clone());
                            let (result, new_dir) = tools::execute(
                                &func_name, &tc.function.arguments,
                                &current_dir, CancellationToken::new(),
                                tool_tx, client, config,
                            ).await;
                            let (result, _) = tools::handle_large_output(&tc_id, result);
                            current_dir = new_dir;
                            context.add_tool_message(tc_id, &func_name, &result);
                            full_response.clear();
                            cancel = CancellationToken::new();
                            context.set_cwd(current_dir.clone());
                            parser.reset();
                            trigger_llm_request(
                                client.clone(), config.clone(), &context,
                                tx.clone(), cancel.clone(), false, None,
                            );
                        }
                        Some(Err((err, _))) => {
                            cancel.cancel();
                            context.add_message("assistant", &full_response);
                            context.add_tool_message("err".to_string(), "syntax_error",
                                &format!("Syntax error: {}", err));
                            full_response.clear();
                            cancel = CancellationToken::new();
                            parser.reset();
                            trigger_llm_request(
                                client.clone(), config.clone(), &context,
                                tx.clone(), cancel.clone(), false, None,
                            );
                        }
                        None => {}
                    }
                }
            }
            Some(StreamEvent::ToolProgress(msg)) => {
                if print_output {
                    print!("\r[…] {}          ", msg.replace('\n', " | "));
                    let _ = std::io::Write::flush(&mut std::io::stdout());
                }
                if let Some(ref ptx) = progress_tx {
                    let _ = ptx.send(StreamEvent::ToolProgress(msg));
                }
            }
            Some(StreamEvent::Done { .. }) => {
                if print_output {
                    print!("\r{:60}\r", "");
                }
                // Check for a tool call that arrived at Done
                if parser.state == crate::parser::ParserState::Text
                    || parser.state == crate::parser::ParserState::ToolCall
                {
                    match parser::find_tool_call(&full_response, true) {
                        Some(Ok((tc, _))) => {
                            cancel.cancel();
                            cancel = CancellationToken::new();
                            context.add_message("assistant", &full_response);
                            let func_name = tc.function.name.clone();
                            let tc_id = tc.id.clone();
                            let tool_tx = progress_tx.as_ref().map(|p| p.clone()).unwrap_or_else(|| tx.clone());
                            let (result, new_dir) = tools::execute(
                                &func_name, &tc.function.arguments,
                                &current_dir, cancel.clone(),
                                tool_tx, client, config,
                            ).await;
                            let (result, _) = tools::handle_large_output(&tc_id, result);
                            current_dir = new_dir;
                            context.add_tool_message(tc_id, &func_name, &result);
                            full_response.clear();
                            cancel = CancellationToken::new();
                            context.set_cwd(current_dir.clone());
                            parser.reset();
                            trigger_llm_request(
                                client.clone(), config.clone(), &context,
                                tx.clone(), cancel.clone(), false, None,
                            );
                            continue;
                        }
                        _ => {}
                    }
                }

                if !full_response.is_empty() {
                    let last = context.get_messages().last().map(|m| m.role.clone());
                    if last.as_deref() != Some("assistant") {
                        context.add_message("assistant", &full_response);
                    }
                }

                // Stop if we have a real assistant response (not just a tool result)
                let last = context.get_messages().last().map(|m| m.role.clone());
                if last.as_deref() == Some("assistant") && !full_response.is_empty() {
                    return Ok(full_response);
                }
                if last.as_deref() == Some("tool") {
                    continue;
                }
                return Ok(full_response);
            }
            Some(StreamEvent::Error(e)) => return Err(e),
            None => break,
            _ => {}
        }
    }

    Ok(full_response)
}
