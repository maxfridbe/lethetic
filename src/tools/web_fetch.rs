use serde_json::json;
use crate::tools::{Tool, FunctionDefinition};
use super::icons;
use super::llm_tokens;
use reqwest::Client;

pub fn get_definition() -> Tool {
    Tool {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "web_fetch".to_string(),
            description: "Fetch the content of a URL".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "tool_call_id": {
                        "type": "string",
                        "description": "A unique, descriptive string identifier for this call (e.g., 'read_main_rs', 'check_folders'). Do not use simple numbers."
                    },
                    "url": {
                        "type": "string",
                        "description": "The URL to fetch content from"
                    },
                    "description": {
                        "type": "string",
                        "description": "Short description of the action"
                    }
                },
                "required": ["url", "description", "tool_call_id"]
            }),
        },
    }
}

pub fn get_prompt_template() -> String {
    format!("{}declaration:web_fetch{{description:<|\">Fetch the content of a URL.<|\">,parameters:{{properties:{{tool_call_id:{{description:<|\">A unique, descriptive string identifier for this call (e.g., 'read_main_rs', 'check_folders'). Do not use simple numbers.<|\">,type:<|\">STRING<|\">}},url:{{description:<|\">The URL to fetch<|\">,type:<|\">STRING<|\">}},description:{{description:<|\">Short description of the action<|\">,type:<|\">STRING<|\">}}}},required:[<|\">url<|\">,<|\">description<|\">,<|\">tool_call_id<|\">],type:<|\">OBJECT<|\">}}}}{}", llm_tokens::TOOL_CALL_OPEN, llm_tokens::TOOL_CALL_CLOSE)
}

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    if let Some(desc) = arguments["description"].as_str() {
        return format!("{} {}", icons::WEATHER, desc);
    }
    let url = arguments["url"].as_str().unwrap_or("");
    format!("{} Fetching URL: `{}`", icons::WEATHER, url)
}

pub async fn execute(url: &str, cancellation_token: tokio_util::sync::CancellationToken) -> String {

    let client = Client::new();
    
    tokio::select! {
        _ = cancellation_token.cancelled() => {
            "[Operation Cancelled by User]".to_string()
        }
        res = async {
            match client.get(url).send().await {
                Ok(res) => {
                    match res.text().await {
                        Ok(text) => text,
                        Err(e) => format!("ERROR: Failed to read response body: {}", e),
                    }
                }
                Err(e) => format!("ERROR: Failed to fetch URL {}: {}", url, e),
            }
        } => res
    }
}
