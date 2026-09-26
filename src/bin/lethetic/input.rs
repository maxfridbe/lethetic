use std::io;

use crossterm::{
    event::{
        DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyEventKind,
        KeyModifiers, MouseButton, MouseEventKind,
    },
    execute,
};
use futures_util::StreamExt;

use lethetic::app::{BlockType, handle_key};
use lethetic::icons;
use lethetic::ui::ui;

use crate::app_events::AppEventControl;
use crate::context::RuntimeContext;
use crate::lifecycle::ShutdownReason;
use crate::provider::request_lsp_install_cancellation;
use crate::terminal::InteractiveTerminal;

#[derive(Debug)]
pub(crate) enum TerminalInput {
    Event(Event),
    EndOfStream,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputControl {
    Handled,
    ContinueRunLoop,
}

pub(crate) struct InteractiveSurface {
    terminal: InteractiveTerminal,
    reader: EventStream,
    mouse_captured: bool,
}

impl InteractiveSurface {
    pub(crate) fn enter() -> io::Result<Self> {
        // Loading syntect is useful only to the terminal renderer. Service and
        // browser-only interactive phases never construct this surface.
        std::thread::spawn(lethetic::markdown::warm_highlighter);
        Ok(Self {
            terminal: InteractiveTerminal::enter()?,
            reader: EventStream::new(),
            mouse_captured: true,
        })
    }

    pub(crate) fn into_terminal(self) -> InteractiveTerminal {
        self.terminal
    }

    pub(crate) fn draw(&mut self, app: &mut lethetic::app::App) -> io::Result<()> {
        self.terminal.terminal_mut().draw(|frame| ui(frame, app))?;
        app.should_redraw = false;
        Ok(())
    }

    pub(crate) async fn next(&mut self) -> TerminalInput {
        classify_stream_item(self.reader.next().await)
    }

    pub(crate) fn terminal_loss(&self) -> Option<TerminalInput> {
        detect_terminal_loss()
    }

    fn toggle_mouse_capture(&mut self) -> bool {
        self.mouse_captured = !self.mouse_captured;
        let mut output = io::stdout();
        if self.mouse_captured {
            let _ = execute!(output, EnableMouseCapture);
        } else {
            let _ = execute!(output, DisableMouseCapture);
        }
        self.mouse_captured
    }
}

fn classify_stream_item(item: Option<io::Result<Event>>) -> TerminalInput {
    match item {
        Some(Ok(event)) => TerminalInput::Event(event),
        Some(Err(_)) => TerminalInput::Error,
        None => TerminalInput::EndOfStream,
    }
}

#[cfg(unix)]
fn detect_terminal_loss() -> Option<TerminalInput> {
    let mut descriptor = libc::pollfd {
        fd: libc::STDIN_FILENO,
        // POLLHUP/POLLERR/POLLNVAL are reported even when not requested.
        // Omitting POLLIN lets Crossterm remain the sole input consumer.
        events: 0,
        revents: 0,
    };
    let result = unsafe { libc::poll(&mut descriptor, 1, 0) };
    if result < 0 {
        let error = io::Error::last_os_error();
        return (error.kind() != io::ErrorKind::Interrupted).then_some(TerminalInput::Error);
    }
    classify_terminal_revents(descriptor.revents)
}

#[cfg(unix)]
fn classify_terminal_revents(revents: libc::c_short) -> Option<TerminalInput> {
    if revents & libc::POLLHUP != 0 {
        Some(TerminalInput::EndOfStream)
    } else if revents & (libc::POLLERR | libc::POLLNVAL) != 0 {
        Some(TerminalInput::Error)
    } else {
        None
    }
}

#[cfg(not(unix))]
fn detect_terminal_loss() -> Option<TerminalInput> {
    None
}

pub(crate) async fn handle_terminal_input(
    context: &mut RuntimeContext<'_>,
    surface: &mut InteractiveSurface,
    input: TerminalInput,
) -> InputControl {
    let event = match input {
        TerminalInput::EndOfStream => {
            context.begin_shutdown(ShutdownReason::TerminalClosed);
            return InputControl::ContinueRunLoop;
        }
        TerminalInput::Error => {
            context.begin_shutdown(ShutdownReason::TerminalError);
            return InputControl::ContinueRunLoop;
        }
        TerminalInput::Event(event) => event,
    };

    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            // The compaction popup owns all input while open, including Ctrl+C.
            if crate::compaction::handle_popup_key(context.app, key) {
                return InputControl::Handled;
            }
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                return handle_control_c(context);
            }

            if key.code == KeyCode::F(10) {
                let captured = surface.toggle_mouse_capture();
                context.app.stop_reason = if captured {
                    "Mouse capture ON — wheel scrolls output"
                } else {
                    "Mouse capture OFF — terminal text selection enabled"
                }
                .to_string();
                context.app.should_redraw = true;
                return InputControl::ContinueRunLoop;
            }

            let outcome = handle_key(context.app, key);
            if context.dispatch_app_event(outcome).await == AppEventControl::ContinueRunLoop {
                InputControl::ContinueRunLoop
            } else {
                InputControl::Handled
            }
        }
        Event::Paste(text) => {
            context.app.handle_paste(&text);
            InputControl::Handled
        }
        Event::Mouse(mouse) => {
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    context.app.scroll_output_up(1);
                    context.app.should_redraw = true;
                }
                MouseEventKind::ScrollDown => {
                    context.app.scroll_output_down(1);
                    context.app.should_redraw = true;
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    crate::compaction::try_copy_block_at_click(
                        context.app,
                        mouse.column,
                        mouse.row,
                    );
                }
                _ => {}
            }
            InputControl::Handled
        }
        _ => InputControl::Handled,
    }
}

fn handle_control_c(context: &mut RuntimeContext<'_>) -> InputControl {
    if context.app.python_setup_is_busy() {
        if !context.cancel_python_setup_operation(false) {
            context.cancellation_token.cancel();
        }
        context.app.stop_reason = "Cancelling Python setup operation…".to_string();
        context.app.should_redraw = true;
        return InputControl::ContinueRunLoop;
    }

    if request_lsp_install_cancellation(
        context.app,
        &context.cancellation_token,
        "Cancelling LSP server installation…",
    ) {
        return InputControl::ContinueRunLoop;
    }

    if context.app.show_approval_prompt || context.app.is_asking_user {
        context.begin_shutdown(ShutdownReason::Interrupt);
        return InputControl::ContinueRunLoop;
    }

    if context.app.is_processing || context.app.is_executing_tool {
        if context.cancellation_pending {
            context.app.stop_reason = "Waiting for cancellation containment…".to_string();
            context.app.should_redraw = true;
            return InputControl::ContinueRunLoop;
        }
        context.cancellation_token.cancel();
        context.cancellation_pending = true;
        context.app.stop_reason = "Cancelling and containing active work…".to_string();
        context.app.add_segment(
            format!("\n{} [STOPPING]\n", icons::WARNING),
            BlockType::Text,
        );
        context.app.should_redraw = true;
        return InputControl::ContinueRunLoop;
    }

    context.begin_shutdown(ShutdownReason::Interrupt);
    InputControl::ContinueRunLoop
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn control_c_cancels_initial_python_probe_operation() {
        let workspace = tempfile::tempdir().unwrap();
        let workspace = workspace.path().canonicalize().unwrap();
        let mut config = lethetic::config::Config::default();
        let mut app = lethetic::app::App::new(&config);
        app.python_setup = Some(lethetic::python_setup::PythonSetupDialog::new(
            &config, workspace,
        ));
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut context = RuntimeContext::new(
            &mut app,
            &mut config,
            tx,
            crate::lifecycle::RuntimeMode::Interactive,
        );
        crate::context::PythonSetupOperation::start(
            &mut context.python_setup_operation,
            |cancellation| async move {
                cancellation.cancelled().await;
                crate::context::PythonSetupCompletion::Capabilities(Err(
                    "probe cancelled".to_string()
                ))
            },
        )
        .unwrap();

        assert_eq!(
            handle_control_c(&mut context),
            InputControl::ContinueRunLoop
        );
        assert!(context.app.stop_reason.contains("Cancelling Python setup"));
        let settlement = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            context.settle_python_setup_operation(),
        )
        .await
        .expect("probe cancellation did not settle")
        .expect("probe operation disappeared");
        assert!(matches!(
            settlement.completion,
            Ok(crate::context::PythonSetupCompletion::Capabilities(Err(error)))
                if error == "probe cancelled"
        ));
    }

    #[test]
    fn terminal_eof_and_error_have_explicit_graceful_exit_classifications() {
        assert!(matches!(
            classify_stream_item(None),
            TerminalInput::EndOfStream
        ));
        assert!(matches!(
            classify_stream_item(Some(Err(io::Error::other("terminal failed")))),
            TerminalInput::Error
        ));
        assert!(matches!(
            classify_stream_item(Some(Ok(Event::FocusGained))),
            TerminalInput::Event(Event::FocusGained)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn terminal_poll_hangup_and_errors_are_normalized() {
        assert!(matches!(
            classify_terminal_revents(libc::POLLHUP),
            Some(TerminalInput::EndOfStream)
        ));
        assert!(matches!(
            classify_terminal_revents(libc::POLLERR),
            Some(TerminalInput::Error)
        ));
        assert!(matches!(
            classify_terminal_revents(libc::POLLNVAL),
            Some(TerminalInput::Error)
        ));
        assert!(classify_terminal_revents(0).is_none());
    }
}
