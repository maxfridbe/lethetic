use super::icons;
use crate::client::{RequestStartedHook, StreamEvent, summarize_llm_accounted_with_hook};
use crate::config::Config;
use crate::tools::{FunctionDefinition, Tool};
use reqwest::Client;
use serde_json::json;
use std::fs;
use std::path::Path;
use tokio_util::sync::CancellationToken;

pub fn get_definition() -> Tool {
    Tool {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "summarize_content".to_string(),
            description: "Summarize a file or text string using the LLM. Provide either 'path' (reads the file) or 'content' (inline text) — at least one is required. Use 'prompt' to focus the summary on what matters.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to summarize (required if 'content' is not provided)"
                    },
                    "content": {
                        "type": "string",
                        "description": "Raw text to summarize (required if 'path' is not provided)"
                    },
                    "prompt": {
                        "type": "string",
                        "description": "What to focus on in the summary — errors, key decisions, API surface, etc."
                    },
                    "tool_call_id": {
                        "type": "string",
                        "description": "Unique identifier for this call"
                    }
                },
                "required": ["prompt", "tool_call_id"]
            }),
        },
    }
}

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    let path = arguments["path"].as_str().unwrap_or("content");
    format!("{} Summarizing: `{}`", icons::COMMAND, path)
}

pub async fn execute(
    path: Option<&str>,
    content: Option<&str>,
    prompt: Option<&str>,
    cwd: &str,
    client: &Client,
    config: &Config,
    tx: &tokio::sync::mpsc::UnboundedSender<StreamEvent>,
    cancellation: CancellationToken,
) -> String {
    execute_with_hook(
        path,
        content,
        prompt,
        cwd,
        client,
        config,
        tx,
        cancellation,
        None,
    )
    .await
}

pub async fn execute_with_hook(
    path: Option<&str>,
    content: Option<&str>,
    prompt: Option<&str>,
    cwd: &str,
    client: &Client,
    config: &Config,
    tx: &tokio::sync::mpsc::UnboundedSender<StreamEvent>,
    cancellation: CancellationToken,
    request_hook: Option<RequestStartedHook>,
) -> String {
    if path.is_none() && content.is_none() {
        return "ERROR: Provide either 'path' (file to read) or 'content' (inline text)."
            .to_string();
    }

    let raw_content = if let Some(p) = path {
        let p = p.trim_matches(|c| c == '\'' || c == '"');
        let full_path = Path::new(cwd).join(p);
        match fs::read_to_string(&full_path) {
            Ok(c) => c,
            Err(e) => return format!("ERROR: Failed to read file {}: {}", full_path.display(), e),
        }
    } else {
        content.unwrap().to_string()
    };

    let summary_prompt = prompt.unwrap_or(
        "Summarize the following content, highlighting the most important information, results, or errors.",
    );

    match summarize_llm_accounted_with_hook(
        client,
        config,
        &raw_content,
        summary_prompt,
        tx,
        cancellation,
        request_hook,
    )
    .await
    {
        Ok(summary) => format!("SUMMARY:\n{}", summary),
        Err(e) => format!("ERROR: Summarization failed: {}", e),
    }
}
