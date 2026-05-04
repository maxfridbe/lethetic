use serde_json::json;
use crate::tools::{Tool, FunctionDefinition};
use super::icons;
use h2m_search::{SearchClient, SearchQuery};

pub fn get_definition() -> Tool {
    Tool {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "web_search".to_string(),
            description: "Search the web using DuckDuckGo".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "tool_call_id": {
                        "type": "string",
                        "description": "A unique, descriptive string identifier for this call."
                    },
                    "query": {
                        "type": "string",
                        "description": "The search query"
                    },
                    "description": {
                        "type": "string",
                        "description": "Short description of the search"
                    }
                },
                "required": ["query", "description", "tool_call_id"]
            }),
        },
    }
}

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    if let Some(desc) = arguments["description"].as_str() {
        return format!("{} Searching: {}", icons::WEATHER, desc);
    }
    let query = arguments["query"].as_str().unwrap_or("");
    format!("{} Web Search: `{}`", icons::WEATHER, query)
}

pub async fn execute(query: &str, cancellation_token: tokio_util::sync::CancellationToken) -> String {
    let client = match SearchClient::from_env() {
        Ok(c) => c,
        Err(e) => return format!("ERROR: Failed to initialize search client: {}", e),
    };
    let search_query = SearchQuery::new(query).with_limit(10);

    tokio::select! {
        _ = cancellation_token.cancelled() => {
            "[Operation Cancelled by User]".to_string()
        }
        res = async {
            match client.search(&search_query).await {
                Ok(response) => {
                    let mut output = String::new();
                    for (i, hit) in response.web.iter().enumerate() {
                        let description = hit.description.as_deref().unwrap_or("");
                        output.push_str(&format!("{}. {}\n   URL: {}\n   Snippet: {}\n\n", i + 1, hit.title, hit.url, description));
                    }
                    if output.is_empty() {
                        "No results found.".to_string()
                    } else {
                        output
                    }
                }
                Err(e) => format!("ERROR: Web search failed: {}", e),
            }
        } => res
    }
}
