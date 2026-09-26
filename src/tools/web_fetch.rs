use super::icons;
use crate::tools::{FunctionDefinition, Tool, ToolExecution};
use reqwest::Client;
use serde_json::json;

pub fn get_definition() -> Tool {
    Tool {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "web_fetch".to_string(),
            description: "Fetch the raw content of a URL. For general information retrieval from web pages, prefer the 'read_page' tool which returns cleaner Markdown.".to_string(),
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

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    if let Some(desc) = arguments["description"].as_str() {
        return format!("{} {}", icons::WEATHER, desc);
    }
    let url = arguments["url"].as_str().unwrap_or("");
    format!("{} Fetching URL: `{}`", icons::WEATHER, url)
}

pub async fn execute(url: &str, cancellation_token: tokio_util::sync::CancellationToken) -> String {
    execute_classified(url, ".", cancellation_token)
        .await
        .output
}

pub(super) async fn execute_classified(
    url: &str,
    cwd: &str,
    cancellation_token: tokio_util::sync::CancellationToken,
) -> ToolExecution {
    let client = Client::new();

    let result = tokio::select! {
        biased;
        _ = cancellation_token.cancelled() => {
            return ToolExecution::error("[Operation Cancelled by User]", cwd);
        }
        result = fetch(url, &client) => result,
    };

    match result {
        Ok(body) => ToolExecution::success(body, cwd),
        Err(error) => ToolExecution::error(error, cwd),
    }
}

async fn fetch(url: &str, client: &Client) -> Result<String, String> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| format!("ERROR: Failed to fetch URL {url}: {error}"))?;
    let status = response.status();
    let body = response.text().await.map_err(|error| {
        format!("ERROR: Failed to read HTTP {status} response body for {url}: {error}")
    })?;
    super::http_response::classify_body(url, status, body)
}
