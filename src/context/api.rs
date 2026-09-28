use super::{
    CHARS_PER_TOKEN, ContextManager, ContextMode, estimate_byte_len_tokens, sanitize_file_content,
    strip_delimited,
};
use crate::transport::{Content, ContentPart, Message, Role, ToolCall};
use std::io::Read;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct PreparedApiContext {
    messages: Vec<Message>,
    estimated_tokens: usize,
}

impl PreparedApiContext {
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn estimated_tokens(&self) -> usize {
        self.estimated_tokens
    }

    pub fn into_messages(self) -> Vec<Message> {
        self.messages
    }
}

#[derive(Default)]
struct Projection {
    root_system: Option<String>,
    history_system: Vec<String>,
    conversation: Vec<Message>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileTier {
    Active,
    Latest,
}

#[derive(Default)]
struct FileSections {
    active: Vec<String>,
    latest: Vec<String>,
    /// The model's current todo list, rendered for context.
    todos: Option<String>,
    /// Background tasks still running or finished but not yet reviewed.
    background: Option<String>,
}

impl FileSections {
    fn entries_mut(&mut self, tier: FileTier) -> &mut Vec<String> {
        match tier {
            FileTier::Active => &mut self.active,
            FileTier::Latest => &mut self.latest,
        }
    }

    fn rendered_parts(&self, mode: ContextMode) -> Vec<String> {
        match mode {
            ContextMode::Lethetic => {
                let mut parts = Vec::new();
                if !self.latest.is_empty() {
                    parts.push(format!(
                        "<latest_files>\n{}</latest_files>",
                        self.latest.concat()
                    ));
                }
                if !self.active.is_empty() {
                    parts.push(format!(
                        "<active_file>\n{}</active_file>",
                        self.active.concat()
                    ));
                }
                parts
            }
            ContextMode::Vercel => {
                let entries = self
                    .latest
                    .iter()
                    .chain(self.active.iter())
                    .cloned()
                    .collect::<Vec<_>>();
                if entries.is_empty() {
                    Vec::new()
                } else {
                    vec![entries.join("\n\n")]
                }
            }
        }
    }

    fn added_bytes(&self, mode: ContextMode) -> usize {
        match mode {
            ContextMode::Lethetic => {
                let latest = grouped_bytes(&self.latest, "<latest_files>\n", "</latest_files>");
                let active = grouped_bytes(&self.active, "<active_file>\n", "</active_file>");
                latest.saturating_add(active)
            }
            ContextMode::Vercel => {
                let count = self.latest.len().saturating_add(self.active.len());
                if count == 0 {
                    0
                } else {
                    self.latest
                        .iter()
                        .chain(self.active.iter())
                        .fold(0_usize, |total, entry| total.saturating_add(entry.len()))
                        .saturating_add(count.saturating_sub(1).saturating_mul(2))
                        // One possible separator from the non-file system projection.
                        .saturating_add(2)
                }
            }
        }
    }

    fn try_push(
        &mut self,
        tier: FileTier,
        entry: String,
        mode: ContextMode,
        byte_budget: usize,
    ) -> bool {
        self.entries_mut(tier).push(entry);
        if self.added_bytes(mode) <= byte_budget {
            true
        } else {
            self.entries_mut(tier).pop();
            false
        }
    }
}

fn grouped_bytes(entries: &[String], opening: &str, closing: &str) -> usize {
    if entries.is_empty() {
        return 0;
    }
    entries
        .iter()
        .fold(opening.len(), |total, entry| {
            total.saturating_add(entry.len())
        })
        .saturating_add(closing.len())
        // One possible separator from another system part.
        .saturating_add(2)
}

enum FileRead {
    Complete(String),
    TooLarge,
    Unavailable,
    Changed,
    NonUtf8,
}

/// Tool results older than the most recent `FULL_TOOL_RESULTS` are shortened
/// to their first and last lines in the request (the session keeps them in
/// full). Tool output was ~87% of long sessions' context.
const FULL_TOOL_RESULTS: usize = 6;
const SHORTENED_HEAD_LINES: usize = 20;
const SHORTENED_TAIL_LINES: usize = 20;
/// Outputs with few but very long lines are also cut by characters.
const SHORTENED_MAX_CHARS: usize = 4_000;

fn shorten_tool_output(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() > SHORTENED_HEAD_LINES + SHORTENED_TAIL_LINES + 5 {
        let omitted = lines.len() - SHORTENED_HEAD_LINES - SHORTENED_TAIL_LINES;
        return Some(format!(
            "{}\n… {omitted} lines omitted from this older tool output (re-run the tool if you need them) …\n{}",
            lines[..SHORTENED_HEAD_LINES].join("\n"),
            lines[lines.len() - SHORTENED_TAIL_LINES..].join("\n")
        ));
    }
    if text.len() > SHORTENED_MAX_CHARS {
        let head_end = floor_char_boundary(text, SHORTENED_MAX_CHARS / 2);
        let tail_start = floor_char_boundary(text, text.len() - SHORTENED_MAX_CHARS / 4);
        let omitted = tail_start - head_end;
        return Some(format!(
            "{}\n… {omitted} characters omitted from this older tool output …\n{}",
            &text[..head_end],
            &text[tail_start..]
        ));
    }
    None
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn shorten_old_tool_results(conversation: &mut [Message]) {
    let tool_indices: Vec<usize> = conversation
        .iter()
        .enumerate()
        .filter(|(_, message)| message.role == Role::Tool)
        .map(|(index, _)| index)
        .collect();
    let keep_from = tool_indices.len().saturating_sub(FULL_TOOL_RESULTS);
    for &index in &tool_indices[..keep_from] {
        if let Content::Text(text) = &conversation[index].content
            && let Some(shortened) = shorten_tool_output(text)
        {
            conversation[index].content = Content::Text(shortened);
        }
    }
}

pub(super) fn prepare(ctx: &ContextManager) -> PreparedApiContext {
    let mut projection = project_messages(ctx);
    shorten_old_tool_results(&mut projection.conversation);
    let base_messages = assemble_messages(ctx.mode, &projection, &FileSections::default());
    let mut base_estimate = estimate_api_messages(&base_messages);
    trim_projected_conversation(
        &mut projection.conversation,
        &mut base_estimate,
        ctx.max_tokens,
    );
    let file_ceiling = ctx
        .max_tokens
        .saturating_mul(super::LATEST_FILES_BUDGET_FRACTION)
        / 100;
    let available = ctx.max_tokens.saturating_sub(base_estimate);
    let file_tokens = file_ceiling.min(available);
    let file_byte_budget = file_tokens.saturating_mul(CHARS_PER_TOKEN);
    let mut files = prepare_files(ctx, file_byte_budget);
    files.todos = ctx.todo_summary.clone();
    files.background = ctx.background_summary.clone();
    let messages = assemble_messages(ctx.mode, &projection, &files);
    let estimated_tokens = estimate_api_messages(&messages);

    PreparedApiContext {
        messages,
        estimated_tokens,
    }
}

fn trim_projected_conversation(
    conversation: &mut Vec<Message>,
    estimated_tokens: &mut usize,
    max_tokens: usize,
) {
    let mut protected_start = conversation
        .iter()
        .rposition(|message| message.role == Role::User)
        .unwrap_or(conversation.len());
    while *estimated_tokens > max_tokens && protected_start > 0 {
        let mut unit_len = 1;
        if conversation[0].role == Role::Assistant && !conversation[0].tool_calls.is_empty() {
            while unit_len < protected_start && conversation[unit_len].role == Role::Tool {
                unit_len += 1;
            }
        } else if conversation[0].role == Role::Tool {
            while unit_len < protected_start && conversation[unit_len].role == Role::Tool {
                unit_len += 1;
            }
        }
        let removed_tokens = conversation[..unit_len]
            .iter()
            .fold(0_usize, |total, message| {
                total.saturating_add(estimate_api_message(message))
            });
        conversation.drain(..unit_len);
        protected_start -= unit_len;
        *estimated_tokens = estimated_tokens.saturating_sub(removed_tokens);
    }
}

fn project_messages(ctx: &ContextManager) -> Projection {
    let root_system = ctx.system_prompt.as_ref().and_then(|system| {
        let mut clean = system.clone();
        strip_delimited(&mut clean, "<|tool>", "<tool|>");
        clean = clean
            .replace("<|think|>", "")
            .replace("<|turn>", "")
            .replace("<turn|>", "");
        let clean = clean.trim().to_string();
        (!clean.is_empty()).then_some(clean)
    });

    let mut history_system = Vec::new();
    let mut conversation = Vec::new();
    for message in &ctx.messages {
        match message.role.as_str() {
            "system" => {
                if !message.content.is_empty() {
                    history_system.push(message.content.clone());
                }
            }
            "user" => conversation.push(Message::user(message.content.clone())),
            "assistant"
                if message.tool_calls.as_deref().unwrap_or_default().is_empty()
                    && ctx.strip_thinking(&message.content).trim().is_empty() =>
            {
                // A reply that failed before producing anything (stream
                // error, cancellation). Say so instead of sending a blank turn.
                conversation.push(Message::assistant(
                    "[My previous reply was interrupted before it produced any output.]",
                ));
            }
            "assistant" => {
                let calls = message
                    .tool_calls
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .map(|call| ToolCall {
                        id: call.id.clone(),
                        name: call.function.name.clone(),
                        arguments: call.function.arguments.clone(),
                    })
                    .collect();
                conversation.push(Message::assistant_with_tools(
                    ctx.strip_thinking(&message.content),
                    calls,
                    message.provider_content.clone(),
                ));
            }
            "tool" => {
                let (tool_call_id, result) = ContextManager::extract_tool_result_fields(
                    &message.content,
                )
                .unwrap_or_else(|| {
                    let tool_call_id = ContextManager::extract_delimited(
                        &message.content,
                        "tool_call_id:<|'|>",
                        "<|'|>",
                    )
                    .unwrap_or("unknown".into());
                    let result = ContextManager::extract_delimited(
                        &message.content,
                        "result:<|'|>",
                        "<|'|>",
                    )
                    .unwrap_or(message.content.clone());
                    (tool_call_id, result)
                });
                conversation.push(Message::tool_result_with_status(
                    tool_call_id,
                    result,
                    message.tool_result_is_error,
                ));
            }
            _ => {}
        }
    }

    Projection {
        root_system,
        history_system,
        conversation,
    }
}

/// Build the request. The system message holds only the stable prompt, so the
/// server can reuse its processed prefix across turns; cached files and the
/// todo list, which change as the model works, ride on the latest user
/// message instead. Without a user message they fall back to the system one.
fn assemble_messages(
    mode: ContextMode,
    projection: &Projection,
    files: &FileSections,
) -> Vec<Message> {
    let file_parts = files.rendered_parts(mode);
    let mut working = Vec::new();
    match mode {
        ContextMode::Lethetic => {
            if let Some(latest) = file_parts.first()
                && !files.latest.is_empty()
            {
                working.push(latest.clone());
            }
            if !files.active.is_empty()
                && let Some(active) = file_parts.last()
            {
                working.push(active.clone());
            }
        }
        ContextMode::Vercel => working.extend(file_parts),
    }
    if let Some(todos) = &files.todos {
        working.push(todos.clone());
    }
    if let Some(background) = &files.background {
        working.push(background.clone());
    }

    let mut system = Vec::new();
    if let Some(root) = &projection.root_system {
        system.push(root.clone());
    }
    system.extend(projection.history_system.iter().cloned());

    let mut conversation = projection.conversation.clone();
    let latest_user = conversation
        .iter()
        .rposition(|message| message.role == Role::User);
    match latest_user {
        Some(index) if !working.is_empty() => {
            let block = working.join("\n\n");
            let message = &mut conversation[index];
            message.content = match &message.content {
                Content::Text(text) => Content::Text(format!("{block}\n\n{text}")),
                Content::Parts(parts) => {
                    let mut with_block = vec![ContentPart::Text(block)];
                    with_block.extend(parts.iter().cloned());
                    Content::Parts(with_block)
                }
            };
        }
        _ => system.extend(working),
    }

    let mut messages = Vec::with_capacity(conversation.len() + 1);
    if !system.is_empty() {
        messages.push(Message::system(system.join("\n\n")));
    }
    messages.extend(conversation);
    messages
}

fn prepare_files(ctx: &ContextManager, byte_budget: usize) -> FileSections {
    if byte_budget == 0 {
        return FileSections::default();
    }

    let mut active = ctx.active_files.iter().collect::<Vec<_>>();
    active.sort_by(|(path_a, file_a), (path_b, file_b)| {
        file_b
            .timestamp
            .cmp(&file_a.timestamp)
            .then_with(|| path_a.cmp(path_b))
    });
    let mut latest = ctx.latest_files.iter().collect::<Vec<_>>();
    latest.sort_by(|(path_a, file_a), (path_b, file_b)| {
        file_b
            .timestamp
            .cmp(&file_a.timestamp)
            .then_with(|| path_a.cmp(path_b))
    });

    let candidates = active
        .into_iter()
        .map(|(path, _)| (FileTier::Active, path.as_str()))
        .chain(
            latest
                .into_iter()
                .map(|(path, _)| (FileTier::Latest, path.as_str())),
        )
        .collect::<Vec<_>>();
    let mut sections = FileSections::default();
    let mut unreported_active = 0_usize;
    let mut unreported_latest = 0_usize;

    for (tier, path) in candidates {
        let remaining = byte_budget.saturating_sub(sections.added_bytes(ctx.mode));
        if remaining < 32 {
            match tier {
                FileTier::Active => unreported_active = unreported_active.saturating_add(1),
                FileTier::Latest => unreported_latest = unreported_latest.saturating_add(1),
            }
            continue;
        }
        let absolute = Path::new(&ctx.cwd).join(path);
        let read = read_complete_file(&absolute, remaining);
        let entry = render_file_entry(ctx.mode, path, read);
        if sections.try_push(tier, entry, ctx.mode, byte_budget) {
            continue;
        }

        let notice = render_file_entry(ctx.mode, path, FileRead::TooLarge);
        if !sections.try_push(tier, notice, ctx.mode, byte_budget) {
            match tier {
                FileTier::Active => unreported_active = unreported_active.saturating_add(1),
                FileTier::Latest => unreported_latest = unreported_latest.saturating_add(1),
            }
        }
    }

    for (tier, count) in [
        (FileTier::Active, unreported_active),
        (FileTier::Latest, unreported_latest),
    ] {
        if count == 0 {
            continue;
        }
        let notice = match ctx.mode {
            ContextMode::Lethetic => {
                format!(
                    "{count} additional cached file(s) were omitted by the file context budget.\n"
                )
            }
            ContextMode::Vercel => {
                format!(
                    "{count} additional cached file(s) were omitted by the file context budget."
                )
            }
        };
        let _ = sections.try_push(tier, notice, ctx.mode, byte_budget);
    }

    sections
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileSnapshot {
    len: u64,
    modified: Option<std::time::SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    modified_seconds: i64,
    #[cfg(unix)]
    modified_nanoseconds: i64,
    #[cfg(unix)]
    changed_seconds: i64,
    #[cfg(unix)]
    changed_nanoseconds: i64,
}

fn snapshot_metadata(metadata: &std::fs::Metadata) -> FileSnapshot {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;

    FileSnapshot {
        len: metadata.len(),
        modified: metadata.modified().ok(),
        #[cfg(unix)]
        device: metadata.dev(),
        #[cfg(unix)]
        inode: metadata.ino(),
        #[cfg(unix)]
        modified_seconds: metadata.mtime(),
        #[cfg(unix)]
        modified_nanoseconds: metadata.mtime_nsec(),
        #[cfg(unix)]
        changed_seconds: metadata.ctime(),
        #[cfg(unix)]
        changed_nanoseconds: metadata.ctime_nsec(),
    }
}

#[cfg(unix)]
fn open_for_bounded_read(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
}

#[cfg(not(unix))]
fn open_for_bounded_read(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::File::open(path)
}

fn read_complete_file(path: &Path, byte_limit: usize) -> FileRead {
    // Open once, then budget and verify that exact handle. O_NONBLOCK prevents a
    // path replacement with a FIFO from stalling Unix request preparation.
    let mut file = match open_for_bounded_read(path) {
        Ok(file) => file,
        Err(_) => return FileRead::Unavailable,
    };
    let initial_metadata = match file.metadata() {
        Ok(metadata) if metadata.is_file() => metadata,
        _ => return FileRead::Unavailable,
    };
    let initial = snapshot_metadata(&initial_metadata);
    if initial.len > u64::try_from(byte_limit).unwrap_or(u64::MAX) {
        return FileRead::TooLarge;
    }

    let read_limit = u64::try_from(byte_limit)
        .unwrap_or(u64::MAX - 1)
        .saturating_add(1);
    let initial_capacity = usize::try_from(initial.len)
        .unwrap_or(byte_limit)
        .min(byte_limit)
        .min(64 * 1024);
    let mut bytes = Vec::with_capacity(initial_capacity);
    if file
        .by_ref()
        .take(read_limit)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return FileRead::Unavailable;
    }
    if bytes.len() > byte_limit {
        return FileRead::TooLarge;
    }
    let final_metadata = match file.metadata() {
        Ok(metadata) if metadata.is_file() => metadata,
        _ => return FileRead::Changed,
    };
    let final_snapshot = snapshot_metadata(&final_metadata);
    if final_snapshot != initial || initial.len != u64::try_from(bytes.len()).unwrap_or(u64::MAX) {
        return FileRead::Changed;
    }

    match String::from_utf8(bytes) {
        Ok(content) => FileRead::Complete(content),
        Err(_) => FileRead::NonUtf8,
    }
}

fn render_file_entry(mode: ContextMode, path: &str, read: FileRead) -> String {
    let path = bounded_display_path(path);
    match (mode, read) {
        (ContextMode::Lethetic, FileRead::Complete(content)) => {
            let content = sanitize_file_content(&content);
            format!(
                "File: `{path}` (complete, {} lines)\n```\n{content}\n```\n",
                content.lines().count()
            )
        }
        (ContextMode::Vercel, FileRead::Complete(content)) => {
            let content = sanitize_file_content(&content);
            format!("## File: {path}\n```\n{content}\n```")
        }
        (ContextMode::Lethetic, reason) => {
            format!("File: `{path}` (omitted: {}).\n", omission_reason(reason))
        }
        (ContextMode::Vercel, reason) => {
            format!(
                "## File: {path}\n[Complete file omitted: {}.]",
                omission_reason(reason)
            )
        }
    }
}

fn omission_reason(read: FileRead) -> &'static str {
    match read {
        FileRead::TooLarge => "it does not fit the file context budget",
        FileRead::Unavailable => "it is unavailable on disk",
        FileRead::Changed => "it changed while a bounded snapshot was read",
        FileRead::NonUtf8 => "it is not valid UTF-8",
        FileRead::Complete(_) => "it could not be prepared",
    }
}

fn bounded_display_path(path: &str) -> String {
    const MAX_BYTES: usize = 512;
    let cleaned = path
        .chars()
        .map(|character| {
            if character.is_control() {
                '\u{fffd}'
            } else {
                character
            }
        })
        .collect::<String>();
    if cleaned.len() <= MAX_BYTES {
        return cleaned;
    }
    let mut end = MAX_BYTES.saturating_sub(3);
    while !cleaned.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &cleaned[..end])
}

fn estimate_api_messages(messages: &[Message]) -> usize {
    messages.iter().fold(0_usize, |total, message| {
        total.saturating_add(estimate_api_message(message))
    })
}

fn estimate_api_message(message: &Message) -> usize {
    let content_bytes = match &message.content {
        Content::Text(text) => text.len(),
        Content::Parts(parts) => parts.iter().fold(0_usize, |total, part| {
            total.saturating_add(match part {
                ContentPart::Text(text) => text.len(),
                ContentPart::Image { media_type, data } => {
                    media_type.len().saturating_add(data.len())
                }
            })
        }),
    };
    let tool_bytes = message.tool_calls.iter().fold(0_usize, |total, call| {
        let argument_bytes = serde_json::to_vec(&call.arguments)
            .map(|encoded| encoded.len())
            .unwrap_or_default();
        total
            .saturating_add(call.id.len())
            .saturating_add(call.name.len())
            .saturating_add(argument_bytes)
            .saturating_add(32)
    });
    let portable_bytes = content_bytes.saturating_add(tool_bytes).saturating_add(
        message
            .tool_call_id
            .as_deref()
            .map(str::len)
            .unwrap_or_default(),
    );
    let provider_bytes = message
        .provider_content
        .as_ref()
        .filter(|blocks| !blocks.is_empty())
        .and_then(|blocks| serde_json::to_vec(blocks).ok())
        .map(|bytes| bytes.len())
        .unwrap_or_default();
    let selected_bytes = if message.role == Role::Assistant {
        portable_bytes.max(provider_bytes)
    } else {
        portable_bytes
    };
    estimate_byte_len_tokens(selected_bytes)
}
