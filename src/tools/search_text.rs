use serde_json::json;
use crate::tools::{Tool, FunctionDefinition};
use super::icons;
use super::llm_tokens;
use tokio::process::Command;

pub fn get_definition() -> Tool {
    Tool {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "search_text".to_string(),
            description: "Search for a regular expression pattern within files in a directory".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Directory or file to search. Defaults to '.' if omitted."
                    },
                    "pattern": {
                        "type": "string",
                        "description": "The regex pattern to search for"
                    },
                    "description": {
                        "type": "string",
                        "description": "Short description of the action"
                    },
                    "tool_call_id": {
                        "type": "string",
                        "description": "A unique, descriptive string identifier for this call (e.g., 'read_main_rs', 'check_folders'). Do not use simple numbers."
                    }
                },
                "required": ["pattern", "description", "tool_call_id"]
            }),
        },
    }
}

pub fn get_prompt_template() -> String {
    format!("{}declaration:search_text{{description:<|\">Search for a regular expression pattern within file contents.<|\">,parameters:{{properties:{{path:{{description:<|\">Directory or file to search (recursive if directory)<|\">,type:<|\">STRING<|\">}},pattern:{{description:<|\">The regex pattern to search for<|\">,type:<|\">STRING<|\">}},description:{{description:<|\">Short description of the action<|\">,type:<|\">STRING<|\">}},tool_call_id:{{description:<|\">A unique, descriptive string identifier for this call (e.g., 'read_main_rs', 'check_folders'). Do not use simple numbers.<|\">,type:<|\">STRING<|\">}}}},required:[<|\">pattern<|\">,<|\">description<|\">,<|\">tool_call_id<|\">],type:<|\">OBJECT<|\">}}}}{}", llm_tokens::TOOL_CALL_OPEN, llm_tokens::TOOL_CALL_CLOSE)
}

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    if let Some(desc) = arguments["description"].as_str() {
        return format!("{} {}", icons::SEARCH, desc);
    }
    let pattern = arguments["pattern"].as_str().unwrap_or("");
    let path = arguments["path"].as_str().unwrap_or(".");
    format!("{} Searching for `{}` in `{}`", icons::SEARCH, pattern, path)
}

pub async fn execute(pattern: &str, path: &str, cwd: &str) -> String {

    let search_path = if path.is_empty() { "." } else { path };

    let output = Command::new("grep")
        .arg("-rn")
        .arg("--color=never")
        .arg("-I")
        .arg(pattern)
        .arg(search_path)
        .current_dir(cwd)
        .output()
        .await;

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let status = out.status.code().map_or("signaled".to_string(), |c| c.to_string());
            if stdout.is_empty() && stderr.is_empty() && status == "1" {
                return "No matches found.".to_string();
            }
            format!("EXIT_CODE: {}\nSTDOUT:\n{}\nSTDERR:\n{}", status, stdout, stderr)
        }
        Err(e) => format!("ERROR: {}", e),
    }
}
