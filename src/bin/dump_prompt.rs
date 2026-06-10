use std::fs;

fn main() {
    let mut config = lethetic::config::Config::load("config.yml").expect("Failed to load config");
    config.merge_matching_server_settings();
    
    let sys_prompt = lethetic::system_prompt::SystemPromptManager::resolve_prompt(lethetic::system_prompt::DEFAULT_PROMPT_TEMPLATE, ".", &config);
    let mut context_manager = lethetic::context::ContextManager::new(config.context_size, Some(sys_prompt));
    if let Some(mode) = config.context_mode {
        context_manager.mode = mode;
    }
    
    let original = "private int _foo = 1;";
    let new_content = "private int _bar = 1;";
    let prompt = format!("In `App.cs`, replace the following line:\n```csharp\n{}\n```\nWith:\n```csharp\n{}\n```\nUse the `apply_patch` tool directly without checking if the file exists.", original, new_content);
    
    context_manager.add_message("user", &prompt);
    let raw = context_manager.get_raw_prompt();
    fs::write("test_prompt.txt", raw).unwrap();
}
