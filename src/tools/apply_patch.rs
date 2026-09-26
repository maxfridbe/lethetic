use super::icons;
use crate::tools::{FunctionDefinition, Tool, ToolExecution};
use serde_json::json;
use std::fs;
use std::path::Path;

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
        if line.len() >= 7
            && line
                .chars()
                .take(6)
                .all(|c| c.is_whitespace() || c.is_ascii_digit())
            && line.chars().nth(6) == Some('\t')
        {
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

fn classify_patch_process_output(
    file_path: &str,
    success: bool,
    stdout: &str,
    stderr: &str,
) -> Result<String, String> {
    if success && stderr.is_empty() {
        Ok(format!("Successfully patched {file_path}"))
    } else {
        Err(format!("STDOUT:\n{stdout}\nSTDERR:\n{stderr}"))
    }
}

pub async fn execute(
    file_path: &str,
    old_content: &str,
    new_content: &str,
    cwd: &str,
    cancellation_token: tokio_util::sync::CancellationToken,
) -> String {
    execute_classified(file_path, old_content, new_content, cwd, cancellation_token)
        .await
        .output
}

pub(super) async fn execute_classified(
    file_path: &str,
    old_content: &str,
    new_content: &str,
    cwd: &str,
    cancellation_token: tokio_util::sync::CancellationToken,
) -> ToolExecution {
    if cancellation_token.is_cancelled() {
        return ToolExecution::error("[Operation Cancelled by User]", cwd);
    }

    let mut cleaned_old = strip_line_numbers(old_content);
    let mut cleaned_new = strip_line_numbers(new_content);

    // If the LLM wrapped it in markdown code blocks, strip them
    if cleaned_old.starts_with("```") {
        let lines: Vec<&str> = cleaned_old.lines().collect();
        if lines.len() >= 2 && lines.last().unwrap_or(&"") == &"```" {
            cleaned_old = lines[1..lines.len() - 1].join("\n");
            if !cleaned_old.is_empty() && old_content.ends_with('\n') {
                cleaned_old.push('\n');
            }
        }
    }
    if cleaned_new.starts_with("```") {
        let lines: Vec<&str> = cleaned_new.lines().collect();
        if lines.len() >= 2 && lines.last().unwrap_or(&"") == &"```" {
            cleaned_new = lines[1..lines.len() - 1].join("\n");
            if new_content.ends_with('\n') {
                cleaned_new.push('\n');
            }
        }
    }
    if cleaned_old.is_empty() {
        return ToolExecution::error("ERROR: old_content must not be empty.", cwd);
    }

    let full_path = Path::new(cwd).join(file_path);
    let original_file_content = match fs::read_to_string(&full_path) {
        Ok(content) => content,
        Err(error) => {
            return ToolExecution::error(
                format!(
                    "ERROR: Failed to read file {}: {error}",
                    full_path.display()
                ),
                cwd,
            );
        }
    };
    if cancellation_token.is_cancelled() {
        return ToolExecution::error("[Operation Cancelled by User]", cwd);
    }

    let match_count = original_file_content.match_indices(&cleaned_old).count();
    if match_count == 0 {
        return ToolExecution::error(
            format!("ERROR: The `old_content` block was not found in {file_path}."),
            cwd,
        );
    }
    if match_count > 1 {
        return ToolExecution::error(
            format!(
                "ERROR: The `old_content` block matched {match_count} locations in {file_path}; include more surrounding context so it is unique."
            ),
            cwd,
        );
    }

    let new_file_content = original_file_content.replace(&cleaned_old, &cleaned_new);

    let patch = diffy::create_patch(&original_file_content, &new_file_content);
    let patch_str = patch.to_string();
    if cancellation_token.is_cancelled() {
        return ToolExecution::error("[Operation Cancelled by User]", cwd);
    }

    let patch_file = Path::new(cwd).join(".tmp.patch");
    if let Err(error) = fs::write(&patch_file, &patch_str) {
        return ToolExecution::error(
            format!("ERROR: Failed to write temp patch file: {error}"),
            cwd,
        );
    }

    let args: [&std::ffi::OsStr; 4] = [
        "-u".as_ref(),
        file_path.as_ref(),
        "-i".as_ref(),
        patch_file.as_os_str(),
    ];

    let result = tokio::select! {
        biased;
        _ = cancellation_token.cancelled() => {
            ToolExecution::error("[Operation Cancelled by User]", cwd)
        }
        output = crate::platform::command_output("patch", args, Some(cwd)) => {
            match output {
                Ok(out) => {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    match classify_patch_process_output(
                        file_path,
                        out.status.success(),
                        &stdout,
                        &stderr,
                    ) {
                        Ok(output) => ToolExecution::success(output, cwd),
                        Err(error) => ToolExecution::error(error, cwd),
                    }
                }
                Err(error) => ToolExecution::error(
                    format!(
                        "ERROR: Failed to run patch utility: {error}. Make sure 'patch' is installed on your system."
                    ),
                    cwd,
                ),
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
    fn failed_patch_process_output_remains_local_but_is_typed_as_an_error() {
        let raw = classify_patch_process_output(
            "fixture.txt",
            false,
            "patch stdout tenant violet req-patch-7",
            "patch stderr detail",
        )
        .unwrap_err();
        assert!(raw.contains("req-patch-7"));
        assert!(raw.starts_with("STDOUT:"));
        assert!(classify_patch_process_output("fixture.txt", true, "ok", "").is_ok());
        assert!(classify_patch_process_output("fixture.txt", true, "ok", "warning").is_err());
    }

    #[test]
    fn test_strip_line_numbers() {
        let input_with_numbers =
            "     1\tfunction test() {\n     2\t    console.log('hi');\n     3\t}";
        let expected = "function test() {\n    console.log('hi');\n}";
        assert_eq!(strip_line_numbers(input_with_numbers), expected);

        let input_without_numbers = "function test() {\n    console.log('hi');\n}";
        assert_eq!(
            strip_line_numbers(input_without_numbers),
            input_without_numbers
        );
    }

    #[tokio::test]
    async fn empty_old_content_is_rejected_without_writing() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fixture.txt");
        fs::write(&path, "ab\n").unwrap();

        for old_content in ["", "```text\n```\n"] {
            let result = execute_classified(
                "fixture.txt",
                old_content,
                "X",
                directory.path().to_str().unwrap(),
                tokio_util::sync::CancellationToken::new(),
            )
            .await;

            assert!(result.is_error);
            assert_eq!(result.output, "ERROR: old_content must not be empty.");
            assert_eq!(fs::read_to_string(&path).unwrap(), "ab\n");
            assert!(!directory.path().join(".tmp.patch").exists());
        }
    }

    #[tokio::test]
    async fn ambiguous_old_content_is_rejected_without_writing() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fixture.txt");
        fs::write(&path, "repeat\nrepeat\n").unwrap();

        let result = execute_classified(
            "fixture.txt",
            "repeat",
            "changed",
            directory.path().to_str().unwrap(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await;

        assert!(result.is_error);
        assert!(result.output.contains("matched 2 locations"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "repeat\nrepeat\n");
        assert!(!directory.path().join(".tmp.patch").exists());
    }

    #[tokio::test]
    async fn pre_cancelled_patch_does_not_write_or_create_patch_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fixture.txt");
        fs::write(&path, "old\n").unwrap();
        let cancellation = tokio_util::sync::CancellationToken::new();
        cancellation.cancel();

        let result = execute_classified(
            "fixture.txt",
            "old",
            "new",
            directory.path().to_str().unwrap(),
            cancellation,
        )
        .await;

        assert!(result.is_error);
        assert_eq!(result.output, "[Operation Cancelled by User]");
        assert_eq!(fs::read_to_string(path).unwrap(), "old\n");
        assert!(!directory.path().join(".tmp.patch").exists());
    }
}
