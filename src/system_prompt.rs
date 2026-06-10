use std::path::PathBuf;
use std::fs;
use crate::tools;

pub const DEFAULT_PROMPT_TEMPLATE: &str = r#"You are a coding agent. You help users with software engineering tasks using the tools available to you.

CurrentWorkingDir:[CWD]

[TOOLS_DEFINITIONS]

# Tone
Be concise and direct. Minimize output tokens. Answer in 1–4 lines unless the user asks for detail.
Do NOT add preamble ("Here is...", "I will now...", "Let me...") or postamble (summaries of what you just did).
Do NOT add code comments unless explicitly asked.

# Tool use
When multiple independent pieces of information are needed, issue all tool calls in parallel in a single turn.
Always verify your work — run builds, tests, or lint after making changes when possible.

# Code conventions
Before using a library or framework, verify it already exists in the project (check package.json, Cargo.toml, imports).
Mimic existing code style, naming, and patterns. Never introduce inconsistencies.

# Pathing
Always specify full paths relative to CurrentWorkingDir.

[TOOL_CALL_FORMAT]
"#;

pub const TOOL_CALL_FORMAT_GEMMA4: &str = r#"# Tool call format
ALL tool call argument values that are strings MUST be wrapped in asymmetric markers:
  <|"|>your content here<|"|>
Strings inside those markers do not need escaping. Do NOT use <|'|> markers."#;

pub const TOOL_CALL_FORMAT_QWEN3: &str = r#"# Tool call format
Use standard JSON format for all tool call arguments. Do NOT wrap strings in any special markers."#;

pub struct SystemPromptManager {
    prompts_dir: PathBuf,
}

impl Default for SystemPromptManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemPromptManager {
    pub fn new() -> Self {
        let prompts_dir = crate::platform::lethetic_config_dir().join("prompts");
        
        if !prompts_dir.exists() {
            let _ = fs::create_dir_all(&prompts_dir);
        }

        // Ensure the latest default template is available (write only when it differs
        // to avoid disk IO on every startup)
        let default_path = prompts_dir.join("software_engineer.md");
        if fs::read_to_string(&default_path).ok().as_deref() != Some(DEFAULT_PROMPT_TEMPLATE) {
            let _ = fs::write(default_path, DEFAULT_PROMPT_TEMPLATE);
        }
        
        Self { prompts_dir }
    }

    pub fn list_prompts(&self) -> Vec<String> {
        let mut prompts = Vec::new();
        if let Ok(entries) = fs::read_dir(&self.prompts_dir) {
            for entry in entries.filter_map(Result::ok) {
                if let Some(name) = entry.file_name().to_str()
                    && name.ends_with(".md") {
                        prompts.push(name.trim_end_matches(".md").to_string());
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
        // Choose tool call format based on active server's parser setting
        let active_parser = config.model_servers.iter()
            .find(|s| s.url == config.server_url)
            .map(|s| s.parser.as_str())
            .unwrap_or("gemma4");
        let tool_call_fmt = match active_parser {
            "qwen3" | "default" | "generic" => TOOL_CALL_FORMAT_QWEN3,
            _ => TOOL_CALL_FORMAT_GEMMA4,
        };
        template
            .replace("[TOOLS_DEFINITIONS]", &tool_declarations)
            .replace("[CWD]", cwd)
            .replace("[TOOL_CALL_FORMAT]", tool_call_fmt)
    }
}
