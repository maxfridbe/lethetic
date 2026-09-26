use std::io::{self, Write};

use crate::lifecycle::ShutdownReason;

/// Deliberately small operational vocabulary for browser-only service mode.
/// The only non-static payload is the typed, already-sanitized WFE connection
/// event (IP and bounded numeric/status fields; never ports, credentials,
/// headers, cookies, proofs, provider output, or controller content).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ServiceConsoleEvent {
    Ready,
    #[cfg(unix)]
    HangupIgnored,
    WfeConnection(lethetic::wfe::runtime::WfeConnectionEvent),
    ShutdownRequested(ShutdownReason),
    WfeFailed,
    ShutdownComplete,
}

impl ServiceConsoleEvent {
    fn message(&self) -> String {
        match self {
            Self::Ready => "Lethetic browser service ready.".to_string(),
            #[cfg(unix)]
            Self::HangupIgnored => "SIGHUP ignored; browser service remains active.".to_string(),
            Self::WfeConnection(event) => crate::formatting::format_wfe_connection_event(event),
            Self::ShutdownRequested(ShutdownReason::Interrupt) => {
                "Interrupt received; shutting down.".to_string()
            }
            #[cfg(unix)]
            Self::ShutdownRequested(ShutdownReason::Terminate) => {
                "Termination requested; shutting down.".to_string()
            }
            #[cfg(unix)]
            Self::ShutdownRequested(ShutdownReason::Hangup) => {
                "Terminal hangup received; shutting down.".to_string()
            }
            Self::ShutdownRequested(ShutdownReason::TerminalClosed) => {
                "Terminal input closed; shutting down.".to_string()
            }
            Self::ShutdownRequested(ShutdownReason::TerminalError) => {
                "Terminal input failed; shutting down.".to_string()
            }
            Self::ShutdownRequested(ShutdownReason::UserExit) => {
                "Browser requested shutdown.".to_string()
            }
            Self::ShutdownRequested(ShutdownReason::WfeFailure)
            | Self::ShutdownRequested(ShutdownReason::SignalListenerFailure)
            | Self::WfeFailed => "Service infrastructure failed; shutting down.".to_string(),
            Self::ShutdownComplete => "Lethetic browser service stopped.".to_string(),
        }
    }
}

pub(crate) struct ServiceConsole {
    output: io::Stdout,
    unavailable: bool,
}

impl ServiceConsole {
    pub(crate) fn stdout() -> Self {
        Self {
            output: io::stdout(),
            unavailable: false,
        }
    }

    pub(crate) fn emit(&mut self, event: ServiceConsoleEvent) -> io::Result<()> {
        emit_with_state(&mut self.output, &mut self.unavailable, event)
    }
}

fn emit_with_state(
    output: &mut impl Write,
    unavailable: &mut bool,
    event: ServiceConsoleEvent,
) -> io::Result<()> {
    if *unavailable {
        return Ok(());
    }
    match write_event(output, event) {
        Ok(()) => Ok(()),
        Err(error) if is_detached_console_error(&error) => {
            *unavailable = true;
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn write_event(output: &mut impl Write, event: ServiceConsoleEvent) -> io::Result<()> {
    writeln!(output, "{}", event.message())?;
    output.flush()
}

fn is_detached_console_error(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::BrokenPipe || error.raw_os_error() == Some(libc::EIO)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FailingWriter {
        kind: Option<io::ErrorKind>,
        raw: Option<i32>,
    }

    impl Write for FailingWriter {
        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            Err(match (self.kind, self.raw) {
                (_, Some(raw)) => io::Error::from_raw_os_error(raw),
                (Some(kind), None) => io::Error::from(kind),
                (None, None) => io::Error::other("unexpected output failure"),
            })
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn console_vocabulary_is_sparse_and_contains_no_dynamic_text() {
        let mut output = Vec::new();
        let connection = lethetic::wfe::runtime::WfeConnectionEvent::Connected(
            lethetic::wfe::runtime::WfeConnectedEvent {
                peer_ip: "192.0.2.10".parse().unwrap(),
                connection_ordinal: 7,
                active_clients: 1,
                authentication_mode:
                    lethetic::wfe::security::ControllerAuthenticationMode::TokenRequired,
                initial_sequence: 8,
                initial_revision: 9,
                round_trip_time: Some(std::time::Duration::from_millis(2)),
            },
        );
        let mut events = vec![ServiceConsoleEvent::Ready];
        #[cfg(unix)]
        events.push(ServiceConsoleEvent::HangupIgnored);
        events.extend([
            ServiceConsoleEvent::WfeConnection(connection),
            ServiceConsoleEvent::ShutdownRequested(ShutdownReason::Interrupt),
            ServiceConsoleEvent::WfeFailed,
            ServiceConsoleEvent::ShutdownComplete,
        ]);
        let expected_lines = events.len();
        for event in events {
            write_event(&mut output, event).unwrap();
        }
        let rendered = String::from_utf8(output).unwrap();
        for forbidden in [
            "secret-controller-token",
            ":443",
            "cookie=",
            "proof=",
            "authorization:",
            "provider response",
            "attacker supplied command",
        ] {
            assert!(!rendered.contains(forbidden));
        }
        assert!(rendered.contains("ip=192.0.2.10"));
        assert!(rendered.contains("auth=token-required"));
        assert_eq!(rendered.lines().count(), expected_lines);
    }

    #[test]
    fn detached_terminal_errors_are_swallowed() {
        for mut writer in [
            FailingWriter {
                kind: Some(io::ErrorKind::BrokenPipe),
                raw: None,
            },
            FailingWriter {
                kind: None,
                raw: Some(libc::EIO),
            },
        ] {
            let mut unavailable = false;
            emit_with_state(&mut writer, &mut unavailable, ServiceConsoleEvent::Ready).unwrap();
            assert!(unavailable);
            // Once detached, subsequent events are swallowed without touching
            // the failed descriptor again.
            emit_with_state(
                &mut writer,
                &mut unavailable,
                ServiceConsoleEvent::ShutdownComplete,
            )
            .unwrap();
        }
        let mut unexpected = FailingWriter::default();
        let mut unavailable = false;
        let error = emit_with_state(
            &mut unexpected,
            &mut unavailable,
            ServiceConsoleEvent::Ready,
        )
        .unwrap_err();
        assert!(!is_detached_console_error(&error));
        assert!(!unavailable);
    }
}
