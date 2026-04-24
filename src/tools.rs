use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

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

pub fn get_standard_tools() -> Vec<Tool> {
    vec![
        Tool {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "read_file".to_string(),
                description: "Read the complete content of a file".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "The path to the file"
                        },
                        "tool_call_id": {
                            "type": "string",
                            "description": "Required tracking ID"
                        }
                    },
                    "required": ["path", "tool_call_id"]
                }),
            },
        },
        Tool {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "read_file_lines".to_string(),
                description: "Read a specific range of lines from a file".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "The path to the file"
                        },
                        "start_line": {
                            "type": "integer",
                            "description": "The first line to read (1-indexed)"
                        },
                        "end_line": {
                            "type": "integer",
                            "description": "The last line to read (inclusive)"
                        },
                        "tool_call_id": {
                            "type": "string",
                            "description": "Required tracking ID"
                        }
                    },
                    "required": ["path", "start_line", "end_line", "tool_call_id"]
                }),
            },
        },
        Tool {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "apply_patch".to_string(),
                description: "Apply a unified diff patch to a file".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "The path to the file to patch"
                        },
                        "patch": {
                            "type": "string",
                            "description": "The unified diff content to apply"
                        },
                        "tool_call_id": {
                            "type": "string",
                            "description": "Required tracking ID"
                        }
                    },
                    "required": ["path", "patch", "tool_call_id"]
                }),
            },
        },
        Tool {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "run_shell_command".to_string(),
                description: "Run a bash command on the local system and return the output".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "The exact bash command to execute"
                        },
                        "tool_call_id": {
                            "type": "string",
                            "description": "Required tracking ID"
                        }
                    },
                    "required": ["command", "tool_call_id"]
                }),
            },
        },
        Tool {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "write_file".to_string(),
                description: "Write content to a file (overwrites existing)".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "The path to the file"
                        },
                        "content": {
                            "type": "string",
                            "description": "The full content to write"
                        },
                        "tool_call_id": {
                            "type": "string",
                            "description": "Required tracking ID"
                        }
                    },
                    "required": ["path", "content", "tool_call_id"]
                }),
            },
        },
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
                        "tool_call_id": {
                            "type": "string",
                            "description": "Required tracking ID"
                        }
                    },
                    "required": ["name", "content", "tool_call_id"]
                }),
            },
        },
        Tool {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "replace_text".to_string(),
                description: "Replace a literal string within a file with a new string. MUST match exactly one occurrence.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "The path to the file to modify"
                        },
                        "old_string": {
                            "type": "string",
                            "description": "The exact literal string to find and replace"
                        },
                        "new_string": {
                            "type": "string",
                            "description": "The new literal string to replace with"
                        },
                        "tool_call_id": {
                            "type": "string",
                            "description": "Required tracking ID"
                        }
                    },
                    "required": ["path", "old_string", "new_string", "tool_call_id"]
                }),
            },
        },
        Tool {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "calculate".to_string(),
                description: "Perform a mathematical calculation".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "expression": {
                            "type": "string",
                            "description": "The math expression to evaluate, e.g. '2 + 2' or 'sin(pi/2)'"
                        },
                        "tool_call_id": {
                            "type": "string",
                            "description": "Required tracking ID"
                        }
                    },
                    "required": ["expression", "tool_call_id"]
                }),
            },
        },
        Tool {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "ask_the_user".to_string(),
                description: "Ask the user for data, clarification, or to make a decision. Use this to pause execution and wait for human input.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "question": {
                            "type": "string",
                            "description": "The question to ask the user"
                        },
                        "tool_call_id": {
                            "type": "string",
                            "description": "Required tracking ID"
                        }
                    },
                    "required": ["question", "tool_call_id"]
                }),
            },
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_definitions() {
        let tools = get_standard_tools();
        let shell = tools.iter().find(|t| t.function.name == "run_shell_command").unwrap();
        assert!(shell.function.parameters["required"].as_array().unwrap().iter().any(|v| v == "tool_call_id"));
    }
}
