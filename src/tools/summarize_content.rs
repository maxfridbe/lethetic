use serde_json::json;
use crate::tools::{Tool, FunctionDefinition};
use super::icons;
use std::fs;
use std::path::Path;
use crate::client::summarize_llm;
use crate::config::Config;
use reqwest::Client;

pub fn get_definition() -> Tool {
    Tool {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "summarize_content".to_string(),
            description: "Summarize a file's content or a long string using the LLM. Use this when tool output was too large and saved to a file.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The path to the file to summarize (optional if content is provided)"
                    },
                    "content": {
                        "type": "string",
                        "description": "The raw content to summarize (optional if path is provided)"
                    },
                    "prompt": {
                        "type": "string",
                        "description": "Mandatory instructions on what to focus on in the summary."
                    },
                    "tool_call_id": {
                        "type": "string",
                        "description": "A unique identifier for this call."
                    }
                },
                "required": ["tool_call_id"]
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
) -> String {
    let raw_content = if let Some(p) = path {
        let p = p.trim_matches(|c| c == '\'' || c == '\"');
        let full_path = Path::new(cwd).join(p);
        match fs::read_to_string(&full_path) {
            Ok(c) => c,
            Err(e) => return format!("ERROR: Failed to read file {}: {}", full_path.display(), e),
        }
    } else if let Some(c) = content {
        c.to_string()
    } else {
        return "ERROR: Either 'path' or 'content' must be provided.".to_string();
    };

    let summary_prompt = prompt.unwrap_or("Summarize the following content, highlighting the most important information, results, or errors.");

    match summarize_llm(client, config, &raw_content, summary_prompt).await {
        Ok(summary) => format!("SUMMARY:\n{}", summary),
        Err(e) => format!("ERROR: Summarization failed: {}", e),
    }
}
