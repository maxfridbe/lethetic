use super::*;
use crate::config::{PythonRuntimeConfig, SandboxConfig};

#[cfg(target_os = "linux")]
const IMAGE_ID: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn mount(path: PathBuf, access: AccessMode, is_workspace: bool) -> ValidatedMount {
    ValidatedMount {
        path,
        access,
        is_directory: true,
        is_workspace,
    }
}

fn args_text(spec: &LaunchSpec) -> Vec<String> {
    spec.args
        .iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn validates_and_orders_nested_mounts() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("workspace");
    let nested = workspace.join("data");
    std::fs::create_dir_all(&nested).unwrap();
    let mounts = validate_mounts(
        &workspace,
        AccessMode::ReadOnly,
        &[
            PathGrant {
                path: nested.clone(),
                access: AccessMode::ReadWrite,
            },
            PathGrant {
                path: nested,
                access: AccessMode::ReadOnly,
            },
        ],
    )
    .unwrap();
    assert_eq!(mounts.len(), 2);
    assert!(mounts[0].is_workspace);
    assert_eq!(mounts[1].access, AccessMode::ReadOnly);
    assert!(mounts[1].path.starts_with(&mounts[0].path));
}

#[cfg(unix)]
#[test]
fn rejects_symlink_and_socket_grants() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let link = dir.path().join("link");
    symlink(&workspace, &link).unwrap();
    let error = validate_mounts(
        &workspace,
        AccessMode::ReadOnly,
        &[PathGrant {
            path: link,
            access: AccessMode::ReadOnly,
        }],
    )
    .unwrap_err();
    assert!(error.contains("symlinks"));

    let socket = dir.path().join("socket");
    let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let error = validate_mounts(
        &workspace,
        AccessMode::ReadOnly,
        &[PathGrant {
            path: socket,
            access: AccessMode::ReadOnly,
        }],
    )
    .unwrap_err();
    assert!(error.contains("socket"));
}

#[test]
fn bubblewrap_builder_contains_security_and_network_flags() {
    if !cfg!(target_os = "linux") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().canonicalize().unwrap();
    let mounts = vec![mount(workspace.clone(), AccessMode::ReadOnly, true)];
    let spec = build_bubblewrap_spec(
        Path::new("/bin/true"),
        Path::new("/bin/true"),
        &mounts,
        &workspace,
        NetworkAccess::None,
    )
    .unwrap();
    let args = args_text(&spec);
    for required in [
        "--die-with-parent",
        "--new-session",
        "--unshare-user",
        "--unshare-pid",
        "--unshare-net",
        "--clearenv",
        "--ro-bind",
    ] {
        assert!(
            args.iter().any(|value| value == required),
            "missing {required}"
        );
    }
    assert!(
        !args
            .iter()
            .any(|value| value == "-c" && value.contains("bwrap"))
    );

    let full = build_bubblewrap_spec(
        Path::new("/bin/true"),
        Path::new("/bin/true"),
        &mounts,
        &workspace,
        NetworkAccess::Full,
    )
    .unwrap();
    assert!(
        !args_text(&full)
            .iter()
            .any(|value| value == "--unshare-net")
    );

    let error = build_bubblewrap_spec(
        Path::new("/bin/true"),
        Path::new("/bin/true"),
        &mounts,
        &workspace,
        NetworkAccess::Nonlocal,
    )
    .unwrap_err();
    assert!(error.contains("retained Podman runtime"), "{error}");
}

#[cfg(target_os = "linux")]
#[test]
fn podman_builder_is_hardened_and_never_pulls() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().canonicalize().unwrap();
    let nested = workspace.join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    let mounts = vec![
        mount(workspace.clone(), AccessMode::ReadOnly, true),
        mount(nested, AccessMode::ReadWrite, false),
    ];
    let spec = build_podman_spec(
        Path::new("/bin/true"),
        IMAGE_ID,
        &mounts,
        &workspace,
        NetworkAccess::None,
    )
    .unwrap();
    let args = args_text(&spec);
    for required in [
        "create",
        "--rm",
        "--interactive",
        "--pull=never",
        "--read-only",
        "--cap-drop=ALL",
        "--security-opt=no-new-privileges",
        "--security-opt=label=disable",
        "--userns=keep-id",
        "--network=none",
        "--entrypoint=python3",
    ] {
        assert!(
            args.iter().any(|value| value == required),
            "missing {required}"
        );
    }
    let expected_user = podman_user_arg().unwrap().to_string_lossy().into_owned();
    assert!(args.iter().any(|value| value == &expected_user));
    assert!(
        args.iter()
            .any(|value| value == "--tmpfs=/home/lethetic:rw,nosuid,nodev,mode=1777")
    );
    assert!(args.iter().any(|value| value.ends_with(":ro")));
    assert!(args.iter().any(|value| value.ends_with(":rw")));
    assert!(!args.iter().any(|value| value == "run"));
    assert!(!args.iter().any(|value| value == "pull"));
    assert!(!args.iter().any(|value| value.contains("/.lethetic:")));
    assert!(spec.cleanup.is_none());
    assert!(matches!(spec.kind, LaunchKind::TransientPodman(_)));

    std::fs::create_dir(workspace.join(".lethetic")).unwrap();
    let masked = build_podman_spec_with_mask(
        Path::new("/bin/true"),
        IMAGE_ID,
        &mounts,
        &workspace,
        NetworkAccess::None,
        true,
    )
    .unwrap();
    let expected_mask = format!(
        "--tmpfs={}:ro,nosuid,nodev,noexec,notmpcopyup,mode=000,size=1048576",
        workspace.join(".lethetic").display()
    );
    assert!(
        args_text(&masked)
            .iter()
            .any(|value| value == &expected_mask)
    );

    let full = build_podman_spec(
        Path::new("/bin/true"),
        IMAGE_ID,
        &mounts,
        &workspace,
        NetworkAccess::Full,
    )
    .unwrap();
    assert!(
        args_text(&full)
            .iter()
            .any(|value| value == "--network=host")
    );

    let error = build_podman_spec(
        Path::new("/bin/true"),
        IMAGE_ID,
        &mounts,
        &workspace,
        NetworkAccess::Nonlocal,
    )
    .unwrap_err();
    assert!(error.contains("retained Podman runtime"), "{error}");
}

#[cfg(unix)]
#[test]
fn accepts_distrobox_podman_link_but_rejects_direct_forwarder() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("distrobox-host-exec");
    std::fs::write(&target, "#!/bin/sh\nexit 1\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(&target).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&target, permissions).unwrap();
    let link = dir.path().join("podman");
    symlink(&target, &link).unwrap();

    let resolved =
        resolve_invocation_path_with_search_path("podman", dir.path().as_os_str()).unwrap();
    assert_eq!(resolved, link);
    validate_podman_executable(&resolved).unwrap();
    let error = validate_podman_executable(&target).unwrap_err();
    assert!(error.contains("'podman' compatibility symlink"));
}

#[cfg(unix)]
#[tokio::test]
async fn cancelled_initial_backend_probe_reaps_leader_and_descendants_before_completion() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let command = root.path().join("hanging-probe");
    let marker = root.path().join("pid");
    std::fs::write(
            &command,
            format!(
                "#!/bin/sh\n/bin/sleep 30 &\ndescendant=$!\nprintf '%s %s' \"$$\" \"$descendant\" > '{}'\nwait \"$descendant\"\n",
                marker.display()
            ),
        )
        .unwrap();
    std::fs::set_permissions(&command, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = Config::default();
    config.python_runtime.python_executable = command.to_string_lossy().into_owned();
    let workspace = root.path().canonicalize().unwrap();

    let cancellation = CancellationToken::new();
    let task_cancellation = cancellation.clone();
    let task = tokio::spawn(async move {
        probe_backend_with_cancellation(
            &config,
            &workspace,
            PythonBackendChoice::Host,
            task_cancellation,
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !marker.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("initial backend probe command did not start");
    let pids = std::fs::read_to_string(&marker)
        .unwrap()
        .split_whitespace()
        .map(|value| value.parse::<libc::pid_t>().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(pids.len(), 2, "probe did not record its descendant PID");

    cancellation.cancel();
    let error = tokio::time::timeout(std::time::Duration::from_secs(6), task)
        .await
        .expect("cancelled initial backend probe did not settle")
        .expect("probe task panicked")
        .unwrap_err();
    assert!(error.contains("cancelled"), "{error}");
    assert!(!error.contains("containment failed"), "{error}");
    for pid in pids {
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        assert!(
            !alive,
            "probe process-tree member {pid} survived cancellation settlement"
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH),
            "cancelled probe process-tree member was not fully contained"
        );
    }
}

#[cfg(windows)]
#[tokio::test]
async fn cancelled_windows_probe_job_reaps_leader_and_descendant_before_completion() {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("probe-pids.txt");
    let marker_literal = marker.to_string_lossy().replace('\'', "''");
    let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let powershell = PathBuf::from(system_root)
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");
    let script = format!(
        "$child = Start-Process -FilePath ($env:SystemRoot + '\\System32\\ping.exe') -ArgumentList '-n','120','127.0.0.1' -NoNewWindow -PassThru; Set-Content -NoNewline -Encoding ascii -LiteralPath '{marker_literal}' -Value \"$PID $($child.Id)\"; Wait-Process -Id $child.Id"
    );
    let args = [
        OsString::from("-NoLogo"),
        OsString::from("-NoProfile"),
        OsString::from("-NonInteractive"),
        OsString::from("-Command"),
        OsString::from(script),
    ];
    let cancellation = CancellationToken::new();
    let task_cancellation = cancellation.clone();
    let task_powershell = powershell.clone();
    let task = tokio::spawn(async move {
        run_output_with_cancellation(
            task_powershell.as_os_str(),
            &args,
            std::time::Duration::from_secs(30),
            &task_cancellation,
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !marker.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Windows probe process tree did not start");
    let pids = std::fs::read_to_string(&marker)
        .unwrap()
        .split_whitespace()
        .map(|value| value.parse::<u32>().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(pids.len(), 2);

    cancellation.cancel();
    let error = tokio::time::timeout(std::time::Duration::from_secs(8), task)
        .await
        .expect("Windows probe job did not settle after cancellation")
        .expect("Windows probe task panicked")
        .unwrap_err();
    assert!(error.contains("cancelled"), "{error}");
    assert!(!error.contains("containment failed"), "{error}");
    for pid in pids {
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if !process.is_null() {
            unsafe {
                CloseHandle(process);
            }
            panic!("Windows probe process-tree member {pid} survived job settlement");
        }
    }
}

#[test]
fn incomplete_runtime_never_resolves() {
    let config = Config {
        tool_profile: ToolProfile::PythonOnly,
        python_runtime: PythonRuntimeConfig {
            target: Some(PythonExecutionTarget::Sandbox),
            sandbox: SandboxConfig::default(),
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(config.python_mode_validation_error().is_some());
}
