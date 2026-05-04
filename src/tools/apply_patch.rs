use serde_json::json;
use crate::tools::{Tool, FunctionDefinition};
use super::icons;
use std::fs;
use std::path::Path;
use tokio::process::Command;

pub fn get_definition() -> Tool {
    Tool {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "apply_patch".to_string(),
            description: "Modify a file by replacing a block of text/code. Provide the smallest unique `old_content` block to be replaced and the corresponding `new_content`. The tool will generate and apply a patch.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "The path to the file to modify."
                    },
                    "old_content": {
                        "type": "string",
                        "description": "The exact, unique, multi-line block of text/code to find and replace."
                    },
                    "new_content": {
                        "type": "string",
                        "description": "The new multi-line block of text/code to insert."
                    },
                    "description": {
                        "type": "string",
                        "description": "Short description of the change."
                    },
                    "tool_call_id": {
                        "type": "string",
                        "description": "A unique identifier for this call."
                    }
                },
                "required": ["file_path", "old_content", "new_content", "description", "tool_call_id"]
            }),
        },
    }
}

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    if let Some(desc) = arguments["description"].as_str() {
        return format!("{} {}", icons::COMMAND, desc);
    }
    let path = arguments["file_path"].as_str().unwrap_or("");
    format!("{} Patching `{}`", icons::COMMAND, path)
}

fn strip_line_numbers(text: &str) -> String {
    let mut result = String::new();
    let mut stripped_any = false;
    for line in text.lines() {
        if line.len() >= 7 && line.chars().take(6).all(|c| c.is_whitespace() || c.is_ascii_digit()) && line.chars().nth(6) == Some('\t') {
            result.push_str(&line[7..]);
            stripped_any = true;
        } else {
            result.push_str(line);
        }
        result.push('\n');
    }
    
    if !stripped_any {
        return text.to_string();
    }
    
    if text.ends_with('\n') {
        result
    } else {
        result.trim_end_matches('\n').to_string()
    }
}

pub async fn execute(file_path: &str, old_content: &str, new_content: &str, cwd: &str, cancellation_token: tokio_util::sync::CancellationToken) -> String {
    let mut cleaned_old = strip_line_numbers(old_content);
    let mut cleaned_new = strip_line_numbers(new_content);
    
    // If the LLM wrapped it in markdown code blocks, strip them
    if cleaned_old.starts_with("```") {
        let lines: Vec<&str> = cleaned_old.lines().collect();
        if lines.len() >= 2 && lines.last().unwrap_or(&"") == &"```" {
            cleaned_old = lines[1..lines.len()-1].join("\n");
            if old_content.ends_with('\n') { cleaned_old.push('\n'); }
        }
    }
    if cleaned_new.starts_with("```") {
        let lines: Vec<&str> = cleaned_new.lines().collect();
        if lines.len() >= 2 && lines.last().unwrap_or(&"") == &"```" {
            cleaned_new = lines[1..lines.len()-1].join("\n");
            if new_content.ends_with('\n') { cleaned_new.push('\n'); }
        }
    }

    let full_path = Path::new(cwd).join(file_path);
    let original_file_content = match fs::read_to_string(&full_path) {
        Ok(content) => content,
        Err(e) => return format!("ERROR: Failed to read file {}: {}", full_path.display(), e),
    };

    if !original_file_content.contains(&cleaned_old) {
        return format!("ERROR: The `old_content` block was not found in {}.", file_path);
    }

    let new_file_content = original_file_content.replace(&cleaned_old, &cleaned_new);
    
    let patch = diffy::create_patch(&original_file_content, &new_file_content);
    let patch_str = patch.to_string();

    let patch_file = Path::new(cwd).join(".tmp.patch");
    if let Err(e) = fs::write(&patch_file, &patch_str) {
        return format!("ERROR: Failed to write temp patch file: {}", e);
    }

    let mut cmd = Command::new("patch");
    cmd.arg("-u")
        .arg(file_path)
        .arg("-i")
        .arg(&patch_file)
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    let child = cmd.spawn().expect("Failed to spawn patch");

    let result = tokio::select! {
        _ = cancellation_token.cancelled() => {
            "[Operation Cancelled by User]".to_string()
        }
        output = child.wait_with_output() => {
            match output {
                Ok(out) => {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    if out.status.success() && stderr.is_empty() {
                        format!("Successfully patched {}", file_path)
                    } else {
                        format!("STDOUT:
{}
STDERR:
{}", stdout, stderr)
                    }
                }
                Err(e) => format!("ERROR: {}", e),
            }
        }
    };

    let _ = fs::remove_file(patch_file);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_line_numbers() {
        let input_with_numbers = "     1\tfunction test() {\n     2\t    console.log('hi');\n     3\t}";
        let expected = "function test() {\n    console.log('hi');\n}";
        assert_eq!(strip_line_numbers(input_with_numbers), expected);

        let input_without_numbers = "function test() {\n    console.log('hi');\n}";
        assert_eq!(strip_line_numbers(input_without_numbers), input_without_numbers);
    }
}
