//! Interactive chooser for `--rc` without a target: lists this machine's
//! addresses and asks how the browser controller should be exposed.

use std::io::{self, IsTerminal, Write as _};
use std::net::IpAddr;

use crate::cli::{Cli, WfeFilesMode};
use crate::lifecycle::{RuntimeMode, ShutdownReason, SignalAction, SignalListener, signal_action};
use crate::line_reader::BootstrapLineReader;

pub(crate) const DEFAULT_RC_PORT: u16 = 11223;

/// One selectable bind identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BindChoice {
    pub(crate) host: String,
    pub(crate) note: String,
}

pub(crate) enum WizardOutcome {
    Configured,
    Shutdown(ShutdownReason),
}

/// Normalise a user-supplied `--rc` target into the exact HTTPS URL form the
/// WFE security layer expects. Accepts `host`, `host:port`, `[v6]`,
/// `[v6]:port`, and full `https://` URLs.
pub(crate) fn normalize_rc_target(value: &str) -> String {
    let value = value.trim();
    let (scheme, authority) = match value.split_once("://") {
        Some((scheme, rest)) => (Some(scheme), rest),
        None => (None, value),
    };
    let has_port = if authority.starts_with('[') {
        authority
            .rsplit_once("]:")
            .is_some_and(|(_, port)| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()))
    } else {
        authority
            .rsplit_once(':')
            .is_some_and(|(_, port)| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()))
    };
    let authority = if has_port {
        authority.to_string()
    } else {
        format!("{authority}:{DEFAULT_RC_PORT}")
    };
    // Any explicit scheme is passed through so the security layer can reject
    // it with its own precise message; a bare authority gets https.
    match scheme {
        Some(scheme) => format!("{scheme}://{authority}"),
        None => format!("https://{authority}"),
    }
}

fn hostname() -> Option<String> {
    #[cfg(unix)]
    {
        let mut buffer = [0u8; 256];
        let result =
            unsafe { libc::gethostname(buffer.as_mut_ptr().cast::<libc::c_char>(), buffer.len()) };
        if result != 0 {
            return None;
        }
        let end = buffer.iter().position(|b| *b == 0).unwrap_or(buffer.len());
        let name = String::from_utf8_lossy(&buffer[..end]).to_ascii_lowercase();
        let valid = !name.is_empty()
            && name != "localhost"
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'.'));
        valid.then_some(name)
    }
    #[cfg(not(unix))]
    {
        None
    }
}

#[cfg(unix)]
fn interface_addresses() -> Vec<(String, IpAddr)> {
    use std::net::{Ipv4Addr, Ipv6Addr};

    let mut out = Vec::new();
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills `list` with a linked list that is released by
    // freeifaddrs below; every pointer read is checked for null first.
    if unsafe { libc::getifaddrs(&mut list) } != 0 {
        return out;
    }
    let mut cursor = list;
    while !cursor.is_null() {
        let entry = unsafe { &*cursor };
        cursor = entry.ifa_next;
        if entry.ifa_addr.is_null() || entry.ifa_flags & (libc::IFF_UP as u32) == 0 {
            continue;
        }
        let name = unsafe { std::ffi::CStr::from_ptr(entry.ifa_name) }
            .to_string_lossy()
            .into_owned();
        let family = i32::from(unsafe { (*entry.ifa_addr).sa_family });
        if family == libc::AF_INET {
            let address = unsafe { &*(entry.ifa_addr as *const libc::sockaddr_in) };
            let ip = Ipv4Addr::from(u32::from_be(address.sin_addr.s_addr));
            out.push((name, IpAddr::V4(ip)));
        } else if family == libc::AF_INET6 {
            let address = unsafe { &*(entry.ifa_addr as *const libc::sockaddr_in6) };
            let ip = Ipv6Addr::from(address.sin6_addr.s6_addr);
            if !ip.is_unicast_link_local() {
                out.push((name, IpAddr::V6(ip)));
            }
        }
    }
    unsafe { libc::freeifaddrs(list) };
    out
}

#[cfg(not(unix))]
fn interface_addresses() -> Vec<(String, IpAddr)> {
    Vec::new()
}

/// Loopback first, then IPv4 interfaces, then IPv6, then the host name.
pub(crate) fn bind_choices() -> Vec<BindChoice> {
    let mut choices = vec![BindChoice {
        host: "127.0.0.1".to_string(),
        note: "this machine only".to_string(),
    }];
    let mut addresses = interface_addresses();
    addresses.retain(|(_, ip)| !ip.is_loopback() && !ip.is_unspecified());
    addresses.sort_by_key(|(name, ip)| (ip.is_ipv6(), name.clone()));
    for (name, ip) in addresses {
        let host = match ip {
            IpAddr::V4(v4) => v4.to_string(),
            IpAddr::V6(v6) => format!("[{v6}]"),
        };
        if choices.iter().any(|choice| choice.host == host) {
            continue;
        }
        let reach = match ip {
            IpAddr::V4(v4) if v4.is_private() => "LAN / VPN",
            IpAddr::V4(_) => "public",
            IpAddr::V6(_) => "IPv6",
        };
        choices.push(BindChoice {
            host,
            note: format!("{name}, {reach}"),
        });
    }
    if let Some(name) = hostname() {
        choices.push(BindChoice {
            host: name,
            note: "host name, resolved by DNS at startup".to_string(),
        });
    }
    choices
}

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
    fn targets_normalize_to_exact_https_urls() {
        assert_eq!(normalize_rc_target("brainiac"), "https://brainiac:11223");
        assert_eq!(
            normalize_rc_target("brainiac:9443"),
            "https://brainiac:9443"
        );
        assert_eq!(normalize_rc_target("10.0.0.5"), "https://10.0.0.5:11223");
        assert_eq!(normalize_rc_target("[::1]"), "https://[::1]:11223");
        assert_eq!(normalize_rc_target("[::1]:8443"), "https://[::1]:8443");
        assert_eq!(
            normalize_rc_target("https://127.0.0.1:11223"),
            "https://127.0.0.1:11223"
        );
        assert_eq!(
            normalize_rc_target("https://brainiac"),
            "https://brainiac:11223"
        );
        assert_eq!(normalize_rc_target("http://x:1"), "http://x:1");
    }

    #[test]
    fn choices_default_to_loopback_first() {
        let choices = bind_choices();
        assert_eq!(choices[0].host, "127.0.0.1");
        assert!(choices.iter().all(|c| !c.host.is_empty()));
        assert_eq!(parse_choice("", 3, 0), Some(0));
        assert_eq!(parse_choice("3", 3, 0), Some(2));
        assert_eq!(parse_choice("4", 3, 0), None);
        assert_eq!(parse_choice("x", 3, 0), None);
        assert_eq!(parse_yes_no("", false), Some(false));
        assert_eq!(parse_yes_no("Y", false), Some(true));
        assert_eq!(parse_yes_no("maybe", false), None);
    }
}
