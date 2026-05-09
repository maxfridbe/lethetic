use serde_json::json;
use crate::tools::{Tool, FunctionDefinition};
use super::icons;
use std::fs;
use std::path::Path;

pub fn get_definition() -> Tool {
    Tool {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "todowrite".to_string(),
            description: "Update the task todo list. Replaces the entire list with the provided todos. Use this to track your work: create tasks at the start, update status as you go, mark done when complete. Read back the current list by calling with the same todos.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "description": "The complete todo list (replaces the existing list)",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": {
                                    "type": "string",
                                    "description": "Short unique identifier, e.g. 'setup-db', 'write-tests'"
                                },
                                "content": {
                                    "type": "string",
                                    "description": "Description of the task"
                                },
                                "status": {
                                    "type": "string",
                                    "enum": ["pending", "in_progress", "completed", "cancelled"],
                                    "description": "Current status"
                                },
                                "priority": {
                                    "type": "string",
                                    "enum": ["high", "medium", "low"],
                                    "description": "Task priority"
                                }
                            },
                            "required": ["content", "status", "priority"]
                        }
                    },
                    "description": {
                        "type": "string",
                        "description": "Short description of the update"
                    },
                    "tool_call_id": {
                        "type": "string",
                        "description": "Unique identifier for this call"
                    }
                },
                "required": ["todos", "description", "tool_call_id"]
            }),
        },
    }
}

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    if let Some(desc) = arguments["description"].as_str() {
        return format!("{} {}", icons::COMMAND, desc);
    }
    let count = arguments["todos"].as_array().map_or(0, |a| a.len());
    format!("{} Todo update ({} tasks)", icons::COMMAND, count)
}

pub async fn execute(todos: &serde_json::Value, cwd: &str) -> String {
    let todo_dir = Path::new(cwd).join(".lethetic");
    let todo_path = todo_dir.join("todos.json");

    if let Err(e) = fs::create_dir_all(&todo_dir) {
        return format!("ERROR: Could not create .lethetic dir: {}", e);
    }

    if let Err(e) = fs::write(&todo_path, serde_json::to_string_pretty(todos).unwrap_or_default()) {
        return format!("ERROR: Could not write todos: {}", e);
    }

    // Build human-readable summary
    let items = match todos.as_array() {
        Some(a) => a,
        None => return "ERROR: todos must be an array".to_string(),
    };

    let mut output = format!("Todo list updated ({} tasks):\n\n", items.len());
    for item in items {
        let status = item["status"].as_str().unwrap_or("pending");
        let priority = item["priority"].as_str().unwrap_or("medium");
        let content = item["content"].as_str().unwrap_or("(no content)");
        let id = item["id"].as_str().unwrap_or("");

        let status_icon = match status {
            "completed"  => "✓",
            "in_progress"=> "→",
            "cancelled"  => "✗",
            _            => "○",
        };
        let pri_label = match priority {
            "high"   => "[H]",
            "low"    => "[L]",
            _        => "[M]",
        };
        let id_part = if id.is_empty() { String::new() } else { format!(" ({})", id) };
        output.push_str(&format!("{} {} {}{}\n", status_icon, pri_label, content, id_part));
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_todowrite_creates_file() {
        let dir = tempdir().unwrap();
        let todos = json!([
            {"id": "t1", "content": "Write tests", "status": "pending", "priority": "high"},
            {"id": "t2", "content": "Deploy", "status": "completed", "priority": "low"}
        ]);
        let result = execute(&todos, dir.path().to_str().unwrap()).await;
        assert!(result.contains("2 tasks"), "{}", result);
        assert!(result.contains("Write tests"));
        assert!(fs::read_to_string(dir.path().join(".lethetic/todos.json")).is_ok());
    }

    #[tokio::test]
    async fn test_todowrite_status_icons() {
        let dir = tempdir().unwrap();
        let todos = json!([
            {"content": "done task", "status": "completed", "priority": "medium"},
            {"content": "wip task",  "status": "in_progress", "priority": "high"}
        ]);
        let result = execute(&todos, dir.path().to_str().unwrap()).await;
        assert!(result.contains('✓'), "{}", result);
        assert!(result.contains('→'), "{}", result);
    }
}
