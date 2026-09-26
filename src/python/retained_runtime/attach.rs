use super::launch::{
    config_from_manifest, import_legacy_manifest, require_current_attach_abi,
    trusted_current_executable,
};
use super::maintenance::stop_manifest_bound_container;
use crate::python::egress_broker::BROKER_PROTOCOL_ABI;
use crate::python::retained_podman::{
    ContainerProcessIdentity, RetainedPodmanConfig, attest_retained_container,
    build_start_attach_spec, prepare_retained_podman, resolve_retained_podman, run_stop,
    verify_exact_container_stopped,
};
use crate::python::runtime_store::{
    RuntimeLifecycleState, RuntimeLock, RuntimeManifest, RuntimeStore, RuntimeWorkspaceOwnership,
};
use chrono::Utc;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

const CONTAINER_START_TIMEOUT: Duration = Duration::from_secs(15);
const CHILD_EXIT_TIMEOUT: Duration = Duration::from_secs(20);
const CHILD_KILL_TIMEOUT: Duration = Duration::from_secs(2);

pub async fn run_retained_attach(state_root: PathBuf, runtime_id: String) -> Result<i32, String> {
    let (shutdown, received_signal, _signal_guard) = install_attach_signal_handler()?;
    let store = RuntimeStore::open_at(state_root)?;
    let lock = store.try_lock_runtime(&runtime_id)?.ok_or_else(|| {
        "retained Python runtime is already attached or being maintained".to_string()
    })?;
    let mut manifest = store.load_manifest(&lock)?;
    let import_podman = resolve_retained_podman().await?;
    import_legacy_manifest(&store, &lock, &mut manifest, &import_podman).await?;
    require_current_attach_abi(&manifest)?;
    if !manifest.layout_profile.is_attachable() {
        return Err(
            "legacy runtime-local broker layouts are delete-only and cannot be attached"
                .to_string(),
        );
    }
    if manifest.is_expired_at(Utc::now())? {
        return Err("retained Python runtime expired before attach".to_string());
    }
    if manifest.lifecycle != RuntimeLifecycleState::Stopped {
        return Err(format!(
            "retained Python runtime cannot attach from state {:?}",
            manifest.lifecycle
        ));
    }
    manifest.workspace.verify_current()?;
    let (podman, image) = prepare_retained_podman(&manifest.image_id).await?;
    if image.image_id != manifest.image_id {
        return Err("retained runtime image resolution changed".to_string());
    }
    let config = config_from_manifest(&store, &lock, &manifest, podman)?;
    let container_id = manifest
        .container_id
        .clone()
        .ok_or_else(|| "retained runtime manifest has no container ID".to_string())?;
    let before = attest_retained_container(
        &config,
        &container_id,
        &manifest.workspace,
        manifest.selinux_labels.as_ref(),
    )
    .await?;
    if before.running {
        return Err("retained container is already running before attach".to_string());
    }
    if shutdown.is_cancelled() {
        return Ok(signal_exit_code(&received_signal));
    }

    let bootstrap = store.prepare_broker_bootstrap(&lock)?;
    manifest.transition(RuntimeLifecycleState::Attached)?;
    store.save_manifest(&lock, &manifest)?;
    if shutdown.is_cancelled() {
        restore_stopped_manifest(&store, &lock, &mut manifest)?;
        return Ok(signal_exit_code(&received_signal));
    }

    let mut attach = match spawn_attached_container(&config.podman, &container_id) {
        Ok(child) => child,
        Err(error) => {
            restore_stopped_manifest(&store, &lock, &mut manifest)?;
            return Err(error);
        }
    };
    let running =
        match wait_for_running_container(&config, &manifest, &container_id, &mut attach, &shutdown)
            .await
        {
            Ok(identity) => identity,
            Err(error) => {
                let settle = settle_attached_runtime(
                    &store,
                    &lock,
                    &mut manifest,
                    &config,
                    &container_id,
                    &mut attach,
                )
                .await;
                if shutdown.is_cancelled() {
                    settle?;
                    return Ok(signal_exit_code(&received_signal));
                }
                return Err(combine_errors(error, settle.err()));
            }
        };
    let host_pid = running
        .host_pid
        .ok_or_else(|| "running retained container has no host PID".to_string())?;
    if shutdown.is_cancelled() {
        settle_attached_runtime(
            &store,
            &lock,
            &mut manifest,
            &config,
            &container_id,
            &mut attach,
        )
        .await?;
        return Ok(signal_exit_code(&received_signal));
    }
    let mut broker = match spawn_broker(
        &config.podman,
        &bootstrap.socket_path,
        &bootstrap.audit_path,
        &manifest,
        &bootstrap.capability,
        host_pid,
        running.selinux_labels.as_ref(),
    ) {
        Ok(child) => child,
        Err(error) => {
            let settle = settle_attached_runtime(
                &store,
                &lock,
                &mut manifest,
                &config,
                &container_id,
                &mut attach,
            )
            .await;
            return Err(combine_errors(error, settle.err()));
        }
    };

    let cause = wait_for_attach_cause(&mut attach, &mut broker, &shutdown, &received_signal).await;
    let settle = settle_attached_runtime(
        &store,
        &lock,
        &mut manifest,
        &config,
        &container_id,
        &mut attach,
    )
    .await;
    terminate_child_group(&mut broker, Duration::from_secs(5)).await;
    settle?;
    match cause {
        AttachCause::Container(status) => Ok(status.code().unwrap_or(1)),
        AttachCause::Signal(signal) => Ok(128 + signal),
        AttachCause::Broker(status) => Err(format!(
            "egress broker exited while the retained runtime was attached: {status}"
        )),
        AttachCause::WaitError(error) => Err(error),
    }
}

pub fn arm_internal_parent_death_signal() -> Result<(), String> {
    let parent = unsafe { libc::getppid() };
    if parent <= 1 {
        return Err("internal retained runtime process has no live parent".to_string());
    }
    if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM, 0, 0, 0) } != 0 {
        return Err(format!(
            "could not arm internal runtime parent-death handling: {}",
            std::io::Error::last_os_error()
        ));
    }
    if unsafe { libc::getppid() } != parent {
        return Err("internal retained runtime parent exited during startup".to_string());
    }
    Ok(())
}

fn configure_child_parent_death(command: &mut Command) {
    let expected_parent = unsafe { libc::getpid() };
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() != expected_parent {
                return Err(std::io::Error::from_raw_os_error(libc::ECHILD));
            }
            Ok(())
        });
    }
}

fn spawn_attached_container(podman: &Path, container_id: &str) -> Result<Child, String> {
    let spec = build_start_attach_spec(podman, container_id)?;
    let mut command = Command::new(spec.program);
    command
        .args(spec.args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .process_group(0);
    configure_child_parent_death(&mut command);
    command
        .spawn()
        .map_err(|error| format!("could not start attached retained container: {error}"))
}

async fn wait_for_running_container(
    config: &RetainedPodmanConfig,
    manifest: &RuntimeManifest,
    container_id: &str,
    attach: &mut Child,
    shutdown: &CancellationToken,
) -> Result<ContainerProcessIdentity, String> {
    let future = async {
        loop {
            if shutdown.is_cancelled() {
                return Err("retained container startup was cancelled by a signal".to_string());
            }
            if let Some(status) = attach
                .try_wait()
                .map_err(|error| format!("could not poll Podman attach process: {error}"))?
            {
                return Err(format!(
                    "Podman attach exited before the retained container started: {status}"
                ));
            }
            let identity = attest_retained_container(
                config,
                container_id,
                &manifest.workspace,
                manifest.selinux_labels.as_ref(),
            )
            .await?;
            if identity.running {
                return Ok(identity);
            }
            tokio::select! {
                _ = shutdown.cancelled() => {
                    return Err("retained container startup was cancelled by a signal".to_string());
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }
    };
    timeout(CONTAINER_START_TIMEOUT, future)
        .await
        .map_err(|_| "retained container startup timed out".to_string())?
}

fn spawn_broker(
    podman: &Path,
    socket_path: &Path,
    audit_path: &Path,
    manifest: &RuntimeManifest,
    capability: &str,
    host_pid: u32,
    labels: Option<&crate::python::selinux::SelinuxLabels>,
) -> Result<Child, String> {
    let executable = trusted_current_executable()?;
    let (process_label, mount_label) = labels
        .map(|labels| (labels.process_label.as_str(), labels.mount_label.as_str()))
        .unwrap_or(("none", "none"));
    let mut command = Command::new(podman);
    command
        .arg("unshare")
        .arg(executable)
        .arg("--internal-egress-broker")
        .arg(BROKER_PROTOCOL_ABI)
        .arg(socket_path)
        .arg(audit_path)
        .arg(&manifest.runtime_id)
        .arg(capability)
        .arg("auto")
        .arg("auto")
        .arg(host_pid.to_string())
        .arg(process_label)
        .arg(mount_label)
        .arg(
            if manifest.workspace_ownership == RuntimeWorkspaceOwnership::ExternalLaunchCwd {
                "label-disabled"
            } else {
                "selinux-labeled"
            },
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .process_group(0);
    for variable in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "no_proxy",
    ] {
        command.env_remove(variable);
    }
    configure_child_parent_death(&mut command);
    command
        .spawn()
        .map_err(|error| format!("could not start Podman-unshared egress broker: {error}"))
}

struct SignalTaskGuard(tokio::task::JoinHandle<()>);

impl Drop for SignalTaskGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn install_attach_signal_handler()
-> Result<(CancellationToken, Arc<AtomicI32>, SignalTaskGuard), String> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|error| {
        format!("could not install retained attach SIGTERM handler: {error}")
    })?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .map_err(|error| {
        format!("could not install retained attach SIGINT handler: {error}")
    })?;
    let mut hangup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
        .map_err(|error| format!("could not install retained attach SIGHUP handler: {error}"))?;
    let shutdown = CancellationToken::new();
    let received = Arc::new(AtomicI32::new(0));
    let task_shutdown = shutdown.clone();
    let task_received = received.clone();
    let task = tokio::spawn(async move {
        let first = tokio::select! {
            signal = terminate.recv() => signal.map(|_| libc::SIGTERM),
            signal = interrupt.recv() => signal.map(|_| libc::SIGINT),
            signal = hangup.recv() => signal.map(|_| libc::SIGHUP),
        };
        let Some(first) = first else {
            task_received.store(libc::SIGTERM, Ordering::Release);
            task_shutdown.cancel();
            return;
        };
        task_received.store(first, Ordering::Release);
        task_shutdown.cancel();

        let _second = tokio::select! {
            signal = terminate.recv() => signal,
            signal = interrupt.recv() => signal,
            signal = hangup.recv() => signal,
        };
        unsafe { libc::_exit(128 + first) };
    });
    Ok((shutdown, received, SignalTaskGuard(task)))
}

fn signal_exit_code(received: &AtomicI32) -> i32 {
    128 + received.load(Ordering::Acquire).max(1)
}

enum AttachCause {
    Container(std::process::ExitStatus),
    Broker(std::process::ExitStatus),
    Signal(i32),
    WaitError(String),
}

async fn wait_for_attach_cause(
    attach: &mut Child,
    broker: &mut Child,
    shutdown: &CancellationToken,
    received_signal: &AtomicI32,
) -> AttachCause {
    tokio::select! {
        result = attach.wait() => match result {
            Ok(status) => AttachCause::Container(status),
            Err(error) => AttachCause::WaitError(format!("could not wait for Podman attach: {error}")),
        },
        result = broker.wait() => match result {
            Ok(status) => AttachCause::Broker(status),
            Err(error) => AttachCause::WaitError(format!("could not wait for egress broker: {error}")),
        },
        _ = shutdown.cancelled() => AttachCause::Signal(
            received_signal.load(Ordering::Acquire).max(1),
        ),
    }
}

async fn settle_attached_runtime(
    store: &RuntimeStore,
    lock: &RuntimeLock,
    manifest: &mut RuntimeManifest,
    config: &RetainedPodmanConfig,
    container_id: &str,
    attach: &mut Child,
) -> Result<(), String> {
    if manifest.lifecycle == RuntimeLifecycleState::Attached {
        manifest.transition(RuntimeLifecycleState::Stopping)?;
        store.save_manifest(lock, manifest)?;
    }
    let identity = match attest_retained_container(
        config,
        container_id,
        &manifest.workspace,
        manifest.selinux_labels.as_ref(),
    )
    .await
    {
        Ok(identity) => identity,
        Err(attestation_error) => {
            let containment = stop_manifest_bound_container(config, container_id).await;
            let attach_finish = finish_attach_child(attach).await;
            let mut error = format!("pre-stop attestation failed: {attestation_error}");
            if let Err(containment_error) = containment {
                error.push_str(&format!(
                    "; exact-ID containment failed: {containment_error}"
                ));
            }
            if let Err(finish_error) = attach_finish {
                error.push_str(&format!(
                    "; attach-process containment failed: {finish_error}"
                ));
            }
            if let Err(quarantine_error) = quarantine_manifest(store, lock, manifest, error.clone())
            {
                error.push_str(&format!(
                    "; quarantine persistence failed: {quarantine_error}"
                ));
            }
            return Err(error);
        }
    };
    if identity.running {
        if let Err(error) = run_stop(&config.podman, container_id).await {
            let verification = verify_exact_container_stopped(&config.podman, container_id).await;
            let attach_finish = finish_attach_child(attach).await;
            let mut detail = format!("container stop failed: {error}");
            if let Err(verification_error) = verification {
                detail.push_str(&format!(
                    "; exact stopped-state verification failed: {verification_error}"
                ));
            }
            if let Err(finish_error) = attach_finish {
                detail.push_str(&format!(
                    "; attach-process containment failed: {finish_error}"
                ));
            }
            quarantine_manifest(store, lock, manifest, detail.clone())?;
            return Err(detail);
        }
    }
    if let Err(error) = finish_attach_child_then(attach, Duration::ZERO, CHILD_KILL_TIMEOUT, || {
        verify_exact_container_stopped(&config.podman, container_id)
    })
    .await
    {
        let containment = stop_manifest_bound_container(config, container_id).await;
        let mut detail = format!(
            "final attach-process containment or exact stopped-state verification failed: {error}"
        );
        if let Err(containment_error) = containment {
            detail.push_str(&format!(
                "; final exact-ID containment failed: {containment_error}"
            ));
        }
        quarantine_manifest(store, lock, manifest, detail.clone())?;
        return Err(detail);
    }
    let stopped = match attest_retained_container(
        config,
        container_id,
        &manifest.workspace,
        manifest.selinux_labels.as_ref(),
    )
    .await
    {
        Ok(identity) => identity,
        Err(error) => {
            let containment = stop_manifest_bound_container(config, container_id).await;
            let mut detail = format!("post-stop attestation failed: {error}");
            if let Err(containment_error) = containment {
                detail.push_str(&format!(
                    "; final exact-ID containment failed: {containment_error}"
                ));
            }
            quarantine_manifest(store, lock, manifest, detail.clone())?;
            return Err(detail);
        }
    };
    if stopped.running || stopped.host_pid.is_some() {
        let containment = stop_manifest_bound_container(config, container_id).await;
        let mut error = "retained container remained running after stop".to_string();
        if let Err(containment_error) = containment {
            error.push_str(&format!(
                "; final exact-ID containment failed: {containment_error}"
            ));
        }
        quarantine_manifest(store, lock, manifest, error.clone())?;
        return Err(error);
    }
    manifest.transition(RuntimeLifecycleState::Stopped)?;
    manifest.touch(Utc::now())?;
    store.save_manifest(lock, manifest)
}

async fn finish_attach_child(attach: &mut Child) -> Result<(), String> {
    finish_attach_child_with_timeouts(attach, CHILD_EXIT_TIMEOUT, CHILD_KILL_TIMEOUT).await
}

pub(super) async fn finish_attach_child_with_timeouts(
    attach: &mut Child,
    exit_timeout: Duration,
    kill_timeout: Duration,
) -> Result<(), String> {
    let mut containment_errors = Vec::new();
    match timeout(exit_timeout, attach.wait()).await {
        Ok(Ok(_)) => return Ok(()),
        Ok(Err(error)) => {
            containment_errors.push(format!("initial attach-process wait failed: {error}"));
        }
        Err(_) => {}
    }

    if let Some(pid) = attach.id().and_then(|pid| i32::try_from(pid).ok()) {
        if unsafe { libc::kill(-pid, libc::SIGKILL) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                containment_errors.push(format!("process-group SIGKILL failed: {error}"));
            }
        }
    } else {
        containment_errors.push("attach process has no valid process-group ID".to_string());
    }
    if let Err(error) = attach.start_kill()
        && !matches!(
            error.kind(),
            std::io::ErrorKind::InvalidInput | std::io::ErrorKind::NotFound
        )
    {
        containment_errors.push(format!("direct attach-process kill failed: {error}"));
    }

    match timeout(kill_timeout, attach.wait()).await {
        Ok(Ok(_)) if containment_errors.is_empty() => Ok(()),
        Ok(Ok(_)) => Err(format!(
            "Podman attach process was reaped with incomplete containment: {}",
            containment_errors.join("; ")
        )),
        Ok(Err(error)) => Err(format!(
            "could not reap killed Podman attach process: {error}{}",
            if containment_errors.is_empty() {
                String::new()
            } else {
                format!("; {}", containment_errors.join("; "))
            }
        )),
        Err(_) => Err(format!(
            "Podman attach process was not reaped within the {kill_timeout:?} kill deadline{}",
            if containment_errors.is_empty() {
                String::new()
            } else {
                format!("; {}", containment_errors.join("; "))
            }
        )),
    }
}

pub(super) async fn finish_attach_child_then<T, F, Fut>(
    attach: &mut Child,
    exit_timeout: Duration,
    kill_timeout: Duration,
    after_reap: F,
) -> Result<T, String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    finish_attach_child_with_timeouts(attach, exit_timeout, kill_timeout).await?;
    after_reap().await
}

fn restore_stopped_manifest(
    store: &RuntimeStore,
    lock: &RuntimeLock,
    manifest: &mut RuntimeManifest,
) -> Result<(), String> {
    manifest.transition(RuntimeLifecycleState::Stopping)?;
    manifest.transition(RuntimeLifecycleState::Stopped)?;
    store.save_manifest(lock, manifest)
}

fn quarantine_manifest(
    store: &RuntimeStore,
    lock: &RuntimeLock,
    manifest: &mut RuntimeManifest,
    reason: String,
) -> Result<(), String> {
    manifest.quarantine(reason)?;
    store.save_manifest(lock, manifest)
}

async fn terminate_child_group(child: &mut Child, grace: Duration) {
    if let Some(pid) = child.id().and_then(|pid| i32::try_from(pid).ok()) {
        let _ = unsafe { libc::kill(-pid, libc::SIGTERM) };
    }
    if timeout(grace, child.wait()).await.is_err() {
        if let Some(pid) = child.id().and_then(|pid| i32::try_from(pid).ok()) {
            let _ = unsafe { libc::kill(-pid, libc::SIGKILL) };
        }
        let _ = child.start_kill();
        let _ = timeout(Duration::from_secs(2), child.wait()).await;
    }
}

fn combine_errors(primary: String, secondary: Option<String>) -> String {
    match secondary {
        Some(secondary) => format!("{primary}; cleanup also failed: {secondary}"),
        None => primary,
    }
}
