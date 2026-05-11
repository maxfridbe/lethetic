pub mod lsp;
pub mod task;
pub mod read_file;
pub mod read_file_lines;
pub mod read_folder;
pub mod search_text;
pub mod run_shell_command;
pub mod write_file;
pub mod replace_text;
pub mod edit;
pub mod glob;
pub mod find_symbol;
pub mod fetch_url;
pub mod web_fetch;    // kept for backwards-compat dispatch only
pub mod web_search;
pub mod read_page;    // kept for backwards-compat dispatch only
pub mod calculate;
pub mod ask_the_user;
pub mod process_image;
pub mod process_pdf_image;
pub mod get_pdf_text;
pub mod summarize_content;
pub mod apply_patch;
pub mod todowrite;
pub mod repo_overview;

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

/// Truncate large tool outputs and save to file. Returns (context_msg, ui_msg).
pub fn handle_large_output(id: &str, result: String) -> (String, String) {
    if result.len() > 20_000 {
        let file_id = if id.is_empty() { "unknown" } else { id };
        let dir_path = ".lethetic/tool_responses";
        let _ = std::fs::create_dir_all(dir_path);
        let file_path = format!("{}/{}.txt", dir_path, file_id);
        let _ = std::fs::write(&file_path, &result);

        let mut exit_status = String::new();
        if result.starts_with("EXIT_CODE: ") {
            if let Some(first) = result.lines().next() {
                exit_status = format!("{}\n", first);
            }
        }
        let truncated = format!(
            "{}... [Output truncated. Full output ({} characters) saved to {}] ...",
            exit_status, result.len(), file_path
        );
        let context_msg = format!(
            "{}... [OUTPUT TRUNCATED ({} characters). Full output saved to `{}`.\n\
             Recommended strategy:\n\
             1. Use `summarize_content` with the file path and prompt='describe all classes, functions, and what is not yet implemented' to understand the structure.\n\
             2. Then use `read_file_lines` to read only the specific sections you need.\n\
             Do NOT call read_file again on this path — it will always be truncated.] ...",
            exit_status, result.len(), file_path
        );
        (context_msg, truncated)
    } else {
        (result.clone(), result)
    }
}

/// Like get_all_prompt_templates but filters out named tools.
pub fn get_prompt_templates_excluding(config: &crate::config::Config, exclude: &[&str]) -> String {
    let mut templates = String::new();
    for tool in get_all_tools(config) {
        if !exclude.contains(&tool.function.name.as_str()) {
            templates.push_str("<|tool>\n");
            templates.push_str(&serde_json::to_string_pretty(&tool.function).unwrap());
            templates.push_str("\n<tool|>\n");
        }
    }
    templates
}

pub fn get_all_tools(config: &crate::config::Config) -> Vec<Tool> {
    let mut tools = vec![
        read_file::get_definition(),
        read_file_lines::get_definition(),
        read_folder::get_definition(),
        search_text::get_definition(),
        run_shell_command::get_definition(),
        write_file::get_definition(),
        replace_text::get_definition(),
        edit::get_definition(),
        glob::get_definition(),
        find_symbol::get_definition(),
        fetch_url::get_definition(),       // replaces web_fetch + read_page
        web_search::get_definition(),
        calculate::get_definition(),
        ask_the_user::get_definition(),
        apply_patch::get_definition(),
        get_pdf_text::get_definition(),
        summarize_content::get_definition(),
        todowrite::get_definition(),
        repo_overview::get_definition(),
        lsp::get_definition(),
        task::get_definition(),
    ];

    if config.enable_image_processing_tool {
        tools.push(process_image::get_definition());
        tools.push(process_pdf_image::get_definition());
    }

    tools
}

pub fn get_tool_parameter_names(func_name: &str, config: &crate::config::Config) -> Vec<String> {
    let tools = get_all_tools(config);
    if let Some(tool) = tools.iter().find(|t| t.function.name == func_name) {
        if let Some(properties) = tool.function.parameters.get("properties") {
            if let Some(obj) = properties.as_object() {
                return obj.keys().cloned().collect();
            }
        }
    }
    vec![]
}

pub fn get_all_prompt_templates(config: &crate::config::Config) -> String {
    let tools = get_all_tools(config);
    let mut templates = String::new();
    for tool in tools {
        templates.push_str("<|tool>
");
        templates.push_str(&serde_json::to_string_pretty(&tool.function).unwrap());
        templates.push_str("
<tool|>
");
    }
    templates
}

pub fn get_ui_description(func_name: &str, arguments: &serde_json::Value) -> String {
    match func_name {
        "read_file" => read_file::get_ui_description(arguments),
        "read_file_lines" => read_file_lines::get_ui_description(arguments),
        "read_folder" => read_folder::get_ui_description(arguments),
        "search_text" => search_text::get_ui_description(arguments),
        "run_shell_command" => run_shell_command::get_ui_description(arguments),
        "write_file" => write_file::get_ui_description(arguments),
        "replace_text" => replace_text::get_ui_description(arguments),
        "edit" => edit::get_ui_description(arguments),
        "glob" => glob::get_ui_description(arguments),
        "find_symbol" => find_symbol::get_ui_description(arguments),
        "fetch_url" => fetch_url::get_ui_description(arguments),
        "web_fetch" => web_fetch::get_ui_description(arguments),
        "web_search" => web_search::get_ui_description(arguments),
        "read_page" => read_page::get_ui_description(arguments),
        "calculate" => calculate::get_ui_description(arguments),
        "ask_the_user" => ask_the_user::get_ui_description(arguments),
        "apply_patch" => apply_patch::get_ui_description(arguments),
        "process_image" => process_image::get_ui_description(arguments),
        "process_pdf_image" => process_pdf_image::get_ui_description(arguments),
        "get_pdf_text" => get_pdf_text::get_ui_description(arguments),
        "summarize_content" => summarize_content::get_ui_description(arguments),
        "todowrite" => todowrite::get_ui_description(arguments),
        "repo_overview" => repo_overview::get_ui_description(arguments),
        "lsp" => lsp::get_ui_description(arguments),
        "task" => task::get_ui_description(arguments),
        _ => format!("{} {}: {}", icons::COMMAND, func_name, arguments),
    }
}

pub fn execute<'a>(
    func_name: &'a str,
    arguments: &'a serde_json::Value,
    cwd: &'a str,
    cancellation_token: tokio_util::sync::CancellationToken,
    tx: tokio::sync::mpsc::UnboundedSender<crate::client::StreamEvent>,
    client: &'a reqwest::Client,
    config: &'a crate::config::Config,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = (String, String)> + Send + 'a>> {
    Box::pin(async move {
    match func_name {
        "read_file" => {
            let path = arguments["path"].as_str().unwrap_or("");
            let max_lines = arguments["max_lines"].as_u64().map(|v| v as usize);
            (read_file::execute(path, max_lines, cwd, cancellation_token).await, cwd.to_string())
        }
        "read_file_lines" => {
            let path = arguments["path"].as_str().unwrap_or("");
            let start = arguments["start_line"].as_u64().unwrap_or(1) as usize;
            let end = arguments["end_line"].as_u64().unwrap_or(1) as usize;
            (read_file_lines::execute(path, start, end, cwd, cancellation_token).await, cwd.to_string())
        }
        "read_folder" => {
            let path = arguments["path"].as_str().unwrap_or(".");
            (read_folder::execute(path, cwd, cancellation_token).await, cwd.to_string())
        }
        "search_text" => {
            let pattern = arguments["pattern"].as_str().unwrap_or("");
            let path = arguments["path"].as_str().unwrap_or(".");
            (search_text::execute(pattern, path, cwd, cancellation_token).await, cwd.to_string())
        }
        "run_shell_command" => {
            let command = arguments["command"].as_str().unwrap_or("");
            run_shell_command::execute(command, cwd, cancellation_token, tx).await
        }
        "write_file" => {
            let path = arguments["path"].as_str().unwrap_or("");
            let content = arguments["content"].as_str().unwrap_or("");
            (write_file::execute(path, content, cwd, cancellation_token).await, cwd.to_string())
        }
        "replace_text" => {
            let path = arguments["path"].as_str().unwrap_or("");
            let old_string = arguments["old_string"].as_str().unwrap_or("");
            let new_string = arguments["new_string"].as_str().unwrap_or("");
            let replace_all = arguments["replace_all"].as_bool().unwrap_or(false);
            (replace_text::execute(path, old_string, new_string, replace_all, cwd, cancellation_token).await, cwd.to_string())
        }
        "edit" => {
            let path = arguments["path"].as_str().unwrap_or("");
            let old_string = arguments["old_string"].as_str().unwrap_or("");
            let new_string = arguments["new_string"].as_str().unwrap_or("");
            (edit::execute(path, old_string, new_string, cwd, cancellation_token).await, cwd.to_string())
        }
        "glob" => {
            let pattern = arguments["pattern"].as_str().unwrap_or("*");
            let path = arguments["path"].as_str().unwrap_or(".");
            (glob::execute(pattern, path, cwd, cancellation_token).await, cwd.to_string())
        }
        "find_symbol" => {
            let operation = arguments["operation"].as_str().unwrap_or("references");
            let symbol = arguments["symbol"].as_str().unwrap_or("");
            let path = arguments["path"].as_str().unwrap_or(".");
            (find_symbol::execute(operation, symbol, path, cwd, cancellation_token).await, cwd.to_string())
        }
        "lsp" => {
            let operation = arguments["operation"].as_str().unwrap_or("");
            let file_path = arguments["filePath"].as_str();
            let line = arguments["line"].as_u64().map(|v| v as u32);
            let character = arguments["character"].as_u64().map(|v| v as u32);
            let query = arguments["query"].as_str();
            (lsp::execute(operation, file_path, line, character, query, cwd, cancellation_token, tx).await, cwd.to_string())
        }
        "task" => {
            let prompt = arguments["prompt"].as_str().unwrap_or("");
            (task::execute(prompt, cwd, cancellation_token, tx, client, config).await, cwd.to_string())
        }
        "fetch_url" => {
            let url = arguments["url"].as_str().unwrap_or("");
            let format = arguments["format"].as_str().unwrap_or("markdown");
            (fetch_url::execute(url, format, cancellation_token).await, cwd.to_string())
        }
        // Legacy backwards-compat: keep dispatching web_fetch and read_page
        "web_fetch" => {
            let url = arguments["url"].as_str().unwrap_or("");
            (web_fetch::execute(url, cancellation_token).await, cwd.to_string())
        }
        "web_search" => {
            let query = arguments["query"].as_str().unwrap_or("");
            let num_results = arguments["num_results"].as_u64().unwrap_or(10) as usize;
            (web_search::execute(query, num_results, cancellation_token).await, cwd.to_string())
        }
        "read_page" => {
            let url = arguments["url"].as_str().unwrap_or("");
            (read_page::execute(url, cancellation_token).await, cwd.to_string())
        }
        "calculate" => {
            let expression = arguments["expression"].as_str().unwrap_or("");
            (calculate::execute(expression).await, cwd.to_string())
        }
        "ask_the_user" => {
            let question = arguments["question"].as_str().unwrap_or("");
            (question.to_string(), cwd.to_string())
        }
        "apply_patch" => {
            let file_path = arguments["file_path"].as_str().unwrap_or("");
            let old_content = arguments["old_content"].as_str().unwrap_or("");
            let new_content = arguments["new_content"].as_str().unwrap_or("");
            (apply_patch::execute(file_path, old_content, new_content, cwd, cancellation_token).await, cwd.to_string())
        }
        "process_image" => {
            if !config.enable_image_processing_tool {
                return (format!("ERROR: Image processing tool is disabled in config."), cwd.to_string());
            }
            let prompt = arguments["prompt"].as_str().unwrap_or("");
            let image_path = arguments["image_path"].as_str().unwrap_or("");
            let max_size = arguments["max_size"].as_u64().map(|v| v as u32);
            (process_image::execute(prompt, image_path, max_size, cwd, client, config, &tx).await, cwd.to_string())
        }
        "process_pdf_image" => {
            if !config.enable_image_processing_tool {
                return (format!("ERROR: PDF image processing tool is disabled in config."), cwd.to_string());
            }
            let prompt = arguments["prompt"].as_str().unwrap_or("");
            let pdf_path = arguments["pdf_path"].as_str().unwrap_or("");
            let page_num = arguments["page_num"].as_u64().unwrap_or(1) as usize;
            let max_size = arguments["max_size"].as_u64().map(|v| v as u32);
            (process_pdf_image::execute(prompt, pdf_path, page_num, max_size, cwd, client, config, &tx).await, cwd.to_string())
        }
        "get_pdf_text" => {
            let pdf_path = arguments["pdf_path"].as_str().unwrap_or("");
            (get_pdf_text::execute(pdf_path, cwd, &tx).await, cwd.to_string())
        }
        "summarize_content" => {
            let path = arguments["path"].as_str();
            let content = arguments["content"].as_str();
            let prompt = arguments["prompt"].as_str();
            (summarize_content::execute(path, content, prompt, cwd, client, config).await, cwd.to_string())
        }
        "todowrite" => {
            let todos = &arguments["todos"];
            (todowrite::execute(todos, cwd).await, cwd.to_string())
        }
        "repo_overview" => {
            let path = arguments["path"].as_str().unwrap_or(".");
            (repo_overview::execute(path, cwd, cancellation_token).await, cwd.to_string())
        }
        _ => (format!("Unknown tool: {}", func_name), cwd.to_string()),
    }
    }) // Box::pin
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
    use crate::config::Config;

    #[test]
    fn test_tool_definitions() {
        let config = Config {
            server_url: "".to_string(),
            model: "".to_string(),
            context_size: 0,
            tool_wrapper: None,
            enable_image_processing_tool: false,
            theme: None,
        
            model_servers: Vec::new(),
        };
        let tools = get_all_tools(&config);
        let shell = tools.iter().find(|t| t.function.name == "run_shell_command").unwrap();
        assert!(shell.function.parameters["required"].as_array().unwrap().iter().any(|v| v == "tool_call_id"));
        assert!(shell.function.parameters["required"].as_array().unwrap().iter().any(|v| v == "description"));
    }
}
