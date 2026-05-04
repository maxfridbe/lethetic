use std::fs;
use std::io::Write;

fn main() {
    let config_content = match fs::read_to_string("config.yml") {
        Ok(c) => c,
        Err(_) => panic!("Could not read config.yml"),
    };
    let config: lethetic::config::Config = serde_yaml::from_str(&config_content).expect("Failed to parse config");
    
    let sys_prompt = lethetic::system_prompt::SystemPromptManager::resolve_prompt(lethetic::system_prompt::DEFAULT_PROMPT_TEMPLATE, ".", &config);
    let mut context_manager = lethetic::context::ContextManager::new(config.context_size, Some(sys_prompt));
    
    let original = "private int _foo = 1;";
    let new_content = "private int _bar = 1;";
    let prompt = format!("In `App.cs`, replace the following line:\n```csharp\n{}\n```\nWith:\n```csharp\n{}\n```\nUse the `apply_patch` tool directly without checking if the file exists.", original, new_content);
    
    context_manager.add_message("user", &prompt);
    let raw = context_manager.get_raw_prompt();
    fs::write("test_prompt.txt", raw).unwrap();
}
