use std::fs;
use reqwest::Client;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use lethetic::config::Config;
use lethetic::context::ContextManager;
use lethetic::system_prompt;
use lethetic::client::{trigger_llm_request, StreamEvent};

#[tokio::test]
async fn test_live_client_stream() -> Result<(), String> {
    let config_content = match fs::read_to_string("config.yml") {
        Ok(c) => c,
        Err(_) => return Err("Could not read config.yml".to_string()),
    };
    let config: Config = match serde_yaml::from_str(&config_content) {
        Ok(c) => c,
        Err(e) => return Err(format!("Failed to parse config: {}", e)),
    };
    
    let client = Client::new();
    let sys_prompt = system_prompt::SystemPromptManager::resolve_prompt(system_prompt::DEFAULT_PROMPT_TEMPLATE, ".", &config);
    let mut context_manager = ContextManager::new(config.context_size, Some(sys_prompt));
    
    context_manager.add_message("user", "hello");
    
    let (tx, mut rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    
    trigger_llm_request(client, config, &context_manager, tx, cancel, true, Some("ignore/.lethetic/sessions/test".to_string()));
    
    let mut got_chunks = false;
    let mut done = false;
    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::Chunk(text) => {
                println!("Got chunk: {:?}", text);
                got_chunks = true;
            }
            StreamEvent::Done { .. } => {
                println!("Got Done");
                done = true;
                break;
            }
            StreamEvent::Error(e) => {
                println!("Got Error: {}", e);
                return Err(e);
            }
            _ => {}
        }
    }
    
    if !got_chunks {
        return Err("No chunks received".to_string());
    }
    if !done {
        return Err("No done event received".to_string());
    }
    
    Ok(())
}
