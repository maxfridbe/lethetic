use super::*;

#[cfg(unix)]
#[tokio::test]
async fn scoped_probe_cancellation_reaps_podman_command_before_returning() {
    let root = tempfile::tempdir().unwrap();
    let fake = root.path().join("fake-command");
    std::fs::write(
        &fake,
        "#!/bin/sh\n/bin/sleep 30 &\nchild=$!\nprintf '%s %s' \"$$\" \"$child\" > \"$1\"\nwait \"$child\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
    let marker = root.path().join("cancelled-pid");
    let cancellation = tokio_util::sync::CancellationToken::new();
    let task_cancellation = cancellation.clone();
    let task_fake = fake.clone();
    let task_marker = marker.clone();
    let task = tokio::spawn(async move {
        with_podman_command_cancellation(
            task_cancellation,
            run_podman_output(
                &task_fake,
                &[task_marker.into_os_string()],
                Duration::from_secs(30),
            ),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while !marker.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("scoped Podman command did not start");
    let pids = std::fs::read_to_string(&marker)
        .unwrap()
        .split_whitespace()
        .map(|value| value.parse::<libc::pid_t>().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        pids.len(),
        2,
        "fake Podman command did not spawn a descendant"
    );

    cancellation.cancel();
    let error = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("cancelled Podman command did not settle")
        .expect("Podman command task panicked")
        .unwrap_err();
    assert!(error.contains("cancelled"), "{error}");
    assert!(!error.contains("containment"), "{error}");
    for pid in pids {
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        assert!(
            !alive,
            "Podman command process-tree member {pid} survived cancellation"
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH),
            "cancelled Podman command process-tree member was not fully reaped"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn timed_out_child_reaping_is_bounded_and_has_background_fallback() {
    let root = tempfile::tempdir().unwrap();
    let fake = root.path().join("fake-command");
    std::fs::write(
        &fake,
        "#!/bin/sh\nprintf '%s' \"$$\" > \"$1\"\nexec /bin/sleep 30\n",
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();

    let marker = root.path().join("timed-out-pid");
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        run_podman_output(
            &fake,
            &[marker.clone().into_os_string()],
            Duration::from_millis(30),
        ),
    )
    .await
    .expect("command timeout cleanup exceeded its outer bound")
    .unwrap_err();
    assert!(result.contains("timed out"), "{result}");

    let fallback_marker = root.path().join("fallback-pid");
    let child = tokio::process::Command::new(&fake)
        .arg(&fallback_marker)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    for _ in 0..100 {
        if fallback_marker.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let pid = std::fs::read_to_string(&fallback_marker).unwrap();
    assert!(terminate_podman_child_for_test(child, Duration::ZERO).await);
    for _ in 0..100 {
        if !Path::new("/proc").join(pid.trim()).exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("background child reaper did not reap PID {}", pid.trim());
}
