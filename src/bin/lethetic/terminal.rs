use std::{
    io,
    sync::atomic::{AtomicBool, Ordering},
};

use crossterm::{
    cursor::Show,
    event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

pub(crate) type AppTerminal = Terminal<CrosstermBackend<io::Stdout>>;

static TERMINAL_CONTROLS_ARMED: AtomicBool = AtomicBool::new(false);

struct TerminalRestoreGuard {
    armed: bool,
}

impl TerminalRestoreGuard {
    fn armed() -> Self {
        TERMINAL_CONTROLS_ARMED.store(true, Ordering::SeqCst);
        Self { armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
        TERMINAL_CONTROLS_ARMED.store(false, Ordering::SeqCst);
    }
}

impl Drop for TerminalRestoreGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        restore_terminal_best_effort();
    }
}

/// Owns the alternate-screen/raw-mode lifetime. Service mode never constructs
/// this type, and outer entry finalization is the only normal restoration path.
pub(crate) struct InteractiveTerminal {
    terminal: AppTerminal,
    restore: TerminalRestoreGuard,
}

impl InteractiveTerminal {
    pub(crate) fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let restore = TerminalRestoreGuard::armed();
        let mut stdout = io::stdout();
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableBracketedPaste,
            EnableMouseCapture
        )?;
        let terminal = Terminal::new(CrosstermBackend::new(stdout))?;
        Ok(Self { terminal, restore })
    }

    pub(crate) fn terminal_mut(&mut self) -> &mut AppTerminal {
        &mut self.terminal
    }

    pub(crate) fn restore(&mut self) -> io::Result<()> {
        let mut first_error = None;
        record_restore_result(disable_raw_mode(), &mut first_error);
        record_restore_result(
            execute!(
                self.terminal.backend_mut(),
                LeaveAlternateScreen,
                DisableBracketedPaste,
                DisableMouseCapture,
                Show
            ),
            &mut first_error,
        );
        record_restore_result(self.terminal.show_cursor(), &mut first_error);
        if let Some(error) = first_error {
            return Err(error);
        }
        self.restore.disarm();
        Ok(())
    }
}

fn record_restore_result(result: io::Result<()>, first_error: &mut Option<io::Error>) {
    if let Err(error) = result
        && !is_detached_terminal_error(&error)
        && first_error.is_none()
    {
        *first_error = Some(error);
    }
}

fn is_detached_terminal_error(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::BrokenPipe
        || matches!(
            error.raw_os_error(),
            Some(libc::EIO | libc::ENXIO | libc::ENODEV | libc::ENOTTY)
        )
}

fn restore_terminal_best_effort() {
    if !TERMINAL_CONTROLS_ARMED.swap(false, Ordering::SeqCst) {
        return;
    }
    let _ = disable_raw_mode();
    let _ = execute!(
        io::stdout(),
        LeaveAlternateScreen,
        DisableBracketedPaste,
        DisableMouseCapture,
        Show
    );
}

/// Installs the existing crash log hook for every public mode. Only the
/// interactive mode is permitted to emit terminal-control restoration or the
/// terminal crash dialog.
pub(crate) fn setup_panic_hook(interactive: bool) {
    std::panic::set_hook(Box::new(move |panic_info| {
        #[cfg(not(test))]
        if interactive {
            restore_terminal_best_effort();
        }

        let timestamp = chrono::Local::now()
            .format("%Y-%m-%d %H:%M:%S%.3f")
            .to_string();
        let payload = panic_info.payload();
        let message = if let Some(value) = payload.downcast_ref::<&str>() {
            *value
        } else if let Some(value) = payload.downcast_ref::<String>() {
            value.as_str()
        } else {
            "Unknown panic payload"
        };
        let location = panic_info
            .location()
            .map(|location| format!("{}:{}", location.file(), location.line()))
            .unwrap_or_else(|| "unknown location".to_string());
        let backtrace = std::backtrace::Backtrace::capture();

        let _ = std::fs::create_dir_all(".lethetic");
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(".lethetic/panic.log")
        {
            use std::io::Write as _;
            let _ = writeln!(
                file,
                "========================================\n\
                 Timestamp: {}\n\
                 Panic at {}\n\
                 Message: {}\n\n\
                 Backtrace:\n{}\n",
                timestamp, location, message, backtrace
            );
        }

        #[cfg(not(test))]
        if interactive {
            let message_clean = message.replace('\n', " ").replace('\r', "");
            let msg_trimmed = truncate_chars(&message_clean, 43, 40);
            let loc_trimmed = truncate_chars(&location, 42, 39);
            eprintln!(
                "\n\
                ┌────────────────────────────────────────────────────────┐\n\
                │                   APPLICATION CRASH                    │\n\
                ├────────────────────────────────────────────────────────┤\n\
                │ Lethetic encountered an unrecoverable error (panic).   │\n\
                │                                                        │\n\
                │ Message: {:<46} │\n\
                │ Location: {:<45} │\n\
                │                                                        │\n\
                │ A detailed crash log with backtrace has been saved to: │\n\
                │ .lethetic/panic.log                                    │\n\
                └────────────────────────────────────────────────────────┘\n",
                msg_trimmed, loc_trimmed
            );
        }

        #[cfg(test)]
        let _ = interactive;
    }));
}

fn truncate_chars(value: &str, maximum: usize, prefix: usize) -> String {
    if value.chars().count() > maximum {
        format!("{}...", value.chars().take(prefix).collect::<String>())
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::Path};

    #[test]
    #[serial_test::serial]
    fn panic_hook_keeps_existing_diagnostic_log_behavior() {
        setup_panic_hook(false);
        let log_path = Path::new(".lethetic/panic.log");
        let initial_len = fs::metadata(log_path).map(|value| value.len()).unwrap_or(0);

        let result = std::panic::catch_unwind(|| {
            panic!("Test panic message for hook validation");
        });
        assert!(result.is_err());

        let log_content = fs::read_to_string(log_path).expect("panic log was not created");
        assert!(log_content.len() as u64 > initial_len);
        assert!(log_content.contains("Test panic message for hook validation"));
        assert!(log_content.contains("Timestamp:"));
        let _ = std::panic::take_hook();
    }

    #[test]
    #[serial_test::serial]
    fn restoration_controls_are_armed_only_for_an_entered_terminal() {
        TERMINAL_CONTROLS_ARMED.store(false, Ordering::SeqCst);
        restore_terminal_best_effort();
        assert!(!TERMINAL_CONTROLS_ARMED.load(Ordering::SeqCst));

        let mut guard = TerminalRestoreGuard::armed();
        assert!(TERMINAL_CONTROLS_ARMED.load(Ordering::SeqCst));
        guard.disarm();
        assert!(!TERMINAL_CONTROLS_ARMED.load(Ordering::SeqCst));
    }

    #[test]
    fn crash_dialog_truncation_is_unicode_safe() {
        let value = "λ".repeat(50);
        assert_eq!(truncate_chars(&value, 43, 40).chars().count(), 43);
    }

    #[test]
    fn detached_terminal_restore_errors_are_nonfatal() {
        assert!(is_detached_terminal_error(&io::Error::from(
            io::ErrorKind::BrokenPipe
        )));
        assert!(is_detached_terminal_error(&io::Error::from_raw_os_error(
            libc::EIO
        )));
        assert!(is_detached_terminal_error(&io::Error::from_raw_os_error(
            libc::ENXIO
        )));
        assert!(!is_detached_terminal_error(&io::Error::other(
            "unexpected restore failure"
        )));
    }
}
