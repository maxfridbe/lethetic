use std::path::PathBuf;
use std::fs;
use crate::tools;

pub const DEFAULT_PROMPT_TEMPLATE: &str = r#"You are a helpful assistant.
CurrentWorkingDir:[CWD]

[TOOLS_DEFINITIONS]

Guidelines:
1. Pathing: Always specify full paths relative to the CurrentWorkingDir.
2. Tool Calls: For ALL tool call argument values that are strings, you MUST wrap the value in asymmetric markers: `<|tool_parameter>your content here<tool_parameter|>`. Do NOT use standard double quotes (") or single quotes (') as delimiters.
3. Verification: Verify your work using tool results before finalizing.
4. Finalize: Once all tasks are complete, provide a final summary of your actions.
"#;

pub struct SystemPromptManager {
    prompts_dir: PathBuf,
}

impl SystemPromptManager {
    pub fn new() -> Self {
        let config_dir = dirs::config_dir().unwrap_or_else(|| {
            dirs::home_dir().map(|h| h.join(".config")).unwrap_or_else(|| PathBuf::from("."))
        });
        let prompts_dir = config_dir.join("lethetic").join("prompts");
        
        if !prompts_dir.exists() {
            let _ = fs::create_dir_all(&prompts_dir);
        }

        // Always ensure the latest default template is available
        let default_path = prompts_dir.join("software_engineer.md");
        let _ = fs::write(default_path, DEFAULT_PROMPT_TEMPLATE);
        
        Self { prompts_dir }
    }

    pub fn list_prompts(&self) -> Vec<String> {
        let mut prompts = Vec::new();
        if let Ok(entries) = fs::read_dir(&self.prompts_dir) {
            for entry in entries.filter_map(Result::ok) {
                if let Some(name) = entry.file_name().to_str() {
                    if name.ends_with(".md") {
                        prompts.push(name.trim_end_matches(".md").to_string());
                    }
                }
            }
        }
        prompts.sort();
        prompts
    }

    pub fn load_prompt(&self, name: &str) -> Option<String> {
        let path = self.prompts_dir.join(format!("{}.md", name));
        fs::read_to_string(path).ok()
    }

    pub fn save_prompt(&self, name: &str, content: &str) -> std::io::Result<()> {
        let path = self.prompts_dir.join(format!("{}.md", name));
        fs::write(path, content)
    }

    pub fn resolve_prompt(template: &str, cwd: &str, config: &crate::config::Config) -> String {
        let tool_declarations = tools::get_all_prompt_templates(config);
        template.replace("[TOOLS_DEFINITIONS]", &tool_declarations).replace("[CWD]", cwd)
    }
}
