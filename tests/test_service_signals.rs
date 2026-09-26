#![cfg(target_os = "linux")]

use std::fs::File;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const ENTER_ALTERNATE_SCREEN: &str = "\u{1b}[?1049h";
const LEAVE_ALTERNATE_SCREEN: &str = "\u{1b}[?1049l";

struct ChildGuard {
    child: Child,
    finished: bool,
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

struct Pty {
    master: File,
    slave: File,
}

#[derive(Clone, Copy)]
enum InitialHangupDisposition {
    Default,
    Ignore,
}

#[test]
fn service_ignores_hup_survives_without_clients_and_handles_term_without_tui() {
    let temporary = isolated_runtime();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);

    let mut pty = open_pty();
    set_nonblocking(&pty.master);
    let stdin = duplicate(&pty.slave);
    let stdout = duplicate(&pty.slave);
    let mut command = isolated_command(&temporary);
    command.args([
        "--service",
        "--wfe-remote-control",
        &format!("https://127.0.0.1:{}", address.port()),
    ]);
    configure_stdio_and_signals(
        &mut command,
        stdin,
        stdout,
        InitialHangupDisposition::Default,
    );
    let child = command.spawn().unwrap();
    drop(pty.slave);
    let mut guard = ChildGuard {
        child,
        finished: false,
    };
    let mut output = Vec::new();

    wait_for_text(
        &mut guard,
        &mut pty.master,
        &mut output,
        "Press Enter to start the foreground browser-only service",
        Duration::from_secs(20),
    );
    pty.master.write_all(b"\n").unwrap();
    pty.master.flush().unwrap();
    wait_for_text(
        &mut guard,
        &mut pty.master,
        &mut output,
        "Lethetic browser service ready.",
        Duration::from_secs(20),
    );

    // No authenticated browser is connected. A short-lived raw TCP peer must
    // not turn zero active browser clients into a lifecycle event.
    TcpStream::connect_timeout(&address, Duration::from_secs(2)).unwrap();
    std::thread::sleep(Duration::from_millis(150));
    assert!(guard.child.try_wait().unwrap().is_none());

    send_signal(&guard.child, libc::SIGHUP);
    wait_for_text(
        &mut guard,
        &mut pty.master,
        &mut output,
        "SIGHUP ignored; browser service remains active.",
        Duration::from_secs(10),
    );
    assert!(guard.child.try_wait().unwrap().is_none());
    TcpStream::connect_timeout(&address, Duration::from_secs(2)).unwrap();

    send_signal(&guard.child, libc::SIGTERM);
    wait_for_text(
        &mut guard,
        &mut pty.master,
        &mut output,
        "Lethetic browser service stopped.",
        Duration::from_secs(20),
    );
    let status = wait_for_exit(&mut guard.child, Duration::from_secs(10));
    guard.finished = true;
    drain(&mut pty.master, &mut output);
    assert_successful_signal_shutdown(status, "service SIGTERM");
    assert_service_never_entered_tui(&output);
}

#[test]
fn service_handles_sigint_without_tui() {
    let temporary = isolated_runtime();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);

    let mut pty = open_pty();
    set_nonblocking(&pty.master);
    let stdin = duplicate(&pty.slave);
    let stdout = duplicate(&pty.slave);
    let mut command = isolated_command(&temporary);
    command.args([
        "--service",
        "--wfe-remote-control",
        &format!("https://127.0.0.1:{}", address.port()),
    ]);
    configure_stdio_and_signals(
        &mut command,
        stdin,
        stdout,
        InitialHangupDisposition::Default,
    );
    let child = command.spawn().unwrap();
    drop(pty.slave);
    let mut guard = ChildGuard {
        child,
        finished: false,
    };
    let mut output = Vec::new();

    wait_for_text(
        &mut guard,
        &mut pty.master,
        &mut output,
        "Press Enter to start the foreground browser-only service",
        Duration::from_secs(20),
    );
    pty.master.write_all(b"\n").unwrap();
    pty.master.flush().unwrap();
    wait_for_text(
        &mut guard,
        &mut pty.master,
        &mut output,
        "Lethetic browser service ready.",
        Duration::from_secs(20),
    );

    send_signal(&guard.child, libc::SIGINT);
    wait_for_text(
        &mut guard,
        &mut pty.master,
        &mut output,
        "Lethetic browser service stopped.",
        Duration::from_secs(20),
    );
    let status = wait_for_exit(&mut guard.child, Duration::from_secs(10));
    guard.finished = true;
    drain(&mut pty.master, &mut output);
    assert_successful_signal_shutdown(status, "service SIGINT");
    assert_service_never_entered_tui(&output);
}

#[test]
fn service_pre_ack_hup_is_ignored_even_when_inherited_as_ignored() {
    let temporary = isolated_runtime();
    let (mut guard, mut master, address) =
        spawn_wfe_bootstrap(&temporary, true, false, InitialHangupDisposition::Ignore);
    let mut output = Vec::new();
    wait_for_text(
        &mut guard,
        &mut master,
        &mut output,
        "Press Enter to start the foreground browser-only service",
        Duration::from_secs(20),
    );

    send_signal(&guard.child, libc::SIGHUP);
    wait_for_text(
        &mut guard,
        &mut master,
        &mut output,
        "SIGHUP ignored; browser service remains active.",
        Duration::from_secs(10),
    );
    assert!(guard.child.try_wait().unwrap().is_none());

    master.write_all(b"\n").unwrap();
    master.flush().unwrap();
    wait_for_text(
        &mut guard,
        &mut master,
        &mut output,
        "Lethetic browser service ready.",
        Duration::from_secs(20),
    );
    send_signal(&guard.child, libc::SIGTERM);
    wait_for_text(
        &mut guard,
        &mut master,
        &mut output,
        "Lethetic browser service stopped.",
        Duration::from_secs(20),
    );
    let status = wait_for_exit(&mut guard.child, Duration::from_secs(10));
    guard.finished = true;
    drain(&mut master, &mut output);
    assert_successful_signal_shutdown(status, "service pre-ack SIGHUP then SIGTERM");
    assert_service_never_entered_tui(&output);
    TcpListener::bind(address).unwrap();
}

#[test]
fn interactive_wfe_uses_browser_first_then_optional_tui_gate() {
    for tokenless in [false, true] {
        let temporary = isolated_runtime();
        let (mut guard, mut master, address) = spawn_wfe_bootstrap(
            &temporary,
            false,
            tokenless,
            InitialHangupDisposition::Default,
        );
        let mut output = Vec::new();
        let first_prompt = if tokenless {
            "Press Enter to acknowledge this warning and activate browser control"
        } else {
            "Press Enter to acknowledge this controller information and activate browser control"
        };
        wait_for_text(
            &mut guard,
            &mut master,
            &mut output,
            first_prompt,
            Duration::from_secs(20),
        );
        assert_service_never_entered_tui(&output);

        master.write_all(b"\n").unwrap();
        master.flush().unwrap();
        wait_for_text(
            &mut guard,
            &mut master,
            &mut output,
            "Browser control is active. Continue in the browser, or press Enter at any time to enter the terminal UI",
            Duration::from_secs(20),
        );
        assert_service_never_entered_tui(&output);
        assert!(guard.child.try_wait().unwrap().is_none());
        TcpStream::connect_timeout(&address, Duration::from_secs(2)).unwrap();

        master.write_all(b"\n").unwrap();
        master.flush().unwrap();
        wait_for_first_interactive_frame(
            &mut guard,
            &mut master,
            &mut output,
            Duration::from_secs(20),
        );
        send_signal(&guard.child, libc::SIGTERM);
        let status = wait_for_exit(&mut guard.child, Duration::from_secs(10));
        guard.finished = true;
        drain(&mut master, &mut output);
        assert_successful_signal_shutdown(status, "interactive WFE second gate");
        assert_terminal_was_restored(&output);
        TcpListener::bind(address).unwrap();
    }
}

#[tokio::test]
async fn browser_snapshot_command_works_before_optional_tui_gate() {
    use futures_util::{SinkExt as _, StreamExt as _};
    use rustls::pki_types::CertificateDer;
    use tokio_tungstenite::Connector;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
    use tokio_tungstenite::tungstenite::http::HeaderValue;
    use tokio_tungstenite::tungstenite::http::header::{COOKIE, ORIGIN, SEC_WEBSOCKET_PROTOCOL};

    let temporary = isolated_runtime();
    let (mut guard, mut master, address) =
        spawn_wfe_bootstrap(&temporary, false, true, InitialHangupDisposition::Default);
    let mut output = Vec::new();
    wait_for_text(
        &mut guard,
        &mut master,
        &mut output,
        "Press Enter to acknowledge this warning and activate browser control",
        Duration::from_secs(20),
    );
    master.write_all(b"\n").unwrap();
    master.flush().unwrap();
    wait_for_text(
        &mut guard,
        &mut master,
        &mut output,
        "Browser control is active",
        Duration::from_secs(20),
    );
    assert_service_never_entered_tui(&output);

    let origin = format!("https://{address}");
    let client = reqwest::Client::builder()
        .no_proxy()
        .tls_danger_accept_invalid_certs(true)
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let authenticated = client
        .post(format!("{origin}/auth/session"))
        .header(reqwest::header::ORIGIN, &origin)
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(authenticated.status(), reqwest::StatusCode::NO_CONTENT);
    let websocket_protocol = authenticated
        .headers()
        .get("x-lethetic-websocket-protocol")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let cookie = authenticated
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();

    let certificate_path = temporary
        .path()
        .join("state/lethetic/wfe/ipv4-7f000001/certificate.der");
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from(
            std::fs::read(certificate_path).unwrap(),
        ))
        .unwrap();
    let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
    let client_tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = Connector::Rustls(std::sync::Arc::new(client_tls));
    let mut request = format!("wss://{address}/ws").into_client_request().unwrap();
    request
        .headers_mut()
        .insert(ORIGIN, HeaderValue::from_str(&origin).unwrap());
    request
        .headers_mut()
        .insert(COOKIE, HeaderValue::from_str(&cookie).unwrap());
    request.headers_mut().insert(
        SEC_WEBSOCKET_PROTOCOL,
        HeaderValue::from_str(&websocket_protocol).unwrap(),
    );
    let (mut socket, _) =
        tokio_tungstenite::connect_async_tls_with_config(request, None, false, Some(connector))
            .await
            .unwrap();

    let mut saw_hello = false;
    let mut revision = None;
    let mut replied_to_ping = false;
    for _ in 0..8 {
        let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        match message {
            tokio_tungstenite::tungstenite::Message::Text(text) => {
                match serde_json::from_str::<lethetic::wfe::contracts::IServerMessage>(
                    text.as_str(),
                )
                .unwrap()
                {
                    lethetic::wfe::contracts::IServerMessage::Hello { .. } => saw_hello = true,
                    lethetic::wfe::contracts::IServerMessage::StateSnapshot { snapshot } => {
                        revision = Some(snapshot.revision);
                    }
                    _ => {}
                }
            }
            tokio_tungstenite::tungstenite::Message::Ping(payload) => {
                socket
                    .send(tokio_tungstenite::tungstenite::Message::Pong(payload))
                    .await
                    .unwrap();
                replied_to_ping = true;
            }
            _ => {}
        }
        if saw_hello && revision.is_some() && replied_to_ping {
            break;
        }
    }
    assert!(saw_hello);
    assert!(replied_to_ping);
    let revision = revision.expect("browser did not receive its initial snapshot");

    let request_id = "between-gates-snapshot";
    let snapshot_request = lethetic::wfe::contracts::ICommandRequest {
        id: request_id.to_string(),
        expected_revision: revision,
        command: lethetic::wfe::contracts::WebCommand::RequestSnapshot,
    };
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&snapshot_request).unwrap().into(),
        ))
        .await
        .unwrap();
    let mut command_completed = false;
    let mut snapshot_received = false;
    for _ in 0..8 {
        let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        match message {
            tokio_tungstenite::tungstenite::Message::Text(text) => {
                match serde_json::from_str::<lethetic::wfe::contracts::IServerMessage>(
                    text.as_str(),
                )
                .unwrap()
                {
                    lethetic::wfe::contracts::IServerMessage::CommandResponse { response }
                        if response.id == request_id =>
                    {
                        command_completed = true;
                    }
                    lethetic::wfe::contracts::IServerMessage::StateSnapshot { .. } => {
                        snapshot_received = true;
                    }
                    _ => {}
                }
            }
            tokio_tungstenite::tungstenite::Message::Ping(payload) => {
                socket
                    .send(tokio_tungstenite::tungstenite::Message::Pong(payload))
                    .await
                    .unwrap();
            }
            _ => {}
        }
        if command_completed && snapshot_received {
            break;
        }
    }
    assert!(command_completed);
    assert!(snapshot_received);
    assert_service_never_entered_tui(&output);
    socket.close(None).await.unwrap();

    send_signal(&guard.child, libc::SIGTERM);
    let status = wait_for_exit(&mut guard.child, Duration::from_secs(10));
    guard.finished = true;
    drain(&mut master, &mut output);
    assert_successful_signal_shutdown(status, "between-gates browser snapshot");
    assert_service_never_entered_tui(&output);
    TcpListener::bind(address).unwrap();
}

#[test]
fn interactive_between_gates_signals_exit_without_terminal_controls() {
    for (signal, label) in [
        (libc::SIGHUP, "interactive between-gates SIGHUP"),
        (libc::SIGINT, "interactive between-gates SIGINT"),
        (libc::SIGTERM, "interactive between-gates SIGTERM"),
    ] {
        let temporary = isolated_runtime();
        let (mut guard, mut master, address) =
            spawn_wfe_bootstrap(&temporary, false, false, InitialHangupDisposition::Ignore);
        let mut output = Vec::new();
        wait_for_text(
            &mut guard,
            &mut master,
            &mut output,
            "Press Enter to acknowledge this controller information and activate browser control",
            Duration::from_secs(20),
        );
        master.write_all(b"\n").unwrap();
        master.flush().unwrap();
        wait_for_text(
            &mut guard,
            &mut master,
            &mut output,
            "Browser control is active",
            Duration::from_secs(20),
        );
        assert_service_never_entered_tui(&output);

        send_signal(&guard.child, signal);
        let status = wait_for_exit(&mut guard.child, Duration::from_secs(10));
        guard.finished = true;
        drain(&mut master, &mut output);
        assert_successful_signal_shutdown(status, label);
        assert_service_never_entered_tui(&output);
        TcpListener::bind(address).unwrap();
    }
}

#[test]
fn interactive_between_gates_terminal_loss_exits_without_terminal_controls() {
    let temporary = isolated_runtime();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let mut input = open_pty();
    let mut output_pty = open_pty();
    set_nonblocking(&output_pty.master);
    let stdin = duplicate(&input.slave);
    let stdout = duplicate(&output_pty.slave);
    let mut command = isolated_command(&temporary);
    command
        .arg("--wfe-remote-control")
        .arg(format!("https://127.0.0.1:{}", address.port()));
    configure_stdio_and_signals(
        &mut command,
        stdin,
        stdout,
        InitialHangupDisposition::Default,
    );
    let child = command.spawn().unwrap();
    drop(input.slave);
    drop(output_pty.slave);
    let mut guard = ChildGuard {
        child,
        finished: false,
    };
    let mut output = Vec::new();
    wait_for_text(
        &mut guard,
        &mut output_pty.master,
        &mut output,
        "Press Enter to acknowledge this controller information and activate browser control",
        Duration::from_secs(20),
    );
    input.master.write_all(b"\n").unwrap();
    input.master.flush().unwrap();
    wait_for_text(
        &mut guard,
        &mut output_pty.master,
        &mut output,
        "Browser control is active",
        Duration::from_secs(20),
    );
    assert_service_never_entered_tui(&output);

    drop(input.master);
    let status = wait_for_exit(&mut guard.child, Duration::from_secs(10));
    guard.finished = true;
    drain(&mut output_pty.master, &mut output);
    assert_successful_signal_shutdown(status, "interactive between-gates terminal loss");
    assert_service_never_entered_tui(&output);
    TcpListener::bind(address).unwrap();
}

#[test]
fn interactive_pre_ack_inherited_ignored_hup_exits_without_entering_tui() {
    let temporary = isolated_runtime();
    let (mut guard, mut master, address) =
        spawn_wfe_bootstrap(&temporary, false, false, InitialHangupDisposition::Ignore);
    let mut output = Vec::new();
    wait_for_text(
        &mut guard,
        &mut master,
        &mut output,
        "Press Enter to acknowledge this controller information and activate browser control",
        Duration::from_secs(20),
    );

    send_signal(&guard.child, libc::SIGHUP);
    let status = wait_for_exit(&mut guard.child, Duration::from_secs(10));
    guard.finished = true;
    drain(&mut master, &mut output);
    assert_successful_signal_shutdown(status, "interactive pre-ack inherited-ignored SIGHUP");
    assert_service_never_entered_tui(&output);
    assert!(!String::from_utf8_lossy(&output).contains("browser service ready"));
    TcpListener::bind(address).unwrap();
}

#[test]
fn service_pre_ack_sigint_and_sigterm_release_listener_without_tui() {
    for (signal, label) in [
        (libc::SIGINT, "service pre-ack SIGINT"),
        (libc::SIGTERM, "service pre-ack SIGTERM"),
    ] {
        let temporary = isolated_runtime();
        let (mut guard, mut master, address) =
            spawn_wfe_bootstrap(&temporary, true, false, InitialHangupDisposition::Default);
        let mut output = Vec::new();
        wait_for_text(
            &mut guard,
            &mut master,
            &mut output,
            "Press Enter to start the foreground browser-only service",
            Duration::from_secs(20),
        );

        send_signal(&guard.child, signal);
        wait_for_text(
            &mut guard,
            &mut master,
            &mut output,
            "Lethetic browser service stopped.",
            Duration::from_secs(20),
        );
        let status = wait_for_exit(&mut guard.child, Duration::from_secs(10));
        guard.finished = true;
        drain(&mut master, &mut output);
        assert_successful_signal_shutdown(status, label);
        assert_service_never_entered_tui(&output);
        assert!(!String::from_utf8_lossy(&output).contains("browser service ready"));
        TcpListener::bind(address).unwrap();
    }
}

#[test]
fn tokenless_service_pre_ack_sigterm_exits_before_listener_bind() {
    let temporary = isolated_runtime();
    let (mut guard, mut master, address) =
        spawn_wfe_bootstrap(&temporary, true, true, InitialHangupDisposition::Default);
    let mut output = Vec::new();
    wait_for_text(
        &mut guard,
        &mut master,
        &mut output,
        "Press Enter to acknowledge this warning and start the foreground browser service",
        Duration::from_secs(20),
    );

    send_signal(&guard.child, libc::SIGTERM);
    wait_for_text(
        &mut guard,
        &mut master,
        &mut output,
        "Lethetic browser service stopped.",
        Duration::from_secs(20),
    );
    let status = wait_for_exit(&mut guard.child, Duration::from_secs(10));
    guard.finished = true;
    drain(&mut master, &mut output);
    assert_successful_signal_shutdown(status, "tokenless service pre-ack SIGTERM");
    assert_service_never_entered_tui(&output);
    assert!(!String::from_utf8_lossy(&output).contains("browser service ready"));
    TcpListener::bind(address).unwrap();
}

#[test]
fn bootstrap_terminal_loss_stops_both_surfaces_without_entering_tui() {
    for service in [false, true] {
        let temporary = isolated_runtime();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);

        let input = open_pty();
        let mut output_pty = open_pty();
        set_nonblocking(&output_pty.master);
        let stdin = duplicate(&input.slave);
        let stdout = duplicate(&output_pty.slave);
        let mut command = isolated_command(&temporary);
        if service {
            command.arg("--service");
        }
        command
            .arg("--wfe-remote-control")
            .arg(format!("https://127.0.0.1:{}", address.port()));
        configure_stdio_and_signals(
            &mut command,
            stdin,
            stdout,
            InitialHangupDisposition::Default,
        );
        let child = command.spawn().unwrap();
        drop(input.slave);
        drop(output_pty.slave);
        let mut guard = ChildGuard {
            child,
            finished: false,
        };
        let mut output = Vec::new();
        let prompt = if service {
            "Press Enter to start the foreground browser-only service"
        } else {
            "Press Enter to acknowledge this controller information and activate browser control"
        };
        wait_for_text(
            &mut guard,
            &mut output_pty.master,
            &mut output,
            prompt,
            Duration::from_secs(20),
        );

        drop(input.master);
        if service {
            wait_for_text(
                &mut guard,
                &mut output_pty.master,
                &mut output,
                "Lethetic browser service stopped.",
                Duration::from_secs(20),
            );
        }
        let status = wait_for_exit(&mut guard.child, Duration::from_secs(10));
        guard.finished = true;
        drain(&mut output_pty.master, &mut output);
        assert_successful_signal_shutdown(status, "secure bootstrap terminal loss");
        assert_service_never_entered_tui(&output);
        TcpListener::bind(address).unwrap();
    }
}

#[test]
fn interactive_sighup_exits_gracefully_and_restores_terminal() {
    assert_interactive_signal_shutdown(
        InitialHangupDisposition::Default,
        libc::SIGHUP,
        "interactive SIGHUP",
    );
}

#[test]
fn interactive_replaces_inherited_ignored_sighup() {
    assert_interactive_signal_shutdown(
        InitialHangupDisposition::Ignore,
        libc::SIGHUP,
        "interactive inherited-ignored SIGHUP",
    );
}

#[test]
fn interactive_sigint_exits_gracefully_and_restores_terminal() {
    assert_interactive_signal_shutdown(
        InitialHangupDisposition::Default,
        libc::SIGINT,
        "interactive SIGINT",
    );
}

#[test]
fn interactive_input_terminal_loss_exits_bounded_and_restores_output_terminal() {
    let temporary = isolated_runtime();
    let input = open_pty();
    let mut output_pty = open_pty();
    set_nonblocking(&output_pty.master);
    let stdin = duplicate(&input.slave);
    let stdout = duplicate(&output_pty.slave);
    let mut command = isolated_command(&temporary);
    configure_stdio_and_signals(
        &mut command,
        stdin,
        stdout,
        InitialHangupDisposition::Default,
    );
    let child = command.spawn().unwrap();
    drop(input.slave);
    drop(output_pty.slave);
    let mut guard = ChildGuard {
        child,
        finished: false,
    };
    let mut output = Vec::new();

    wait_for_first_interactive_frame(
        &mut guard,
        &mut output_pty.master,
        &mut output,
        Duration::from_secs(20),
    );
    drop(input.master);

    let status = wait_for_exit(&mut guard.child, Duration::from_secs(10));
    guard.finished = true;
    drain(&mut output_pty.master, &mut output);
    assert_successful_signal_shutdown(status, "interactive terminal loss");
    assert_terminal_was_restored(&output);
}

fn assert_interactive_signal_shutdown(
    disposition: InitialHangupDisposition,
    signal: libc::c_int,
    label: &str,
) {
    let temporary = isolated_runtime();
    let mut pty = open_pty();
    set_nonblocking(&pty.master);
    let stdin = duplicate(&pty.slave);
    let stdout = duplicate(&pty.slave);
    let mut command = isolated_command(&temporary);
    configure_stdio_and_signals(&mut command, stdin, stdout, disposition);
    let child = command.spawn().unwrap();
    drop(pty.slave);
    let mut guard = ChildGuard {
        child,
        finished: false,
    };
    let mut output = Vec::new();

    wait_for_first_interactive_frame(
        &mut guard,
        &mut pty.master,
        &mut output,
        Duration::from_secs(20),
    );
    send_signal(&guard.child, signal);
    let status = wait_for_exit(&mut guard.child, Duration::from_secs(10));
    guard.finished = true;
    drain(&mut pty.master, &mut output);
    assert_successful_signal_shutdown(status, label);
    assert_terminal_was_restored(&output);
}

fn spawn_wfe_bootstrap(
    temporary: &tempfile::TempDir,
    service: bool,
    tokenless: bool,
    disposition: InitialHangupDisposition,
) -> (ChildGuard, File, SocketAddr) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let pty = open_pty();
    set_nonblocking(&pty.master);
    let stdin = duplicate(&pty.slave);
    let stdout = duplicate(&pty.slave);
    let mut command = isolated_command(temporary);
    if service {
        command.arg("--service");
    }
    command
        .arg("--wfe-remote-control")
        .arg(format!("https://127.0.0.1:{}", address.port()));
    if tokenless {
        command.arg("--wfe-disable-authtoken");
    }
    configure_stdio_and_signals(&mut command, stdin, stdout, disposition);
    let child = command.spawn().unwrap();
    drop(pty.slave);
    (
        ChildGuard {
            child,
            finished: false,
        },
        pty.master,
        address,
    )
}

fn isolated_runtime() -> tempfile::TempDir {
    let temporary = tempfile::tempdir().unwrap();
    std::fs::write(
        temporary.path().join("config.yml"),
        "server_url: http://127.0.0.1:9\nmodel: lifecycle-test-model\ncontext_size: 4096\n",
    )
    .unwrap();
    for directory in ["home", "config", "data", "runtime", "state"] {
        std::fs::create_dir_all(temporary.path().join(directory)).unwrap();
    }
    temporary
}

fn isolated_command(temporary: &tempfile::TempDir) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lethetic"));
    command
        .current_dir(temporary.path())
        .env("HOME", temporary.path().join("home"))
        .env("XDG_CONFIG_HOME", temporary.path().join("config"))
        .env("XDG_DATA_HOME", temporary.path().join("data"))
        .env("XDG_RUNTIME_DIR", temporary.path().join("runtime"))
        .env("XDG_STATE_HOME", temporary.path().join("state"))
        .env("TERM", "xterm-256color");
    command
}

fn configure_stdio_and_signals(
    command: &mut Command,
    stdin: File,
    stdout: File,
    hangup: InitialHangupDisposition,
) {
    command
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::piped());
    unsafe {
        command.pre_exec(move || configure_child_signal_state(hangup));
    }
}

fn configure_child_signal_state(hangup: InitialHangupDisposition) -> io::Result<()> {
    let hangup_handler = match hangup {
        InitialHangupDisposition::Default => libc::SIG_DFL,
        InitialHangupDisposition::Ignore => libc::SIG_IGN,
    };
    unsafe {
        if libc::signal(libc::SIGHUP, hangup_handler) == libc::SIG_ERR {
            return Err(io::Error::last_os_error());
        }
        if libc::signal(libc::SIGINT, libc::SIG_DFL) == libc::SIG_ERR {
            return Err(io::Error::last_os_error());
        }
        if libc::signal(libc::SIGTERM, libc::SIG_DFL) == libc::SIG_ERR {
            return Err(io::Error::last_os_error());
        }
        let mut signals = std::mem::zeroed::<libc::sigset_t>();
        if libc::sigemptyset(&mut signals) != 0
            || libc::sigaddset(&mut signals, libc::SIGHUP) != 0
            || libc::sigaddset(&mut signals, libc::SIGINT) != 0
            || libc::sigaddset(&mut signals, libc::SIGTERM) != 0
            || libc::sigprocmask(libc::SIG_UNBLOCK, &signals, std::ptr::null_mut()) != 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn open_pty() -> Pty {
    let mut master = -1;
    let mut slave = -1;
    let size = libc::winsize {
        ws_row: 40,
        ws_col: 120,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let result = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            &size,
        )
    };
    assert_eq!(result, 0, "openpty failed: {}", io::Error::last_os_error());
    assert!(master >= 0 && slave >= 0);
    let pty = unsafe {
        Pty {
            master: File::from_raw_fd(master),
            slave: File::from_raw_fd(slave),
        }
    };
    set_cloexec(&pty.master);
    set_cloexec(&pty.slave);
    pty
}

fn duplicate(file: &File) -> File {
    let fd = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
    assert!(
        fd >= 0,
        "F_DUPFD_CLOEXEC failed: {}",
        io::Error::last_os_error()
    );
    unsafe { File::from_raw_fd(fd) }
}

fn set_cloexec(file: &File) {
    let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFD) };
    assert!(flags >= 0, "F_GETFD failed: {}", io::Error::last_os_error());
    let result = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFD, flags | libc::FD_CLOEXEC) };
    assert_eq!(result, 0, "F_SETFD failed: {}", io::Error::last_os_error());
}

fn set_nonblocking(file: &File) {
    let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
    assert!(flags >= 0, "F_GETFL failed: {}", io::Error::last_os_error());
    let result = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) };
    assert_eq!(result, 0, "F_SETFL failed: {}", io::Error::last_os_error());
}

fn send_signal(child: &Child, signal: libc::c_int) {
    assert_eq!(unsafe { libc::kill(child.id() as i32, signal) }, 0);
}

fn wait_for_first_interactive_frame(
    child: &mut ChildGuard,
    master: &mut File,
    output: &mut Vec<u8>,
    timeout: Duration,
) {
    let deadline = Instant::now() + timeout;
    loop {
        drain(master, output);
        if output.len() >= 1024 && String::from_utf8_lossy(output).contains(ENTER_ALTERNATE_SCREEN)
        {
            return;
        }
        if let Some(status) = child.child.try_wait().unwrap() {
            child.finished = true;
            let stderr_bytes = child
                .child
                .stderr
                .take()
                .map(|mut stream| {
                    let mut bytes = Vec::new();
                    let _ = stream.read_to_end(&mut bytes);
                    bytes.len()
                })
                .unwrap_or(0);
            panic!(
                "child exited before its first interactive frame: status={status}; stdout_bytes={}; stderr_bytes={stderr_bytes}",
                output.len(),
            );
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for first interactive frame; stdout_bytes={}",
            output.len()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_text(
    child: &mut ChildGuard,
    master: &mut File,
    output: &mut Vec<u8>,
    expected: &str,
    timeout: Duration,
) {
    let deadline = Instant::now() + timeout;
    loop {
        drain(master, output);
        if String::from_utf8_lossy(output).contains(expected) {
            return;
        }
        if let Some(status) = child.child.try_wait().unwrap() {
            child.finished = true;
            let stderr_bytes = child
                .child
                .stderr
                .take()
                .map(|mut stream| {
                    let mut bytes = Vec::new();
                    let _ = stream.read_to_end(&mut bytes);
                    bytes.len()
                })
                .unwrap_or(0);
            panic!(
                "child exited before expected lifecycle marker: status={status}; stdout_bytes={}; stderr_bytes={stderr_bytes}",
                output.len(),
            );
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for lifecycle marker; stdout_bytes={}",
            output.len()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn drain(master: &mut File, output: &mut Vec<u8>) {
    let mut buffer = [0_u8; 4096];
    loop {
        match master.read(&mut buffer) {
            Ok(0) => return,
            Ok(count) => output.extend_from_slice(&buffer[..count]),
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock
                    || error.raw_os_error() == Some(libc::EIO) =>
            {
                return;
            }
            Err(error) => panic!("PTY read failed: {error}"),
        }
    }
}

fn wait_for_exit(child: &mut Child, timeout: Duration) -> std::process::ExitStatus {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        assert!(Instant::now() < deadline, "child did not exit in time");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn assert_successful_signal_shutdown(status: std::process::ExitStatus, label: &str) {
    assert!(
        status.success(),
        "{label} failed: code={:?}, terminating_signal={:?}",
        status.code(),
        status.signal()
    );
    assert_eq!(
        status.signal(),
        None,
        "{label} used default signal termination"
    );
}

fn assert_terminal_was_restored(output: &[u8]) {
    let rendered = String::from_utf8_lossy(output);
    assert!(
        rendered.contains(ENTER_ALTERNATE_SCREEN),
        "interactive process never entered alternate screen; output_bytes={}",
        output.len()
    );
    assert!(
        rendered.contains(LEAVE_ALTERNATE_SCREEN),
        "interactive process did not restore alternate screen; output_bytes={}",
        output.len()
    );
}

fn assert_service_never_entered_tui(output: &[u8]) {
    let rendered = String::from_utf8_lossy(output);
    for terminal_sequence in [ENTER_ALTERNATE_SCREEN, "\u{1b}[?1000h", "\u{1b}[?2004h"] {
        assert!(
            !rendered.contains(terminal_sequence),
            "service entered terminal UI mode"
        );
    }
}
