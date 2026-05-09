use serde_json::json;
use crate::tools::{Tool, FunctionDefinition};
use super::icons;
use reqwest::Client;
use h2m::convert;

pub fn get_definition() -> Tool {
    Tool {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "fetch_url".to_string(),
            description: "Fetch a URL and return its content. Use format:'markdown' (default) for articles/docs, 'text' for plain content, 'html' for raw HTML. Replaces the separate web_fetch and read_page tools.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The URL to fetch"
                    },
                    "format": {
                        "type": "string",
                        "enum": ["markdown", "text", "html"],
                        "description": "'markdown' (default): convert HTML to clean Markdown. 'text': strip all tags, return plain text. 'html': raw response body."
                    },
                    "description": {
                        "type": "string",
                        "description": "Short description of the action"
                    },
                    "tool_call_id": {
                        "type": "string",
                        "description": "Unique identifier for this call"
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
    let fmt = arguments["format"].as_str().unwrap_or("markdown");
    format!("{} Fetching ({}) `{}`", icons::WEATHER, fmt, url)
}

pub async fn execute(
    url: &str,
    format: &str,
    cancellation_token: tokio_util::sync::CancellationToken,
) -> String {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_default();

    tokio::select! {
        _ = cancellation_token.cancelled() => "[Operation Cancelled by User]".to_string(),
        result = fetch(url, format, &client) => result,
    }
}

async fn fetch(url: &str, format: &str, client: &Client) -> String {
    let response = match client.get(url)
        .header("User-Agent", "Mozilla/5.0 (compatible; lethetic/1.0)")
        .send().await
    {
        Ok(r) => r,
        Err(e) => return format!("ERROR: Failed to fetch {}: {}", url, e),
    };

    if !response.status().is_success() {
        return format!("ERROR: HTTP {} for {}", response.status(), url);
    }

    let body = match response.text().await {
        Ok(t) => t,
        Err(e) => return format!("ERROR: Failed to read response body: {}", e),
    };

    match format {
        "html" => body,
        "text" => strip_html(&body),
        _ => convert(&body), // markdown (default)
    }
}

fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    // Collapse excess whitespace
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_html() {
        let html = "<h1>Hello</h1> <p>World</p>";
        assert_eq!(strip_html(html), "Hello World");
    }
}
