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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_definitions() {
        let tools = get_all_tools();
        let shell = tools.iter().find(|t| t.function.name == "run_shell_command").unwrap();
        assert!(shell.function.parameters["required"].as_array().unwrap().iter().any(|v| v == "tool_call_id"));
    }
}
