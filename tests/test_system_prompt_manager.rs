use lethetic::system_prompt::SystemPromptManager;
use std::fs;
use tempfile::tempdir;
use std::path::PathBuf;

#[test]
fn test_system_prompt_manager_lifecycle() {
    // 1. Create a mock environment for the manager
    // In a real scenario, this writes to ~/.lethetic/prompts, but we can test the resolving logic directly
    // and test the file operations by temporarily changing the home directory or just testing the core functions.
    // Since SystemPromptManager uses `dirs::home_dir`, we can't easily mock it without injecting a path.
    // Let's modify SystemPromptManager to accept a custom directory for testing if needed, or just test `resolve_prompt`.
    
    // For now, let's just test `resolve_prompt` to ensure the placeholder is replaced.
    let template = "Hello\n[TOOLS_DEFINITIONS]\nGoodbye";
    let resolved = SystemPromptManager::resolve_prompt(template, "/mock/cwd");
    
    assert!(resolved.contains("Hello"));
    assert!(resolved.contains("Goodbye"));
    assert!(!resolved.contains("[TOOLS_DEFINITIONS]"));
    
    // Check if some expected tool declarations are present in new JSON format
    assert!(resolved.contains("<|tool>"));
    assert!(resolved.contains("\"name\": \"read_file\""));
    assert!(resolved.contains("\"name\": \"run_shell_command\""));
}
