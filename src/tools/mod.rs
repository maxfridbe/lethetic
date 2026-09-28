pub mod apply_patch;
pub mod ask_the_user;
pub mod calculate;
pub mod edit;
pub mod fetch_url;
pub mod find_symbol;
pub mod get_pdf_text;
pub mod glob;
mod http_response;
pub mod lsp;
mod output;
pub mod process_image;
pub mod process_pdf_image;
pub mod python;
pub mod read_file;
pub mod read_file_lines;
pub mod read_folder;
pub mod read_page; // kept for backwards-compat dispatch only
pub mod replace_text;
pub mod repo_overview;
pub mod background_task;
pub mod run_shell_command;
pub mod search_text;
pub mod summarize_content;
pub mod task;
pub mod todowrite;
pub mod web_fetch; // kept for backwards-compat dispatch only
pub mod web_search;
pub mod write_file;

pub use crate::{icons, llm_tokens};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) use output::present_tool_execution_in;
#[cfg(test)]
use output::{LARGE_OUTPUT_THRESHOLD, handle_large_output_classified_in, large_output_file_name};
pub use output::{
    LargeOutputHandling, PresentedToolExecution, ToolOutputProvenance, handle_large_output,
    handle_large_output_classified, present_tool_execution,
};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolExecution {
    pub output: String,
    pub cwd: String,
    pub is_error: bool,
    pub provenance: ToolOutputProvenance,
}

impl ToolExecution {
    pub fn success(output: impl Into<String>, cwd: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            cwd: cwd.into(),
            is_error: false,
            provenance: ToolOutputProvenance::OrdinaryHost,
        }
    }

    pub fn error(output: impl Into<String>, cwd: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            cwd: cwd.into(),
            is_error: true,
            provenance: ToolOutputProvenance::OrdinaryHost,
        }
    }

    fn from_legacy(output: String, cwd: String) -> Self {
        let is_error = legacy_output_is_error(&output);
        Self {
            output,
            cwd,
            is_error,
            provenance: ToolOutputProvenance::OrdinaryHost,
        }
    }
}

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

pub(crate) fn legacy_persisted_tool_error_marker_offset(output: &str) -> Option<usize> {
    let mut offset = 0usize;
    let mut lines = Vec::new();
    for line in output.split_inclusive('\n') {
        let trimmed = line.trim_start();
        lines.push((offset + line.len().saturating_sub(trimmed.len()), trimmed));
        offset = offset.saturating_add(line.len());
    }
    if output.is_empty() {
        return None;
    }
    let first_nonempty_index = lines.iter().position(|(_, line)| !line.trim().is_empty());

    let mut later_has_stderr = vec![false; lines.len()];
    let mut later_has_marker = vec![false; lines.len()];
    let mut seen_stderr = false;
    let mut seen_marker = false;
    for index in (0..lines.len()).rev() {
        later_has_stderr[index] = seen_stderr;
        later_has_marker[index] = seen_marker;
        let line = lines[index].1.trim();
        seen_stderr |= line.to_ascii_lowercase().starts_with("stderr:");
        seen_marker |= legacy_tool_error_line_is_marker(line);
    }

    let mut shell_header_stage = 0_u8;
    for (index, (line_offset, line)) in lines.iter().enumerate() {
        let line = line.trim_end_matches(['\r', '\n']);
        let lower = line.to_ascii_lowercase();
        if let Some(code) = lower.strip_prefix("exit_code:") {
            if code.trim().parse::<i32>() != Ok(0) {
                return Some(*line_offset);
            }
            if Some(index) == first_nonempty_index {
                shell_header_stage = 1;
            }
            continue;
        }
        if shell_header_stage != 0 && lower.contains("[output truncated") {
            if legacy_large_output_storage_error_line(line) {
                return Some(*line_offset);
            }
            shell_header_stage = 0;
            continue;
        }
        if shell_header_stage == 1 {
            if lower == "stdout:" {
                shell_header_stage = 2;
                continue;
            }
            if !line.trim().is_empty() {
                shell_header_stage = 0;
            }
        }
        if shell_header_stage == 2 && lower == "stderr:" {
            shell_header_stage = 0;
            continue;
        }
        if legacy_tool_error_line_is_marker(line) || lower.starts_with("stderr:") {
            return Some(*line_offset);
        }
        if (lower == "stdout:" || lower == "[find_symbol fallback]") && later_has_stderr[index] {
            return Some(*line_offset);
        }
        if lower == "[find_symbol fallback]" && later_has_marker[index] {
            return Some(*line_offset);
        }
    }
    None
}

/// Like get_all_prompt_templates but filters out named tools.
pub fn get_prompt_templates_excluding(config: &crate::config::Config, exclude: &[&str]) -> String {
    let mut templates = String::new();
    for tool in get_all_tools(config) {
        if !exclude.contains(&tool.function.name.as_str()) {
            templates.push_str("<|tool>\n");
            templates.push_str(&serde_json::to_string_pretty(&tool.function).unwrap());
            templates.push_str("\n<tool|>\n");
        }
    }
    templates
}

pub use crate::tool_runtime::ToolSurface;

pub const INTERNAL_TOOL_CALL_ID_KEY: &str = "__lethetic_canonical_tool_call_id";

fn general_tools(config: &crate::config::Config) -> Vec<Tool> {
    let active_parser = config.active_parser();
    let mut tools = vec![
        read_file::get_definition(),
        read_file_lines::get_definition(),
        read_folder::get_definition(),
        search_text::get_definition(),
        run_shell_command::get_definition(),
        background_task::get_definition(),
        write_file::get_definition(active_parser),
        replace_text::get_definition(),
        edit::get_definition(),
        glob::get_definition(),
        find_symbol::get_definition(),
        fetch_url::get_definition(),
        web_search::get_definition(),
        calculate::get_definition(),
        ask_the_user::get_definition(),
        apply_patch::get_definition(),
        get_pdf_text::get_definition(),
        summarize_content::get_definition(),
        todowrite::get_definition(),
        repo_overview::get_definition(),
        lsp::get_definition(),
        task::get_definition(),
    ];

    if crate::background::mode() == crate::background::BackgroundMode::Off {
        tools.retain(|tool| tool.function.name != "background_task");
    }
    if config.enable_image_processing_tool {
        tools.push(process_image::get_definition());
        tools.push(process_pdf_image::get_definition());
    }
    for tool in &mut tools {
        add_todo_reference(tool);
    }
    tools
}

/// Tools that name a todo item. `todowrite` creates the items, so it is
/// the one tool that does not.
fn serves_todo(name: &str) -> bool {
    name != "todowrite"
}

/// Add a required `todo_id` argument: every call says which plan item it
/// serves, and the harness warns when that item is missing or finished.
fn add_todo_reference(tool: &mut Tool) {
    if !serves_todo(&tool.function.name) {
        return;
    }
    let parameters = &mut tool.function.parameters;
    if let Some(properties) = parameters["properties"].as_object_mut() {
        properties.insert(
            "todo_id".to_string(),
            serde_json::json!({
                "type": "string",
                "description": "id of the todo item (from the <todos> list) this call works on. Create or update the list with todowrite first."
            }),
        );
    }
    if let Some(required) = parameters["required"].as_array_mut() {
        required.push(serde_json::json!("todo_id"));
    }
}

/// Harness note appended to a tool result when its `todo_id` does not match
/// an open item in `.lethetic/todos.json`. Never blocks the call.
pub fn todo_reference_warning(
    func_name: &str,
    arguments: &serde_json::Value,
    workspace: &std::path::Path,
) -> Option<String> {
    if !serves_todo(func_name) {
        return None;
    }
    let snapshot = crate::todo_store::TodoStore::open(workspace)
        .and_then(|store| store.get())
        .unwrap_or_default();
    let ids: Vec<&str> = snapshot
        .todos
        .iter()
        .filter_map(|todo| todo.id.as_deref())
        .collect();
    let known = if ids.is_empty() {
        "the todo list is empty".to_string()
    } else {
        format!("known ids: {}", ids.join(", "))
    };
    let Some(todo_id) = arguments["todo_id"]
        .as_str()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    else {
        return Some(format!(
            "⚠ HARNESS WARNING: this call did not name a todo_id ({known}). Keep the plan current with todowrite and name the item each call serves."
        ));
    };
    match snapshot
        .todos
        .iter()
        .find(|todo| todo.id.as_deref() == Some(todo_id))
    {
        None => Some(format!(
            "⚠ HARNESS WARNING: todo_id '{todo_id}' is not in the todo list ({known}). Add it with todowrite or use an existing id."
        )),
        Some(todo)
            if matches!(
                todo.status,
                crate::todo_store::TodoStatus::Completed | crate::todo_store::TodoStatus::Cancelled
            ) =>
        {
            Some(format!(
                "⚠ HARNESS WARNING: todo_id '{todo_id}' is already {}. Reopen it or pick the item you are actually working on with todowrite.",
                todo.status.as_str()
            ))
        }
        Some(_) => None,
    }
}

pub fn get_tools_for_surface(config: &crate::config::Config, surface: ToolSurface) -> Vec<Tool> {
    match config.tool_profile {
        crate::config::ToolProfile::PythonOnly => vec![python::get_definition()],
        crate::config::ToolProfile::General => {
            let mut tools = general_tools(config);
            if surface == ToolSurface::Headless {
                // Headless runs have no run loop to push finishes back to.
                tools.retain(|tool| {
                    !matches!(
                        tool.function.name.as_str(),
                        "task" | "ask_the_user" | "background_task"
                    )
                });
            }
            tools
        }
    }
}

pub fn get_all_tools(config: &crate::config::Config) -> Vec<Tool> {
    get_tools_for_surface(config, ToolSurface::Interactive)
}

pub fn get_api_tools(
    config: &crate::config::Config,
    surface: ToolSurface,
) -> Vec<crate::transport::ToolDefinition> {
    get_tools_for_surface(config, surface)
        .into_iter()
        .map(|tool| crate::transport::ToolDefinition {
            name: tool.function.name,
            description: tool.function.description,
            input_schema: tool.function.parameters,
        })
        .collect()
}

pub fn is_tool_allowed(config: &crate::config::Config, surface: ToolSurface, name: &str) -> bool {
    if config.tool_profile == crate::config::ToolProfile::General
        && matches!(name, "web_fetch" | "read_page")
    {
        return true;
    }
    get_tools_for_surface(config, surface)
        .iter()
        .any(|tool| tool.function.name == name)
}

pub fn tool_admission_error(
    config: &crate::config::Config,
    surface: ToolSurface,
    name: &str,
) -> Option<String> {
    if let Some(error) = config.python_mode_validation_error() {
        return Some(format!("Invalid Python-only tool policy: {error}"));
    }
    (!is_tool_allowed(config, surface, name)).then(|| {
        format!(
            "Tool '{name}' is not allowed in the {:?} {:?} tool policy.",
            config.tool_profile, surface
        )
    })
}

pub fn get_tool_parameter_names(func_name: &str, config: &crate::config::Config) -> Vec<String> {
    let tools = get_all_tools(config);
    if let Some(tool) = tools.iter().find(|t| t.function.name == func_name)
        && let Some(properties) = tool.function.parameters.get("properties")
        && let Some(obj) = properties.as_object()
    {
        return obj.keys().cloned().collect();
    }
    vec![]
}

pub fn get_all_prompt_templates(config: &crate::config::Config) -> String {
    let tools = get_all_tools(config);
    let mut templates = String::new();
    for tool in tools {
        templates.push_str(
            "<|tool>
",
        );
        templates.push_str(&serde_json::to_string_pretty(&tool.function).unwrap());
        templates.push_str(
            "
<tool|>
",
        );
    }
    templates
}

pub fn get_ui_description(func_name: &str, arguments: &serde_json::Value) -> String {
    match func_name {
        "read_file" => read_file::get_ui_description(arguments),
        "read_file_lines" => read_file_lines::get_ui_description(arguments),
        "read_folder" => read_folder::get_ui_description(arguments),
        "search_text" => search_text::get_ui_description(arguments),
        "run_shell_command" => run_shell_command::get_ui_description(arguments),
        "background_task" => background_task::get_ui_description(arguments),
        "write_file" => write_file::get_ui_description(arguments),
        "replace_text" => replace_text::get_ui_description(arguments),
        "edit" => edit::get_ui_description(arguments),
        "glob" => glob::get_ui_description(arguments),
        "find_symbol" => find_symbol::get_ui_description(arguments),
        "fetch_url" => fetch_url::get_ui_description(arguments),
        "web_fetch" => web_fetch::get_ui_description(arguments),
        "web_search" => web_search::get_ui_description(arguments),
        "read_page" => read_page::get_ui_description(arguments),
        "calculate" => calculate::get_ui_description(arguments),
        "ask_the_user" => ask_the_user::get_ui_description(arguments),
        "apply_patch" => apply_patch::get_ui_description(arguments),
        "process_image" => process_image::get_ui_description(arguments),
        "process_pdf_image" => process_pdf_image::get_ui_description(arguments),
        "get_pdf_text" => get_pdf_text::get_ui_description(arguments),
        "summarize_content" => summarize_content::get_ui_description(arguments),
        "todowrite" => todowrite::get_ui_description(arguments),
        "repo_overview" => repo_overview::get_ui_description(arguments),
        "lsp" => lsp::get_ui_description(arguments),
        "python" => python::get_ui_description(arguments),
        "task" => task::get_ui_description(arguments),
        _ => format!("{} {}: {}", icons::COMMAND, func_name, arguments),
    }
}

pub fn execute<'a>(
    func_name: &'a str,
    arguments: &'a serde_json::Value,
    cwd: &'a str,
    cancellation_token: tokio_util::sync::CancellationToken,
    tx: tokio::sync::mpsc::UnboundedSender<crate::client::StreamEvent>,
    client: &'a reqwest::Client,
    config: &'a crate::config::Config,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolExecution> + Send + 'a>> {
    execute_for_surface(
        ToolSurface::Interactive,
        func_name,
        arguments,
        cwd,
        cancellation_token,
        tx,
        client,
        config,
    )
}

pub fn execute_for_surface<'a>(
    surface: ToolSurface,
    func_name: &'a str,
    arguments: &'a serde_json::Value,
    cwd: &'a str,
    cancellation_token: tokio_util::sync::CancellationToken,
    tx: tokio::sync::mpsc::UnboundedSender<crate::client::StreamEvent>,
    client: &'a reqwest::Client,
    config: &'a crate::config::Config,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolExecution> + Send + 'a>> {
    Box::pin(async move {
        let runtime = crate::tool_runtime::ToolRuntime::new(surface, std::path::PathBuf::from(cwd));
        execute_with_runtime(
            &runtime,
            func_name,
            arguments,
            cwd,
            cancellation_token,
            tx,
            client,
            config,
        )
        .await
    })
}

pub fn execute_with_runtime<'a>(
    runtime: &'a crate::tool_runtime::ToolRuntime,
    func_name: &'a str,
    arguments: &'a serde_json::Value,
    cwd: &'a str,
    cancellation_token: tokio_util::sync::CancellationToken,
    tx: tokio::sync::mpsc::UnboundedSender<crate::client::StreamEvent>,
    client: &'a reqwest::Client,
    config: &'a crate::config::Config,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolExecution> + Send + 'a>> {
    execute_with_runtime_and_request_hook(
        runtime,
        func_name,
        arguments,
        cwd,
        cancellation_token,
        tx,
        client,
        config,
        None,
    )
}

pub fn execute_with_runtime_and_request_hook<'a>(
    runtime: &'a crate::tool_runtime::ToolRuntime,
    func_name: &'a str,
    arguments: &'a serde_json::Value,
    cwd: &'a str,
    cancellation_token: tokio_util::sync::CancellationToken,
    tx: tokio::sync::mpsc::UnboundedSender<crate::client::StreamEvent>,
    client: &'a reqwest::Client,
    config: &'a crate::config::Config,
    request_hook: Option<crate::client::RequestStartedHook>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolExecution> + Send + 'a>> {
    Box::pin(async move {
        let mut execution = execute_dispatch(
            runtime,
            func_name,
            arguments,
            cwd,
            cancellation_token,
            tx,
            client,
            config,
            request_hook,
        )
        .await;
        if config.tool_profile == crate::config::ToolProfile::General
            && !execution.output.starts_with("[Operation Cancelled")
            && let Some(warning) =
                todo_reference_warning(func_name, arguments, runtime.workspace_root())
        {
            execution.output = format!("{}\n\n{warning}", execution.output);
        }
        execution
    })
}

#[allow(clippy::too_many_arguments)]
fn execute_dispatch<'a>(
    runtime: &'a crate::tool_runtime::ToolRuntime,
    func_name: &'a str,
    arguments: &'a serde_json::Value,
    cwd: &'a str,
    cancellation_token: tokio_util::sync::CancellationToken,
    tx: tokio::sync::mpsc::UnboundedSender<crate::client::StreamEvent>,
    client: &'a reqwest::Client,
    config: &'a crate::config::Config,
    request_hook: Option<crate::client::RequestStartedHook>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolExecution> + Send + 'a>> {
    Box::pin(async move {
        let surface = runtime.surface();
        if let Some(error) = tool_admission_error(config, surface, func_name) {
            return ToolExecution::error(format!("ERROR: {error}"), cwd);
        }
        if cancellation_token.is_cancelled() {
            return ToolExecution::error("[Operation Cancelled by User]", cwd);
        }
        let (output, new_cwd) = match func_name {
            "python" => {
                let (code, _) = match python::validate_arguments(arguments) {
                    Ok(arguments) => arguments,
                    Err(error) => {
                        return ToolExecution::error(
                            format!("ERROR: Malformed Python tool call: {error}."),
                            cwd,
                        );
                    }
                };
                let tool_call_id = arguments[INTERNAL_TOOL_CALL_ID_KEY]
                    .as_str()
                    .or_else(|| arguments["tool_call_id"].as_str())
                    .unwrap_or("");
                if tool_call_id.is_empty() {
                    return ToolExecution::error(
                        "ERROR: Python execution is missing its canonical tool-call ID and cannot be audit-checkpointed.",
                        cwd,
                    );
                }
                let execution = runtime
                    .execute_python_audited(
                        config,
                        cwd,
                        code,
                        tool_call_id,
                        cancellation_token,
                        Some(&tx),
                    )
                    .await;
                return ToolExecution {
                    output: execution.output,
                    cwd: execution.cwd,
                    is_error: execution.is_error,
                    provenance: execution.provenance,
                };
            }
            "read_file" => {
                let path = arguments["path"].as_str().unwrap_or("");
                let max_lines = arguments["max_lines"].as_u64().map(|v| v as usize);
                (
                    read_file::execute(path, max_lines, cwd, cancellation_token).await,
                    cwd.to_string(),
                )
            }
            "read_file_lines" => {
                let path = arguments["path"].as_str().unwrap_or("");
                let start = arguments["start_line"].as_u64().unwrap_or(1) as usize;
                let end = arguments["end_line"].as_u64().unwrap_or(1) as usize;
                (
                    read_file_lines::execute(path, start, end, cwd, cancellation_token).await,
                    cwd.to_string(),
                )
            }
            "read_folder" => {
                let path = arguments["path"].as_str().unwrap_or(".");
                (
                    read_folder::execute(path, cwd, cancellation_token).await,
                    cwd.to_string(),
                )
            }
            "search_text" => {
                let pattern = arguments["pattern"].as_str().unwrap_or("");
                let path = arguments["path"].as_str().unwrap_or(".");
                (
                    search_text::execute(pattern, path, cwd, cancellation_token).await,
                    cwd.to_string(),
                )
            }
            "run_shell_command" => {
                let command = arguments["command"].as_str().unwrap_or("");
                run_shell_command::execute(command, cwd, cancellation_token, tx).await
            }
            "write_file" => {
                let path = arguments["path"].as_str().unwrap_or("");
                let content = arguments["content"].as_str().unwrap_or("");
                (
                    write_file::execute(path, content, cwd, cancellation_token).await,
                    cwd.to_string(),
                )
            }
            "replace_text" => {
                let path = arguments["path"].as_str().unwrap_or("");
                let old_string = arguments["old_string"].as_str().unwrap_or("");
                let new_string = arguments["new_string"].as_str().unwrap_or("");
                let replace_all = arguments["replace_all"].as_bool().unwrap_or(false);
                (
                    replace_text::execute(
                        path,
                        old_string,
                        new_string,
                        replace_all,
                        cwd,
                        cancellation_token,
                    )
                    .await,
                    cwd.to_string(),
                )
            }
            "edit" => {
                let path = arguments["path"].as_str().unwrap_or("");
                let old_string = arguments["old_string"].as_str().unwrap_or("");
                let new_string = arguments["new_string"].as_str().unwrap_or("");
                (
                    edit::execute(path, old_string, new_string, cwd, cancellation_token).await,
                    cwd.to_string(),
                )
            }
            "glob" => {
                let pattern = arguments["pattern"].as_str().unwrap_or("*");
                let path = arguments["path"].as_str().unwrap_or(".");
                return glob::execute_classified(pattern, path, cwd, cancellation_token).await;
            }
            "find_symbol" => {
                let operation = arguments["operation"].as_str().unwrap_or("references");
                let symbol = arguments["symbol"].as_str().unwrap_or("");
                let path = arguments["path"].as_str().unwrap_or(".");
                return find_symbol::execute_classified(
                    operation,
                    symbol,
                    path,
                    cwd,
                    cancellation_token,
                )
                .await;
            }
            "lsp" => {
                let operation = arguments["operation"].as_str().unwrap_or("");
                let file_path = arguments["filePath"].as_str();
                let line = arguments["line"].as_u64().map(|v| v as u32);
                let character = arguments["character"].as_u64().map(|v| v as u32);
                let query = arguments["query"].as_str();
                (
                    lsp::execute(
                        operation,
                        file_path,
                        line,
                        character,
                        query,
                        cwd,
                        cancellation_token,
                        tx,
                    )
                    .await,
                    cwd.to_string(),
                )
            }
            "task" => {
                let prompt = arguments["prompt"].as_str().unwrap_or("");
                return task::execute_classified_with_hook(
                    prompt,
                    cwd,
                    cancellation_token,
                    tx,
                    client,
                    config,
                    request_hook.clone(),
                )
                .await;
            }
            "fetch_url" => {
                let url = arguments["url"].as_str().unwrap_or("");
                let format = arguments["format"].as_str().unwrap_or("markdown");
                (
                    fetch_url::execute(url, format, cancellation_token).await,
                    cwd.to_string(),
                )
            }
            // Legacy backwards-compat: keep dispatching web_fetch and read_page.
            "web_fetch" => {
                let url = arguments["url"].as_str().unwrap_or("");
                return web_fetch::execute_classified(url, cwd, cancellation_token).await;
            }
            "web_search" => {
                let query = arguments["query"].as_str().unwrap_or("");
                let num_results = arguments["num_results"].as_u64().unwrap_or(10) as usize;
                (
                    web_search::execute(query, num_results, cancellation_token).await,
                    cwd.to_string(),
                )
            }
            "read_page" => {
                let url = arguments["url"].as_str().unwrap_or("");
                return read_page::execute_classified(url, cwd, cancellation_token).await;
            }
            "calculate" => {
                let expression = arguments["expression"].as_str().unwrap_or("");
                return calculate::execute_classified(expression, cwd, cancellation_token).await;
            }
            "ask_the_user" => {
                let question = arguments["question"].as_str().unwrap_or("");
                (question.to_string(), cwd.to_string())
            }
            "apply_patch" => {
                let file_path = arguments["file_path"].as_str().unwrap_or("");
                let old_content = arguments["old_content"].as_str().unwrap_or("");
                let new_content = arguments["new_content"].as_str().unwrap_or("");
                return apply_patch::execute_classified(
                    file_path,
                    old_content,
                    new_content,
                    cwd,
                    cancellation_token,
                )
                .await;
            }
            "process_image" => {
                if !config.enable_image_processing_tool {
                    return ToolExecution::error(
                        "ERROR: Image processing tool is disabled in config.",
                        cwd,
                    );
                }
                let prompt = arguments["prompt"].as_str().unwrap_or("");
                let image_path = arguments["image_path"].as_str().unwrap_or("");
                let max_size = arguments["max_size"].as_u64().map(|v| v as u32);
                (
                    process_image::execute_with_hook(
                        prompt,
                        image_path,
                        max_size,
                        cwd,
                        client,
                        config,
                        &tx,
                        cancellation_token,
                        request_hook.clone(),
                    )
                    .await,
                    cwd.to_string(),
                )
            }
            "process_pdf_image" => {
                if !config.enable_image_processing_tool {
                    return ToolExecution::error(
                        "ERROR: PDF image processing tool is disabled in config.",
                        cwd,
                    );
                }
                let prompt = arguments["prompt"].as_str().unwrap_or("");
                let pdf_path = arguments["pdf_path"].as_str().unwrap_or("");
                let page_num = arguments["page_num"].as_u64().unwrap_or(1) as usize;
                let max_size = arguments["max_size"].as_u64().map(|v| v as u32);
                (
                    process_pdf_image::execute_with_hook(
                        prompt,
                        pdf_path,
                        page_num,
                        max_size,
                        cwd,
                        client,
                        config,
                        &tx,
                        cancellation_token,
                        request_hook.clone(),
                    )
                    .await,
                    cwd.to_string(),
                )
            }
            "get_pdf_text" => {
                let pdf_path = arguments["pdf_path"].as_str().unwrap_or("");
                return get_pdf_text::execute_classified(pdf_path, cwd, &tx, cancellation_token)
                    .await;
            }
            "summarize_content" => {
                let path = arguments["path"].as_str();
                let content = arguments["content"].as_str();
                let prompt = arguments["prompt"].as_str();
                (
                    summarize_content::execute_with_hook(
                        path,
                        content,
                        prompt,
                        cwd,
                        client,
                        config,
                        &tx,
                        cancellation_token,
                        request_hook.clone(),
                    )
                    .await,
                    cwd.to_string(),
                )
            }
            "background_task" => {
                return match background_task::execute(arguments, cwd, cancellation_token).await {
                    Ok(output) => ToolExecution::success(output, cwd),
                    Err(output) => ToolExecution::error(output, cwd),
                };
            }
            "todowrite" => {
                let todos = &arguments["todos"];
                return todowrite::execute_classified(todos, cwd, cancellation_token).await;
            }
            "repo_overview" => {
                let path = arguments["path"].as_str().unwrap_or(".");
                return repo_overview::execute_classified(path, cwd, cancellation_token).await;
            }
            _ => (format!("Unknown tool: {}", func_name), cwd.to_string()),
        };
        ToolExecution::from_legacy(output, new_cwd)
    }) // Box::pin
}

pub async fn get_git_info() -> String {
    let status =
        crate::platform::command_output("git", ["status", "--porcelain=v2", "--branch"], None)
            .await;

    match status {
        Ok(out) => {
            if !out.status.success() {
                return "not a git repo".to_string();
            }
            let s = String::from_utf8_lossy(&out.stdout);
            if s.trim().is_empty() {
                return "clean".to_string();
            }

            let mut branch = String::from("unknown");
            let mut untracked = 0;
            let mut modified = 0;
            let mut staged = 0;
            let mut renamed = 0;
            let mut deleted = 0;

            for line in s.lines() {
                if line.starts_with("# branch.head") {
                    branch = line
                        .split_whitespace()
                        .nth(2)
                        .unwrap_or("detached")
                        .to_string();
                } else if line.starts_with("?") {
                    untracked += 1;
                } else if line.starts_with("1 ") || line.starts_with("2 ") {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() > 1 {
                        let codes = parts[1];
                        let staged_code = codes.chars().next().unwrap_or('.');
                        let unstaged_code = codes.chars().nth(1).unwrap_or('.');

                        if staged_code != '.' {
                            staged += 1;
                        }
                        if unstaged_code == 'M' {
                            modified += 1;
                        }
                        if unstaged_code == 'D' {
                            deleted += 1;
                        }
                        if staged_code == 'R' {
                            renamed += 1;
                        }
                    }
                }
            }

            let mut res = format!(" {}", branch);
            if staged > 0 {
                res.push_str(&format!(" +{}", staged));
            }
            if modified > 0 {
                res.push_str(&format!(" ~{}", modified));
            }
            if deleted > 0 {
                res.push_str(&format!(" -{}", deleted));
            }
            if untracked > 0 {
                res.push_str(&format!(" ?{}", untracked));
            }
            if renamed > 0 {
                res.push_str(&format!(" r{}", renamed));
            }

            if staged == 0 && modified == 0 && untracked == 0 && deleted == 0 && renamed == 0 {
                format!(" {} (clean)", branch)
            } else {
                res
            }
        }
        Err(_) => "not a git repo".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn legacy_tool_failures_are_classified_from_the_first_nonempty_line() {
        for output in [
            "ERROR: LSP operation timed out after 30 seconds.",
            "\nERROR: No safe automatic installer is configured.",
            "Unknown tool: unsupported",
            "EXIT_CODE: 1\nSTDERR:\nfailed",
            "EXIT_CODE: signaled\nSTDERR:\nkilled",
            "EXIT_CODE: malformed",
            "STDOUT:\npatch diagnostic\nSTDERR:\npatch failed",
            "[Operation Cancelled by User]",
            "Sub-agent failed: quota denied tenant violet req-42",
            "Sub-agent cancelled.",
            "Sub-agent cancellation did not settle within 30 seconds.",
            "Sub-agent timed out after 5 minutes.",
            "Syntax Error in tool call: malformed arguments",
            "Failed to run install command: permission denied",
            "Auto-install of `rust-analyzer` failed.",
            "LSP error: connection closed",
            "goToDefinition requires filePath, line, and character.",
            "documentSymbol requires filePath.",
            "Unknown LSP operation: unsupported",
            "[find_symbol fallback]\nERROR: symbol search failed",
            "--- Page 1 Error: PDF is encrypted and requires a password ---",
        ] {
            assert!(
                ToolExecution::from_legacy(output.to_string(), ".".to_string()).is_error,
                "expected an error for {output:?}"
            );
        }

        for output in ["ok", "EXIT_CODE: 0\nSTDOUT:\nok"] {
            assert!(
                !ToolExecution::from_legacy(output.to_string(), ".".to_string()).is_error,
                "expected success for {output:?}"
            );
        }
    }

    #[test]
    fn persisted_error_marker_finds_merged_and_partial_failures() {
        for (output, expected_tail) in [
            (
                "ok\nERROR: quota denied tenant violet",
                "ERROR: quota denied",
            ),
            (
                "match.rs:1:default\nSTDERR: rg: private: Permission denied",
                "STDERR:",
            ),
            (
                "[find_symbol fallback]\nERROR: fallback failed",
                "[find_symbol fallback]",
            ),
            (
                "STDOUT:\npatch diagnostic\nSTDERR:\npatch failed",
                "STDOUT:",
            ),
            (
                "EXIT_CODE: 0\nSTDOUT:\nok\nSTDERR:\n\nEXIT_CODE: 1\nSTDOUT:\n\nSTDERR:\nquota denied tenant violet",
                "EXIT_CODE: 1",
            ),
            (
                "EXIT_CODE: 0\nSTDOUT:\nok\nSTDERR:\nSTDOUT:\npatch diagnostic\nSTDERR:\npatch failed",
                "STDOUT:\npatch diagnostic",
            ),
            (
                "EXIT_CODE: 0\n... [Output truncated. Full output (25000 characters) saved locally] ...\n\nSTDOUT:\npatch diagnostic tenant violet truncatedpatch\nSTDERR:\npatch failed",
                "STDOUT:\npatch diagnostic",
            ),
            (
                "EXIT_CODE: 0\n... [Output truncated. Full output was not saved because secure host storage rejected the path: refusing symlink storageviolet] ...",
                "... [Output truncated.",
            ),
            (
                "EXIT_CODE: 0\nSTDOUT:\nok\nSTDERR:\nEXIT_CODE: 0\nSTDOUT:\npatch diagnostic stderrheader-violet\nSTDERR:\npatch failed",
                "STDOUT:\npatch diagnostic",
            ),
        ] {
            let offset = legacy_persisted_tool_error_marker_offset(output)
                .unwrap_or_else(|| panic!("expected an error marker in {output:?}"));
            assert!(output[offset..].starts_with(expected_tail), "{output:?}");
        }

        assert!(
            legacy_persisted_tool_error_marker_offset(
                "EXIT_CODE: 0\nSTDOUT:\nprogram printed ERROR: as ordinary text\nSTDERR:\n"
            )
            .is_none()
        );
    }

    #[test]
    fn test_tool_definitions() {
        let config = Config {
            server_url: "".to_string(),
            model: "".to_string(),
            context_size: 0,
            tool_wrapper: None,
            tool_profile: Default::default(),
            python_runtime: Default::default(),
            python_invocation: Default::default(),
            active_server: None,
            connection_kind: Default::default(),
            api_key: None,
            estimate_cost: None,
            pricing: None,
            input_cost_per_1m: None,
            output_cost_per_1m: None,
            enable_image_processing_tool: false,
            background_tasks: Default::default(),
            tool_calls: Default::default(),
            provider_retries: None,
            theme: None,
            model_servers: Vec::new(),
            thinking: None,
            extra_body: None,
            context_mode: None,
        };
        let tools = get_all_tools(&config);
        let shell = tools
            .iter()
            .find(|t| t.function.name == "run_shell_command")
            .unwrap();
        assert!(
            shell.function.parameters["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "tool_call_id")
        );
        assert!(
            shell.function.parameters["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "description")
        );
    }

    #[test]
    fn test_python_only_exposes_exact_tools() {
        let config = Config {
            tool_profile: crate::config::ToolProfile::PythonOnly,
            python_runtime: crate::config::PythonRuntimeConfig {
                target: Some(crate::config::PythonExecutionTarget::Host),
                ..Default::default()
            },
            ..Default::default()
        };
        let names = get_all_tools(&config)
            .into_iter()
            .map(|tool| tool.function.name)
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["python"]);
        assert!(!is_tool_allowed(
            &config,
            ToolSurface::Interactive,
            "todowrite"
        ));
        assert!(!is_tool_allowed(&config, ToolSurface::Headless, "task"));
    }

    #[tokio::test]
    async fn test_python_only_denies_direct_shell_dispatch() {
        let config = Config {
            tool_profile: crate::config::ToolProfile::PythonOnly,
            python_runtime: crate::config::PythonRuntimeConfig {
                target: Some(crate::config::PythonExecutionTarget::Host),
                ..Default::default()
            },
            ..Default::default()
        };
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let execution = execute_for_surface(
            ToolSurface::Interactive,
            "run_shell_command",
            &serde_json::json!({"command": "printf should-not-run"}),
            ".",
            tokio_util::sync::CancellationToken::new(),
            tx,
            &reqwest::Client::new(),
            &config,
        )
        .await;
        assert!(execution.is_error);
        assert!(execution.output.contains("not allowed"));
    }

    #[tokio::test]
    async fn final_dispatch_rejects_invalid_policy_and_headless_only_exclusions() {
        let invalid_python = Config {
            tool_profile: crate::config::ToolProfile::PythonOnly,
            ..Default::default()
        };
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let execution = execute_for_surface(
            ToolSurface::Headless,
            "python",
            &serde_json::json!({"code": "1 + 1", "description": "must not run"}),
            ".",
            tokio_util::sync::CancellationToken::new(),
            tx,
            &reqwest::Client::new(),
            &invalid_python,
        )
        .await;
        assert!(execution.is_error);
        assert!(execution.output.contains("Invalid Python-only tool policy"));

        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let execution = execute_for_surface(
            ToolSurface::Headless,
            "task",
            &serde_json::json!({}),
            ".",
            tokio_util::sync::CancellationToken::new(),
            tx,
            &reqwest::Client::new(),
            &Config::default(),
        )
        .await;
        assert!(execution.is_error);
        assert!(execution.output.contains("not allowed"));
    }

    #[tokio::test]
    async fn cancelled_legacy_tool_output_is_classified_as_an_error() {
        let workspace = tempfile::tempdir().unwrap();
        let runtime = crate::tool_runtime::ToolRuntime::interactive(workspace.path());
        let config = Config::default();
        let cancellation = tokio_util::sync::CancellationToken::new();
        cancellation.cancel();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        let execution = execute_with_runtime(
            &runtime,
            "fetch_url",
            &serde_json::json!({
                "url": "https://example.invalid/never-started",
                "format": "text"
            }),
            workspace.path().to_str().unwrap(),
            cancellation,
            tx,
            &reqwest::Client::new(),
            &config,
        )
        .await;

        assert!(execution.is_error);
        assert_eq!(execution.output, "[Operation Cancelled by User]");
    }

    #[tokio::test]
    async fn tool_results_warn_when_todo_id_is_missing_unknown_or_finished() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        let none = todo_reference_warning("read_file", &serde_json::json!({}), root).unwrap();
        assert!(none.contains("did not name a todo_id") && none.contains("empty"));
        crate::tools::todowrite::execute(
            &serde_json::json!([
                {"id": "build", "content": "fix build", "status": "in_progress", "priority": "high"},
                {"content": "write docs", "status": "completed", "priority": "low"}
            ]),
            root.to_str().unwrap(),
        )
        .await;
        assert!(
            todo_reference_warning("edit", &serde_json::json!({"todo_id": "build"}), root)
                .is_none()
        );
        let unknown =
            todo_reference_warning("edit", &serde_json::json!({"todo_id": "nope"}), root).unwrap();
        assert!(
            unknown.contains("'nope' is not in the todo list") && unknown.contains("build, t1")
        );
        let finished =
            todo_reference_warning("edit", &serde_json::json!({"todo_id": "t1"}), root).unwrap();
        assert!(finished.contains("already completed"));
        assert!(todo_reference_warning("todowrite", &serde_json::json!({}), root).is_none());
        let config = Config::default();
        let shell = get_all_tools(&config)
            .into_iter()
            .find(|tool| tool.function.name == "run_shell_command")
            .unwrap();
        assert!(
            shell.function.parameters["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "todo_id")
        );
    }

    #[tokio::test]
    async fn malformed_python_dispatch_never_starts_a_worker() {
        let config = Config {
            tool_profile: crate::config::ToolProfile::PythonOnly,
            python_runtime: crate::config::PythonRuntimeConfig {
                target: Some(crate::config::PythonExecutionTarget::Host),
                ..Default::default()
            },
            ..Default::default()
        };
        let workspace = tempfile::tempdir().unwrap();
        let runtime = crate::tool_runtime::ToolRuntime::interactive(workspace.path());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let execution = execute_with_runtime(
            &runtime,
            "python",
            &serde_json::json!({
                "code": 42,
                "description": "invalid",
                "__lethetic_canonical_tool_call_id": "tool-malformed"
            }),
            workspace.path().to_str().unwrap(),
            tokio_util::sync::CancellationToken::new(),
            tx,
            &reqwest::Client::new(),
            &config,
        )
        .await;
        assert!(execution.is_error);
        assert!(execution.output.contains("Malformed Python tool call"));
        assert!(!runtime.is_running().await);
    }

    #[tokio::test]
    async fn test_python_tool_uses_persistent_runtime() {
        if !crate::platform::binary_on_path("python3") {
            return;
        }
        let config = Config {
            tool_profile: crate::config::ToolProfile::PythonOnly,
            python_runtime: crate::config::PythonRuntimeConfig {
                target: Some(crate::config::PythonExecutionTarget::Host),
                ..Default::default()
            },
            ..Default::default()
        };
        let workspace = tempfile::tempdir().unwrap();
        let cwd = workspace.path().canonicalize().unwrap();
        let runtime = crate::tool_runtime::ToolRuntime::interactive(cwd.clone());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let client = reqwest::Client::new();
        runtime
            .begin_python_audit_attempt(
                None,
                &config,
                "tool-python-1",
                "tool_value = 20",
                "initialize value",
                cwd.to_str().unwrap(),
            )
            .unwrap();
        runtime
            .mark_python_audit_status(
                "tool-python-1",
                crate::python::notebook::NotebookAttemptStatus::Approved,
                None,
            )
            .unwrap();
        let first = execute_with_runtime(
            &runtime,
            "python",
            &serde_json::json!({
                "code": "tool_value = 20",
                "description": "initialize value",
                "__lethetic_canonical_tool_call_id": "tool-python-1"
            }),
            cwd.to_str().unwrap(),
            tokio_util::sync::CancellationToken::new(),
            tx.clone(),
            &client,
            &config,
        )
        .await;
        assert!(!first.is_error, "{}", first.output);
        runtime
            .begin_python_audit_attempt(
                None,
                &config,
                "tool-python-2",
                "tool_value + 22",
                "read value",
                &first.cwd,
            )
            .unwrap();
        runtime
            .mark_python_audit_status(
                "tool-python-2",
                crate::python::notebook::NotebookAttemptStatus::Approved,
                None,
            )
            .unwrap();
        let second = execute_with_runtime(
            &runtime,
            "python",
            &serde_json::json!({
                "code": "tool_value + 22",
                "description": "read value",
                "__lethetic_canonical_tool_call_id": "tool-python-2"
            }),
            &first.cwd,
            tokio_util::sync::CancellationToken::new(),
            tx,
            &client,
            &config,
        )
        .await;
        assert!(!second.is_error, "{}", second.output);
        assert!(second.output.contains("42"));
    }

    #[tokio::test]
    async fn python_only_denies_legacy_todowrite_dispatch() {
        let workspace = tempfile::tempdir().unwrap();
        let runtime = crate::tool_runtime::ToolRuntime::interactive(workspace.path());
        let config = Config {
            tool_profile: crate::config::ToolProfile::PythonOnly,
            python_runtime: crate::config::PythonRuntimeConfig {
                target: Some(crate::config::PythonExecutionTarget::Host),
                ..Default::default()
            },
            ..Default::default()
        };
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let execution = execute_with_runtime(
            &runtime,
            "todowrite",
            &serde_json::json!({"todos": []}),
            workspace.path().to_str().unwrap(),
            tokio_util::sync::CancellationToken::new(),
            tx,
            &reqwest::Client::new(),
            &config,
        )
        .await;

        assert!(execution.is_error);
        assert!(
            execution.output.contains("not allowed"),
            "{}",
            execution.output
        );
        assert!(!workspace.path().join(".lethetic/todos.json").exists());
    }

    #[tokio::test]
    async fn general_todowrite_keeps_current_cwd_behavior() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        let nested = workspace.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        let runtime = crate::tool_runtime::ToolRuntime::interactive(&workspace);
        let config = Config::default();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        let execution = execute_with_runtime(
            &runtime,
            "todowrite",
            &serde_json::json!({"todos": []}),
            nested.to_str().unwrap(),
            tokio_util::sync::CancellationToken::new(),
            tx,
            &reqwest::Client::new(),
            &config,
        )
        .await;

        assert!(!execution.is_error, "{}", execution.output);
        assert!(nested.join(".lethetic/todos.json").is_file());
        assert!(!workspace.join(".lethetic/todos.json").exists());
    }

    #[test]
    fn large_output_id_cannot_traverse_outside_workspace() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let outside = root.path().join("outside.txt");
        std::fs::write(&outside, "outside-safe").unwrap();

        let output = "x".repeat(LARGE_OUTPUT_THRESHOLD + 1);
        let handled =
            handle_large_output_classified_in(&workspace, "../../../outside", output.clone());

        assert!(!handled.storage_failed);
        assert!(handled.context.contains("OUTPUT TRUNCATED"));
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "outside-safe");
        let file_name = large_output_file_name("../../../outside", output.as_bytes());
        assert!(!file_name.contains('/'));
        assert!(!file_name.contains(".."));
        assert!(
            workspace
                .join(".lethetic/tool_responses")
                .join(file_name)
                .is_file()
        );
    }

    #[cfg(unix)]
    #[test]
    fn large_output_rejects_symlinked_storage_parent() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        let outside = root.path().join("outside");
        std::fs::create_dir_all(workspace.join(".lethetic")).unwrap();
        std::fs::create_dir(&outside).unwrap();
        symlink(&outside, workspace.join(".lethetic/tool_responses")).unwrap();

        let handled = handle_large_output_classified_in(
            &workspace,
            "safe-id",
            "x".repeat(LARGE_OUTPUT_THRESHOLD + 1),
        );

        assert!(handled.storage_failed);
        assert!(handled.effective_is_error(false));
        assert!(handled.context.contains("not saved"), "{}", handled.context);
        assert!(handled.context.contains("symlink"), "{}", handled.context);
        assert!(std::fs::read_dir(outside).unwrap().next().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn large_output_rejects_symlinked_destination() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        let responses = workspace.join(".lethetic/tool_responses");
        std::fs::create_dir_all(&responses).unwrap();
        let outside = root.path().join("outside.txt");
        std::fs::write(&outside, "outside-safe").unwrap();
        let output = "x".repeat(LARGE_OUTPUT_THRESHOLD + 1);
        symlink(
            &outside,
            responses.join(large_output_file_name("safe-id", output.as_bytes())),
        )
        .unwrap();

        let handled = handle_large_output_classified_in(&workspace, "safe-id", output);

        assert!(handled.storage_failed);
        assert!(handled.context.contains("not saved"), "{}", handled.context);
        assert!(handled.context.contains("symlink"), "{}", handled.context);
        assert_eq!(std::fs::read_to_string(outside).unwrap(), "outside-safe");
    }
}
