//! Interactive chooser for `--rc` without a target: lists this machine's
//! addresses and asks how the browser controller should be exposed.

use std::io::{self, IsTerminal, Write as _};

use crate::cli::{Cli, WfeFilesMode};
use crate::lifecycle::{RuntimeMode, ShutdownReason, SignalAction, SignalListener, signal_action};
use crate::line_reader::BootstrapLineReader;
pub(crate) use lethetic::remote_control::{DEFAULT_RC_PORT, bind_choices, normalize_rc_target};

async fn ask(
    prompt: &str,
    signals: &mut SignalListener,
) -> io::Result<Result<String, ShutdownReason>> {
    {
        let mut output = io::stdout().lock();
        write!(output, "{prompt}")?;
        output.flush()?;
    }
    let mut input = BootstrapLineReader::spawn()?;
    loop {
        tokio::select! {
            biased;
            signal = signals.recv() => {
                match signal_action(RuntimeMode::Interactive, signal?) {
                    SignalAction::BeginShutdown(reason) => return Ok(Err(reason)),
                    #[allow(unreachable_patterns)]
                    _ => {}
                }
            }
            line = input.receive() => {
                return Ok(match line {
                    Ok(line) if line.ends_with('\n') => Ok(line.trim().to_string()),
                    Ok(_) | Err(_) => Err(ShutdownReason::TerminalClosed),
                });
            }
        }
    }
}

pub(crate) enum WizardOutcome {
    Configured,
    Shutdown(ShutdownReason),
}

fn parse_choice(answer: &str, count: usize, default: usize) -> Option<usize> {
    if answer.is_empty() {
        return Some(default);
    }
    answer
        .parse::<usize>()
        .ok()
        .filter(|choice| (1..=count).contains(choice))
        .map(|choice| choice - 1)
}

fn parse_yes_no(answer: &str, default: bool) -> Option<bool> {
    match answer.to_ascii_lowercase().as_str() {
        "" => Some(default),
        "y" | "yes" => Some(true),
        "n" | "no" => Some(false),
        _ => None,
    }
}

macro_rules! answer {
    ($expr:expr) => {
        match $expr.await? {
            Ok(answer) => answer,
            Err(reason) => return Ok(WizardOutcome::Shutdown(reason)),
        }
    };
}

/// Fill the remote-control fields of `cli` from terminal answers. Questions
/// already answered by explicit flags are skipped.
pub(crate) async fn run(cli: &mut Cli, signals: &mut SignalListener) -> io::Result<WizardOutcome> {
    if !(io::stdin().is_terminal() && io::stdout().is_terminal()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "--rc without a target needs an attached terminal; pass --rc <host[:port]> instead",
        ));
    }
    let choices = bind_choices();
    let width = choices.iter().map(|c| c.host.len()).max().unwrap_or(0);
    {
        let mut output = io::stdout().lock();
        writeln!(
            output,
            "Remote control: which address should the browser connect to?"
        )?;
        for (index, choice) in choices.iter().enumerate() {
            writeln!(
                output,
                "  {}) {:<width$}  ({})",
                index + 1,
                choice.host,
                choice.note,
                width = width
            )?;
        }
        writeln!(output, "  {}) custom host or IP", choices.len() + 1)?;
        output.flush()?;
    }
    let host = loop {
        let answer = answer!(ask("Choice [1]: ", signals));
        match parse_choice(&answer, choices.len() + 1, 0) {
            Some(index) if index < choices.len() => break choices[index].host.clone(),
            Some(_) => {
                let custom = answer!(ask("Host or IP: ", signals));
                if !custom.is_empty() {
                    break custom;
                }
            }
            None => {}
        }
    };
    let port = loop {
        let answer = answer!(ask(&format!("Port [{DEFAULT_RC_PORT}]: "), signals));
        if answer.is_empty() {
            break DEFAULT_RC_PORT;
        }
        if let Ok(port) = answer.parse::<u16>()
            && port != 0
        {
            break port;
        }
    };
    cli.wfe_remote_control = Some(normalize_rc_target(&format!("{host}:{port}")));

    if !cli.wfe_disable_authtoken && cli.wfe_auth_token_file.is_none() {
        {
            let mut output = io::stdout().lock();
            writeln!(output, "How should browsers authenticate?")?;
            writeln!(
                output,
                "  1) Private token URL printed at startup (recommended)"
            )?;
            writeln!(
                output,
                "  2) Open: anyone who can reach {host}:{port} gets full control"
            )?;
            output.flush()?;
        }
        loop {
            let answer = answer!(ask("Choice [1]: ", signals));
            match parse_choice(&answer, 2, 0) {
                Some(0) => break,
                Some(_) => {
                    cli.wfe_disable_authtoken = true;
                    break;
                }
                None => {}
            }
        }
    }

    if cli.wfe_files.is_none() {
        loop {
            let answer = answer!(ask(
                "Share the launch directory read-only in the browser? [y/N]: ",
                signals
            ));
            match parse_yes_no(&answer, false) {
                Some(true) => {
                    cli.wfe_files = Some(WfeFilesMode::LocalOnly);
                    break;
                }
                Some(false) => break,
                None => {}
            }
        }
    }

    if !cli.service {
        {
            let mut output = io::stdout().lock();
            writeln!(output, "Which surface?")?;
            writeln!(output, "  1) Terminal UI plus browser")?;
            writeln!(output, "  2) Browser only, no terminal UI")?;
            output.flush()?;
        }
        loop {
            let answer = answer!(ask("Choice [1]: ", signals));
            match parse_choice(&answer, 2, 0) {
                Some(0) => break,
                Some(_) => {
                    cli.service = true;
                    break;
                }
                None => {}
            }
        }
    }

    {
        let mut output = io::stdout().lock();
        writeln!(output, "Equivalent flags: {}", cli.rc_flags_summary())?;
        writeln!(output)?;
        output.flush()?;
    }
    Ok(WizardOutcome::Configured)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_parse_with_defaults() {
        assert_eq!(parse_choice("", 3, 0), Some(0));
        assert_eq!(parse_choice("3", 3, 0), Some(2));
        assert_eq!(parse_choice("4", 3, 0), None);
        assert_eq!(parse_choice("x", 3, 0), None);
        assert_eq!(parse_yes_no("", false), Some(false));
        assert_eq!(parse_yes_no("Y", false), Some(true));
        assert_eq!(parse_yes_no("maybe", false), None);
    }
}
