use super::*;

fn host_launch() -> LaunchSpec {
    LaunchSpec::host("python3", std::env::current_dir().unwrap())
}

fn output_section(
    result: &PythonCellResult,
    section: PythonOutputSection,
) -> &PythonOutputSectionMetadata {
    result
        .output_metadata
        .sections
        .iter()
        .find(|metadata| metadata.section == section)
        .expect("worker omitted output section metadata")
}

fn artifact_literal(result: &PythonCellResult) -> String {
    serde_json::to_string(&result.output_metadata.artifact_id).unwrap()
}

#[test]
fn operational_container_names_accept_only_canonical_nonsecret_identities() {
    let runtime_id = "550e8400-e29b-41d4-a716-446655440000";
    assert_eq!(
        PythonContainerIdentity::retained(runtime_id, false),
        Some(PythonContainerIdentity {
            kind: PythonContainerKind::Retained,
            name: format!("lethetic-python-{runtime_id}"),
            active: false,
        })
    );
    assert!(PythonContainerIdentity::retained(&runtime_id.to_uppercase(), true).is_none());
    assert_eq!(
        PythonContainerIdentity::transient("lethetic-python-transient-123-0"),
        Some(PythonContainerIdentity {
            kind: PythonContainerKind::Transient,
            name: "lethetic-python-transient-123-0".to_string(),
            active: true,
        })
    );
    for invalid in [
        "lethetic-python-transient-0-1",
        "lethetic-python-transient-0123-1",
        "lethetic-python-transient-123-01",
        "lethetic-python-transient-123-1-extra",
        "untrusted-123-1",
    ] {
        assert!(
            PythonContainerIdentity::transient(invalid).is_none(),
            "{invalid}"
        );
    }
}

#[tokio::test]
async fn operational_identity_tracks_success_replacement_failure_and_reset() {
    if !crate::platform::binary_on_path("python3") {
        return;
    }
    let runspace = PythonRunspace::new();
    let identity = PythonContainerIdentity::transient("lethetic-python-transient-123-7").unwrap();
    let mut identified = host_launch();
    identified.container_identity = Some(identity.clone());
    runspace
        .ensure_ready(
            identified.clone(),
            "identity-a".to_string(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(runspace.operational_identity(), Some(identity.clone()));

    runspace
        .ensure_ready(
            host_launch(),
            "identity-b".to_string(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(runspace.operational_identity(), None);

    runspace
        .ensure_ready(
            identified.clone(),
            "identity-c".to_string(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(runspace.operational_identity(), Some(identity));
    assert!(
        runspace
            .execute(
                identified,
                "identity-c".to_string(),
                "import os; os._exit(17)",
                CancellationToken::new(),
            )
            .await
            .is_err()
    );
    assert_eq!(runspace.operational_identity(), None);

    runspace.reset_checked().await.unwrap();
    assert_eq!(runspace.operational_identity(), None);
}

#[test]
fn runtime_notices_render_exact_network_postures() {
    let cwd = PathBuf::from("/tmp/project");
    let id = "a".repeat(64);
    let name = "lethetic-python-transient-123-9";
    let none = PythonRuntimeNotice {
        container_id: id.clone(),
        container_name: name.to_string(),
        action: RuntimeLaunchAction::Created,
        network: crate::config::NetworkAccess::None,
        mounted_cwd: cwd.clone(),
    };
    assert_eq!(
        none.render(),
        format!("Podman container {name} created; network: none; mounted R/W cwd: /tmp/project")
    );
    let nonlocal = PythonRuntimeNotice {
        container_id: id.clone(),
        container_name: name.to_string(),
        action: RuntimeLaunchAction::Resumed,
        network: crate::config::NetworkAccess::Nonlocal,
        mounted_cwd: cwd.clone(),
    };
    assert_eq!(
        nonlocal.render(),
        format!(
            "Podman container {name} resumed; direct network: disabled; constrained public HTTP(S) broker: available; mounted R/W cwd: /tmp/project"
        )
    );
    let full = PythonRuntimeNotice {
        container_id: id.clone(),
        container_name: name.to_string(),
        action: RuntimeLaunchAction::Created,
        network: crate::config::NetworkAccess::Full,
        mounted_cwd: cwd,
    };
    assert_eq!(
        full.render(),
        format!(
            "Podman container {name} created; network: full (host/localhost/LAN/VPN/Internet reachable); mounted R/W cwd: /tmp/project"
        )
    );
    for notice in [&none, &nonlocal, &full] {
        assert!(!notice.render().contains(&id));
        assert!(notice.render().contains(name));
    }
}

#[test]
fn host_call_frames_require_exact_ids_operations_and_fields() {
    assert!(matches!(
        parse_host_call_frame(
            json!({
                "type": "host_call",
                "id": 7,
                "sub_id": 1,
                "operation": "todo.get"
            }),
            7,
            1,
        )
        .unwrap(),
        HostCallRequest::TodoGet { sub_id: 1 }
    ));
    assert!(matches!(
        parse_host_call_frame(
            json!({
                "type": "host_call",
                "id": 7,
                "sub_id": 2,
                "operation": "todo.set",
                "todos": [],
                "expected_revision": 0
            }),
            7,
            2,
        )
        .unwrap(),
        HostCallRequest::TodoSet {
            sub_id: 2,
            expected_revision: 0,
            ..
        }
    ));
    for malformed in [
        json!({
            "type": "host_call",
            "id": 8,
            "sub_id": 1,
            "operation": "todo.get"
        }),
        json!({
            "type": "host_call",
            "id": 7,
            "sub_id": 2,
            "operation": "todo.get"
        }),
        json!({
            "type": "host_call",
            "id": 7,
            "sub_id": 1,
            "operation": "todo.delete"
        }),
        json!({
            "type": "host_call",
            "id": 7,
            "sub_id": 1,
            "operation": "todo.get",
            "path": "/tmp/forbidden"
        }),
        json!({
            "type": "host_call",
            "id": 7,
            "sub_id": 1,
            "operation": "todo.set",
            "todos": [],
            "expected_revision": true
        }),
    ] {
        assert!(parse_host_call_frame(malformed, 7, 1).is_err());
    }
}

#[test]
fn todo_set_emits_typed_update_when_the_audited_host_call_commits() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let notebook = Arc::new(notebook::PythonNotebook::non_durable(&root, "todo-event").unwrap());
    notebook
        .begin_attempt(notebook::NotebookAttemptStart {
            tool_call_id: "tool-todo-event",
            source: "import lethetic_todo",
            description: "update todos",
            cwd: root.to_str().unwrap(),
            policy_fingerprint: "policy",
        })
        .unwrap();
    notebook
        .mark_status(
            "tool-todo-event",
            notebook::NotebookAttemptStatus::Approved,
            None,
        )
        .unwrap();
    notebook
        .mark_running("tool-todo-event", "Host", "policy")
        .unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let context = PythonHostCallContext::new(&root, notebook, "tool-todo-event", Some(tx)).unwrap();

    assert!(matches!(
        context
            .handle(HostCallRequest::TodoSet {
                sub_id: 1,
                todos: json!([{
                    "content": "Refresh immediately",
                    "status": "in_progress",
                    "priority": "high"
                }]),
                expected_revision: 0,
            })
            .unwrap(),
        HostCallReply::Success(_)
    ));
    let crate::client::StreamEvent::TodoUpdated(snapshot) = rx.try_recv().unwrap() else {
        panic!("todo.set emitted the wrong host event");
    };
    assert_eq!(snapshot.revision, 1);
    assert_eq!(snapshot.todos[0].content, "Refresh immediately");
}

#[tokio::test]
async fn diagnostic_reader_abort_is_joined_before_settlement() {
    struct DropWitness(std::sync::Arc<std::sync::atomic::AtomicBool>);
    impl Drop for DropWitness {
        fn drop(&mut self) {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let task_dropped = dropped.clone();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let mut task = tokio::spawn(async move {
        let _witness = DropWitness(task_dropped);
        let _ = started_tx.send(());
        std::future::pending::<()>().await;
    });
    started_rx.await.unwrap();

    abort_and_join_diagnostic_reader(&mut task).await;

    assert!(task.is_finished());
    assert!(
        dropped.load(std::sync::atomic::Ordering::SeqCst),
        "diagnostic reader future survived tracked settlement"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn cancelled_worker_startup_reaps_child_before_ensure_ready_returns() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let command = directory.path().join("hanging-worker");
    let marker = directory.path().join("worker-pid");
    std::fs::write(
            &command,
            format!(
                "#!/bin/sh\nprintf '%s' \"$$\" > '{}'\nprintf 'waiting for hello' >&2\nexec /bin/sleep 30\n",
                marker.display()
            ),
        )
        .unwrap();
    std::fs::set_permissions(&command, std::fs::Permissions::from_mode(0o700)).unwrap();
    let spec = LaunchSpec {
        kind: LaunchKind::Direct,
        container_identity: None,
        program: command.into_os_string(),
        args: Vec::new(),
        cwd: Some(directory.path().to_path_buf()),
        clear_env: false,
        env: Vec::new(),
        cleanup: None,
        startup_notice: None,
        startup_timeout: std::time::Duration::from_secs(30),
        graceful_shutdown: None,
    };
    let runspace = std::sync::Arc::new(PythonRunspace::new());
    let task_runspace = runspace.clone();
    let cancellation = CancellationToken::new();
    let task_cancellation = cancellation.clone();
    let task = tokio::spawn(async move {
        task_runspace
            .ensure_ready(spec, "cancelled-startup".to_string(), task_cancellation)
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !marker.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("fake worker did not start");
    let pid = std::fs::read_to_string(&marker)
        .unwrap()
        .parse::<libc::pid_t>()
        .unwrap();

    cancellation.cancel();
    let error = tokio::time::timeout(std::time::Duration::from_secs(6), task)
        .await
        .expect("cancelled worker startup did not settle")
        .expect("worker startup task panicked")
        .unwrap_err();
    assert!(error.contains("cancelled"), "{error}");
    assert!(!error.contains("cleanup failed"), "{error}");
    assert!(!runspace.is_running().await);
    let alive = unsafe { libc::kill(pid, 0) } == 0;
    assert!(!alive, "worker child survived startup cancellation");
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
}

#[tokio::test]
async fn oversized_newline_free_worker_header_is_rejected_at_the_small_header_cap() {
    if !crate::platform::binary_on_path("python3") {
        return;
    }
    let spec = LaunchSpec {
        kind: LaunchKind::Direct,
        container_identity: None,
        program: OsString::from("python3"),
        args: vec![
            OsString::from("-u"),
            OsString::from("-c"),
            OsString::from("import os, time\nos.write(1, b'A' * (1024 * 1024))\ntime.sleep(30)"),
        ],
        cwd: Some(std::env::current_dir().unwrap()),
        clear_env: false,
        env: Vec::new(),
        cleanup: None,
        startup_notice: None,
        startup_timeout: std::time::Duration::from_secs(10),
        graceful_shutdown: None,
    };
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(6),
        Worker::spawn(spec, CancellationToken::new()),
    )
    .await
    .expect("oversized worker header was not rejected within the containment bound");
    let error = match result {
        Ok(worker) => {
            let _ = worker.terminate().await;
            panic!("worker with an oversized header was accepted")
        }
        Err(error) => error,
    };
    assert!(error.contains("frame header exceeded 128 bytes"), "{error}");
}

#[cfg(unix)]
#[tokio::test]
async fn worker_startup_timeout_settles_descendants_before_returning() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let command = directory.path().join("timed-out-worker");
    let ready = directory.path().join("timeout-ready");
    let late = directory.path().join("timeout-late-sentinel");
    std::fs::write(
        &command,
        format!(
            "#!/bin/sh\n( sleep 1; printf late > '{}' ) &\nprintf ready > '{}'\nwait\n",
            late.display(),
            ready.display(),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&command, std::fs::Permissions::from_mode(0o700)).unwrap();
    let spec = LaunchSpec {
        kind: LaunchKind::Direct,
        container_identity: None,
        program: command.into_os_string(),
        args: Vec::new(),
        cwd: Some(directory.path().to_path_buf()),
        clear_env: false,
        env: Vec::new(),
        cleanup: None,
        startup_notice: None,
        startup_timeout: std::time::Duration::from_millis(500),
        graceful_shutdown: None,
    };

    let error = match Worker::spawn(spec, CancellationToken::new()).await {
        Ok(worker) => {
            let _ = worker.terminate().await;
            panic!("fake worker unexpectedly completed its hello")
        }
        Err(error) => error,
    };
    assert!(error.contains("startup timed out"), "{error}");
    assert!(ready.exists(), "fake worker did not reach its ready marker");
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;
    assert!(
        !late.exists(),
        "startup-timeout descendant fired after cleanup was reported complete"
    );
}

#[tokio::test]
async fn graceful_worker_shutdown_sends_sigterm_before_kill() {
    if !crate::platform::binary_on_path("python3") {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("terminated");
    let script = r#"
import json, os, signal, sys, time
marker = sys.argv[1]
def terminate(_signal, _frame):
    with open(marker, "w", encoding="utf-8") as output:
        output.write("terminated")
    raise SystemExit(0)
signal.signal(signal.SIGTERM, terminate)
payload = json.dumps({"type":"hello", "protocol":3, "worker_abi":"lethetic-python-worker-v4", "capabilities":["lethetic-output-v2"], "python":"test", "cwd":os.getcwd()}).encode()
sys.stdout.buffer.write(f"LETHETIC_PYTHON 3 {len(payload)}\n".encode() + payload)
sys.stdout.buffer.flush()
while True:
    time.sleep(0.1)
"#;
    let spec = LaunchSpec {
        kind: LaunchKind::Direct,
        container_identity: None,
        program: OsString::from("python3"),
        args: vec![
            OsString::from("-u"),
            OsString::from("-c"),
            OsString::from(script),
            marker.clone().into_os_string(),
        ],
        cwd: Some(directory.path().to_path_buf()),
        clear_env: false,
        env: Vec::new(),
        cleanup: None,
        startup_notice: None,
        startup_timeout: std::time::Duration::from_secs(2),
        graceful_shutdown: Some(std::time::Duration::from_secs(2)),
    };
    Worker::spawn(spec, CancellationToken::new())
        .await
        .unwrap()
        .terminate()
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(marker).unwrap(), "terminated");
}

#[tokio::test]
async fn advertised_protocol_without_output_capability_fails_before_execution() {
    if !crate::platform::binary_on_path("python3") {
        return;
    }
    let script = r#"
import json, os, sys, time
payload = json.dumps({"type":"hello", "protocol":3, "worker_abi":"lethetic-python-worker-v4", "capabilities":[], "python":"test", "cwd":os.getcwd()}).encode()
sys.stdout.buffer.write(f"LETHETIC_PYTHON 3 {len(payload)}\n".encode() + payload)
sys.stdout.buffer.flush()
while True:
    time.sleep(0.1)
"#;
    let spec = LaunchSpec {
        kind: LaunchKind::Direct,
        container_identity: None,
        program: OsString::from("python3"),
        args: vec![
            OsString::from("-u"),
            OsString::from("-c"),
            OsString::from(script),
        ],
        cwd: Some(std::env::current_dir().unwrap()),
        clear_env: false,
        env: Vec::new(),
        cleanup: None,
        startup_notice: None,
        startup_timeout: std::time::Duration::from_secs(2),
        graceful_shutdown: None,
    };
    let error = match Worker::spawn(spec, CancellationToken::new()).await {
        Ok(worker) => {
            let _ = worker.terminate().await;
            panic!("worker without output recovery was accepted")
        }
        Err(error) => error,
    };
    assert!(error.contains("worker ABI mismatch"), "{error}");
    assert!(error.contains("lethetic-output-v2"), "{error}");
}

#[tokio::test]
async fn todo_host_calls_require_an_audited_execution_context() {
    if !crate::platform::binary_on_path("python3") {
        return;
    }
    let runspace = PythonRunspace::new();
    let error = runspace
        .execute(
            host_launch(),
            "host".to_string(),
            "import lethetic_todo\nlethetic_todo.get()",
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(error.contains("outside an audited model call"), "{error}");
    assert!(!runspace.is_running().await);
    let recovered = runspace
        .execute(
            host_launch(),
            "host".to_string(),
            "1 + 1",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(recovered.value_repr, "2");
}

#[tokio::test]
async fn variables_and_imports_persist() {
    if !crate::platform::binary_on_path("python3") {
        return;
    }
    let runspace = PythonRunspace::new();
    let token = CancellationToken::new();
    let first = runspace
        .execute(
            host_launch(),
            "host".to_string(),
            "import math\nx = 6 * 7",
            token.clone(),
        )
        .await
        .unwrap();
    assert!(!first.is_error);
    let second = runspace
        .execute(
            host_launch(),
            "host".to_string(),
            "(x, math.sqrt(81))",
            token,
        )
        .await
        .unwrap();
    assert_eq!(second.value_repr, "(42, 9.0)");
}

#[tokio::test]
async fn captures_output_and_recovers_after_exception() {
    if !crate::platform::binary_on_path("python3") {
        return;
    }
    let runspace = PythonRunspace::new();
    let failed = runspace
        .execute(
            host_launch(),
            "host".to_string(),
            "import sys\nprint('out')\nprint('err', file=sys.stderr)\ny = 10\n1 / 0",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(failed.is_error);
    assert_eq!(failed.stdout.trim(), "out");
    assert_eq!(failed.stderr.trim(), "err");
    assert!(failed.traceback.contains("ZeroDivisionError"));

    let recovered = runspace
        .execute(
            host_launch(),
            "host".to_string(),
            "y + 5",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(recovered.value_repr, "15");
}

#[tokio::test]
async fn persistent_logging_handler_follows_each_cells_stderr_capture() {
    if !crate::platform::binary_on_path("python3") {
        return;
    }
    let runspace = PythonRunspace::new();
    let first = runspace
        .execute(
            host_launch(),
            "host".to_string(),
            concat!(
                "import logging\n",
                "logger = logging.getLogger('lethetic-persistent-handler')\n",
                "logger.handlers.clear()\n",
                "logger.propagate = False\n",
                "logger.setLevel(logging.INFO)\n",
                "handler = logging.StreamHandler()\n",
                "handler.setFormatter(logging.Formatter('%(message)s'))\n",
                "logger.addHandler(handler)\n",
                "logger.info('first-cell-log')",
            ),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(first.stderr.trim(), "first-cell-log");

    let second = runspace
        .execute(
            host_launch(),
            "host".to_string(),
            "logger.info('second-cell-log')",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(!second.is_error, "{}", second.render());
    assert_eq!(second.stderr.trim(), "second-cell-log");
    assert!(!second.stderr.contains("closed file"));
}

#[tokio::test]
async fn native_fd_and_subprocess_output_do_not_corrupt_protocol() {
    if !crate::platform::binary_on_path("python3") {
        return;
    }
    let runspace = PythonRunspace::new();
    let result = runspace
            .execute(
                host_launch(),
                "host".to_string(),
                "import os, subprocess\nos.write(1, b'native\\n')\nsubprocess.run(['python3', '-c', 'print(\"child\")'])\n'complete'",
                CancellationToken::new(),
            )
            .await
            .unwrap();
    assert!(!result.is_error);
    assert!(result.stdout.contains("native"));
    assert!(result.stdout.contains("child"));
    assert_eq!(result.value_repr, "'complete'");
}

#[tokio::test]
async fn native_and_subprocess_stdin_see_eof_without_consuming_control_stream() {
    if !crate::platform::binary_on_path("python3") {
        return;
    }
    let runspace = PythonRunspace::new();
    let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            runspace.execute(
                host_launch(),
                "host".to_string(),
                "import os, subprocess, sys\n\
                 native_stdin = os.read(0, 1)\n\
                 subprocess.run([sys.executable, '-c', 'import sys; print(repr(sys.stdin.buffer.read()))'], check=True)\n\
                 native_stdin",
                CancellationToken::new(),
            ),
        )
        .await
        .expect("native fd 0 read blocked on the control stream")
        .unwrap();

    assert!(!result.is_error, "{}", result.render());
    assert_eq!(result.value_repr, "b''");
    assert!(result.stdout.contains("b''"), "{}", result.stdout);

    let recovered = runspace
        .execute(
            host_launch(),
            "host".to_string(),
            "6 * 7",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(recovered.value_repr, "42");
}

#[tokio::test]
async fn high_volume_native_and_subprocess_output_is_bounded_and_recovers() {
    if !crate::platform::binary_on_path("python3") {
        return;
    }
    let runspace = PythonRunspace::new();
    let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            runspace.execute(
                host_launch(),
                "host".to_string(),
                "import os, subprocess, sys\n\
                 os.write(1, b'a' * (4 * 1024 * 1024))\n\
                 subprocess.run([sys.executable, '-c', 'import os; os.write(1, b\"b\" * (4 * 1024 * 1024))'], check=True)\n\
                 'complete'",
                CancellationToken::new(),
            ),
        )
        .await
        .expect("bounded output capture blocked while draining a pipe")
        .unwrap();

    assert!(!result.is_error, "{}", result.render());
    assert!(result.output_was_truncated);
    assert!(result.stdout.contains("truncated by Lethetic"));
    assert!(result.stdout.len() <= 1024 * 1024);
    assert_eq!(result.value_repr, "'complete'");

    let recovered = runspace
        .execute(
            host_launch(),
            "host".to_string(),
            "21 * 2",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(recovered.value_repr, "42");
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn bounded_pipe_capture_retries_after_stop_races_with_stale_empty_probe() {
    if !crate::platform::binary_on_path("python3") {
        return;
    }
    let runspace = PythonRunspace::new();
    let code = r#"
import __main__, errno, os, threading
real_start = threading.Thread.start
threading.Thread.start = lambda _thread: None
try:
    capture = __main__._BoundedPipeCapture(1024)
finally:
    threading.Thread.start = real_start

writer = os.dup(capture.write_fd)
probe_entered = threading.Event()
injected = [False]
if os.name == "nt":
    capture.nonblocking = False
    real_probe = __main__._windows_pipe_bytes_available
    def stale_probe(fd):
        if fd == capture.read_fd and not injected[0]:
            injected[0] = True
            probe_entered.set()
            if not capture.stop.wait(2):
                raise RuntimeError("capture finish never signaled stop")
            os.write(writer, b"late")
            return 0
        return real_probe(fd)
    __main__._windows_pipe_bytes_available = stale_probe
else:
    real_read = os.read
    def stale_read(fd, size):
        if fd == capture.read_fd and not injected[0]:
            injected[0] = True
            probe_entered.set()
            if not capture.stop.wait(2):
                raise RuntimeError("capture finish never signaled stop")
            os.write(writer, b"late")
            raise BlockingIOError(errno.EAGAIN, "stale empty read")
        return real_read(fd, size)
    os.read = stale_read
try:
    capture.thread.start()
    if not probe_entered.wait(2):
        raise RuntimeError("capture reader never entered the controlled probe")
    text, truncated, original = capture.finish()
finally:
    capture.stop.set()
    capture.thread.join(timeout=1)
    os.close(writer)
    if os.name == "nt":
        __main__._windows_pipe_bytes_available = real_probe
    else:
        os.read = real_read
(text, original, truncated, injected[0], capture.thread.is_alive())
"#;

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        runspace.execute(
            host_launch(),
            "host".to_string(),
            code,
            CancellationToken::new(),
        ),
    )
    .await
    .expect("controlled stale-empty probe interleave did not settle")
    .unwrap();
    assert!(!result.is_error, "{}", result.render());
    assert_eq!(result.value_repr, "('late', 4, False, True, False)");
}

#[cfg(windows)]
#[tokio::test]
async fn windows_blocking_pipe_fallback_joins_readers_with_inherited_writers() {
    if !crate::platform::binary_on_path("python3") {
        return;
    }
    let runspace = PythonRunspace::new();
    let baseline = runspace
        .execute(
            host_launch(),
            "host".to_string(),
            concat!(
                "import __main__, threading\n",
                "saved_set_blocking = __main__.os.set_blocking\n",
                "def unavailable_set_blocking(*_args):\n",
                "    raise OSError('forced fallback')\n",
                "__main__.os.set_blocking = unavailable_set_blocking\n",
                "threading.active_count()",
            ),
            CancellationToken::new(),
        )
        .await
        .unwrap()
        .value_repr;
    for _ in 0..3 {
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            runspace.execute(
                host_launch(),
                "host".to_string(),
                concat!(
                    "import subprocess, sys, threading\n",
                    "subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)'])\n",
                    "threading.active_count()",
                ),
                CancellationToken::new(),
            ),
        )
        .await
        .expect("blocking-pipe fallback did not settle its readers")
        .unwrap();
        assert_eq!(result.value_repr, baseline);
    }
    runspace
        .execute(
            host_launch(),
            "host".to_string(),
            "import __main__\n__main__.os.set_blocking = saved_set_blocking",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    runspace.reset_checked().await.unwrap();
}

#[tokio::test]
async fn worker_local_output_recovery_returns_full_capture_in_bounded_chunks() {
    if !crate::platform::binary_on_path("python3") {
        return;
    }
    let runspace = PythonRunspace::new();
    let first = runspace
        .execute(
            host_launch(),
            "host".to_string(),
            "payload = 'BEGIN-' + ('x' * 200000) + '-END'\nprint(payload)",
            CancellationToken::new(),
        )
        .await
        .unwrap();

    let stdout = output_section(&first, PythonOutputSection::Stdout);
    assert_eq!(first.output_metadata.cell, 1);
    assert!(first.output_metadata.retained);
    assert_eq!(stdout.captured_bytes, 200_011);
    assert_eq!(stdout.original_bytes, 200_011);
    assert_eq!(stdout.excerpt_bytes, PYTHON_OUTPUT_READ_MAX_BYTES);
    assert!(!stdout.truncated);
    assert!(first.output_was_truncated);
    assert!(first.stdout.contains("lethetic_output.read"));
    let artifact = artifact_literal(&first);
    let recovery_code = format!(
        concat!(
            "import lethetic_output\n",
            "info = lethetic_output.info({artifact})\n",
            "parts = []\n",
            "offset = 0\n",
            "while True:\n",
            "    chunk = lethetic_output.read({artifact}, 'stdout', offset, 65536)\n",
            "    parts.append(chunk['text'])\n",
            "    offset = chunk['next_offset']\n",
            "    if chunk['eof']:\n",
            "        break\n",
            "recovered = ''.join(parts)\n",
            "(recovered == payload + '\\n', info['sections']['stdout']['captured_bytes'], offset, len(parts))",
        ),
        artifact = artifact,
    );

    let recovered = runspace
        .execute(
            host_launch(),
            "host".to_string(),
            &recovery_code,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(recovered.value_repr, "(True, 200011, 200011, 4)");
}

#[tokio::test]
async fn escaped_control_output_stays_within_frame_and_read_chunk_bounds() {
    if !crate::platform::binary_on_path("python3") {
        return;
    }
    let runspace = PythonRunspace::new();
    let first = runspace
        .execute(
            host_launch(),
            "host".to_string(),
            "import os\nos.write(1, b'\\0' * (1024 * 1024));",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let stdout = output_section(&first, PythonOutputSection::Stdout);
    assert_eq!(stdout.original_bytes, 1024 * 1024);
    assert_eq!(stdout.captured_bytes, 1024 * 1024);
    assert_eq!(stdout.excerpt_bytes, PYTHON_OUTPUT_READ_MAX_BYTES);
    assert!(!stdout.truncated);
    assert_eq!(first.stdout.len() as u64, PYTHON_OUTPUT_READ_MAX_BYTES);
    let artifact = artifact_literal(&first);
    let checked_code = format!(
        concat!(
            "import lethetic_output\n",
            "info = lethetic_output.info({artifact})\n",
            "chunk = lethetic_output.read({artifact}, 'stdout', 0, 65536)\n",
            "rejected = False\n",
            "try:\n",
            "    lethetic_output.read({artifact}, 'stdout', 0, 65537)\n",
            "except ValueError:\n",
            "    rejected = True\n",
            "(info['max_read_bytes'], chunk['total_bytes'], len(chunk['text']), chunk['text'].count('\\0'), rejected)",
        ),
        artifact = artifact,
    );

    let checked = runspace
        .execute(
            host_launch(),
            "host".to_string(),
            &checked_code,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(checked.value_repr, "(65536, 1048576, 65536, 65536, True)");
}

#[tokio::test]
async fn output_read_uses_utf8_boundaries_for_byte_offsets() {
    if !crate::platform::binary_on_path("python3") {
        return;
    }
    let runspace = PythonRunspace::new();
    let first = runspace
        .execute(
            host_launch(),
            "host".to_string(),
            "payload = '🙂' * 20000\nprint(payload)",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        output_section(&first, PythonOutputSection::Stdout).captured_bytes,
        80_001
    );
    let artifact = artifact_literal(&first);
    let checked_code = format!(
        concat!(
            "import lethetic_output\n",
            "chunk = lethetic_output.read({artifact}, 'stdout', 0, 5)\n",
            "bad_offset = bad_limit = False\n",
            "try:\n",
            "    lethetic_output.read({artifact}, 'stdout', 1, 8)\n",
            "except ValueError:\n",
            "    bad_offset = True\n",
            "try:\n",
            "    lethetic_output.read({artifact}, 'stdout', 0, 1)\n",
            "except ValueError:\n",
            "    bad_limit = True\n",
            "(chunk['text'], chunk['next_offset'], bad_offset, bad_limit)",
        ),
        artifact = artifact,
    );

    let checked = runspace
        .execute(
            host_launch(),
            "host".to_string(),
            &checked_code,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(checked.value_repr, "('🙂', 4, True, True)");
}

#[tokio::test]
async fn output_ring_enforces_aggregate_bytes_before_artifact_count() {
    if !crate::platform::binary_on_path("python3") {
        return;
    }
    let runspace = PythonRunspace::new();
    let mut artifacts = Vec::new();
    for _ in 0..5 {
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            runspace.execute(
                host_launch(),
                "host".to_string(),
                "import os\nos.write(1, b'a' * (1024 * 1024))\nos.write(2, b'b' * (1024 * 1024));",
                CancellationToken::new(),
            ),
        )
        .await
        .expect("bounded output cell blocked")
        .unwrap();
        assert!(result.output_metadata.retained);
        artifacts.push(artifact_literal(&result));
    }
    let checked_code = format!(
        concat!(
            "import lethetic_output\n",
            "def available(artifact_id):\n",
            "    try:\n",
            "        lethetic_output.info(artifact_id)\n",
            "        return True\n",
            "    except KeyError:\n",
            "        return False\n",
            "(available({first}), available({second}), lethetic_output.info({fifth})['retained_bytes'])",
        ),
        first = artifacts[0],
        second = artifacts[1],
        fifth = artifacts[4],
    );
    let checked = runspace
        .execute(
            host_launch(),
            "host".to_string(),
            &checked_code,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(checked.value_repr, "(False, True, 2097152)");
}

#[tokio::test]
async fn output_ring_enforces_artifact_count_and_reset_lifetime() {
    if !crate::platform::binary_on_path("python3") {
        return;
    }
    let runspace = PythonRunspace::new();
    let mut artifacts = Vec::new();
    for value in 1..=9 {
        let result = runspace
            .execute(
                host_launch(),
                "host".to_string(),
                &format!("print({value})"),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        artifacts.push(artifact_literal(&result));
    }
    let checked_code = format!(
        concat!(
            "import lethetic_output\n",
            "def available(artifact_id):\n",
            "    try:\n",
            "        lethetic_output.info(artifact_id)\n",
            "        return True\n",
            "    except KeyError:\n",
            "        return False\n",
            "(available({first}), available({second}))",
        ),
        first = artifacts[0],
        second = artifacts[1],
    );
    let checked = runspace
        .execute(
            host_launch(),
            "host".to_string(),
            &checked_code,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(checked.value_repr, "(False, True)");

    let old_artifact_id = artifacts[8].clone();
    runspace.reset_checked().await.unwrap();
    let new_result = runspace
        .execute(
            host_launch(),
            "host".to_string(),
            "print('new worker')",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_ne!(
        new_result.output_metadata.artifact_id,
        serde_json::from_str::<String>(&old_artifact_id).unwrap()
    );
    let new_artifact_id = artifact_literal(&new_result);
    let after_reset_code = format!(
        concat!(
            "import lethetic_output\n",
            "old_available = True\n",
            "try:\n",
            "    lethetic_output.info({old})\n",
            "except KeyError:\n",
            "    old_available = False\n",
            "numeric_rejected = False\n",
            "try:\n",
            "    lethetic_output.info(1)\n",
            "except TypeError:\n",
            "    numeric_rejected = True\n",
            "(old_available, lethetic_output.info({new})['cell'], numeric_rejected)",
        ),
        old = old_artifact_id,
        new = new_artifact_id,
    );
    let after_reset = runspace
        .execute(
            host_launch(),
            "host".to_string(),
            &after_reset_code,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(!after_reset.is_error, "{}", after_reset.render());
    assert_eq!(after_reset.value_repr, "(False, 1, True)");
}

#[tokio::test]
async fn traceback_capture_and_excerpt_retain_the_error_tail() {
    if !crate::platform::binary_on_path("python3") {
        return;
    }
    let runspace = PythonRunspace::new();
    let failed = runspace
        .execute(
            host_launch(),
            "host".to_string(),
            "raise RuntimeError(('x' * 300000) + 'TRACEBACK-TAIL-SENTINEL')",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let traceback = output_section(&failed, PythonOutputSection::Traceback);
    assert!(failed.is_error);
    assert!(traceback.truncated);
    assert_eq!(traceback.captured_bytes, 256 * 1024);
    assert!(traceback.original_bytes > traceback.captured_bytes);
    assert!(failed.traceback.contains("TRACEBACK-TAIL-SENTINEL"));
    let artifact = artifact_literal(&failed);
    let recovery_code = format!(
        concat!(
            "import lethetic_output\n",
            "info = lethetic_output.info({artifact})\n",
            "size = info['sections']['traceback']['captured_bytes']\n",
            "tail = lethetic_output.read({artifact}, 'traceback', size - 65536, 65536)\n",
            "('TRACEBACK-TAIL-SENTINEL' in tail['text'], tail['eof'])",
        ),
        artifact = artifact,
    );

    let recovered = runspace
        .execute(
            host_launch(),
            "host".to_string(),
            &recovery_code,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(recovered.value_repr, "(True, True)");
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn cancellation_settles_subprocess_run_before_a_late_sentinel_can_fire() {
    if !crate::platform::binary_on_path("python3") {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let ready = directory.path().join("descendant-ready");
    let late = directory.path().join("late-sentinel");
    let ready_literal = serde_json::to_string(&ready.to_string_lossy()).unwrap();
    let late_literal = serde_json::to_string(&late.to_string_lossy()).unwrap();
    let child_code = format!(
        "from pathlib import Path; import time; Path({ready_literal}).write_text('ready'); time.sleep(1.0); Path({late_literal}).write_text('late')"
    );
    let code = format!(
        "import subprocess, sys\nsubprocess.run([sys.executable, '-c', {}], check=True)",
        serde_json::to_string(&child_code).unwrap()
    );
    let runspace = Arc::new(PythonRunspace::new());
    let cancellation = CancellationToken::new();
    let task_runspace = runspace.clone();
    let task_cancellation = cancellation.clone();
    let task = tokio::spawn(async move {
        task_runspace
            .execute(host_launch(), "host".to_string(), &code, task_cancellation)
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !ready.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("subprocess.run descendant did not publish its ready marker");

    cancellation.cancel();
    let error = tokio::time::timeout(std::time::Duration::from_secs(6), task)
        .await
        .expect("cancelled subprocess.run cell did not settle")
        .expect("cancelled subprocess.run task panicked")
        .unwrap_err();
    assert!(error.contains("cancelled"), "{error}");
    assert!(!error.contains("cleanup failed"), "{error}");
    assert!(!runspace.is_running().await);
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    assert!(
        !late.exists(),
        "subprocess descendant fired after cancellation was reported complete"
    );
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn checked_reset_settles_inherited_descendants() {
    if !crate::platform::binary_on_path("python3") {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let ready = directory.path().join("reset-descendant-ready");
    let late = directory.path().join("reset-late-sentinel");
    let ready_literal = serde_json::to_string(&ready.to_string_lossy()).unwrap();
    let late_literal = serde_json::to_string(&late.to_string_lossy()).unwrap();
    let child_code = format!(
        "from pathlib import Path; import time; Path({ready_literal}).write_text('ready'); time.sleep(1.0); Path({late_literal}).write_text('late')"
    );
    let code = format!(
        "import subprocess, sys\nsubprocess.Popen([sys.executable, '-c', {}])",
        serde_json::to_string(&child_code).unwrap()
    );
    let runspace = PythonRunspace::new();
    let result = runspace
        .execute(
            host_launch(),
            "host".to_string(),
            &code,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(!result.is_error, "{}", result.render());
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !ready.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("background descendant did not publish its reset marker");

    tokio::time::timeout(std::time::Duration::from_secs(6), runspace.reset_checked())
        .await
        .expect("checked reset exceeded its containment deadline")
        .unwrap();
    assert!(!runspace.is_running().await);
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    assert!(
        !late.exists(),
        "inherited descendant fired after checked reset completed"
    );
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn worker_drop_signals_inherited_descendants() {
    if !crate::platform::binary_on_path("python3") {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let ready = directory.path().join("drop-descendant-ready");
    let late = directory.path().join("drop-late-sentinel");
    let ready_literal = serde_json::to_string(&ready.to_string_lossy()).unwrap();
    let late_literal = serde_json::to_string(&late.to_string_lossy()).unwrap();
    let child_code = format!(
        "from pathlib import Path; import time; Path({ready_literal}).write_text('ready'); time.sleep(1.0); Path({late_literal}).write_text('late')"
    );
    let code = format!(
        "import subprocess, sys\nsubprocess.Popen([sys.executable, '-c', {}])",
        serde_json::to_string(&child_code).unwrap()
    );
    let runspace = PythonRunspace::new();
    let result = runspace
        .execute(
            host_launch(),
            "host".to_string(),
            &code,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(!result.is_error, "{}", result.render());
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !ready.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("background descendant did not publish its ready marker");

    drop(runspace);
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    assert!(
        !late.exists(),
        "inherited descendant fired after its Worker guard was dropped"
    );
}

#[tokio::test]
async fn cancellation_resets_worker() {
    if !crate::platform::binary_on_path("python3") {
        return;
    }
    let runspace = Arc::new(PythonRunspace::new());
    let token = CancellationToken::new();
    let task_runspace = runspace.clone();
    let task_token = token.clone();
    let task = tokio::spawn(async move {
        task_runspace
            .execute(
                host_launch(),
                "host".to_string(),
                "value_before_cancel = 99\nwhile True:\n    pass",
                task_token,
            )
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    token.cancel();
    let error = task.await.unwrap().unwrap_err();
    assert!(error.contains("cancelled"), "{error}");
    assert!(!error.contains("cleanup failed"), "{error}");

    let result = runspace
        .execute(
            host_launch(),
            "host".to_string(),
            "value_before_cancel",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(result.is_error);
    assert!(result.traceback.contains("NameError"));
}

#[tokio::test]
async fn process_crash_restarts_lazily() {
    if !crate::platform::binary_on_path("python3") {
        return;
    }
    let runspace = PythonRunspace::new();
    let error = runspace
        .execute(
            host_launch(),
            "host".to_string(),
            "import os\nos._exit(17)",
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(error.contains("exited") || error.contains("frame"));
    let recovered = runspace
        .execute(
            host_launch(),
            "host".to_string(),
            "6 * 7",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(recovered.value_repr, "42");
}

#[tokio::test]
async fn reset_discards_globals() {
    if !crate::platform::binary_on_path("python3") {
        return;
    }
    let runspace = PythonRunspace::new();
    runspace
        .execute(
            host_launch(),
            "host".to_string(),
            "secret = 123",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    runspace.reset().await;
    let result = runspace
        .execute(
            host_launch(),
            "host".to_string(),
            "secret",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(result.is_error);
    assert!(result.traceback.contains("NameError"));
}
