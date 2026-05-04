use lethetic::system_prompt::SystemPromptManager;
use lethetic::config::Config;

#[test]
fn test_system_prompt_manager_lifecycle() {
    // For now, let's just test `resolve_prompt` to ensure the placeholder is replaced.
    let template = "Hello\n[TOOLS_DEFINITIONS]\nGoodbye";
    let config = Config {
        server_url: "".to_string(),
        model: "".to_string(),
        context_size: 0,
        tool_wrapper: None,
        enable_image_processing_tool: false,
    };
    let resolved = SystemPromptManager::resolve_prompt(template, "/mock/cwd", &config);
    
    assert!(resolved.contains("Hello"));
    assert!(resolved.contains("Goodbye"));
    assert!(!resolved.contains("[TOOLS_DEFINITIONS]"));
    
    // Check if some expected tool declarations are present in new JSON format
    assert!(resolved.contains("<|tool>"));
    assert!(resolved.contains("\"name\": \"read_file\""));
    assert!(resolved.contains("\"name\": \"run_shell_command\""));
}
