
//! Vercel-style prompt building and message formatting.
//!
//! This module provides functions to format conversation history, system prompt,
//! and files in context using standard Markdown block format rather than proprietary
//! XML-like tags. This aligns with OpenCode TypeScript Vercel-style structures.

use crate::context::{ContextManager, sanitize_file_content, format_gemma4_call};

/// Formats the context into a single raw prompt string.
///
/// This constructs a raw text prompt for turn-based generation, using
/// Markdown headers for active and latest files instead of XML tags.
pub fn get_raw_prompt_vercel(ctx: &ContextManager) -> String {
    let mut prompt = String::from("<bos>");

    // ── 1. Latest files (background context) ──
    if !ctx.latest_files.is_empty() {
        prompt.push_str("<|turn>system\n<|think|>\n");
        for (path, cached_file) in &ctx.latest_files {
            prompt.push_str(&format!("## File: {}\n```\n{}\n```\n",
                path, sanitize_file_content(&cached_file.content)));
        }
        prompt.push_str("<turn|>\n");
    }

    // ── 2. System prompt ──
    if let Some(sys) = &ctx.system_prompt {
        prompt.push_str("<|turn>system\n<|think|>\n");
        prompt.push_str(sys);
        prompt.push_str("<turn|>\n");
    }

    // ── 3. Message history ──
    let mut current_turn_role = String::new();

    for msg in &ctx.messages {
        match msg.role.as_str() {
            "system" => {
                if current_turn_role == "model" { prompt.push_str("<turn|>\n"); }
                prompt.push_str("<|turn>system\n<|think|>\n");
                prompt.push_str(&msg.content);
                prompt.push_str("<turn|>\n");
                current_turn_role = String::new();
            }
            "user" => {
                if current_turn_role == "model" { prompt.push_str("<turn|>\n"); }
                prompt.push_str("<|turn>user\n");
                prompt.push_str(&msg.content);
                prompt.push_str("<turn|>\n");
                current_turn_role = String::new();
            }
            "assistant" => {
                if current_turn_role != "model" { prompt.push_str("<|turn>model\n"); }
                let mut clean_content = msg.content.clone();
                for (start_tag, end_tag) in [
                    ("<|channel>thought", "<channel|>"),
                    ("<thought>", "</thought>"),
                    ("<think>", "</think>"),
                ] {
                    while let Some(start_idx) = clean_content.find(start_tag) {
                        if let Some(end_idx_rel) = clean_content[start_idx..].find(end_tag) {
                            let end_pos = start_idx + end_idx_rel + end_tag.len();
                            clean_content.replace_range(start_idx..end_pos, "");
                        } else {
                            clean_content.truncate(start_idx);
                            break;
                        }
                    }
                }
                clean_content = clean_content.replace("<|channel>text\n", "").replace("<|channel>text", "");
                if msg.tool_calls.is_some()
                    && let Some(idx) = clean_content.find("<|tool_call>") {
                        clean_content.truncate(idx);
                    }
                prompt.push_str(clean_content.trim());
                prompt.push('\n');
                if let Some(calls) = &msg.tool_calls {
                    for tc in calls {
                        let call_str = format_gemma4_call(&tc.function.name, &tc.function.arguments);
                        prompt.push_str(&format!("<|tool_call>{}<tool_call|>", call_str));
                    }
                    current_turn_role = "model".to_string();
                } else if msg.content.contains("<|tool_call>") {
                    current_turn_role = "model".to_string();
                } else {
                    prompt.push_str("<turn|>\n");
                    current_turn_role = String::new();
                }
            }
            "tool" => {
                if current_turn_role != "model" { prompt.push_str("<|turn>model\n"); }
                prompt.push_str(&msg.content);
                current_turn_role = "model".to_string();
            }
            _ => {}
        }
    }

    // ── 4. Active file ──
    if !ctx.active_files.is_empty() {
        if current_turn_role == "model" { prompt.push_str("<turn|>\n"); }
        prompt.push_str("<|turn>system\n<|think|>\n");
        for (path, cached_file) in &ctx.active_files {
            prompt.push_str(&format!("## File: {}\n```\n{}\n```\n",
                path, sanitize_file_content(&cached_file.content)));
        }
        prompt.push_str("<turn|>\n");
        current_turn_role = String::new();
    }

    // ── 5. Start model generation ──
    if current_turn_role != "model" {
        prompt.push_str("<|turn>model\n");
    }

    prompt
}

/// Formats the context into a list of messages for the LLM API.
///
/// This combines the system prompt and all files into a single system message
/// at index 0, followed by the conversation turns.
pub fn get_messages_for_api_vercel(ctx: &ContextManager) -> Vec<gemma_chat::Message> {
    let mut msgs = Vec::new();

    // ── 1. System prompt (if any) ──
    let mut system_content = String::new();
    if let Some(sys) = &ctx.system_prompt {
        let mut clean = sys.clone();
        crate::context::strip_delimited(&mut clean, "<|tool>", "<tool|>");
        clean = clean.replace("<|think|>", "").replace("<|turn>", "").replace("<turn|>", "");
        let clean = clean.trim().to_string();
        if !clean.is_empty() {
            system_content.push_str(&clean);
        }
    }

    // ── 2. Add files to the system content ──
    let mut files_content = String::new();

    // Latest files
    if !ctx.latest_files.is_empty() {
        for path in ctx.latest_files.keys() {
            if !files_content.is_empty() {
                files_content.push_str("\n\n");
            }
            let abs = std::path::Path::new(&ctx.cwd).join(path);
            let body = match std::fs::read_to_string(&abs) {
                Ok(c)  => format!("```\n{}\n```", sanitize_file_content(&c)),
                Err(_) => format!("⚠ File `{}` was deleted or no longer exists on disk.", path),
            };
            files_content.push_str(&format!("## File: {}\n{}", path, body));
        }
    }

    // Active files
    if !ctx.active_files.is_empty() {
        for path in ctx.active_files.keys() {
            if !files_content.is_empty() {
                files_content.push_str("\n\n");
            }
            let abs = std::path::Path::new(&ctx.cwd).join(path);
            let body = match std::fs::read_to_string(&abs) {
                Ok(c)  => format!("```\n{}\n```", sanitize_file_content(&c)),
                Err(_) => format!("⚠ File `{}` was deleted or no longer exists on disk.", path),
            };
            files_content.push_str(&format!("## File: {}\n{}", path, body));
        }
    }

    if !files_content.is_empty() {
        if !system_content.is_empty() {
            system_content.push_str("\n\n");
        }
        system_content.push_str(&files_content);
    }

    if !system_content.is_empty() {
        msgs.push(gemma_chat::Message::system(system_content));
    }

    // ── 3. Message history ──
    for msg in &ctx.messages {
        match msg.role.as_str() {
            "system" => msgs.push(gemma_chat::Message::system(msg.content.clone())),
            "user"   => msgs.push(gemma_chat::Message::user(msg.content.clone())),
            "assistant" => {
                let clean = ctx.strip_thinking(&msg.content);
                if let Some(calls) = &msg.tool_calls {
                    let gc: Vec<gemma_chat::AssistantToolCall> = calls.iter().map(|tc| gemma_chat::AssistantToolCall {
                        id: tc.id.clone(),
                        kind: "function".into(),
                        function: gemma_chat::FunctionCall {
                            name: tc.function.name.clone(),
                            arguments: serde_json::to_string(&tc.function.arguments).unwrap_or_default(),
                        },
                    }).collect();
                    msgs.push(gemma_chat::Message::assistant_with_tools(clean, gc, None));
                } else {
                    msgs.push(gemma_chat::Message::assistant(clean));
                }
            }
            "tool" => {
                let tc_id = ContextManager::extract_delimited(&msg.content, "tool_call_id:<|'|>", "<|'|>")
                    .unwrap_or("unknown".into());
                let result = ContextManager::extract_delimited(&msg.content, "result:<|'|>", "<|'|>")
                    .unwrap_or(msg.content.clone());
                msgs.push(gemma_chat::Message::tool_result(tc_id, result));
            }
            _ => {}
        }
    }

    // ── 4. Merge all system messages into one ──
    let (sys_msgs, conv_msgs): (Vec<_>, Vec<_>) = msgs
        .into_iter()
        .partition(|m| matches!(m.role, gemma_chat::Role::System));

    let combined_sys = sys_msgs
        .into_iter()
        .filter_map(|m| match m.content {
            serde_json::Value::String(s) if !s.is_empty() => Some(s),
            _ => None,
        })
        .collect::<Vec<_>>();

    let mut result = Vec::new();
    if !combined_sys.is_empty() {
        result.push(gemma_chat::Message::system(combined_sys.join("\n\n")));
    }
    result.extend(conv_msgs);
    result
}
