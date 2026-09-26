use super::{FunctionDefinition, Tool};
use serde_json::json;

pub fn get_definition() -> Tool {
    Tool {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "python".to_string(),
            description: "Execute one cell in the persistent Python runspace for this chat session. Imports, variables, and the working directory persist between cells. The injected lethetic_todo module provides revisioned host-backed task tracking. Large output is retained in bounded worker-local artifacts; recover it from a later Python cell with `import lethetic_output`, then lethetic_output.info(artifact_id) and lethetic_output.read(artifact_id, section, offset, limit). Returns bounded stdout, stderr, final-expression repr, or traceback excerpts. Interactive stdin is unavailable.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "code": {
                        "type": "string",
                        "description": "Complete Python source for one notebook-style cell."
                    },
                    "description": {
                        "type": "string",
                        "description": "A short description of what this cell does."
                    },
                    "tool_call_id": {
                        "type": "string",
                        "description": "The tool call identifier supplied by the model runtime."
                    }
                },
                "required": ["code", "description", "tool_call_id"]
            }),
        },
    }
}

pub fn validate_arguments(arguments: &serde_json::Value) -> Result<(&str, &str), String> {
    let object = arguments
        .as_object()
        .ok_or_else(|| "Python tool arguments must be a JSON object".to_string())?;
    let code = object
        .get("code")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "Python tool argument 'code' must be a string".to_string())?;
    let description = object
        .get("description")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "Python tool argument 'description' must be a string".to_string())?;
    Ok((code, description))
}

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    arguments["description"]
        .as_str()
        .filter(|description| !description.is_empty())
        .map(|description| format!("Python cell: {description}"))
        .unwrap_or_else(|| "Execute Python cell".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_code_and_description_types() {
        assert_eq!(
            validate_arguments(&json!({"code": "x = 1", "description": "assign"})).unwrap(),
            ("x = 1", "assign")
        );
        assert!(validate_arguments(&json!([])).is_err());
        assert!(validate_arguments(&json!({"description": "missing"})).is_err());
        assert!(validate_arguments(&json!({"code": 1, "description": "bad"})).is_err());
        assert!(validate_arguments(&json!({"code": "x", "description": false})).is_err());
    }

    #[test]
    fn definition_requires_code_description_and_call_id() {
        let definition = get_definition();
        let required = definition.function.parameters["required"]
            .as_array()
            .unwrap();
        for field in ["code", "description", "tool_call_id"] {
            assert!(required.iter().any(|value| value == field));
        }
    }
}
