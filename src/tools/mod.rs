pub mod read_file;
pub mod read_file_lines;
pub mod read_folder;
pub mod search_text;
pub mod apply_patch;
pub mod run_shell_command;
pub mod write_file;
pub mod replace_text;
pub mod web_fetch;
pub mod calculate;
pub mod ask_the_user;
pub mod code_snippet;

#[path = "../icons.rs"]
pub mod icons;
#[path = "../llm_tokens.rs"]
pub mod llm_tokens;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Tool {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: FunctionDefinition,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

pub fn get_all_tools() -> Vec<Tool> {
    vec![
        read_file::get_definition(),
        read_file_lines::get_definition(),
        read_folder::get_definition(),
        search_text::get_definition(),
        apply_patch::get_definition(),
        run_shell_command::get_definition(),
        write_file::get_definition(),
        replace_text::get_definition(),
        web_fetch::get_definition(),
        calculate::get_definition(),
        ask_the_user::get_definition(),
        code_snippet::get_definition(),
    ]
}

pub fn get_all_prompt_templates() -> String {
    let mut templates = String::new();
    templates.push_str(&read_file::get_prompt_template());
    templates.push('\n');
    templates.push_str(&read_file_lines::get_prompt_template());
    templates.push('\n');
    templates.push_str(&read_folder::get_prompt_template());
    templates.push('\n');
    templates.push_str(&search_text::get_prompt_template());
    templates.push('\n');
    templates.push_str(&apply_patch::get_prompt_template());
    templates.push('\n');
    templates.push_str(&run_shell_command::get_prompt_template());
    templates.push('\n');
    templates.push_str(&write_file::get_prompt_template());
    templates.push('\n');
    templates.push_str(&replace_text::get_prompt_template());
    templates.push('\n');
    templates.push_str(&web_fetch::get_prompt_template());
    templates.push('\n');
    templates.push_str(&calculate::get_prompt_template());
    templates.push('\n');
    templates.push_str(&ask_the_user::get_prompt_template());
    templates.push('\n');
    templates.push_str(&code_snippet::get_prompt_template());
    templates
}

pub fn get_ui_description(func_name: &str, arguments: &serde_json::Value) -> String {
    match func_name {
        "read_file" => read_file::get_ui_description(arguments),
        "read_file_lines" => read_file_lines::get_ui_description(arguments),
        "read_folder" => read_folder::get_ui_description(arguments),
        "search_text" => search_text::get_ui_description(arguments),
        "apply_patch" => apply_patch::get_ui_description(arguments),
        "run_shell_command" => run_shell_command::get_ui_description(arguments),
        "write_file" => write_file::get_ui_description(arguments),
        "replace_text" => replace_text::get_ui_description(arguments),
        "web_fetch" => web_fetch::get_ui_description(arguments),
        "calculate" => calculate::get_ui_description(arguments),
        "ask_the_user" => ask_the_user::get_ui_description(arguments),
        "code_snippet" => code_snippet::get_ui_description(arguments),
        _ => format!("{} {}: {}", icons::COMMAND, func_name, arguments),
    }
}

pub async fn execute(func_name: &str, arguments: &serde_json::Value, cwd: &str, cancellation_token: tokio_util::sync::CancellationToken) -> (String, String) {
    match func_name {
        "read_file" => {
            let path = arguments["path"].as_str().unwrap_or("");
            (read_file::execute(path, cwd).await, cwd.to_string())
        }
        "read_file_lines" => {
            let path = arguments["path"].as_str().unwrap_or("");
            let start = arguments["start_line"].as_u64().unwrap_or(1) as usize;
            let end = arguments["end_line"].as_u64().unwrap_or(1) as usize;
            (read_file_lines::execute(path, start, end, cwd).await, cwd.to_string())
        }
        "read_folder" => {
            let path = arguments["path"].as_str().unwrap_or(".");
            (read_folder::execute(path, cwd).await, cwd.to_string())
        }
        "search_text" => {
            let pattern = arguments["pattern"].as_str().unwrap_or("");
            let path = arguments["path"].as_str().unwrap_or(".");
            (search_text::execute(pattern, path, cwd).await, cwd.to_string())
        }
        "apply_patch" => {
            let path = arguments["path"].as_str().unwrap_or("");
            let patch = arguments["patch"].as_str().unwrap_or("");
            (apply_patch::execute(path, patch, cwd).await, cwd.to_string())
        }
        "run_shell_command" => {
            let command = arguments["command"].as_str().unwrap_or("");
            run_shell_command::execute(command, cwd, cancellation_token).await
        }
        "write_file" => {
            let path = arguments["path"].as_str().unwrap_or("");
            let content = arguments["content"].as_str().unwrap_or("");
            (write_file::execute(path, content, cwd).await, cwd.to_string())
        }
        "replace_text" => {
            let path = arguments["path"].as_str().unwrap_or("");
            let old_string = arguments["old_string"].as_str().unwrap_or("");
            let new_string = arguments["new_string"].as_str().unwrap_or("");
            (replace_text::execute(path, old_string, new_string, cwd).await, cwd.to_string())
        }
        "web_fetch" => {
            let url = arguments["url"].as_str().unwrap_or("");
            (web_fetch::execute(url).await, cwd.to_string())
        }
        "calculate" => {
            let expression = arguments["expression"].as_str().unwrap_or("");
            (calculate::execute(expression).await, cwd.to_string())
        }
        "ask_the_user" => {
            // This is handled specially in the app, but we can return the question if called here
            let question = arguments["question"].as_str().unwrap_or("");
            (question.to_string(), cwd.to_string())
        }
        "code_snippet" => {
            let name = arguments["name"].as_str().unwrap_or("");
            (format!("Snippet '{}' stored.", name), cwd.to_string())
        }
        _ => (format!("Unknown tool: {}", func_name), cwd.to_string()),
    }
}

pub async fn get_git_info() -> String {
    use tokio::process::Command;
    let status = Command::new("git")
        .arg("status")
        .arg("--porcelain=v2")
        .arg("--branch")
        .output()
        .await;

    match status {
        Ok(out) => {
            let s = String::from_utf8_lossy(&out.stdout);
            if s.trim().is_empty() { return "clean".to_string(); }

            let mut branch = String::from("unknown");
            let mut untracked = 0;
            let mut modified = 0;
            let mut staged = 0;
            let mut renamed = 0;
            let mut deleted = 0;

            for line in s.lines() {
                if line.starts_with("# branch.head") {
                    branch = line.split_whitespace().nth(2).unwrap_or("detached").to_string();
                } else if line.starts_with("?") {
                    untracked += 1;
                } else if line.starts_with("1 ") || line.starts_with("2 ") {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() > 1 {
                        let codes = parts[1];
                        let staged_code = codes.chars().nth(0).unwrap_or('.');
                        let unstaged_code = codes.chars().nth(1).unwrap_or('.');
                        
                        if staged_code != '.' { staged += 1; }
                        if unstaged_code == 'M' { modified += 1; }
                        if unstaged_code == 'D' { deleted += 1; }
                        if staged_code == 'R' { renamed += 1; }
                    }
                }
            }

            let mut res = format!(" {}", branch);
            if staged > 0 { res.push_str(&format!(" +{}", staged)); }
            if modified > 0 { res.push_str(&format!(" ~{}", modified)); }
            if deleted > 0 { res.push_str(&format!(" -{}", deleted)); }
            if untracked > 0 { res.push_str(&format!(" ?{}", untracked)); }
            if renamed > 0 { res.push_str(&format!(" r{}", renamed)); }
            
            if staged == 0 && modified == 0 && untracked == 0 && deleted == 0 && renamed == 0 {
                format!(" {} (clean)", branch)
            } else {
                res
            }
        }
        Err(_) => "not a git repo".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_definitions() {
        let tools = get_all_tools();
        let shell = tools.iter().find(|t| t.function.name == "run_shell_command").unwrap();
        assert!(shell.function.parameters["required"].as_array().unwrap().iter().any(|v| v == "tool_call_id"));
        assert!(shell.function.parameters["required"].as_array().unwrap().iter().any(|v| v == "description"));
    }
}
