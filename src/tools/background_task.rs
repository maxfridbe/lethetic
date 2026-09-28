use super::icons;
use crate::background::{self, NotifyTarget, StartSpec, TaskSnapshot};
use crate::tools::{FunctionDefinition, Tool};
use serde_json::json;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Longest a single `wait` blocks before returning the current status.
const MAX_WAIT_SECONDS: u64 = 600;

pub fn get_definition() -> Tool {
    Tool {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "background_task".to_string(),
            description: "Run a long shell command in the background and keep working. Use it for downloads, builds, test suites, servers or anything slower than a minute. \
action=start launches `command` and returns an id (bg1, bg2…) at once. \
action=status shows state, elapsed time, progress, the time since its last output, and recent output. \
action=wait blocks until the task finishes or timeout_seconds pass. \
action=output returns more output lines. action=stop ends it. action=list shows every task. \
notify=model (default) tells you when it finishes; user tells only the person; none means you poll. \
Progress is read from percentages in the output; set progress_pattern for other formats, or watch_path (plus expected_bytes when known) for silent downloads. \
Full output is logged under .lethetic/background/.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["start", "status", "wait", "output", "stop", "list"],
                        "description": "What to do"
                    },
                    "command": {
                        "type": "string",
                        "description": "start: the bash command to run in the background"
                    },
                    "id": {
                        "type": "string",
                        "description": "status/wait/output/stop: the task id returned by start, e.g. bg1"
                    },
                    "notify": {
                        "type": "string",
                        "enum": ["model", "user", "none"],
                        "description": "start: who is told when it finishes (default model)"
                    },
                    "progress_pattern": {
                        "type": "string",
                        "description": "start: regex with one group (percent) or two groups (done, total), e.g. \"step (\\\\d+)/(\\\\d+)\""
                    },
                    "watch_path": {
                        "type": "string",
                        "description": "start: a file whose growing size shows progress, e.g. the download target"
                    },
                    "expected_bytes": {
                        "type": "integer",
                        "description": "start: final size of watch_path in bytes, when known"
                    },
                    "timeout_seconds": {
                        "type": "integer",
                        "description": "wait: most seconds to block (default 60, max 600)"
                    },
                    "lines": {
                        "type": "integer",
                        "description": "output: how many recent lines (default 50, max 400)"
                    },
                    "description": {
                        "type": "string",
                        "description": "Short description of the action"
                    },
                    "tool_call_id": {
                        "type": "string",
                        "description": "Unique identifier for this call"
                    }
                },
                "required": ["action", "description", "tool_call_id"]
            }),
        },
    }
}

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    let action = arguments["action"].as_str().unwrap_or("start");
    let detail = arguments["description"]
        .as_str()
        .or_else(|| arguments["command"].as_str())
        .unwrap_or("");
    let id = arguments["id"].as_str().map(|id| format!(" {id}")).unwrap_or_default();
    format!("{} Background {action}{id}: {detail}", icons::SHELL)
}

fn render(task: &TaskSnapshot, recent: usize) -> String {
    let mut out = format!(
        "{}\n[{}] {}\nCommand: {}\nNotify: {}\n",
        background::status_line(task),
        background::progress_bar(task.progress, 20, task.elapsed),
        task.progress_label.as_deref().unwrap_or("progress unknown"),
        task.command,
        task.notify.label(),
    );
    if let Some(path) = &task.log_path {
        out.push_str(&format!("Log: {}\n", path.display()));
    }
    if recent > 0
        && let Some(lines) = background::output_tail(&task.id, recent)
        && !lines.is_empty()
    {
        out.push_str(&format!("Recent output ({} lines total):\n", task.output_lines));
        for line in lines {
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

fn task_id(arguments: &serde_json::Value) -> Result<String, String> {
    arguments["id"]
        .as_str()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "ERROR: this action needs the task id, e.g. \"bg1\".".to_string())
}

fn unknown(id: &str) -> String {
    format!("ERROR: no background task {id}. Use action=list to see tasks.")
}

pub async fn execute(
    arguments: &serde_json::Value,
    cwd: &str,
    cancellation_token: CancellationToken,
) -> Result<String, String> {
    let action = arguments["action"].as_str().unwrap_or("").trim();
    match action {
        "start" => {
            let command = arguments["command"]
                .as_str()
                .map(str::trim)
                .filter(|command| !command.is_empty())
                .ok_or("ERROR: action=start needs a command.")?;
            let notify = NotifyTarget::parse(arguments["notify"].as_str())
                .map_err(|error| format!("ERROR: {error}"))?;
            let description = arguments["description"]
                .as_str()
                .filter(|text| !text.trim().is_empty())
                .unwrap_or(command)
                .to_string();
            let task = background::start(StartSpec {
                command: command.to_string(),
                description,
                cwd: cwd.to_string(),
                notify,
                progress_pattern: arguments["progress_pattern"].as_str().map(str::to_string),
                watch_path: arguments["watch_path"].as_str().map(str::to_string),
                expected_bytes: arguments["expected_bytes"].as_u64(),
                todo_id: arguments["todo_id"].as_str().map(str::to_string),
            })
            .map_err(|error| format!("ERROR: {error}"))?;
            let follow = match task.notify {
                NotifyTarget::User if notify == NotifyTarget::Model => {
                    "Automatic notification is turned off; poll with status or wait."
                }
                NotifyTarget::Model => "You will be told when it finishes; keep working meanwhile.",
                NotifyTarget::User => "Only the user is told when it finishes; poll with status or wait.",
                NotifyTarget::None => "Nobody is told when it finishes; poll with status or wait.",
            };
            Ok(format!(
                "Started background task {}. {follow}\n{}",
                task.id,
                render(&task, 0)
            ))
        }
        "status" => {
            let id = task_id(arguments)?;
            let task = background::snapshot(&id).ok_or_else(|| unknown(&id))?;
            if !task.state.is_running() {
                background::mark_delivered(&id);
            }
            Ok(render(&task, 20))
        }
        "wait" => {
            let id = task_id(arguments)?;
            let seconds = arguments["timeout_seconds"]
                .as_u64()
                .unwrap_or(60)
                .clamp(1, MAX_WAIT_SECONDS);
            let task = background::wait(&id, Duration::from_secs(seconds), &cancellation_token)
                .await
                .ok_or_else(|| unknown(&id))?;
            let prefix = if task.state.is_running() {
                format!("Still running after waiting {seconds}s.\n")
            } else {
                String::new()
            };
            Ok(format!("{prefix}{}", render(&task, 20)))
        }
        "output" => {
            let id = task_id(arguments)?;
            let task = background::snapshot(&id).ok_or_else(|| unknown(&id))?;
            let lines = arguments["lines"].as_u64().unwrap_or(50).clamp(1, 400) as usize;
            Ok(render(&task, lines))
        }
        "stop" => {
            let id = task_id(arguments)?;
            if background::snapshot(&id).is_none() {
                return Err(unknown(&id));
            }
            if !background::stop(&id) {
                return Ok(format!("Background task {id} had already finished."));
            }
            let task = background::wait(&id, Duration::from_secs(10), &cancellation_token)
                .await
                .ok_or_else(|| unknown(&id))?;
            Ok(render(&task, 5))
        }
        "list" => {
            let tasks = background::list();
            if tasks.is_empty() {
                return Ok("No background tasks.".to_string());
            }
            Ok(tasks
                .iter()
                .map(background::status_line)
                .collect::<Vec<_>>()
                .join("\n"))
        }
        other => Err(format!(
            "ERROR: unknown action {other:?}; use start, status, wait, output, stop or list."
        )),
    }
}
