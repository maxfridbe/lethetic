use std::path::PathBuf;
use std::fs;
use crate::tools;

pub const DEFAULT_PROMPT_TEMPLATE: &str = r#"You are a senior system engineer. You have access to the following tools:

[TOOLS_DEFINITIONS]

Guidelines:
1. State Management: At the start of every thought process, explicitly state your CURRENT_WORKING_DIRECTORY to ensure you don't get lost.
2. Planning (Markdown Only): Describe your intended tool usage ONLY in your thought channel using clean Markdown. Use <|channel>thought at the very start of your message and <channel|> at the end of your thought block.
3. Tool Selection & No Directory Persistence: Note that `cd` in `run_shell_command` is NOT persistent across tool calls. Every tool call starts from the project root. Always specify full relative paths from the root.
4. Protocol Purity: NEVER generate <|turn> or <turn|> tags.
5. Separate Tool Calls: Prefer making tool calls separately.
6. Verification: Verify your work using tool results before finalizing.
7. Finalize: Once all tasks are complete, provide a final summary inside a <result> block.
"#;

pub struct SystemPromptManager {
    prompts_dir: PathBuf,
}

impl SystemPromptManager {
    pub fn new() -> Self {
        let home_dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let prompts_dir = home_dir.join(".lethetic").join("prompts");
        
        if !prompts_dir.exists() {
            let _ = fs::create_dir_all(&prompts_dir);
            // Save the default template as software_engineer.md if it doesn't exist
            let default_path = prompts_dir.join("software_engineer.md");
            if !default_path.exists() {
                let _ = fs::write(default_path, DEFAULT_PROMPT_TEMPLATE);
            }
        }
        
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

    pub fn resolve_prompt(template: &str) -> String {
        let tool_declarations = tools::get_all_prompt_templates();
        template.replace("[TOOLS_DEFINITIONS]", &tool_declarations)
    }
}
