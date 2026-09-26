use std::collections::VecDeque;

use super::contracts::{
    ActivityKind, CommandErrorCode, DiagnosticCode, DiagnosticSeverity, DiagnosticView,
};
use super::runtime::WfeDisconnectCategory;

pub const OPERATIONAL_DIAGNOSTIC_CAPACITY: usize = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderPhase {
    Started,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolPhase {
    Requested,
    Completed,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationResult {
    Completed,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaintenanceResult {
    Completed,
    Degraded,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShutdownKind {
    UserExit,
    Interrupt,
    Terminate,
    Hangup,
    TerminalClosed,
    TerminalError,
    WfeFailure,
    SignalListenerFailure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationalEvent {
    ActorStarting,
    ActorReady,
    Activity(ActivityKind),
    Provider(ProviderPhase),
    Tool(ToolPhase),
    CommandCompleted,
    CommandRejected(CommandErrorCode),
    Save(OperationResult),
    CheckpointRequested,
    Maintenance(MaintenanceResult),
    ConnectionEstablished {
        active_clients: u32,
    },
    ConnectionEnded {
        category: WfeDisconnectCategory,
        active_clients: u32,
    },
    HangupIgnored,
    Shutdown(ShutdownKind),
}

#[derive(Clone, Default)]
pub(crate) struct OperationalDiagnostics {
    entries: VecDeque<DiagnosticView>,
    omitted_before: u32,
}

impl OperationalDiagnostics {
    pub(crate) fn record(&mut self, event: OperationalEvent) {
        if self.entries.len() == OPERATIONAL_DIAGNOSTIC_CAPACITY {
            self.entries.pop_front();
            self.omitted_before = self.omitted_before.saturating_add(1);
        }
        self.entries.push_back(project_event(event));
    }

    pub(crate) fn newest_first(&self) -> Vec<DiagnosticView> {
        self.entries.iter().rev().cloned().collect()
    }

    pub(crate) fn omitted_before(&self) -> u32 {
        self.omitted_before
    }
}

fn diagnostic(
    code: DiagnosticCode,
    severity: DiagnosticSeverity,
    message: impl Into<String>,
) -> DiagnosticView {
    DiagnosticView {
        code,
        severity,
        message: message.into(),
    }
}

fn project_event(event: OperationalEvent) -> DiagnosticView {
    match event {
        OperationalEvent::ActorStarting => diagnostic(
            DiagnosticCode::Unknown,
            DiagnosticSeverity::Info,
            "Application actor is starting.",
        ),
        OperationalEvent::ActorReady => diagnostic(
            DiagnosticCode::Unknown,
            DiagnosticSeverity::Info,
            "Application actor is ready.",
        ),
        OperationalEvent::Activity(activity) => activity_diagnostic(activity),
        OperationalEvent::Provider(phase) => provider_diagnostic(phase),
        OperationalEvent::Tool(phase) => tool_diagnostic(phase),
        OperationalEvent::CommandCompleted => diagnostic(
            DiagnosticCode::Unknown,
            DiagnosticSeverity::Info,
            "Browser command completed.",
        ),
        OperationalEvent::CommandRejected(code) => command_diagnostic(code),
        OperationalEvent::Save(OperationResult::Completed) => diagnostic(
            DiagnosticCode::Unknown,
            DiagnosticSeverity::Info,
            "Session save completed.",
        ),
        OperationalEvent::Save(OperationResult::Failed) => diagnostic(
            DiagnosticCode::SaveFailed,
            DiagnosticSeverity::Error,
            "Session save failed.",
        ),
        OperationalEvent::CheckpointRequested => diagnostic(
            DiagnosticCode::Unknown,
            DiagnosticSeverity::Info,
            "Provider checkpoint persistence was requested.",
        ),
        OperationalEvent::Maintenance(result) => maintenance_diagnostic(result),
        OperationalEvent::ConnectionEstablished { active_clients } => diagnostic(
            DiagnosticCode::Unknown,
            DiagnosticSeverity::Info,
            format!("Browser controller connected; active controllers: {active_clients}."),
        ),
        OperationalEvent::ConnectionEnded {
            category,
            active_clients,
        } => connection_diagnostic(category, active_clients),
        OperationalEvent::HangupIgnored => diagnostic(
            DiagnosticCode::Unknown,
            DiagnosticSeverity::Info,
            "Service hangup was ignored by policy.",
        ),
        OperationalEvent::Shutdown(kind) => shutdown_diagnostic(kind),
    }
}

fn activity_diagnostic(activity: ActivityKind) -> DiagnosticView {
    let message = match activity {
        ActivityKind::Idle => "Actor activity changed to idle.",
        ActivityKind::LoadingSession => "Actor activity changed to session loading.",
        ActivityKind::AwaitingApproval => "Actor is awaiting a tool approval.",
        ActivityKind::ExecutingTool => "Actor activity changed to tool execution.",
        ActivityKind::AwaitingAnswer => "Actor is awaiting a user answer.",
        ActivityKind::Processing => "Actor activity changed to provider processing.",
        ActivityKind::ManagingLsp => "Actor activity changed to LSP management.",
    };
    diagnostic(DiagnosticCode::Unknown, DiagnosticSeverity::Info, message)
}

fn provider_diagnostic(phase: ProviderPhase) -> DiagnosticView {
    match phase {
        ProviderPhase::Started => diagnostic(
            DiagnosticCode::Unknown,
            DiagnosticSeverity::Info,
            "Provider request started.",
        ),
        ProviderPhase::Completed => diagnostic(
            DiagnosticCode::Unknown,
            DiagnosticSeverity::Info,
            "Provider request completed.",
        ),
        ProviderPhase::Failed => diagnostic(
            DiagnosticCode::RemoteControlDegraded,
            DiagnosticSeverity::Error,
            "Provider request failed.",
        ),
        ProviderPhase::Cancelled => diagnostic(
            DiagnosticCode::Unknown,
            DiagnosticSeverity::Warning,
            "Provider request was cancelled.",
        ),
    }
}

fn tool_diagnostic(phase: ToolPhase) -> DiagnosticView {
    match phase {
        ToolPhase::Requested => diagnostic(
            DiagnosticCode::Unknown,
            DiagnosticSeverity::Info,
            "Tool work was requested.",
        ),
        ToolPhase::Completed => diagnostic(
            DiagnosticCode::Unknown,
            DiagnosticSeverity::Info,
            "Tool work completed.",
        ),
        ToolPhase::Failed => diagnostic(
            DiagnosticCode::ToolFailed,
            DiagnosticSeverity::Error,
            "Tool work failed.",
        ),
    }
}

fn command_diagnostic(code: CommandErrorCode) -> DiagnosticView {
    let message = match code {
        CommandErrorCode::BadRequest => "Browser command was rejected as invalid.",
        CommandErrorCode::ProtocolMismatch => "Browser command used an incompatible protocol.",
        CommandErrorCode::StaleRevision => "Browser command used a stale state revision.",
        CommandErrorCode::SessionMismatch => "Browser command did not match the active session.",
        CommandErrorCode::ToolCallMismatch => {
            "Browser command did not match the pending tool call."
        }
        CommandErrorCode::RequestIdConflict => {
            "Browser command conflicted with an earlier request."
        }
        CommandErrorCode::Busy => "Browser command was rejected while the actor was busy.",
        CommandErrorCode::ConfirmationRequired => "Browser command requires confirmation.",
        CommandErrorCode::NotFound => "Browser command target was not found.",
        CommandErrorCode::SaveFailed => "Browser command could not save its state.",
        CommandErrorCode::BackendUnavailable => "Browser command backend was unavailable.",
        CommandErrorCode::Internal => "Browser command failed internally.",
    };
    let diagnostic_code = if code == CommandErrorCode::SaveFailed {
        DiagnosticCode::SaveFailed
    } else {
        DiagnosticCode::RemoteControlDegraded
    };
    diagnostic(diagnostic_code, DiagnosticSeverity::Warning, message)
}

fn maintenance_diagnostic(result: MaintenanceResult) -> DiagnosticView {
    match result {
        MaintenanceResult::Completed => diagnostic(
            DiagnosticCode::Unknown,
            DiagnosticSeverity::Info,
            "Python runtime maintenance completed.",
        ),
        MaintenanceResult::Degraded => diagnostic(
            DiagnosticCode::RemoteControlDegraded,
            DiagnosticSeverity::Warning,
            "Python runtime maintenance completed with contained errors.",
        ),
        MaintenanceResult::Failed => diagnostic(
            DiagnosticCode::RemoteControlDegraded,
            DiagnosticSeverity::Error,
            "Python runtime maintenance failed.",
        ),
    }
}

fn connection_diagnostic(category: WfeDisconnectCategory, active_clients: u32) -> DiagnosticView {
    let message = match category {
        WfeDisconnectCategory::PeerClosed => "Browser controller disconnected normally.",
        WfeDisconnectCategory::TransportError => {
            "Browser controller disconnected after a transport error."
        }
        WfeDisconnectCategory::ProtocolViolation => {
            "Browser controller disconnected after a protocol violation."
        }
        WfeDisconnectCategory::ServerShutdown => {
            "Browser controller disconnected during server shutdown."
        }
        WfeDisconnectCategory::BackendUnavailable => {
            "Browser controller disconnected because the backend was unavailable."
        }
        WfeDisconnectCategory::StateChannelClosed => {
            "Browser controller disconnected because mirror state closed."
        }
        WfeDisconnectCategory::SendFailed => {
            "Browser controller disconnected after a send failure."
        }
        WfeDisconnectCategory::SetupFailed => "Browser controller disconnected during setup.",
    };
    let severity = if category == WfeDisconnectCategory::PeerClosed
        || category == WfeDisconnectCategory::ServerShutdown
    {
        DiagnosticSeverity::Info
    } else {
        DiagnosticSeverity::Warning
    };
    diagnostic(
        DiagnosticCode::ConnectionInterrupted,
        severity,
        format!("{message} Active controllers: {active_clients}."),
    )
}

fn shutdown_diagnostic(kind: ShutdownKind) -> DiagnosticView {
    let message = match kind {
        ShutdownKind::UserExit => "Shutdown was requested by the application.",
        ShutdownKind::Interrupt => "Shutdown was requested by an interrupt.",
        ShutdownKind::Terminate => "Shutdown was requested by termination signal.",
        ShutdownKind::Hangup => "Shutdown was requested by terminal hangup.",
        ShutdownKind::TerminalClosed => "Shutdown began because terminal input closed.",
        ShutdownKind::TerminalError => "Shutdown began because terminal input failed.",
        ShutdownKind::WfeFailure => "Shutdown began because browser control failed.",
        ShutdownKind::SignalListenerFailure => "Shutdown began because signal supervision failed.",
    };
    let severity = if matches!(
        kind,
        ShutdownKind::TerminalError
            | ShutdownKind::WfeFailure
            | ShutdownKind::SignalListenerFailure
    ) {
        DiagnosticSeverity::Error
    } else {
        DiagnosticSeverity::Info
    };
    diagnostic(DiagnosticCode::Unknown, severity, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_keeps_the_newest_fixed_events_and_counts_evictions_cumulatively() {
        let mut ring = OperationalDiagnostics::default();
        for index in 0..(OPERATIONAL_DIAGNOSTIC_CAPACITY + 3) {
            ring.record(if index % 2 == 0 {
                OperationalEvent::ActorStarting
            } else {
                OperationalEvent::ActorReady
            });
        }
        let first_projection = ring.newest_first();
        assert_eq!(first_projection.len(), OPERATIONAL_DIAGNOSTIC_CAPACITY);
        assert_eq!(ring.omitted_before(), 3);
        assert_eq!(
            first_projection.first().unwrap().message,
            "Application actor is starting."
        );
        assert_eq!(
            first_projection.last().unwrap().message,
            "Application actor is ready."
        );

        ring.record(OperationalEvent::HangupIgnored);
        ring.record(OperationalEvent::CommandCompleted);
        let second_projection = ring.newest_first();
        assert_eq!(second_projection.len(), OPERATIONAL_DIAGNOSTIC_CAPACITY);
        assert_eq!(ring.omitted_before(), 5);
        assert_eq!(
            second_projection.first().unwrap().message,
            "Browser command completed."
        );
        assert_eq!(
            second_projection.get(1).unwrap().message,
            "Service hangup was ignored by policy."
        );
    }

    #[test]
    fn all_event_categories_project_to_bounded_fixed_messages() {
        let events = [
            OperationalEvent::ActorStarting,
            OperationalEvent::ActorReady,
            OperationalEvent::Activity(ActivityKind::Idle),
            OperationalEvent::Activity(ActivityKind::LoadingSession),
            OperationalEvent::Activity(ActivityKind::AwaitingApproval),
            OperationalEvent::Activity(ActivityKind::ExecutingTool),
            OperationalEvent::Activity(ActivityKind::AwaitingAnswer),
            OperationalEvent::Activity(ActivityKind::Processing),
            OperationalEvent::Activity(ActivityKind::ManagingLsp),
            OperationalEvent::Provider(ProviderPhase::Started),
            OperationalEvent::Provider(ProviderPhase::Completed),
            OperationalEvent::Provider(ProviderPhase::Failed),
            OperationalEvent::Provider(ProviderPhase::Cancelled),
            OperationalEvent::Tool(ToolPhase::Requested),
            OperationalEvent::Tool(ToolPhase::Completed),
            OperationalEvent::Tool(ToolPhase::Failed),
            OperationalEvent::CommandCompleted,
            OperationalEvent::CommandRejected(CommandErrorCode::BadRequest),
            OperationalEvent::CommandRejected(CommandErrorCode::ProtocolMismatch),
            OperationalEvent::CommandRejected(CommandErrorCode::StaleRevision),
            OperationalEvent::CommandRejected(CommandErrorCode::SessionMismatch),
            OperationalEvent::CommandRejected(CommandErrorCode::ToolCallMismatch),
            OperationalEvent::CommandRejected(CommandErrorCode::RequestIdConflict),
            OperationalEvent::CommandRejected(CommandErrorCode::Busy),
            OperationalEvent::CommandRejected(CommandErrorCode::ConfirmationRequired),
            OperationalEvent::CommandRejected(CommandErrorCode::NotFound),
            OperationalEvent::CommandRejected(CommandErrorCode::SaveFailed),
            OperationalEvent::CommandRejected(CommandErrorCode::BackendUnavailable),
            OperationalEvent::CommandRejected(CommandErrorCode::Internal),
            OperationalEvent::Save(OperationResult::Completed),
            OperationalEvent::Save(OperationResult::Failed),
            OperationalEvent::CheckpointRequested,
            OperationalEvent::Maintenance(MaintenanceResult::Completed),
            OperationalEvent::Maintenance(MaintenanceResult::Degraded),
            OperationalEvent::Maintenance(MaintenanceResult::Failed),
            OperationalEvent::ConnectionEstablished { active_clients: 1 },
            OperationalEvent::ConnectionEnded {
                category: WfeDisconnectCategory::PeerClosed,
                active_clients: 0,
            },
            OperationalEvent::ConnectionEnded {
                category: WfeDisconnectCategory::TransportError,
                active_clients: 1,
            },
            OperationalEvent::ConnectionEnded {
                category: WfeDisconnectCategory::ProtocolViolation,
                active_clients: 2,
            },
            OperationalEvent::ConnectionEnded {
                category: WfeDisconnectCategory::ServerShutdown,
                active_clients: 3,
            },
            OperationalEvent::ConnectionEnded {
                category: WfeDisconnectCategory::BackendUnavailable,
                active_clients: 4,
            },
            OperationalEvent::ConnectionEnded {
                category: WfeDisconnectCategory::StateChannelClosed,
                active_clients: 5,
            },
            OperationalEvent::ConnectionEnded {
                category: WfeDisconnectCategory::SendFailed,
                active_clients: 6,
            },
            OperationalEvent::ConnectionEnded {
                category: WfeDisconnectCategory::SetupFailed,
                active_clients: 7,
            },
            OperationalEvent::HangupIgnored,
            OperationalEvent::Shutdown(ShutdownKind::UserExit),
            OperationalEvent::Shutdown(ShutdownKind::Interrupt),
            OperationalEvent::Shutdown(ShutdownKind::Terminate),
            OperationalEvent::Shutdown(ShutdownKind::Hangup),
            OperationalEvent::Shutdown(ShutdownKind::TerminalClosed),
            OperationalEvent::Shutdown(ShutdownKind::TerminalError),
            OperationalEvent::Shutdown(ShutdownKind::WfeFailure),
            OperationalEvent::Shutdown(ShutdownKind::SignalListenerFailure),
        ];
        let projected = events.into_iter().map(project_event).collect::<Vec<_>>();
        assert!(
            projected
                .iter()
                .all(|entry| !entry.message.is_empty() && entry.message.len() <= 128)
        );
        let encoded = serde_json::to_string(&projected).unwrap();
        assert!(!encoded.contains("127.0.0.1"));
        assert!(!encoded.contains("request_id"));
        assert!(!encoded.contains("peer_ip"));
    }
}
