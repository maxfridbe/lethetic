use super::actor::{
    ConnectionId, ReplayLookup, RequestFingerprint, RequestReplayCache, WfeActorHandle,
    WfeActorReceiver, WfeCommandEnvelope, command_channel,
};
use super::contracts::*;
use super::diagnostics::{OperationalDiagnostics, OperationalEvent};
use super::presentation::{
    MAX_WEB_MODELS, ProjectionContext, opaque_choice_id, project_app,
    system_prompt_editor_projection_is_lossy, theme_id,
};
use super::state::{MirrorHandle, MirrorPublisher};
use crate::app::App;
use crate::wfe::security::ControllerAuthenticationMode;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

pub const WFE_CONNECTION_LIFECYCLE_CAPACITY: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WfeDisconnectCategory {
    PeerClosed,
    TransportError,
    ProtocolViolation,
    ServerShutdown,
    BackendUnavailable,
    StateChannelClosed,
    SendFailed,
    SetupFailed,
}

impl WfeDisconnectCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PeerClosed => "peer-closed",
            Self::TransportError => "transport-error",
            Self::ProtocolViolation => "protocol-violation",
            Self::ServerShutdown => "server-shutdown",
            Self::BackendUnavailable => "backend-unavailable",
            Self::StateChannelClosed => "state-channel-closed",
            Self::SendFailed => "send-failed",
            Self::SetupFailed => "setup-failed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WfeConnectedEvent {
    pub peer_ip: IpAddr,
    pub connection_ordinal: u64,
    pub active_clients: usize,
    pub authentication_mode: ControllerAuthenticationMode,
    pub initial_sequence: u64,
    pub initial_revision: u64,
    pub round_trip_time: Option<Duration>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WfeDisconnectedEvent {
    pub peer_ip: IpAddr,
    pub connection_ordinal: u64,
    pub active_clients: usize,
    pub uptime: Duration,
    pub category: WfeDisconnectCategory,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WfeConnectionEvent {
    Connected(WfeConnectedEvent),
    Disconnected(WfeDisconnectedEvent),
}

pub enum WfeRuntimeEvent {
    Command(WfeCommandEnvelope),
    Connection(WfeConnectionEvent),
}

#[derive(Clone)]
pub struct WfeFrontendHandle {
    pub commands: WfeActorHandle,
    pub mirror: MirrorHandle,
    pub(crate) connection_events: mpsc::Sender<WfeConnectionEvent>,
}

pub const WFE_IN_FLIGHT_REQUEST_CAPACITY: usize = 64;

pub struct AdmittedCommand {
    pub connection_id: ConnectionId,
    pub request: ICommandRequest,
    fingerprint: RequestFingerprint,
}

struct InFlightRequest {
    fingerprint: RequestFingerprint,
    responses: Vec<oneshot::Sender<ICommandResponse>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeFailure {
    pub code: CommandErrorCode,
    pub message: String,
    pub retryable: bool,
}

impl RuntimeFailure {
    pub fn new(code: CommandErrorCode, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code,
            message: message.into(),
            retryable,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ApprovalKey {
    session_id: String,
    tool_call_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct QuestionKey {
    session_id: String,
    tool_call_id: String,
}

#[derive(Default)]
struct PresentationState {
    approval_key: Option<ApprovalKey>,
    approval_id: Option<String>,
    question_key: Option<QuestionKey>,
    form_id: Option<String>,
    confirmation: Option<ConfirmationView>,
    confirmation_binding: Option<String>,
    requested_panel_data: Option<PanelDataView>,
    /// The panel data mirrors a terminal dialog, so it closes with it.
    panel_from_app: bool,
    diagnostics: OperationalDiagnostics,
    last_activity: Option<ActivityKind>,
    sensitive_values: Vec<String>,
}

pub struct WfeRuntime {
    receiver: WfeActorReceiver,
    connection_events: mpsc::Receiver<WfeConnectionEvent>,
    commands_open: bool,
    connection_events_open: bool,
    publisher: MirrorPublisher,
    replay: RequestReplayCache,
    in_flight: HashMap<String, InFlightRequest>,
    presentation: PresentationState,
}

impl WfeRuntime {
    pub fn new(
        app: &App,
        sensitive_values: Vec<String>,
    ) -> Result<(WfeFrontendHandle, Self), String> {
        let (commands, receiver) = command_channel();
        let (connection_events, connection_event_receiver) =
            mpsc::channel(WFE_CONNECTION_LIFECYCLE_CAPACITY);
        let mut presentation = PresentationState {
            sensitive_values,
            ..PresentationState::default()
        };
        presentation
            .diagnostics
            .record(OperationalEvent::ActorStarting);
        sync_transient_views(app, &mut presentation);
        let initial_activity = project_with_state(app, &presentation).activity.kind;
        presentation.last_activity = Some(initial_activity);
        presentation
            .diagnostics
            .record(OperationalEvent::Activity(initial_activity));
        let state = project_with_state(app, &presentation);
        let publisher = MirrorPublisher::new(state)?;
        let frontend = WfeFrontendHandle {
            commands,
            mirror: publisher.handle(),
            connection_events,
        };
        Ok((
            frontend,
            Self {
                receiver,
                connection_events: connection_event_receiver,
                commands_open: true,
                connection_events_open: true,
                publisher,
                replay: RequestReplayCache::default(),
                in_flight: HashMap::new(),
                presentation,
            },
        ))
    }

    pub async fn recv(&mut self) -> Option<WfeCommandEnvelope> {
        self.receiver.recv().await
    }

    pub async fn recv_connection_event(&mut self) -> Option<WfeConnectionEvent> {
        self.connection_events.recv().await
    }

    pub async fn recv_event(&mut self) -> Option<WfeRuntimeEvent> {
        loop {
            match (self.commands_open, self.connection_events_open) {
                (true, true) => {
                    tokio::select! {
                        command = self.receiver.recv() => match command {
                            Some(command) => return Some(WfeRuntimeEvent::Command(command)),
                            None => self.commands_open = false,
                        },
                        event = self.connection_events.recv() => match event {
                            Some(event) => return Some(WfeRuntimeEvent::Connection(event)),
                            None => self.connection_events_open = false,
                        },
                    }
                }
                (true, false) => match self.receiver.recv().await {
                    Some(command) => return Some(WfeRuntimeEvent::Command(command)),
                    None => self.commands_open = false,
                },
                (false, true) => match self.connection_events.recv().await {
                    Some(event) => return Some(WfeRuntimeEvent::Connection(event)),
                    None => self.connection_events_open = false,
                },
                (false, false) => return None,
            }
        }
    }

    pub fn close(&mut self) {
        self.receiver.close();
        self.connection_events.close();
    }

    pub fn revision(&self) -> u64 {
        self.publisher.revision()
    }

    pub fn record_operational(&mut self, event: OperationalEvent) {
        self.presentation.diagnostics.record(event);
    }

    pub fn publish(&mut self, app: &App) -> Result<bool, String> {
        sync_transient_views(app, &mut self.presentation);
        let mut state = project_with_state(app, &self.presentation);
        if self.presentation.last_activity != Some(state.activity.kind) {
            self.presentation.last_activity = Some(state.activity.kind);
            self.presentation
                .diagnostics
                .record(OperationalEvent::Activity(state.activity.kind));
            state = project_with_state(app, &self.presentation);
        }
        Ok(self.publisher.publish(state)?.is_some())
    }

    pub fn admit(&mut self, app: &App, envelope: WfeCommandEnvelope) -> Option<AdmittedCommand> {
        let WfeCommandEnvelope {
            connection_id,
            request,
            response,
        } = envelope;
        let fingerprint = match self.replay.lookup(&request, &app.session_id) {
            Ok(ReplayLookup::Replay(replayed)) => {
                let _ = response.send(replayed);
                return None;
            }
            Ok(ReplayLookup::ContentSessionMismatch) => {
                let mismatch = self.error_response(
                    request.id.clone(),
                    RuntimeFailure::new(
                        CommandErrorCode::SessionMismatch,
                        "Content response belongs to a different active session",
                        true,
                    ),
                );
                let _ = response.send(mismatch);
                return None;
            }
            Ok(ReplayLookup::Conflict) => {
                let conflict = self.error_response(
                    request.id.clone(),
                    RuntimeFailure::new(
                        CommandErrorCode::RequestIdConflict,
                        "Request ID was reused with different command data",
                        false,
                    ),
                );
                let _ = response.send(conflict);
                return None;
            }
            Ok(ReplayLookup::Miss(fingerprint)) => fingerprint,
            Err(error) => {
                let failure = self.error_response(
                    request.id.clone(),
                    RuntimeFailure::new(CommandErrorCode::BadRequest, error, false),
                );
                let _ = response.send(failure);
                return None;
            }
        };

        match self.in_flight.get_mut(&request.id) {
            Some(in_flight) if in_flight.fingerprint == fingerprint => {
                in_flight.responses.push(response);
                return None;
            }
            Some(_) => {
                let conflict = self.error_response(
                    request.id.clone(),
                    RuntimeFailure::new(
                        CommandErrorCode::RequestIdConflict,
                        "Request ID was reused with different command data",
                        false,
                    ),
                );
                let _ = response.send(conflict);
                return None;
            }
            None => {}
        }

        if let Err(failure) = self.validate_request(app, &request) {
            let result = self.error_response(request.id.clone(), failure);
            let _ = self
                .replay
                .insert(request.id.clone(), fingerprint, result.clone());
            let _ = response.send(result);
            return None;
        }

        if self.in_flight.len() >= WFE_IN_FLIGHT_REQUEST_CAPACITY {
            let busy = self.error_response(
                request.id.clone(),
                RuntimeFailure::new(
                    CommandErrorCode::Busy,
                    "Too many remote commands are awaiting completion",
                    true,
                ),
            );
            let _ = response.send(busy);
            return None;
        }

        self.in_flight.insert(
            request.id.clone(),
            InFlightRequest {
                fingerprint,
                responses: vec![response],
            },
        );
        Some(AdmittedCommand {
            connection_id,
            request,
            fingerprint,
        })
    }

    pub fn complete(
        &mut self,
        command: AdmittedCommand,
        result: Result<CommandOutcome, RuntimeFailure>,
    ) -> Result<(), String> {
        let response = match result {
            Ok(outcome) => {
                self.presentation
                    .diagnostics
                    .record(OperationalEvent::CommandCompleted);
                ICommandResponse {
                    id: command.request.id.clone(),
                    result: CommandResult::Ok {
                        revision: self.revision(),
                        outcome,
                    },
                }
            }
            Err(error) => self.error_response(command.request.id.clone(), error),
        };
        response.validate()?;
        let request_id = command.request.id;
        let in_flight = self
            .in_flight
            .get(&request_id)
            .ok_or_else(|| "completed WFE request was not reserved".to_string())?;
        if in_flight.fingerprint != command.fingerprint {
            return Err("completed WFE request fingerprint changed".to_string());
        }
        self.replay
            .insert(request_id.clone(), command.fingerprint, response.clone())?;
        let in_flight = self
            .in_flight
            .remove(&request_id)
            .ok_or_else(|| "completed WFE request reservation disappeared".to_string())?;
        for waiter in in_flight.responses {
            let _ = waiter.send(response.clone());
        }
        Ok(())
    }

    pub fn begin_confirmation_for_command(
        &mut self,
        command: &WebCommand,
        title: impl Into<String>,
        message: impl Into<String>,
    ) -> Result<String, String> {
        let (confirmed, _, action, session_id) = confirmation(command)
            .ok_or_else(|| "command does not support a confirmation".to_string())?;
        if confirmed {
            return Err(
                "cannot begin a confirmation from an already confirmed command".to_string(),
            );
        }
        let binding = confirmation_binding(command)?;
        let confirmation_id = format!("confirmation-{}", uuid::Uuid::new_v4().simple());
        self.presentation.confirmation = Some(ConfirmationView {
            confirmation_id: confirmation_id.clone(),
            action,
            session_id,
            title: title.into(),
            message: message.into(),
        });
        self.presentation.confirmation_binding = Some(binding);
        self.presentation.panel_from_app = false;
        self.presentation.requested_panel_data = Some(PanelDataView::Confirmation {
            confirmation: self
                .presentation
                .confirmation
                .clone()
                .expect("confirmation was assigned above"),
        });
        Ok(confirmation_id)
    }

    pub fn clear_confirmation(&mut self) {
        self.presentation.confirmation = None;
        self.presentation.confirmation_binding = None;
        if matches!(
            self.presentation.requested_panel_data,
            Some(PanelDataView::Confirmation { .. })
        ) {
            self.presentation.requested_panel_data = None;
        }
    }

    pub fn dismiss_overlay(&mut self, app: &mut App) {
        self.clear_confirmation();
        self.presentation.requested_panel_data = None;

        app.show_cleanup_prompt = false;
        app.show_prompt_save_dialog = false;
        app.show_session_name_dialog = false;
        app.show_hotkeys = false;
        app.show_theme_menu = false;
        app.show_history = false;
        app.show_prompt_editor = false;
        app.is_editing_prompt = false;
        app.show_prompt_manager = false;
        app.show_session_manager = false;
        app.show_latest_files = false;
        app.show_model_switcher = false;
        app.show_lsp_manager = false;
        if !app.python_setup_is_busy() {
            app.python_setup = None;
        }
        app.show_palette = false;
        app.should_redraw = true;
    }

    pub fn set_requested_panel_data(&mut self, data: Option<PanelDataView>) {
        self.presentation.requested_panel_data = data;
        self.presentation.panel_from_app = false;
    }

    pub fn open_loop_modes(&mut self, app: &App) {
        self.presentation.requested_panel_data = Some(loop_modes_panel(app));
        self.presentation.panel_from_app = false;
    }

    pub fn open_agent_modes(&mut self, app: &App) {
        self.presentation.requested_panel_data = Some(agent_modes_panel(app));
        self.presentation.panel_from_app = false;
    }

    pub fn clear_requested_panel(&mut self) {
        if self.presentation.confirmation.is_none() {
            self.presentation.requested_panel_data = None;
        }
    }

    pub fn pending_approval_id(&self) -> Option<&str> {
        self.presentation.approval_id.as_deref()
    }

    pub fn pending_form_id(&self) -> Option<&str> {
        self.presentation.form_id.as_deref()
    }

    pub fn lossless_history_entry(
        &self,
        app: &App,
        entry_id: &str,
    ) -> Result<String, RuntimeFailure> {
        let Some(value) = history_entry_map(app).remove(entry_id) else {
            return Err(RuntimeFailure::new(
                CommandErrorCode::NotFound,
                "History choice is no longer available",
                true,
            ));
        };
        if !super::presentation::history_entry_is_losslessly_viewable(
            app,
            &value,
            &self.presentation.sensitive_values,
        ) {
            return Err(RuntimeFailure::new(
                CommandErrorCode::BadRequest,
                "History entry cannot be returned losslessly in the remote view",
                false,
            ));
        }
        Ok(value)
    }

    fn error_response(&mut self, id: String, failure: RuntimeFailure) -> ICommandResponse {
        self.presentation
            .diagnostics
            .record(OperationalEvent::CommandRejected(failure.code));
        let (message, _) = truncate_message(&failure.message, 1024);
        ICommandResponse {
            id,
            result: CommandResult::Error {
                error: CommandError {
                    code: failure.code,
                    message,
                    current_revision: Some(self.revision()),
                    retryable: failure.retryable,
                },
            },
        }
    }

    fn validate_request(&self, app: &App, request: &ICommandRequest) -> Result<(), RuntimeFailure> {
        request
            .validate()
            .map_err(|error| RuntimeFailure::new(CommandErrorCode::BadRequest, error, false))?;
        if let WebCommand::InvokeCommand { command_id } = &request.command
            && command_id.spec().behavior == crate::commands::CommandBehavior::Confirm
        {
            return Err(RuntimeFailure::new(
                CommandErrorCode::BadRequest,
                "Destructive commands require their dedicated typed confirmation request",
                false,
            ));
        }
        let exact_stop_target = matches!(
            &request.command,
            WebCommand::Stop {
                session_id,
                cancel_id,
            } if session_id == &app.session_id
                && app.live_cancellation_id() == Some(cancel_id.as_str())
        );
        if !matches!(&request.command, WebCommand::RequestSnapshot)
            && !exact_stop_target
            && request.expected_revision != self.revision()
        {
            return Err(RuntimeFailure::new(
                CommandErrorCode::StaleRevision,
                "State revision changed; resynchronize before retrying",
                true,
            ));
        }

        if let Some(session_id) = active_session_id(&request.command)
            && session_id != app.session_id
        {
            return Err(RuntimeFailure::new(
                CommandErrorCode::SessionMismatch,
                "Command does not target the active session",
                true,
            ));
        }

        match &request.command {
            WebCommand::Stop { cancel_id, .. }
                if app.live_cancellation_id() != Some(cancel_id.as_str()) =>
            {
                return Err(RuntimeFailure::new(
                    CommandErrorCode::NotFound,
                    "Cancellation target no longer matches active work",
                    false,
                ));
            }
            WebCommand::ApproveToolOnce {
                approval_id,
                tool_call_id,
                ..
            }
            | WebCommand::ApproveToolAlways {
                approval_id,
                tool_call_id,
                ..
            }
            | WebCommand::DenyTool {
                approval_id,
                tool_call_id,
                ..
            } => {
                let pending = app.pending_tool_call.as_ref();
                if !app.show_approval_prompt
                    || pending.map(|call| call.id.as_str()) != Some(tool_call_id.as_str())
                    || self.pending_approval_id() != Some(approval_id.as_str())
                {
                    return Err(RuntimeFailure::new(
                        CommandErrorCode::ToolCallMismatch,
                        "Tool approval no longer matches the pending call",
                        false,
                    ));
                }
                let (requested_decision, hidden_content_acknowledgment) = match &request.command {
                    WebCommand::ApproveToolOnce {
                        acknowledge_hidden_content,
                        ..
                    } => (
                        ApprovalDecision::ApproveOnce,
                        Some(*acknowledge_hidden_content),
                    ),
                    WebCommand::ApproveToolAlways {
                        acknowledge_hidden_content,
                        ..
                    } => (
                        ApprovalDecision::ApproveAlways,
                        Some(*acknowledge_hidden_content),
                    ),
                    WebCommand::DenyTool { .. } => (ApprovalDecision::Deny, None),
                    _ => unreachable!("the enclosing match accepts only tool decisions"),
                };
                let projected = project_with_state(app, &self.presentation).pending_approval;
                let Some(approval) = projected.as_ref() else {
                    return Err(RuntimeFailure::new(
                        CommandErrorCode::BadRequest,
                        "No browser-presentable tool approval is active",
                        false,
                    ));
                };
                if !approval.allowed_decisions.contains(&requested_decision) {
                    return Err(RuntimeFailure::new(
                        CommandErrorCode::BadRequest,
                        "This browser tool decision is not allowed for the pending call",
                        false,
                    ));
                }
                if let Some(acknowledged) = hidden_content_acknowledgment {
                    let hidden = approval.preview_redacted || approval.preview_truncated;
                    if acknowledged != hidden {
                        return Err(RuntimeFailure::new(
                            CommandErrorCode::BadRequest,
                            if hidden {
                                "Affirmative approval requires explicit acknowledgment of hidden preview content"
                            } else {
                                "Hidden-content acknowledgment is invalid for a complete approval preview"
                            },
                            false,
                        ));
                    }
                }
            }
            WebCommand::AnswerUser {
                tool_call_id,
                form_id,
                answers,
                ..
            } => {
                let pending = app.pending_tool_call.as_ref();
                if !app.is_asking_user
                    || pending.map(|call| call.id.as_str()) != Some(tool_call_id.as_str())
                    || self.pending_form_id() != Some(form_id.as_str())
                {
                    return Err(RuntimeFailure::new(
                        CommandErrorCode::ToolCallMismatch,
                        "User question no longer matches the pending call",
                        false,
                    ));
                }
                if project_with_state(app, &self.presentation)
                    .pending_question
                    .as_ref()
                    .is_none_or(|question| question.content_truncated)
                {
                    return Err(RuntimeFailure::new(
                        CommandErrorCode::BadRequest,
                        "Remote answering is disabled because the exact question cannot be shown",
                        false,
                    ));
                }
                let expected_question_id = opaque_choice_id("question", &[form_id, "question"]);
                if answers.len() != 1
                    || answers[0].question_id != expected_question_id
                    || !answers[0].selected_option_ids.is_empty()
                    || answers[0]
                        .other_text
                        .as_deref()
                        .is_none_or(|answer| answer.trim().is_empty())
                {
                    return Err(RuntimeFailure::new(
                        CommandErrorCode::BadRequest,
                        "Answer must contain the exact pending free-text question",
                        false,
                    ));
                }
            }
            WebCommand::SaveSystemPrompt { .. }
                if !app.show_prompt_editor
                    || system_prompt_editor_projection_is_lossy(
                        app,
                        &self.presentation.sensitive_values,
                    ) =>
            {
                return Err(RuntimeFailure::new(
                    CommandErrorCode::BadRequest,
                    "Remote saving is disabled because no exact system prompt editor projection is active",
                    false,
                ));
            }
            command if confirmation(command).is_some() => {
                let (confirmed, confirmation_id, action, session_id) =
                    confirmation(command).expect("guard checked confirmation fields");
                if confirmed {
                    let binding = confirmation_binding(command).map_err(|_| {
                        RuntimeFailure::new(
                            CommandErrorCode::BadRequest,
                            "Confirmation target could not be validated",
                            false,
                        )
                    })?;
                    let pending = self.presentation.confirmation.as_ref();
                    if pending.map(|view| view.confirmation_id.as_str())
                        != confirmation_id.map(String::as_str)
                        || pending.map(|view| view.action) != Some(action)
                        || pending.and_then(|view| view.session_id.as_deref())
                            != session_id.as_deref()
                        || self.presentation.confirmation_binding.as_deref()
                            != Some(binding.as_str())
                    {
                        return Err(RuntimeFailure::new(
                            CommandErrorCode::ConfirmationRequired,
                            "Confirmation is missing, stale, or targets a different action",
                            false,
                        ));
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}

fn active_session_id(command: &WebCommand) -> Option<&str> {
    match command {
        WebCommand::SendPrompt { session_id, .. }
        | WebCommand::Stop { session_id, .. }
        | WebCommand::ApproveToolOnce { session_id, .. }
        | WebCommand::ApproveToolAlways { session_id, .. }
        | WebCommand::DenyTool { session_id, .. }
        | WebCommand::RenameSession { session_id, .. }
        | WebCommand::AnswerUser { session_id, .. }
        | WebCommand::SelectHistoryEntry { session_id, .. }
        | WebCommand::SelectLatestFile { session_id, .. }
        | WebCommand::SelectSystemPrompt { session_id, .. }
        | WebCommand::SaveSystemPrompt { session_id, .. }
        | WebCommand::SetLoopDetection { session_id, .. }
        | WebCommand::SetAgentMode { session_id, .. }
        | WebCommand::RunLspAction { session_id, .. }
        | WebCommand::ClearContext { session_id, .. }
        | WebCommand::DeletePythonRuntime { session_id, .. } => Some(session_id),
        WebCommand::InvokeCommand { .. }
        | WebCommand::SelectTheme { .. }
        | WebCommand::SelectModel { .. }
        | WebCommand::NewSession
        | WebCommand::ResumeSession { .. }
        | WebCommand::DeleteSession { .. }
        | WebCommand::WipeSessions { .. }
        | WebCommand::DismissOverlay
        | WebCommand::RequestSnapshot
        | WebCommand::Quit { .. } => None,
    }
}

fn confirmation(
    command: &WebCommand,
) -> Option<(bool, Option<&String>, DestructiveActionView, Option<String>)> {
    match command {
        WebCommand::DeleteSession {
            session_id,
            confirmed,
            confirmation_id,
        } => Some((
            *confirmed,
            confirmation_id.as_ref(),
            DestructiveActionView::DeleteSession,
            Some(session_id.clone()),
        )),
        WebCommand::WipeSessions {
            confirmed,
            confirmation_id,
        } => Some((
            *confirmed,
            confirmation_id.as_ref(),
            DestructiveActionView::WipeSessions,
            None,
        )),
        WebCommand::SaveSystemPrompt {
            session_id,
            confirmed_overwrite,
            confirmation_id,
            ..
        } => Some((
            *confirmed_overwrite,
            confirmation_id.as_ref(),
            DestructiveActionView::OverwriteSystemPrompt,
            Some(session_id.clone()),
        )),
        WebCommand::ClearContext {
            session_id,
            confirmed,
            confirmation_id,
        } => Some((
            *confirmed,
            confirmation_id.as_ref(),
            DestructiveActionView::ClearContext,
            Some(session_id.clone()),
        )),
        WebCommand::DeletePythonRuntime {
            session_id,
            confirmed,
            confirmation_id,
        } => Some((
            *confirmed,
            confirmation_id.as_ref(),
            DestructiveActionView::DeletePythonRuntime,
            Some(session_id.clone()),
        )),
        WebCommand::Quit {
            confirmed,
            confirmation_id,
        } => Some((
            *confirmed,
            confirmation_id.as_ref(),
            DestructiveActionView::Quit,
            None,
        )),
        _ => None,
    }
}

fn confirmation_binding(command: &WebCommand) -> Result<String, String> {
    let mut hasher = Sha256::new();
    let mut add = |value: &str| {
        hasher.update((value.len() as u64).to_be_bytes());
        hasher.update(value.as_bytes());
    };
    add("lethetic-wfe-confirmation-v1");
    match command {
        WebCommand::DeleteSession { session_id, .. } => {
            add("delete-session");
            add(session_id);
        }
        WebCommand::WipeSessions { .. } => add("wipe-sessions"),
        WebCommand::SaveSystemPrompt {
            session_id,
            name,
            content,
            ..
        } => {
            add("overwrite-system-prompt");
            add(session_id);
            let normalized = crate::system_prompt::normalize_system_prompt_name(name)
                .unwrap_or_else(|_| name.trim().to_string());
            add(&normalized);
            add(content);
        }
        WebCommand::ClearContext { session_id, .. } => {
            add("clear-context");
            add(session_id);
        }
        WebCommand::DeletePythonRuntime { session_id, .. } => {
            add("delete-python-runtime");
            add(session_id);
        }
        WebCommand::Quit { .. } => add("quit"),
        _ => return Err("command does not support a confirmation".to_string()),
    }
    let digest = hasher.finalize();
    let mut binding = String::with_capacity(64);
    use std::fmt::Write as _;
    for byte in digest {
        write!(binding, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(binding)
}

fn sync_transient_views(app: &App, state: &mut PresentationState) {
    sync_approval(app, state);
    sync_question(app, state);
    if state.approval_key.is_some() || state.question_key.is_some() {
        state.requested_panel_data = None;
    } else if state.confirmation.is_none() {
        match panel_data_for_app(app) {
            Some(panel) => {
                state.requested_panel_data = Some(panel);
                state.panel_from_app = true;
            }
            // The terminal closed the dialog this panel mirrored.
            None if state.panel_from_app => {
                state.requested_panel_data = None;
                state.panel_from_app = false;
            }
            None => {}
        }
    }
}

fn sync_approval(app: &App, state: &mut PresentationState) {
    let next_key = app
        .show_approval_prompt
        .then_some(app.pending_tool_call.as_ref())
        .flatten()
        .map(|call| ApprovalKey {
            session_id: app.session_id.clone(),
            tool_call_id: call.id.clone(),
        });
    if state.approval_key != next_key {
        state.approval_key = next_key;
        state.approval_id = state
            .approval_key
            .as_ref()
            .map(|_| format!("approval-{}", uuid::Uuid::new_v4().simple()));
    }
}

fn sync_question(app: &App, state: &mut PresentationState) {
    let next_key = app
        .is_asking_user
        .then_some(app.pending_tool_call.as_ref())
        .flatten()
        .map(|call| QuestionKey {
            session_id: app.session_id.clone(),
            tool_call_id: call.id.clone(),
        });
    if state.question_key != next_key {
        state.question_key = next_key;
        state.form_id = state
            .question_key
            .as_ref()
            .map(|_| format!("form-{}", uuid::Uuid::new_v4().simple()));
    }
}

fn project_with_state(app: &App, state: &PresentationState) -> WebAppSnapshot {
    let pending_approval = state.approval_key.as_ref().and_then(|key| {
        let call = app.pending_tool_call.as_ref()?;
        let approval_id = state.approval_id.clone()?;
        let description = call.function.arguments["description"]
            .as_str()
            .unwrap_or("Review tool execution")
            .to_string();
        let preview =
            super::presentation::approval_preview(&call.function.name, &call.function.arguments);
        Some(PendingApprovalView {
            approval_id,
            session_id: key.session_id.clone(),
            tool_call_id: key.tool_call_id.clone(),
            tool_name: call.function.name.clone(),
            description,
            preview,
            preview_redacted: false,
            preview_truncated: false,
            can_view_original: call.function.name == "python",
            allowed_decisions: vec![
                ApprovalDecision::ApproveOnce,
                ApprovalDecision::ApproveAlways,
                ApprovalDecision::Deny,
            ],
        })
    });
    let pending_question = state.question_key.as_ref().and_then(|key| {
        let call = app.pending_tool_call.as_ref()?;
        let form_id = state.form_id.clone()?;
        let question = call.function.arguments["question"]
            .as_str()
            .unwrap_or("Please provide an answer")
            .to_string();
        Some(PendingQuestionView {
            form_id: form_id.clone(),
            session_id: key.session_id.clone(),
            tool_call_id: key.tool_call_id.clone(),
            questions: vec![QuestionPromptView {
                question_id: opaque_choice_id("question", &[&form_id, "question"]),
                prompt: question,
                options: Vec::new(),
                multiple: false,
                allows_other: true,
            }],
            content_truncated: false,
        })
    });
    let panel_data = state
        .confirmation
        .as_ref()
        .map(|confirmation| PanelDataView::Confirmation {
            confirmation: confirmation.clone(),
        })
        .or_else(|| state.requested_panel_data.clone());
    project_app(
        app,
        ProjectionContext {
            pending_approval,
            pending_question,
            panel_data,
            diagnostics: state.diagnostics.newest_first(),
            diagnostics_omitted_before: state.diagnostics.omitted_before(),
            additional_sensitive_values: state.sensitive_values.clone(),
        },
    )
}

fn panel_data_for_app(app: &App) -> Option<PanelDataView> {
    if app.show_hotkeys {
        return Some(PanelDataView::Hotkeys {
            shortcuts: vec![
                ShortcutView {
                    keys: "Ctrl+P".to_string(),
                    label: "Command palette".to_string(),
                },
                ShortcutView {
                    keys: "Ctrl+C".to_string(),
                    label: "Stop or exit".to_string(),
                },
            ],
        });
    }
    if app.show_history {
        let entries = app
            .history
            .iter()
            .rev()
            .take(100)
            .enumerate()
            .map(|(index, value)| HistoryEntryView {
                entry_id: opaque_choice_id("history", &[&index.to_string(), value]),
                label: value.clone(),
            })
            .collect();
        return Some(PanelDataView::InputHistory {
            entries,
            has_more: app.history.len() > 100,
        });
    }
    if app.show_latest_files {
        let cached = app.context_manager.all_cached_files();
        let has_more = cached.len() > 100;
        let files = cached
            .into_iter()
            .take(100)
            .map(|(path, _, _)| FileChoiceView {
                file_id: opaque_choice_id("file", &[&path]),
                label: std::path::Path::new(&path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("file")
                    .to_string(),
            })
            .collect();
        return Some(PanelDataView::LatestFiles { files, has_more });
    }
    if app.show_prompt_manager || app.show_prompt_editor {
        let prompts = app
            .prompt_files
            .iter()
            .take(100)
            .map(|name| SystemPromptChoiceView {
                prompt_id: opaque_choice_id("prompt", &[name]),
                label: std::path::Path::new(name)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("prompt")
                    .to_string(),
                selected: app.prompt_save_name == *name,
            })
            .collect();
        return Some(PanelDataView::SystemPrompts {
            prompts,
            editor_content: app.show_prompt_editor.then(|| app.system_prompt.clone()),
            content_truncated: false,
        });
    }
    if app.show_lsp_manager {
        let servers = crate::lsp::registry::SERVERS
            .iter()
            .map(|server| {
                let installed = crate::lsp::registry::check_installed(server);
                LspServerChoiceView {
                    server_id: opaque_choice_id("lsp", &[server.language, server.binary]),
                    label: server.display_name.to_string(),
                    state: if app.lsp_install_in_progress {
                        LspServerStateView::Installing
                    } else if installed {
                        LspServerStateView::Installed
                    } else {
                        LspServerStateView::Available
                    },
                    allowed_actions: if app.lsp_install_in_progress {
                        vec![LspAction::CancelInstall]
                    } else if installed {
                        Vec::new()
                    } else if server.install_cmd.is_some() {
                        vec![LspAction::Install]
                    } else {
                        Vec::new()
                    },
                }
            })
            .collect();
        return Some(PanelDataView::LspServers { servers });
    }
    if app.python_setup.is_some() {
        return Some(agent_modes_panel(app));
    }
    if app.show_session_name_dialog {
        return Some(PanelDataView::NameSession {
            current_name: app.display_name.clone(),
        });
    }
    None
}

fn loop_modes_panel(app: &App) -> PanelDataView {
    use crate::loop_detector::LoopDetectionMode;
    let current = app.loop_detector.config.mode;
    PanelDataView::LoopModes {
        modes: [
            ("off", "Off", LoopDetectionMode::Off),
            ("block-limit", "Block limit", LoopDetectionMode::BlockLimit),
            ("n-gram", "N-gram", LoopDetectionMode::NGram),
            (
                "phrase-frequency",
                "Phrase frequency",
                LoopDetectionMode::PhraseFrequency,
            ),
            ("combined", "Combined", LoopDetectionMode::Combined),
            (
                "combined-block-limit",
                "Combined + block limit",
                LoopDetectionMode::CombinedWithBlockLimit,
            ),
        ]
        .into_iter()
        .map(|(key, label, mode)| ModeChoiceView {
            mode_id: opaque_choice_id("loop-mode", &[key]),
            label: label.to_string(),
            selected: mode == current,
            enabled: true,
            disabled_reason: None,
        })
        .collect(),
    }
}

fn agent_modes_panel(app: &App) -> PanelDataView {
    use crate::config::ToolProfile;
    let current = app.config.tool_profile;
    PanelDataView::AgentModes {
        modes: [
            ("general", "General", ToolProfile::General),
            ("python-only", "Python only", ToolProfile::PythonOnly),
        ]
        .into_iter()
        .map(|(key, label, mode)| ModeChoiceView {
            mode_id: opaque_choice_id("agent-mode", &[key]),
            label: label.to_string(),
            selected: mode == current,
            enabled: app.is_fully_idle(),
            disabled_reason: (!app.is_fully_idle())
                .then(|| "Wait for active work to finish".to_string()),
        })
        .collect(),
    }
}

pub fn loop_mode_map() -> HashMap<String, crate::loop_detector::LoopDetectionMode> {
    use crate::loop_detector::LoopDetectionMode;
    [
        ("off", LoopDetectionMode::Off),
        ("block-limit", LoopDetectionMode::BlockLimit),
        ("n-gram", LoopDetectionMode::NGram),
        ("phrase-frequency", LoopDetectionMode::PhraseFrequency),
        ("combined", LoopDetectionMode::Combined),
        (
            "combined-block-limit",
            LoopDetectionMode::CombinedWithBlockLimit,
        ),
    ]
    .into_iter()
    .map(|(key, mode)| (opaque_choice_id("loop-mode", &[key]), mode))
    .collect()
}

pub fn agent_mode_map() -> HashMap<String, crate::config::ToolProfile> {
    use crate::config::ToolProfile;
    [
        ("general", ToolProfile::General),
        ("python-only", ToolProfile::PythonOnly),
    ]
    .into_iter()
    .map(|(key, mode)| (opaque_choice_id("agent-mode", &[key]), mode))
    .collect()
}

pub fn history_entry_map(app: &App) -> HashMap<String, String> {
    app.history
        .iter()
        .rev()
        .take(100)
        .enumerate()
        .map(|(index, value)| {
            (
                opaque_choice_id("history", &[&index.to_string(), value]),
                value.clone(),
            )
        })
        .collect()
}

pub fn file_choice_map(app: &App) -> HashMap<String, String> {
    app.context_manager
        .all_cached_files()
        .into_iter()
        .take(100)
        .map(|(path, _, _)| (opaque_choice_id("file", &[&path]), path))
        .collect()
}

pub fn prompt_choice_map(app: &App) -> HashMap<String, String> {
    app.prompt_files
        .iter()
        .take(100)
        .map(|name| (opaque_choice_id("prompt", &[name]), name.clone()))
        .collect()
}

pub fn lsp_choice_map() -> HashMap<String, &'static crate::lsp::registry::LspServerDef> {
    crate::lsp::registry::SERVERS
        .iter()
        .map(|server| {
            (
                opaque_choice_id("lsp", &[server.language, server.binary]),
                server,
            )
        })
        .collect()
}

pub fn model_choice_map(app: &App) -> HashMap<String, (String, String)> {
    app.available_models
        .iter()
        .take(MAX_WEB_MODELS)
        .map(|choice| {
            (
                super::presentation::model_choice_id(&choice.connection_id, &choice.model_id),
                (choice.connection_id.clone(), choice.model_id.clone()),
            )
        })
        .collect()
}

pub fn theme_choice_index(app: &App, requested_id: &str) -> Option<usize> {
    app.themes
        .iter()
        .position(|theme| theme_id(&theme.name) == requested_id)
}

fn truncate_message(value: &str, max_bytes: usize) -> (String, bool) {
    if value.len() <= max_bytes {
        return (value.to_string(), false);
    }
    let mut end = max_bytes.saturating_sub(3);
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    (format!("{}...", &value[..end]), true)
}

#[cfg(test)]
mod tests;
