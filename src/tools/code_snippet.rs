use serde_json::json;
use crate::tools::{Tool, FunctionDefinition};
use super::icons;
use super::llm_tokens;

pub fn get_definition() -> Tool {
    Tool {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "code_snippet".to_string(),
            description: "Store a code snippet for later use in other tools using the ***name*** placeholder".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "The unique name for this snippet"
                    },
                    "content": {
                        "type": "string",
                        "description": "The content of the snippet"
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
                "required": ["name", "content", "description", "tool_call_id"]
            }),
        },
    }
}

pub fn get_prompt_template() -> String {
    format!("{}declaration:code_snippet{{description:<|\">Store a code snippet for later use in other tools using the ***name*** placeholder<|\">,parameters:{{properties:{{content:{{description:<|\">The content of the snippet<|\">,type:<|\">STRING<|\">}},name:{{description:<|\">The unique name for this snippet<|\">,type:<|\">STRING<|\">}},description:{{description:<|\">Short description of the action<|\">,type:<|\">STRING<|\">}},tool_call_id:{{description:<|\">A unique, descriptive string identifier for this call (e.g., 'read_main_rs', 'check_folders'). Do not use simple numbers.<|\">,type:<|\">STRING<|\">}}}},required:[<|\">name<|\">,<|\">content<|\">,<|\">description<|\">,<|\">tool_call_id<|\">],type:<|\">OBJECT<|\">}}}}{}", llm_tokens::TOOL_CALL_OPEN, llm_tokens::TOOL_CALL_CLOSE)
}

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    if let Some(desc) = arguments["description"].as_str() {
        return format!("{} {}", icons::COMMAND, desc);
    }
    let name = arguments["name"].as_str().unwrap_or("");
    format!("{} Storing code snippet: `{}`", icons::COMMAND, name)
}
