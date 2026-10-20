//! Shared classification of persisted legacy tool results.
pub(crate) fn legacy_large_output_storage_error_line(line: &str) -> bool {
    line.to_ascii_lowercase()
        .contains("full output was not saved because secure host storage rejected the path:")
}

pub(crate) fn legacy_tool_error_line_is_marker(line: &str) -> bool {
    let lower = line.trim_start().to_ascii_lowercase();
    if let Some(code) = lower.strip_prefix("exit_code:") {
        return code.trim().parse::<i32>() != Ok(0);
    }
    let encrypted_page_error = lower
        .strip_prefix("--- page ")
        .and_then(|rest| rest.split_once(" error:"))
        .is_some_and(|(page, _)| page.trim().parse::<usize>().is_ok());
    lower.starts_with("error:")
        || lower.starts_with("unknown tool:")
        || lower.starts_with("syntax error in tool call:")
        || lower.starts_with("failed to run install command:")
        || (lower.starts_with("auto-install of ") && lower.contains(" failed"))
        || lower.starts_with("lsp error:")
        || lower.starts_with("unknown lsp operation:")
        || lower.starts_with("gotodefinition requires ")
        || lower.starts_with("findreferences requires ")
        || lower.starts_with("hover requires ")
        || lower.starts_with("documentsymbol requires ")
        || lower.starts_with("workspacesymbol requires ")
        || lower == "[operation cancelled by user]"
        || lower.starts_with("sub-agent failed:")
        || lower.starts_with("sub-agent cancelled.")
        || lower.starts_with("sub-agent cancellation did not settle")
        || lower.starts_with("sub-agent timed out ")
        || encrypted_page_error
        || legacy_large_output_storage_error_line(line)
}

pub(crate) fn legacy_output_is_error(output: &str) -> bool {
    let mut nonempty = output.lines().filter(|line| !line.trim().is_empty());
    let first_line = nonempty.next().unwrap_or_default().trim_start();
    if legacy_tool_error_line_is_marker(first_line) {
        return true;
    }
    let lower = first_line.to_ascii_lowercase();
    (lower == "stdout:" && output.lines().any(|line| line.trim() == "STDERR:"))
        || (lower == "[find_symbol fallback]" && nonempty.any(legacy_tool_error_line_is_marker))
}
