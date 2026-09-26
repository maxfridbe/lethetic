use super::icons;
use crate::tools::{FunctionDefinition, Tool, ToolExecution};
use h2m::convert;
use reqwest::Client;
use serde_json::json;

pub fn get_definition() -> Tool {
    Tool {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "read_page".to_string(),
            description: "Fetch a URL and convert its content to Markdown. Use this instead of 'web_fetch' for general information retrieval from web pages.".to_string(),
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
        Ok(html) => ToolExecution::success(convert(&html), cwd),
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
