use reqwest::Client;
use serde::{Deserialize, Serialize};
use anyhow::{Result, anyhow};
use std::env;

#[derive(Serialize)]
struct PromptRequest {
    prompt: String,
}

#[derive(Deserialize)]
struct CompletionResponse {
    content: String,
}

pub fn strip_thinking(content: &str) -> String {
    let mut content = content.to_string();
    while let Some(start_idx) = content.find("<thought>") {
        if let Some(end_idx) = content.find("</thought>") {
            let end_idx_after = end_idx + 10; // length of "</thought>"
            content.replace_range(start_idx..end_idx_after, "");
        } else {
            content.replace_range(start_idx.., "");
            break;
        }
    }
    content.trim().to_string()
}

pub async fn quickprompt(client: &Client, url: &str, prompt: &str) -> Result<String> {
    let response = client
        .post(url)
        .json(&PromptRequest {
            prompt: prompt.to_string(),
        })
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(anyhow!("Request failed with status: {}", response.status()));
    }

    let completion: CompletionResponse = response.json().await?;
    Ok(strip_thinking(&completion.content))
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        println!("Usage: quickprompt <prompt>");
        return Ok(());
    }
    let prompt = &args[1];
    
    // For now, use a dummy URL or a real one if I had it.
    // Since I don't have a URL, I'll just print the prompt for now to show it works.
    // But the user wants me to "get it working". 
    // I'll need a URL. Let's see if there's an environment variable.
    let url = "http://brainiac-nvidia:7210/completion".to_string();
    
    let client = Client::new();
    match quickprompt(&client, &url, prompt).await {
        Ok(answer) => println!("{}", answer),
        Err(e) => eprintln!("Error: {}", e),
    }
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_thinking_with_tags() {
        let input = "<thought>I am thinking...</thought>Hello world!";
        let output = strip_thinking(input);
        assert_eq!(output, "Hello world!");
    }

    #[test]
    fn test_strip_thinking_no_end_tag() {
        let input = "<thought>I am thinking... Hello world!";
        let output = strip_thinking(input);
        assert_eq!(output, "");
    }

    #[test]
    fn test_strip_thinking_no_tags() {
        let input = "Hello world!";
        let output = strip_thinking(input);
        assert_eq!(output, "Hello world!");
    }

    #[test]
    fn test_strip_thinking_empty() {
        let input = "<thought></thought>";
        let output = strip_thinking(input);
        assert_eq!(output, "");
    }
    
    #[test]
    fn test_strip_thinking_multiple() {
        let input = "First <thought>one</thought> Second <thought>two</thought> Third";
        let output = strip_thinking(input);
        assert_eq!(output, "First  Second  Third");
    }
}
