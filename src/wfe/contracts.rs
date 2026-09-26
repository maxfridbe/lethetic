use crate::commands::{CommandBehavior, CommandId, CommandView, IconId};
use serde::de::{Error as _, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
use ts_rs::{Config as TsConfig, TS};

pub const WFE_PROTOCOL_VERSION: u16 = 6;
pub const WFE_MINIMUM_PROTOCOL_VERSION: u16 = 6;
pub const MAX_SAFE_JAVASCRIPT_INTEGER: u64 = 9_007_199_254_740_991;
pub const MAX_REQUEST_ID_BYTES: usize = 64;
pub const MAX_CANCEL_ID_BYTES: usize = 64;
pub const MAX_COMMAND_MESSAGE_BYTES: usize = 256 * 1024;
pub const MAX_SERVER_MESSAGE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_PROMPT_BYTES: usize = 128 * 1024;
pub const MAX_SYSTEM_PROMPT_BYTES: usize = 256 * 1024;
pub const MAX_TOOL_CALL_ID_BYTES: usize = 512;
pub const MAX_CHOICE_ID_BYTES: usize = 128;
pub const MAX_ANSWER_BYTES: usize = 64 * 1024;
pub const MAX_ANSWERS: usize = 32;
pub const MAX_SELECTED_OPTIONS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct ICommandRequest {
    pub id: String,
    pub expected_revision: u64,
    #[serde(flatten)]
    pub command: WebCommand,
}

#[derive(Deserialize)]
struct UncheckedCommandRequest {
    id: String,
    expected_revision: u64,
    #[serde(flatten)]
    command: WebCommand,
}

impl<'de> Deserialize<'de> for ICommandRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct RequestObjectVisitor;

        impl<'de> Visitor<'de> for RequestObjectVisitor {
            type Value = serde_json::Map<String, serde_json::Value>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a web command request object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut object = serde_json::Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if object.contains_key(&key) {
                        return Err(A::Error::custom(format!("duplicate field `{key}`")));
                    }
                    object.insert(key, map.next_value()?);
                }
                Ok(object)
            }
        }

        let object = deserializer.deserialize_map(RequestObjectVisitor)?;
        let command_type = object
            .get("type")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| D::Error::custom("missing or non-string field `type`"))?;
        let allowed = WebCommand::allowed_fields(command_type)
            .ok_or_else(|| D::Error::custom(format!("unknown command type `{command_type}`")))?;
        for field in object.keys() {
            if !matches!(field.as_str(), "id" | "expected_revision" | "type")
                && !allowed.contains(&field.as_str())
            {
                return Err(D::Error::custom(format!("unknown field `{field}`")));
            }
        }

        let unchecked: UncheckedCommandRequest =
            serde_json::from_value(serde_json::Value::Object(object)).map_err(D::Error::custom)?;
        let request = Self {
            id: unchecked.id,
            expected_revision: unchecked.expected_revision,
            command: unchecked.command,
        };
        request.validate().map_err(D::Error::custom)?;
        Ok(request)
    }
}

impl ICommandRequest {
    pub fn validate(&self) -> Result<(), String> {
        validate_wire_id("request id", &self.id, MAX_REQUEST_ID_BYTES)?;
        validate_safe_integer("expected revision", self.expected_revision)?;
        self.command.validate()?;
        let encoded = serde_json::to_vec(self)
            .map_err(|error| format!("request could not be serialized: {error}"))?;
        if encoded.len() > MAX_COMMAND_MESSAGE_BYTES {
            return Err(format!(
                "request exceeds {MAX_COMMAND_MESSAGE_BYTES} encoded UTF-8 bytes"
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WebCommand {
    InvokeCommand {
        command_id: CommandId,
    },
    SendPrompt {
        session_id: String,
        prompt: String,
    },
    Stop {
        session_id: String,
        cancel_id: String,
    },
    ApproveToolOnce {
        session_id: String,
        approval_id: String,
        tool_call_id: String,
        acknowledge_hidden_content: bool,
    },
    ApproveToolAlways {
        session_id: String,
        approval_id: String,
        tool_call_id: String,
        acknowledge_hidden_content: bool,
    },
    DenyTool {
        session_id: String,
        approval_id: String,
        tool_call_id: String,
    },
    RenameSession {
        session_id: String,
        name: Option<String>,
    },
    AnswerUser {
        session_id: String,
        tool_call_id: String,
        form_id: String,
        answers: Vec<UserAnswer>,
    },
    SelectTheme {
        theme_id: String,
    },
    SelectModel {
        model_id: String,
    },
    NewSession,
    ResumeSession {
        session_id: String,
    },
    DeleteSession {
        session_id: String,
        confirmed: bool,
        confirmation_id: Option<String>,
    },
    WipeSessions {
        confirmed: bool,
        confirmation_id: Option<String>,
    },
    SelectHistoryEntry {
        session_id: String,
        entry_id: String,
    },
    SelectLatestFile {
        session_id: String,
        file_id: String,
    },
    SelectSystemPrompt {
        session_id: String,
        prompt_id: String,
    },
    SaveSystemPrompt {
        session_id: String,
        name: String,
        content: String,
        confirmed_overwrite: bool,
        confirmation_id: Option<String>,
    },
    SetLoopDetection {
        session_id: String,
        mode_id: String,
    },
    SetAgentMode {
        session_id: String,
        mode_id: String,
    },
    RunLspAction {
        session_id: String,
        server_id: String,
        action: LspAction,
    },
    ClearContext {
        session_id: String,
        confirmed: bool,
        confirmation_id: Option<String>,
    },
    DeletePythonRuntime {
        session_id: String,
        confirmed: bool,
        confirmation_id: Option<String>,
    },
    DismissOverlay,
    RequestSnapshot,
    Quit {
        confirmed: bool,
        confirmation_id: Option<String>,
    },
}

impl WebCommand {
    fn allowed_fields(command_type: &str) -> Option<&'static [&'static str]> {
        Some(match command_type {
            "invoke_command" => &["command_id"],
            "send_prompt" => &["session_id", "prompt"],
            "stop" => &["session_id", "cancel_id"],
            "approve_tool_once" | "approve_tool_always" => &[
                "session_id",
                "approval_id",
                "tool_call_id",
                "acknowledge_hidden_content",
            ],
            "deny_tool" => &["session_id", "approval_id", "tool_call_id"],
            "rename_session" => &["session_id", "name"],
            "answer_user" => &["session_id", "tool_call_id", "form_id", "answers"],
            "select_theme" => &["theme_id"],
            "select_model" => &["model_id"],
            "new_session" | "dismiss_overlay" | "request_snapshot" => &[],
            "resume_session" => &["session_id"],
            "delete_session" | "clear_context" | "delete_python_runtime" => {
                &["session_id", "confirmed", "confirmation_id"]
            }
            "wipe_sessions" | "quit" => &["confirmed", "confirmation_id"],
            "select_history_entry" => &["session_id", "entry_id"],
            "select_latest_file" => &["session_id", "file_id"],
            "select_system_prompt" => &["session_id", "prompt_id"],
            "save_system_prompt" => &[
                "session_id",
                "name",
                "content",
                "confirmed_overwrite",
                "confirmation_id",
            ],
            "set_loop_detection" | "set_agent_mode" => &["session_id", "mode_id"],
            "run_lsp_action" => &["session_id", "server_id", "action"],
            _ => return None,
        })
    }

    pub fn wire_type(&self) -> &'static str {
        match self {
            Self::InvokeCommand { .. } => "invoke_command",
            Self::SendPrompt { .. } => "send_prompt",
            Self::Stop { .. } => "stop",
            Self::ApproveToolOnce { .. } => "approve_tool_once",
            Self::ApproveToolAlways { .. } => "approve_tool_always",
            Self::DenyTool { .. } => "deny_tool",
            Self::RenameSession { .. } => "rename_session",
            Self::AnswerUser { .. } => "answer_user",
            Self::SelectTheme { .. } => "select_theme",
            Self::SelectModel { .. } => "select_model",
            Self::NewSession => "new_session",
            Self::ResumeSession { .. } => "resume_session",
            Self::DeleteSession { .. } => "delete_session",
            Self::WipeSessions { .. } => "wipe_sessions",
            Self::SelectHistoryEntry { .. } => "select_history_entry",
            Self::SelectLatestFile { .. } => "select_latest_file",
            Self::SelectSystemPrompt { .. } => "select_system_prompt",
            Self::SaveSystemPrompt { .. } => "save_system_prompt",
            Self::SetLoopDetection { .. } => "set_loop_detection",
            Self::SetAgentMode { .. } => "set_agent_mode",
            Self::RunLspAction { .. } => "run_lsp_action",
            Self::ClearContext { .. } => "clear_context",
            Self::DeletePythonRuntime { .. } => "delete_python_runtime",
            Self::DismissOverlay => "dismiss_overlay",
            Self::RequestSnapshot => "request_snapshot",
            Self::Quit { .. } => "quit",
        }
    }

    fn validate(&self) -> Result<(), String> {
        match self {
            Self::InvokeCommand { command_id } => {
                if command_id.spec().behavior == CommandBehavior::Confirm {
                    return Err(format!(
                        "command `{}` requires its dedicated confirmed request",
                        command_id.spec().base_label
                    ));
                }
                Ok(())
            }
            Self::NewSession | Self::DismissOverlay | Self::RequestSnapshot => Ok(()),
            Self::SendPrompt { session_id, prompt } => {
                validate_session_id(session_id)?;
                validate_nonempty_bounded("prompt", prompt, MAX_PROMPT_BYTES)
            }
            Self::Stop {
                session_id,
                cancel_id,
            } => {
                validate_session_id(session_id)?;
                validate_wire_id("cancellation id", cancel_id, MAX_CANCEL_ID_BYTES)
            }
            Self::ResumeSession { session_id } => validate_session_id(session_id),
            Self::DeleteSession {
                session_id,
                confirmed,
                confirmation_id,
            }
            | Self::ClearContext {
                session_id,
                confirmed,
                confirmation_id,
            }
            | Self::DeletePythonRuntime {
                session_id,
                confirmed,
                confirmation_id,
            } => {
                validate_session_id(session_id)?;
                validate_confirmation(*confirmed, confirmation_id)
            }
            Self::ApproveToolOnce {
                session_id,
                approval_id,
                tool_call_id,
                ..
            }
            | Self::ApproveToolAlways {
                session_id,
                approval_id,
                tool_call_id,
                ..
            }
            | Self::DenyTool {
                session_id,
                approval_id,
                tool_call_id,
            } => {
                validate_session_id(session_id)?;
                validate_wire_id("approval id", approval_id, MAX_CHOICE_ID_BYTES)?;
                validate_display_id("tool call id", tool_call_id, MAX_TOOL_CALL_ID_BYTES)
            }
            Self::RenameSession { session_id, name } => {
                validate_session_id(session_id)?;
                if let Some(name) = name {
                    crate::app::normalize_session_display_name(name)?;
                }
                Ok(())
            }
            Self::AnswerUser {
                session_id,
                tool_call_id,
                form_id,
                answers,
            } => {
                validate_session_id(session_id)?;
                validate_display_id("tool call id", tool_call_id, MAX_TOOL_CALL_ID_BYTES)?;
                validate_wire_id("form id", form_id, MAX_CHOICE_ID_BYTES)?;
                if answers.is_empty() || answers.len() > MAX_ANSWERS {
                    return Err(format!(
                        "answers must contain between 1 and {MAX_ANSWERS} entries"
                    ));
                }
                let mut total_bytes = 0usize;
                for answer in answers {
                    answer.validate()?;
                    total_bytes = total_bytes
                        .saturating_add(answer.question_id.len())
                        .saturating_add(
                            answer
                                .selected_option_ids
                                .iter()
                                .map(String::len)
                                .sum::<usize>(),
                        )
                        .saturating_add(answer.other_text.as_ref().map_or(0, String::len));
                }
                if total_bytes > MAX_ANSWER_BYTES {
                    return Err(format!("answers exceed {MAX_ANSWER_BYTES} UTF-8 bytes"));
                }
                Ok(())
            }
            Self::SelectTheme { theme_id } => {
                validate_wire_id("theme id", theme_id, MAX_CHOICE_ID_BYTES)
            }
            Self::SelectModel { model_id } => {
                validate_wire_id("model id", model_id, MAX_CHOICE_ID_BYTES)
            }
            Self::SelectHistoryEntry {
                session_id,
                entry_id,
            } => {
                validate_session_id(session_id)?;
                validate_wire_id("history entry id", entry_id, MAX_CHOICE_ID_BYTES)
            }
            Self::SelectLatestFile {
                session_id,
                file_id,
            } => {
                validate_session_id(session_id)?;
                validate_wire_id("file id", file_id, MAX_CHOICE_ID_BYTES)
            }
            Self::SelectSystemPrompt {
                session_id,
                prompt_id,
            } => {
                validate_session_id(session_id)?;
                validate_wire_id("system prompt id", prompt_id, MAX_CHOICE_ID_BYTES)
            }
            Self::SaveSystemPrompt {
                session_id,
                name,
                content,
                confirmed_overwrite,
                confirmation_id,
            } => {
                validate_session_id(session_id)?;
                validate_nonempty_bounded("system prompt name", name, 256)?;
                if name.chars().any(char::is_control) {
                    return Err("system prompt name contains a control character".to_string());
                }
                validate_nonempty_bounded(
                    "system prompt content",
                    content,
                    MAX_SYSTEM_PROMPT_BYTES,
                )?;
                validate_confirmation(*confirmed_overwrite, confirmation_id)
            }
            Self::SetLoopDetection {
                session_id,
                mode_id,
            }
            | Self::SetAgentMode {
                session_id,
                mode_id,
            } => {
                validate_session_id(session_id)?;
                validate_wire_id("mode id", mode_id, MAX_CHOICE_ID_BYTES)
            }
            Self::RunLspAction {
                session_id,
                server_id,
                ..
            } => {
                validate_session_id(session_id)?;
                validate_wire_id("LSP server id", server_id, MAX_CHOICE_ID_BYTES)
            }
            Self::WipeSessions {
                confirmed,
                confirmation_id,
            }
            | Self::Quit {
                confirmed,
                confirmation_id,
            } => validate_confirmation(*confirmed, confirmation_id),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct UserAnswer {
    pub question_id: String,
    pub selected_option_ids: Vec<String>,
    pub other_text: Option<String>,
}

impl UserAnswer {
    fn validate(&self) -> Result<(), String> {
        validate_wire_id("answer question id", &self.question_id, MAX_CHOICE_ID_BYTES)?;
        if self.selected_option_ids.len() > MAX_SELECTED_OPTIONS {
            return Err(format!(
                "an answer may select at most {MAX_SELECTED_OPTIONS} options"
            ));
        }
        for option_id in &self.selected_option_ids {
            validate_wire_id("answer option id", option_id, MAX_CHOICE_ID_BYTES)?;
        }
        if let Some(text) = &self.other_text {
            validate_bounded("answer text", text, MAX_ANSWER_BYTES)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum LspAction {
    Install,
    Enable,
    Disable,
    CancelInstall,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct ICommandResponse {
    pub id: String,
    pub result: CommandResult,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UncheckedCommandResponse {
    id: String,
    result: CommandResult,
}

impl<'de> Deserialize<'de> for ICommandResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let unchecked = UncheckedCommandResponse::deserialize(deserializer)?;
        let response = Self {
            id: unchecked.id,
            result: unchecked.result,
        };
        response.validate().map_err(D::Error::custom)?;
        Ok(response)
    }
}

impl ICommandResponse {
    pub fn validate(&self) -> Result<(), String> {
        validate_wire_id("response id", &self.id, MAX_REQUEST_ID_BYTES)?;
        match &self.result {
            CommandResult::Ok { revision, outcome } => {
                validate_safe_integer("response revision", *revision)?;
                outcome.validate()
            }
            CommandResult::Error { error } => error.validate(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum CommandResult {
    Ok {
        revision: u64,
        outcome: CommandOutcome,
    },
    Error {
        error: CommandError,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CommandOutcome {
    Applied,
    PanelOpened {
        panel: PanelId,
    },
    SnapshotQueued,
    PromptAccepted {
        session_id: String,
    },
    HistoryEntrySelected {
        session_id: String,
        entry_id: String,
        editor_content: String,
    },
    ToolDecisionRecorded {
        tool_call_id: String,
    },
    SessionCreated {
        session_id: String,
    },
    SessionLoaded {
        session_id: String,
    },
    SessionRenamed {
        session_id: String,
        display_name: Option<String>,
    },
    SessionDeleted {
        session_id: String,
    },
    SessionsWiped,
    ShuttingDown,
}

impl CommandOutcome {
    fn validate(&self) -> Result<(), String> {
        match self {
            Self::HistoryEntrySelected {
                session_id,
                entry_id,
                editor_content,
            } => {
                validate_session_id(session_id)?;
                validate_wire_id("history entry id", entry_id, MAX_CHOICE_ID_BYTES)?;
                validate_bounded("history editor content", editor_content, MAX_PROMPT_BYTES)
            }
            Self::PromptAccepted { session_id }
            | Self::SessionCreated { session_id }
            | Self::SessionLoaded { session_id }
            | Self::SessionDeleted { session_id } => validate_session_id(session_id),
            Self::SessionRenamed {
                session_id,
                display_name,
            } => {
                validate_session_id(session_id)?;
                if let Some(name) = display_name {
                    crate::app::normalize_session_display_name(name)?;
                }
                Ok(())
            }
            Self::ToolDecisionRecorded { tool_call_id } => {
                validate_display_id("tool call id", tool_call_id, MAX_TOOL_CALL_ID_BYTES)
            }
            Self::Applied
            | Self::PanelOpened { .. }
            | Self::SnapshotQueued
            | Self::SessionsWiped
            | Self::ShuttingDown => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct CommandError {
    pub code: CommandErrorCode,
    pub message: String,
    pub current_revision: Option<u64>,
    pub retryable: bool,
}

impl CommandError {
    fn validate(&self) -> Result<(), String> {
        validate_nonempty_bounded("command error message", &self.message, 1024)?;
        if self
            .message
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
        {
            return Err("command error message contains a control character".to_string());
        }
        if let Some(revision) = self.current_revision {
            validate_safe_integer("command error current revision", revision)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum CommandErrorCode {
    BadRequest,
    ProtocolMismatch,
    StaleRevision,
    SessionMismatch,
    ToolCallMismatch,
    RequestIdConflict,
    Busy,
    ConfirmationRequired,
    NotFound,
    SaveFailed,
    BackendUnavailable,
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    Text,
    User,
    Thought,
    Markdown,
    ToolCall,
    ToolResult,
    Divider,
    Formulating,
    Truncation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ToolBlockKind {
    Call,
    Result,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionTruncationKind {
    SizeLimit,
    InvalidSource,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct ProjectionLossView {
    pub filtered: bool,
    pub redacted: bool,
    pub truncation: Option<ProjectionTruncationKind>,
}

impl ProjectionLossView {
    pub const fn complete() -> Self {
        Self {
            filtered: false,
            redacted: false,
            truncation: None,
        }
    }

    pub const fn is_lossy(self) -> bool {
        self.filtered || self.redacted || self.truncation.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct ToolBlockView {
    pub kind: ToolBlockKind,
    pub tool_name: String,
    pub payload: String,
    pub payload_loss: ProjectionLossView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct RenderBlockView {
    pub kind: BlockKind,
    pub content: String,
    pub content_loss: ProjectionLossView,
    pub tool: Option<ToolBlockView>,
    pub title: Option<String>,
    pub title_loss: ProjectionLossView,
    pub success: Option<bool>,
    pub usage: Option<UsageView>,
    pub estimated_cost: Option<CostView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct BlockListView {
    pub blocks: Vec<RenderBlockView>,
    pub omitted_before: u32,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct UsageView {
    pub uncached_input_tokens: String,
    pub cache_read_input_tokens: String,
    pub cache_creation_input_tokens: String,
    pub output_tokens: String,
    pub total_input_tokens: String,
    pub total_tokens: String,
    pub breakdown_complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct CostView {
    pub display: String,
    pub currency: String,
    pub nanos: String,
    pub incomplete: bool,
    pub mixed_pricing: bool,
    pub long_context_applied: bool,
    pub pricing_effective_as_of: String,
    pub pricing_valid_through: Option<String>,
    pub provenance_kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AccountingTotalsView {
    pub usage: UsageView,
    pub estimated_cost: Option<CostView>,
    pub request_count: String,
    pub long_context_request_count: String,
    pub unpriced_request_count: String,
    pub incomplete_usage_request_count: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct UsageSummaryView {
    pub latest_turn: AccountingTotalsView,
    pub session: AccountingTotalsView,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    Idle,
    LoadingSession,
    AwaitingApproval,
    ExecutingTool,
    AwaitingAnswer,
    Processing,
    ManagingLsp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct ActivityView {
    pub kind: ActivityKind,
    pub fully_idle: bool,
    pub cancellable: bool,
    pub cancel_id: Option<String>,
    pub progress_percent: Option<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    ApproveOnce,
    ApproveAlways,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct PendingApprovalView {
    pub approval_id: String,
    pub session_id: String,
    pub tool_call_id: String,
    pub tool_name: String,
    pub description: String,
    pub preview: String,
    pub preview_redacted: bool,
    pub preview_truncated: bool,
    pub can_view_original: bool,
    pub allowed_decisions: Vec<ApprovalDecision>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct QuestionOptionView {
    pub option_id: String,
    pub label: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct QuestionPromptView {
    pub question_id: String,
    pub prompt: String,
    pub options: Vec<QuestionOptionView>,
    pub multiple: bool,
    pub allows_other: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct PendingQuestionView {
    pub form_id: String,
    pub session_id: String,
    pub tool_call_id: String,
    pub questions: Vec<QuestionPromptView>,
    pub content_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct SessionHeaderView {
    pub session_id: String,
    pub display_name: Option<String>,
    pub fallback_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct SessionChoiceView {
    pub session_id: String,
    pub display_name: Option<String>,
    pub fallback_label: String,
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct SessionListView {
    pub sessions: Vec<SessionChoiceView>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ModelTransportView {
    OpenAiChatCompletions,
    ClaudeCodeProxy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct ModelChoiceView {
    pub model_id: String,
    pub label: String,
    pub model_name: String,
    pub transport: ModelTransportView,
    pub available: bool,
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct ThemeColorsView {
    pub output_fg: String,
    pub input_fg: String,
    pub highlight_fg: String,
    pub system_fg: String,
    pub thought_fg: String,
    pub tool_fg: String,
    pub success_fg: String,
    pub error_fg: String,
    pub warning_fg: String,
    pub json_key_fg: String,
    pub json_val_fg: String,
    pub input_bg: String,
    pub thought_bg: String,
    pub tool_bg: String,
    pub terminal_bg: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct ThemeView {
    pub theme_id: String,
    pub name: String,
    pub colors: ThemeColorsView,
    pub selected: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum GitStateView {
    Clean,
    Dirty,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ContextUsageSourceView {
    ServerUsage,
    ServerPrompt,
    LocalEstimate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum PythonProfileView {
    General,
    PythonOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum PythonTargetView {
    Host,
    Sandbox,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum PythonBackendView {
    Bubblewrap,
    Podman,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum NetworkAccessView {
    None,
    PublicOnly,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceAccessView {
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum PythonPolicySourceView {
    Config,
    Global,
    Project,
    OneTime,
    CliLocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum PythonContainerKindView {
    Retained,
    Transient,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct PythonContainerView {
    pub kind: PythonContainerKindView,
    pub name: String,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct PythonIsolationView {
    pub profile: PythonProfileView,
    pub target: Option<PythonTargetView>,
    pub backend: Option<PythonBackendView>,
    pub network: Option<NetworkAccessView>,
    pub workspace_access: Option<WorkspaceAccessView>,
    pub grant_count: u16,
    pub policy_source: PythonPolicySourceView,
    pub container: Option<PythonContainerView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct StatusView {
    pub stop_reason: String,
    pub stop_reason_loss: ProjectionLossView,
    pub model_label: String,
    pub provider_label: String,
    pub provider_transport: ModelTransportView,
    pub python: PythonIsolationView,
    pub tokens_per_second: Option<String>,
    pub prompt_tokens_per_second: Option<String>,
    pub context_tokens: String,
    pub context_limit_tokens: String,
    pub context_source: ContextUsageSourceView,
    pub request_usage: Option<UsageView>,
    pub memory_mebibytes: String,
    pub file_count: u32,
    pub visible_block_count: u16,
    pub git_state: GitStateView,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCode {
    ConnectionInterrupted,
    SaveFailed,
    SessionLoadFailed,
    ToolFailed,
    RemoteControlDegraded,
    ContentTruncated,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticView {
    pub code: DiagnosticCode,
    pub severity: DiagnosticSeverity,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct DebuggerView {
    pub open: bool,
    pub summary: String,
    pub entries: Vec<DiagnosticView>,
    pub omitted_before: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum PanelId {
    CommandPalette,
    Hotkeys,
    Themes,
    InputHistory,
    LoopDetection,
    SystemPrompt,
    Sessions,
    NameSession,
    LatestFiles,
    Models,
    LspServers,
    AgentMode,
    ToolApproval,
    AskUser,
    Confirmation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct HistoryEntryView {
    pub entry_id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct FileChoiceView {
    pub file_id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct SystemPromptChoiceView {
    pub prompt_id: String,
    pub label: String,
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct ModeChoiceView {
    pub mode_id: String,
    pub label: String,
    pub selected: bool,
    pub enabled: bool,
    pub disabled_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum LspServerStateView {
    Available,
    Installed,
    Installing,
    Enabled,
    Disabled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LspServerChoiceView {
    pub server_id: String,
    pub label: String,
    pub state: LspServerStateView,
    pub allowed_actions: Vec<LspAction>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct ShortcutView {
    pub keys: String,
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum DestructiveActionView {
    ClearContext,
    DeletePythonRuntime,
    DeleteSession,
    WipeSessions,
    Quit,
    OverwriteSystemPrompt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct ConfirmationView {
    pub confirmation_id: String,
    pub action: DestructiveActionView,
    pub session_id: Option<String>,
    pub title: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PanelDataView {
    Hotkeys {
        shortcuts: Vec<ShortcutView>,
    },
    InputHistory {
        entries: Vec<HistoryEntryView>,
        has_more: bool,
    },
    LatestFiles {
        files: Vec<FileChoiceView>,
        has_more: bool,
    },
    SystemPrompts {
        prompts: Vec<SystemPromptChoiceView>,
        editor_content: Option<String>,
        content_truncated: bool,
    },
    LspServers {
        servers: Vec<LspServerChoiceView>,
    },
    LoopModes {
        modes: Vec<ModeChoiceView>,
    },
    AgentModes {
        modes: Vec<ModeChoiceView>,
    },
    NameSession {
        current_name: Option<String>,
    },
    Confirmation {
        confirmation: ConfirmationView,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct OverlayView {
    pub active_panel: Option<PanelId>,
    pub data: Option<PanelDataView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct WebAppSnapshot {
    pub session: SessionHeaderView,
    pub blocks: BlockListView,
    pub activity: ActivityView,
    pub pending_approval: Option<PendingApprovalView>,
    pub pending_question: Option<PendingQuestionView>,
    pub commands: Vec<CommandView>,
    pub sessions: SessionListView,
    pub models: Vec<ModelChoiceView>,
    pub themes: Vec<ThemeView>,
    pub usage: UsageSummaryView,
    pub status: StatusView,
    pub debugger: DebuggerView,
    pub overlay: OverlayView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum StateChange {
    Session { value: SessionHeaderView },
    Blocks { value: BlockListView },
    Activity { value: ActivityView },
    PendingApproval { value: Option<PendingApprovalView> },
    PendingQuestion { value: Option<PendingQuestionView> },
    Commands { value: Vec<CommandView> },
    Sessions { value: SessionListView },
    Models { value: Vec<ModelChoiceView> },
    Themes { value: Vec<ThemeView> },
    Usage { value: Box<UsageSummaryView> },
    Status { value: StatusView },
    Debugger { value: DebuggerView },
    Overlay { value: OverlayView },
}

impl StateChange {
    fn section(&self) -> &'static str {
        match self {
            Self::Session { .. } => "session",
            Self::Blocks { .. } => "blocks",
            Self::Activity { .. } => "activity",
            Self::PendingApproval { .. } => "pending_approval",
            Self::PendingQuestion { .. } => "pending_question",
            Self::Commands { .. } => "commands",
            Self::Sessions { .. } => "sessions",
            Self::Models { .. } => "models",
            Self::Themes { .. } => "themes",
            Self::Usage { .. } => "usage",
            Self::Status { .. } => "status",
            Self::Debugger { .. } => "debugger",
            Self::Overlay { .. } => "overlay",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct IStateSnapshot {
    pub protocol_version: u16,
    pub sequence: u64,
    pub revision: u64,
    pub state: WebAppSnapshot,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UncheckedStateSnapshot {
    protocol_version: u16,
    sequence: u64,
    revision: u64,
    state: WebAppSnapshot,
}

impl<'de> Deserialize<'de> for IStateSnapshot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let unchecked = UncheckedStateSnapshot::deserialize(deserializer)?;
        if unchecked.protocol_version != WFE_PROTOCOL_VERSION {
            return Err(D::Error::custom(format!(
                "unsupported WFE protocol version {}",
                unchecked.protocol_version
            )));
        }
        Self::new(unchecked.sequence, unchecked.revision, unchecked.state).map_err(D::Error::custom)
    }
}

impl IStateSnapshot {
    pub fn new(sequence: u64, revision: u64, state: WebAppSnapshot) -> Result<Self, String> {
        validate_safe_integer("snapshot sequence", sequence)?;
        validate_safe_integer("snapshot revision", revision)?;
        Ok(Self {
            protocol_version: WFE_PROTOCOL_VERSION,
            sequence,
            revision,
            state,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct IStatePatch {
    pub sequence: u64,
    pub base_revision: u64,
    pub revision: u64,
    pub changes: Vec<StateChange>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UncheckedStatePatch {
    sequence: u64,
    base_revision: u64,
    revision: u64,
    changes: Vec<StateChange>,
}

impl<'de> Deserialize<'de> for IStatePatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let unchecked = UncheckedStatePatch::deserialize(deserializer)?;
        let patch = Self {
            sequence: unchecked.sequence,
            base_revision: unchecked.base_revision,
            revision: unchecked.revision,
            changes: unchecked.changes,
        };
        patch.validate().map_err(D::Error::custom)?;
        Ok(patch)
    }
}

impl IStatePatch {
    pub fn validate(&self) -> Result<(), String> {
        validate_safe_integer("patch sequence", self.sequence)?;
        validate_safe_integer("patch base revision", self.base_revision)?;
        validate_safe_integer("patch revision", self.revision)?;
        if self.sequence == 0 {
            return Err("patch sequence must be greater than zero".to_string());
        }
        if self.revision <= self.base_revision {
            return Err("patch revision must be greater than its base revision".to_string());
        }
        if self.changes.is_empty() {
            return Err("patch must contain at least one state change".to_string());
        }
        let mut sections = std::collections::HashSet::new();
        for change in &self.changes {
            if !sections.insert(change.section()) {
                return Err(format!(
                    "patch contains duplicate `{}` changes",
                    change.section()
                ));
            }
        }
        Ok(())
    }

    pub fn between(
        sequence: u64,
        base_revision: u64,
        revision: u64,
        previous: &WebAppSnapshot,
        current: &WebAppSnapshot,
    ) -> Result<Self, String> {
        validate_safe_integer("patch sequence", sequence)?;
        validate_safe_integer("patch base revision", base_revision)?;
        validate_safe_integer("patch revision", revision)?;
        if revision <= base_revision {
            return Err("patch revision must be greater than its base revision".to_string());
        }

        let mut changes = Vec::new();
        push_change(&mut changes, &previous.session, &current.session, || {
            StateChange::Session {
                value: current.session.clone(),
            }
        });
        push_change(&mut changes, &previous.blocks, &current.blocks, || {
            StateChange::Blocks {
                value: current.blocks.clone(),
            }
        });
        push_change(&mut changes, &previous.activity, &current.activity, || {
            StateChange::Activity {
                value: current.activity.clone(),
            }
        });
        push_change(
            &mut changes,
            &previous.pending_approval,
            &current.pending_approval,
            || StateChange::PendingApproval {
                value: current.pending_approval.clone(),
            },
        );
        push_change(
            &mut changes,
            &previous.pending_question,
            &current.pending_question,
            || StateChange::PendingQuestion {
                value: current.pending_question.clone(),
            },
        );
        push_change(&mut changes, &previous.commands, &current.commands, || {
            StateChange::Commands {
                value: current.commands.clone(),
            }
        });
        push_change(&mut changes, &previous.sessions, &current.sessions, || {
            StateChange::Sessions {
                value: current.sessions.clone(),
            }
        });
        push_change(&mut changes, &previous.models, &current.models, || {
            StateChange::Models {
                value: current.models.clone(),
            }
        });
        push_change(&mut changes, &previous.themes, &current.themes, || {
            StateChange::Themes {
                value: current.themes.clone(),
            }
        });
        push_change(&mut changes, &previous.usage, &current.usage, || {
            StateChange::Usage {
                value: Box::new(current.usage.clone()),
            }
        });
        push_change(&mut changes, &previous.status, &current.status, || {
            StateChange::Status {
                value: current.status.clone(),
            }
        });
        push_change(&mut changes, &previous.debugger, &current.debugger, || {
            StateChange::Debugger {
                value: current.debugger.clone(),
            }
        });
        push_change(&mut changes, &previous.overlay, &current.overlay, || {
            StateChange::Overlay {
                value: current.overlay.clone(),
            }
        });

        let patch = Self {
            sequence,
            base_revision,
            revision,
            changes,
        };
        patch.validate()?;
        Ok(patch)
    }
}

fn push_change<T: PartialEq>(
    changes: &mut Vec<StateChange>,
    previous: &T,
    current: &T,
    make: impl FnOnce() -> StateChange,
) {
    if previous != current {
        changes.push(make());
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[serde(deny_unknown_fields)]
pub struct ProtocolCapabilities {
    pub state_patches: bool,
    pub request_replay: bool,
    pub session_names: bool,
    pub exact_tool_approval: bool,
    pub read_only_files: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct IProtocolHello {
    pub protocol_version: u16,
    pub minimum_protocol_version: u16,
    pub server_name: String,
    pub sequence: u64,
    pub revision: u64,
    pub capabilities: ProtocolCapabilities,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UncheckedProtocolHello {
    protocol_version: u16,
    minimum_protocol_version: u16,
    server_name: String,
    sequence: u64,
    revision: u64,
    capabilities: ProtocolCapabilities,
}

impl<'de> Deserialize<'de> for IProtocolHello {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let unchecked = UncheckedProtocolHello::deserialize(deserializer)?;
        if unchecked.protocol_version != WFE_PROTOCOL_VERSION
            || unchecked.minimum_protocol_version != WFE_MINIMUM_PROTOCOL_VERSION
            || unchecked.server_name != "lethetic"
        {
            return Err(D::Error::custom("incompatible WFE protocol hello"));
        }
        Self::new(
            unchecked.sequence,
            unchecked.revision,
            unchecked.capabilities,
        )
        .map_err(D::Error::custom)
    }
}

impl IProtocolHello {
    pub fn new(
        sequence: u64,
        revision: u64,
        capabilities: ProtocolCapabilities,
    ) -> Result<Self, String> {
        validate_safe_integer("hello sequence", sequence)?;
        validate_safe_integer("hello revision", revision)?;
        Ok(Self {
            protocol_version: WFE_PROTOCOL_VERSION,
            minimum_protocol_version: WFE_MINIMUM_PROTOCOL_VERSION,
            server_name: "lethetic".to_string(),
            sequence,
            revision,
            capabilities,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum IServerMessage {
    Hello { hello: IProtocolHello },
    CommandResponse { response: ICommandResponse },
    StateSnapshot { snapshot: Box<IStateSnapshot> },
    StatePatch { patch: IStatePatch },
}

fn validate_confirmation(confirmed: bool, confirmation_id: &Option<String>) -> Result<(), String> {
    match (confirmed, confirmation_id) {
        (true, Some(confirmation_id)) => {
            validate_wire_id("confirmation id", confirmation_id, MAX_CHOICE_ID_BYTES)
        }
        (true, None) => Err("confirmed command is missing its confirmation id".to_string()),
        (false, Some(_)) => {
            Err("unconfirmed command must not include a confirmation id".to_string())
        }
        (false, None) => Ok(()),
    }
}

fn validate_safe_integer(label: &str, value: u64) -> Result<(), String> {
    if value > MAX_SAFE_JAVASCRIPT_INTEGER {
        return Err(format!("{label} exceeds JavaScript's maximum safe integer"));
    }
    Ok(())
}

fn validate_session_id(value: &str) -> Result<(), String> {
    let parsed = uuid::Uuid::parse_str(value)
        .map_err(|_| "session id must be a canonical lowercase UUID".to_string())?;
    if parsed.hyphenated().to_string() != value {
        return Err("session id must be a canonical lowercase UUID".to_string());
    }
    Ok(())
}

fn validate_wire_id(label: &str, value: &str, max_bytes: usize) -> Result<(), String> {
    validate_nonempty_bounded(label, value, max_bytes)?;
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(format!(
            "{label} may contain only ASCII letters, digits, '-', '_', '.', and ':'"
        ));
    }
    Ok(())
}

fn validate_display_id(label: &str, value: &str, max_bytes: usize) -> Result<(), String> {
    validate_nonempty_bounded(label, value, max_bytes)?;
    if value.chars().any(char::is_control) {
        return Err(format!("{label} contains a control character"));
    }
    Ok(())
}

fn validate_nonempty_bounded(label: &str, value: &str, max_bytes: usize) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{label} cannot be empty"));
    }
    validate_bounded(label, value, max_bytes)
}

fn validate_bounded(label: &str, value: &str, max_bytes: usize) -> Result<(), String> {
    if value.len() > max_bytes {
        return Err(format!("{label} exceeds {max_bytes} UTF-8 bytes"));
    }
    Ok(())
}

pub fn typescript_declarations() -> String {
    let config = TsConfig::default().with_large_int("number");
    let mut declarations = String::new();
    macro_rules! declaration {
        ($type:ty) => {{
            declarations.push_str("export ");
            declarations.push_str(&<$type as TS>::decl(&config));
            declarations.push_str("\n\n");
        }};
    }

    declaration!(CommandId);
    declaration!(IconId);
    declaration!(CommandBehavior);
    declaration!(CommandView);
    declaration!(UserAnswer);
    declaration!(LspAction);
    declaration!(WebCommand);
    declaration!(ICommandRequest);
    declaration!(CommandErrorCode);
    declaration!(PanelId);
    declaration!(CommandOutcome);
    declaration!(CommandError);
    declaration!(CommandResult);
    declaration!(ICommandResponse);
    declaration!(BlockKind);
    declaration!(ToolBlockKind);
    declaration!(ProjectionTruncationKind);
    declaration!(ProjectionLossView);
    declaration!(ToolBlockView);
    declaration!(UsageView);
    declaration!(CostView);
    declaration!(RenderBlockView);
    declaration!(BlockListView);
    declaration!(AccountingTotalsView);
    declaration!(UsageSummaryView);
    declaration!(ActivityKind);
    declaration!(ActivityView);
    declaration!(ApprovalDecision);
    declaration!(PendingApprovalView);
    declaration!(QuestionOptionView);
    declaration!(QuestionPromptView);
    declaration!(PendingQuestionView);
    declaration!(SessionHeaderView);
    declaration!(SessionChoiceView);
    declaration!(SessionListView);
    declaration!(ModelTransportView);
    declaration!(ModelChoiceView);
    declaration!(ThemeColorsView);
    declaration!(ThemeView);
    declaration!(GitStateView);
    declaration!(ContextUsageSourceView);
    declaration!(PythonProfileView);
    declaration!(PythonTargetView);
    declaration!(PythonBackendView);
    declaration!(NetworkAccessView);
    declaration!(WorkspaceAccessView);
    declaration!(PythonPolicySourceView);
    declaration!(PythonContainerKindView);
    declaration!(PythonContainerView);
    declaration!(PythonIsolationView);
    declaration!(StatusView);
    declaration!(DiagnosticSeverity);
    declaration!(DiagnosticCode);
    declaration!(DiagnosticView);
    declaration!(DebuggerView);
    declaration!(HistoryEntryView);
    declaration!(FileChoiceView);
    declaration!(SystemPromptChoiceView);
    declaration!(ModeChoiceView);
    declaration!(LspServerStateView);
    declaration!(LspServerChoiceView);
    declaration!(ShortcutView);
    declaration!(DestructiveActionView);
    declaration!(ConfirmationView);
    declaration!(PanelDataView);
    declaration!(OverlayView);
    declaration!(WebAppSnapshot);
    declaration!(StateChange);
    declaration!(IStateSnapshot);
    declaration!(IStatePatch);
    declaration!(ProtocolCapabilities);
    declaration!(IProtocolHello);
    declaration!(IServerMessage);
    declaration!(super::file_contracts::FileEntryKind);
    declaration!(super::file_contracts::FileExclusionCounts);
    declaration!(super::file_contracts::FileListEntry);
    declaration!(super::file_contracts::FilesListRequest);
    declaration!(super::file_contracts::FilesListResponse);
    declaration!(super::file_contracts::FilesReadRequest);
    declaration!(super::file_contracts::FilesReadResponse);
    declaration!(super::file_contracts::FilesDownloadRequest);
    declaration!(super::file_contracts::FilesArchiveRequest);
    declaration!(super::file_contracts::FilesErrorCode);
    declaration!(super::file_contracts::FilesApiError);
    declaration!(super::file_contracts::FilesErrorResponse);
    declarations.push_str(&format!(
        "export const WFE_MAX_COMMAND_MESSAGE_BYTES = {MAX_COMMAND_MESSAGE_BYTES} as const;\n\n"
    ));
    declarations.push_str(&format!(
        "export const WFE_MAX_SERVER_MESSAGE_BYTES = {MAX_SERVER_MESSAGE_BYTES} as const;\n\n"
    ));
    declarations
}

#[cfg(test)]
mod tests;
