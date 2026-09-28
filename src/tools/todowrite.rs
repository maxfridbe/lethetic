use super::icons;
use crate::todo_store::{TodoPriority, TodoStatus, TodoStore};
use crate::tools::{FunctionDefinition, Tool, ToolExecution};
use serde_json::json;
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
                                    "description": "Short unique identifier, e.g. 'setup-db', 'write-tests'. Other tool calls name it in their todo_id."
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
                            "required": ["id", "content", "status", "priority"]
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
    execute_classified(todos, cwd, tokio_util::sync::CancellationToken::new())
        .await
        .output
}

pub(super) async fn execute_classified(
    todos: &serde_json::Value,
    cwd: &str,
    cancellation_token: tokio_util::sync::CancellationToken,
) -> ToolExecution {
    if cancellation_token.is_cancelled() {
        return ToolExecution::error("[Operation Cancelled by User]", cwd);
    }
    let mut todos = match TodoStore::parse_todos(todos) {
        Ok(todos) => todos,
        Err(error) => return ToolExecution::error(format!("ERROR: {error}"), cwd),
    };
    // Every item needs an id so tool calls can name the item they serve.
    let mut next = 1;
    for index in 0..todos.len() {
        if todos[index]
            .id
            .as_deref()
            .is_none_or(|id| id.trim().is_empty())
        {
            while todos
                .iter()
                .any(|todo| todo.id.as_deref() == Some(&format!("t{next}")))
            {
                next += 1;
            }
            todos[index].id = Some(format!("t{next}"));
        }
    }
    let store = match TodoStore::open(Path::new(cwd)) {
        Ok(store) => store,
        Err(error) => {
            return ToolExecution::error(format!("ERROR: Could not open todo store: {error}"), cwd);
        }
    };
    if cancellation_token.is_cancelled() {
        return ToolExecution::error("[Operation Cancelled by User]", cwd);
    }
    let snapshot = match store.replace_current(todos) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return ToolExecution::error(
                format!("ERROR: Could not safely write todos: {error}"),
                cwd,
            );
        }
    };

    let mut output = format!(
        "Todo list updated ({} tasks, revision {}):\n\n",
        snapshot.todos.len(),
        snapshot.revision
    );
    for item in snapshot.todos {
        let status_icon = match item.status {
            TodoStatus::Completed => "✓",
            TodoStatus::InProgress => "→",
            TodoStatus::Cancelled => "✗",
            TodoStatus::Pending => "○",
        };
        let priority = match item.priority {
            TodoPriority::High => "[H]",
            TodoPriority::Medium => "[M]",
            TodoPriority::Low => "[L]",
        };
        let id = item.id.map(|id| format!(" ({id})")).unwrap_or_default();
        output.push_str(&format!("{status_icon} {priority} {}{id}\n", item.content));
    }
    ToolExecution::success(output, cwd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
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
    async fn test_todowrite_rejects_non_array_without_writing() {
        let dir = tempdir().unwrap();
        let todo_dir = dir.path().join(".lethetic");
        fs::create_dir(&todo_dir).unwrap();
        let todo_path = todo_dir.join("todos.json");
        fs::write(&todo_path, "existing-safe").unwrap();

        let result = execute(&json!({"not": "an array"}), dir.path().to_str().unwrap()).await;

        assert!(result.contains("Invalid todo list"), "{result}");
        assert_eq!(fs::read_to_string(todo_path).unwrap(), "existing-safe");
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

    #[tokio::test]
    async fn pre_cancelled_todowrite_does_not_create_a_store() {
        let dir = tempdir().unwrap();
        let token = tokio_util::sync::CancellationToken::new();
        token.cancel();

        let result = execute_classified(
            &json!([{"content": "never written", "status": "pending", "priority": "low"}]),
            dir.path().to_str().unwrap(),
            token,
        )
        .await;

        assert!(result.is_error);
        assert_eq!(result.output, "[Operation Cancelled by User]");
        assert!(!dir.path().join(".lethetic").exists());
    }
}
