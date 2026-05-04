use serde_json::json;
use crate::tools::{Tool, FunctionDefinition};
use super::icons;
use reqwest::Client;
use h2m::convert;

pub fn get_definition() -> Tool {
    Tool {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "read_page".to_string(),
            description: "Fetch a URL and convert its content to Markdown".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "tool_call_id": {
                        "type": "string",
                        "description": "A unique, descriptive string identifier for this call."
                    },
                    "url": {
                        "type": "string",
                        "description": "The URL to fetch and convert"
                    },
                    "description": {
                        "type": "string",
                        "description": "Short description of the page being read"
                    }
                },
                "required": ["url", "description", "tool_call_id"]
            }),
        },
    }
}

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    if let Some(desc) = arguments["description"].as_str() {
        return format!("{} Reading Page: {}", icons::WEATHER, desc);
    }
    let url = arguments["url"].as_str().unwrap_or("");
    format!("{} Reading Page: `{}`", icons::WEATHER, url)
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
                        Ok(html) => {
                            convert(&html)
                        }
                        Err(e) => format!("ERROR: Failed to read response body: {}", e),
                    }
                }
                Err(e) => format!("ERROR: Failed to fetch URL {}: {}", url, e),
            }
        } => res
    }
}
