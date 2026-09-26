//! Vercel-style prompt building and message formatting.
//!
//! This module provides functions to format conversation history, system prompt,
//! and files in context using standard Markdown block format rather than proprietary
//! XML-like tags. This aligns with OpenCode TypeScript Vercel-style structures.

use crate::context::{ContextManager, format_gemma4_call, sanitize_file_content};

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
            prompt.push_str(&format!(
                "## File: {}\n```\n{}\n```\n",
                path,
                sanitize_file_content(&cached_file.content)
            ));
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
                if current_turn_role == "model" {
                    prompt.push_str("<turn|>\n");
                }
                prompt.push_str("<|turn>system\n<|think|>\n");
                prompt.push_str(&msg.content);
                prompt.push_str("<turn|>\n");
                current_turn_role = String::new();
            }
            "user" => {
                if current_turn_role == "model" {
                    prompt.push_str("<turn|>\n");
                }
                prompt.push_str("<|turn>user\n");
                prompt.push_str(&msg.content);
                prompt.push_str("<turn|>\n");
                current_turn_role = String::new();
            }
            "assistant" => {
                if current_turn_role != "model" {
                    prompt.push_str("<|turn>model\n");
                }
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
                clean_content = clean_content
                    .replace("<|channel>text\n", "")
                    .replace("<|channel>text", "");
                if msg.tool_calls.is_some()
                    && let Some(idx) = clean_content.find("<|tool_call>")
                {
                    clean_content.truncate(idx);
                }
                prompt.push_str(clean_content.trim());
                prompt.push('\n');
                if let Some(calls) = &msg.tool_calls {
                    for tc in calls {
                        let call_str =
                            format_gemma4_call(&tc.function.name, &tc.function.arguments);
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
                if current_turn_role != "model" {
                    prompt.push_str("<|turn>model\n");
                }
                prompt.push_str(&msg.content);
                current_turn_role = "model".to_string();
            }
            _ => {}
        }
    }

    // ── 4. Active file ──
    if !ctx.active_files.is_empty() {
        if current_turn_role == "model" {
            prompt.push_str("<turn|>\n");
        }
        prompt.push_str("<|turn>system\n<|think|>\n");
        for (path, cached_file) in &ctx.active_files {
            prompt.push_str(&format!(
                "## File: {}\n```\n{}\n```\n",
                path,
                sanitize_file_content(&cached_file.content)
            ));
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

/// Compatibility entry point for Vercel-mode API projection.
///
/// New request paths should use `ContextManager::prepare_api_context` so the
/// selected messages and their fresh estimate remain one snapshot.
pub fn get_messages_for_api_vercel(ctx: &ContextManager) -> Vec<crate::transport::Message> {
    debug_assert_eq!(ctx.mode, crate::context::ContextMode::Vercel);
    ctx.prepare_api_context().into_messages()
}
