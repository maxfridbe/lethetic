use serde_json::json;
use crate::tools::{Tool, FunctionDefinition};
use super::icons;
use std::fs;
use std::path::Path;

pub fn get_definition() -> Tool {
    Tool {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "edit".to_string(),
            description: "Edit a file by replacing old_string with new_string. Handles minor whitespace and indentation differences that would cause replace_text to fail. Use this for multi-line edits; use replace_text for short exact replacements.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to edit"
                    },
                    "old_string": {
                        "type": "string",
                        "description": "The block of text to replace. Include enough surrounding context (2-3 lines) to make it unique."
                    },
                    "new_string": {
                        "type": "string",
                        "description": "The replacement text"
                    },
                    "description": {
                        "type": "string",
                        "description": "Short description of the change"
                    },
                    "tool_call_id": {
                        "type": "string",
                        "description": "Unique identifier for this call"
                    }
                },
                "required": ["path", "old_string", "new_string", "description", "tool_call_id"]
            }),
        },
    }
}

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    if let Some(desc) = arguments["description"].as_str() {
        return format!("{} {}", icons::SUCCESS, desc);
    }
    let path = arguments["path"].as_str().unwrap_or("");
    format!("{} Editing `{}`", icons::SUCCESS, path)
}

pub async fn execute(
    path: &str,
    old_string: &str,
    new_string: &str,
    cwd: &str,
    cancellation_token: tokio_util::sync::CancellationToken,
) -> String {
    let path = path.trim_matches(|c| c == '\'' || c == '"');
    let full_path = Path::new(cwd).join(path);

    tokio::select! {
        _ = cancellation_token.cancelled() => "[Operation Cancelled by User]".to_string(),
        result = apply_edit(&full_path, old_string, new_string, path) => result,
    }
}

async fn apply_edit(full_path: &Path, old_string: &str, new_string: &str, display_path: &str) -> String {
    let content = match fs::read_to_string(full_path) {
        Ok(c) => c,
        Err(e) => return format!("ERROR: Cannot read {}: {}", display_path, e),
    };

    // Strategy 1: exact match
    let exact_count = content.matches(old_string).count();
    if exact_count == 1 {
        let new_content = content.replacen(old_string, new_string, 1);
        return write_result(full_path, &new_content, display_path, "exact match");
    }
    if exact_count > 1 {
        let lines = match_line_numbers(&content, old_string);
        return format!(
            "ERROR: old_string matches {} times in {} (lines {}).\nAdd more surrounding context to make it unique.",
            exact_count, display_path,
            lines.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(", ")
        );
    }

    // Strategy 2: whitespace-normalized match
    let normalized_old = normalize_whitespace(old_string);
    let (matched, start_byte, end_byte) = find_normalized_match(&content, &normalized_old);

    if matched {
        let new_content = format!("{}{}{}", &content[..start_byte], new_string, &content[end_byte..]);
        return write_result(full_path, &new_content, display_path, "whitespace-normalized match");
    }

    // Strategy 3: line-by-line similarity
    if let Some((start_byte, end_byte, score)) = find_fuzzy_match(&content, old_string) {
        if score >= 0.60 {
            let new_content = format!("{}{}{}", &content[..start_byte], new_string, &content[end_byte..]);
            return write_result(full_path, &new_content, display_path,
                &format!("fuzzy match ({:.0}% similarity)", score * 100.0));
        }
        // Below threshold: show closest match to help the model correct old_string
        let closest = &content[start_byte..end_byte];
        let preview: String = closest.lines().take(5)
            .map(|l| format!("  {}", l))
            .collect::<Vec<_>>().join("\n");
        return format!(
            "ERROR: old_string not found in {}.\nClosest match ({:.0}% similarity):\n{}\n\nAdjust old_string to match exactly.",
            display_path, score * 100.0, preview
        );
    }

    format!("ERROR: old_string not found in {}. Check the text and file path.", display_path)
}

fn write_result(full_path: &Path, content: &str, display_path: &str, strategy: &str) -> String {
    match fs::write(full_path, content) {
        Ok(_) => format!("Successfully edited {} ({})", display_path, strategy),
        Err(e) => format!("ERROR: Failed to write {}: {}", display_path, e),
    }
}

fn match_line_numbers(content: &str, needle: &str) -> Vec<usize> {
    let mut results = Vec::new();
    let mut search_from = 0;
    while let Some(pos) = content[search_from..].find(needle) {
        let abs_pos = search_from + pos;
        let line_no = content[..abs_pos].lines().count() + 1;
        results.push(line_no);
        search_from = abs_pos + needle.len();
    }
    results
}

fn normalize_whitespace(s: &str) -> String {
    s.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn find_normalized_match(content: &str, normalized_old: &str) -> (bool, usize, usize) {
    let old_lines: Vec<&str> = normalized_old.lines().collect();
    let n = old_lines.len();
    if n == 0 { return (false, 0, 0); }

    let content_lines: Vec<&str> = content.lines().collect();
    let total = content_lines.len();

    for i in 0..total.saturating_sub(n) + 1 {
        let window: Vec<&str> = content_lines[i..i + n.min(total - i)]
            .iter()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect();

        if window == old_lines {
            // Find byte offsets for lines i..i+n
            let start_byte = byte_offset_of_line(content, i);
            let end_line = (i + n).min(total);
            let end_byte = if end_line >= total {
                content.len()
            } else {
                byte_offset_of_line(content, end_line)
            };
            return (true, start_byte, end_byte);
        }
    }
    (false, 0, 0)
}

// Sliding-window similarity: returns (start_byte, end_byte, score) for the best match
fn find_fuzzy_match(content: &str, old_string: &str) -> Option<(usize, usize, f64)> {
    let old_lines: Vec<&str> = old_string.lines().collect();
    let n = old_lines.len();
    if n == 0 { return None; }

    let content_lines: Vec<&str> = content.lines().collect();
    let total = content_lines.len();
    if total < n { return None; }

    let mut best_score = 0.0f64;
    let mut best_start = 0usize;
    let mut best_end = 0usize;

    for i in 0..=(total - n) {
        let window = &content_lines[i..i + n];
        let score = line_similarity(old_lines.as_slice(), window);
        if score > best_score {
            best_score = score;
            best_start = byte_offset_of_line(content, i);
            best_end = if i + n >= total {
                content.len()
            } else {
                byte_offset_of_line(content, i + n)
            };
        }
    }

    if best_score > 0.0 {
        Some((best_start, best_end, best_score))
    } else {
        None
    }
}

fn line_similarity(a: &[&str], b: &[&str]) -> f64 {
    if a.len() != b.len() { return 0.0; }
    let total: usize = a.iter().map(|l| l.len().max(1)).sum();
    let matching: usize = a.iter().zip(b.iter())
        .map(|(la, lb)| {
            let la = la.trim();
            let lb = lb.trim();
            if la == lb { la.len().max(1) }
            else { char_overlap(la, lb) }
        })
        .sum();
    matching as f64 / total as f64
}

fn char_overlap(a: &str, b: &str) -> usize {
    let common = a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count();
    common.min(a.len()).min(b.len())
}

fn byte_offset_of_line(content: &str, line_idx: usize) -> usize {
    content.char_indices()
        .filter(|(_, c)| *c == '\n')
        .nth(line_idx.saturating_sub(1))
        .map(|(i, _)| i + 1)
        .unwrap_or(if line_idx == 0 { 0 } else { content.len() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use std::fs;

    #[tokio::test]
    async fn test_edit_exact_match() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.rs");
        fs::write(&path, "fn hello() {\n    println!(\"hi\");\n}\n").unwrap();

        let token = tokio_util::sync::CancellationToken::new();
        let result = execute("test.rs", "println!(\"hi\");", "println!(\"hello world\");",
            dir.path().to_str().unwrap(), token).await;

        assert!(result.contains("Successfully"), "Expected success, got: {}", result);
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("hello world"));
    }

    #[tokio::test]
    async fn test_edit_whitespace_normalized() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.rs");
        // File has 4-space indentation
        fs::write(&path, "fn foo() {\n    let x = 1;\n    let y = 2;\n}\n").unwrap();

        let token = tokio_util::sync::CancellationToken::new();
        // old_string uses different indentation (tabs) — should still match via normalization
        let result = execute("test.rs",
            "let x = 1;\n\tlet y = 2;",
            "let x = 10;\n    let y = 20;",
            dir.path().to_str().unwrap(), token).await;

        assert!(result.contains("Successfully"), "Expected normalized match, got: {}", result);
    }

    #[tokio::test]
    async fn test_edit_multi_match_error() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.rs");
        fs::write(&path, "let x = 1;\nlet x = 1;\n").unwrap();

        let token = tokio_util::sync::CancellationToken::new();
        let result = execute("test.rs", "let x = 1;", "let x = 99;",
            dir.path().to_str().unwrap(), token).await;

        assert!(result.contains("ERROR") && result.contains("2"),
            "Expected multi-match error, got: {}", result);
    }

    #[tokio::test]
    async fn test_edit_not_found() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.rs");
        fs::write(&path, "fn main() {}\n").unwrap();

        let token = tokio_util::sync::CancellationToken::new();
        let result = execute("test.rs", "fn completely_different_name() {}",
            "fn replacement() {}", dir.path().to_str().unwrap(), token).await;

        assert!(result.contains("ERROR"), "Expected not-found error, got: {}", result);
    }
}
