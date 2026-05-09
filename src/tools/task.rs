use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::client::StreamEvent;
use crate::config::Config;
use crate::headless;
use crate::tools::{FunctionDefinition, Tool};
use super::icons;

pub fn get_definition() -> Tool {
    Tool {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "task".to_string(),
            description: "Spawn a sub-agent to handle a self-contained task autonomously. \
                The sub-agent has access to all tools (except task and ask_the_user) and runs \
                until it produces a final response. Use this to delegate complex, isolated \
                sub-problems — e.g. 'investigate and summarize all usages of X', \
                'refactor module Y and verify it compiles', 'research topic Z and write a report'. \
                The sub-agent cannot ask the user questions; make the prompt self-contained.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "The full, self-contained task description for the sub-agent"
                    },
                    "description": {
                        "type": "string",
                        "description": "Short label shown in the UI (3-5 words)"
                    },
                    "tool_call_id": {
                        "type": "string",
                        "description": "A unique, descriptive string identifier for this call"
                    }
                },
                "required": ["prompt", "description", "tool_call_id"]
            }),
        },
    }
}

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    if let Some(desc) = arguments["description"].as_str() {
        return format!("{} Sub-agent: {}", icons::COMMAND, desc);
    }
    format!("{} Running sub-agent task", icons::COMMAND)
}

pub async fn execute(
    prompt: &str,
    _cwd: &str,
    _cancellation_token: CancellationToken,
    tx: mpsc::UnboundedSender<StreamEvent>,
    client: &reqwest::Client,
    config: &Config,
) -> String {
    let _ = tx.send(StreamEvent::ToolProgress(
        format!("Sub-agent started: {}", &prompt.chars().take(80).collect::<String>())
    ));

    match tokio::time::timeout(
        std::time::Duration::from_secs(300),
        headless::run_agent(prompt.to_string(), client, config, false, Some(tx)),
    ).await {
        Ok(Ok(result)) => {
            if result.trim().is_empty() {
                "Sub-agent completed but produced no output.".to_string()
            } else {
                result
            }
        }
        Ok(Err(e)) => format!("Sub-agent failed: {}", e),
        Err(_) => "Sub-agent timed out after 5 minutes.".to_string(),
    }
}
